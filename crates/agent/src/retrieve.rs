//! Retrieval: the hybrid search under the asker's own scope, and the rerank
//! over it when one is configured.
//!
//! Two filters stand between a person and a passage. Row-level security
//! binds the person, their organization and the project they asked from,
//! and the query names the same itself, so another tenant's rows and
//! another person's conversations do not exist in it, under the policies
//! or without them. `visibility` then keeps what their role reads on
//! that project, which the policies cannot know: the project's audit events
//! to a role that reads the audit log, the organization's own chain and
//! feed to its owner and admins, as the console's own pages give them.

use telmoni_shared::TelmoniError;
use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::person_scope;
use telmoni_shared::rbac::{Resource, Verb, can};

use crate::AppState;
use crate::db::{self, Found, Visibility};
use crate::embed::literal;
use crate::index::embedding_text;

/// How many fused results the reranker reads.
const RERANK_POOL: i64 = 30;

/// The visibilities this person's role reads on the acting project.
/// [`Visibility::Author`] is always among them: RLS alone decides whose
/// those are.
#[must_use]
pub fn visibilities(acting: &Acting) -> Vec<Visibility> {
    let mut out = vec![Visibility::Everyone, Visibility::Author];
    if let Some(project) = &acting.project
        && can(project.role, Verb::Read, Resource::Audit)
    {
        out.push(Visibility::Audit);
    }
    if acting
        .organization_role
        .is_some_and(|role| role.can_view_rolled_up_audit())
    {
        out.push(Visibility::OrganizationAdmin);
    }
    out
}

/// The best `limit` passages for `query` that this person may read.
pub async fn search(
    state: &AppState,
    acting: &Acting,
    query: &str,
    limit: i64,
) -> Result<Vec<Found>, TelmoniError> {
    let project = acting.project_or_bad_request()?;
    let embedder = state.embedder()?;
    let vector = embedder
        .embed(&[query.to_owned()])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| TelmoniError::Internal("no embedding for the query".into()))?;
    let embedding = literal(&vector)?;
    let visibility = visibilities(acting);
    let depth = if state.reranker.is_some() {
        RERANK_POOL.max(limit)
    } else {
        limit
    };

    let tx = person_scope(&state.db, &acting.user_id).await?;
    let tx = tx.bind_organization(&acting.organization_id).await?;
    let mut tx = tx.bind_project(&project.project_id).await?;
    let found = db::search(
        &mut tx,
        &embedding,
        embedder.model(),
        query,
        &visibility,
        depth,
    )
    .await?;
    tx.commit().await?;

    let keep = usize::try_from(limit).unwrap_or(usize::MAX);
    let Some(reranker) = state.reranker.as_ref().filter(|_| found.len() > 1) else {
        return Ok(found.into_iter().take(keep).collect());
    };
    let documents: Vec<String> = found
        .iter()
        .map(|f| embedding_text(&f.title, &f.body))
        .collect();
    match reranker.rank(query, &documents).await {
        Ok(order) => {
            let mut slots: Vec<Option<Found>> = found.into_iter().map(Some).collect();
            Ok(order
                .into_iter()
                .filter_map(|i| slots.get_mut(i).and_then(Option::take))
                .take(keep)
                .collect())
        }
        Err(e) => {
            tracing::warn!(error = %e, "rerank failed; the fused order stands");
            Ok(found.into_iter().take(keep).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telmoni_shared::acting::ActingProject;
    use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, UserId};

    fn acting(role: Role, organization_role: Option<OrganizationRole>) -> Acting {
        Acting {
            user_id: UserId::try_new("user_1").unwrap(),
            organization_id: OrganizationId::try_new("org_1").unwrap(),
            organization_role,
            project: Some(ActingProject {
                project_id: ProjectId::try_new("proj_1").unwrap(),
                role,
            }),
            session_id: None,
            expires_at: 0,
        }
    }

    #[test]
    fn a_member_reads_no_audit_and_the_organizations_own_is_its_owner_and_admins() {
        use Visibility::{Audit, Author, Everyone, OrganizationAdmin};
        assert_eq!(
            visibilities(&acting(Role::Member, Some(OrganizationRole::Member))),
            vec![Everyone, Author]
        );
        assert_eq!(
            visibilities(&acting(Role::Admin, Some(OrganizationRole::Member))),
            vec![Everyone, Author, Audit]
        );
        assert_eq!(
            visibilities(&acting(Role::Admin, Some(OrganizationRole::Admin))),
            vec![Everyone, Author, Audit, OrganizationAdmin]
        );
        assert_eq!(
            visibilities(&acting(Role::Owner, Some(OrganizationRole::Owner))),
            vec![Everyone, Author, Audit, OrganizationAdmin]
        );
    }
}
