//! The [`Auth`] seam before auth exists: one call forwarded per method.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use axum::http::HeaderMap;

use telmoni_shared::acting::{Acting, ActingKey};
use telmoni_shared::seam::{
    AuditEventsQuery, Auth, Closing, Confirmation, DocumentCursor, FoundedOrganization,
    MovedProject, OrderedProject, OrganizationFounding, OwnedOrganization, OwnerNotice,
    ProjectHome, ProjectOrder, ReadableProject, Receipt, SourceDocument,
};
use telmoni_shared::{
    AuthError, FlagSet, OrganizationId, OrganizationStatus, ProjectId, TelmoniError, UserId,
};

/// Auth as notifications, telemetry and the agent hold it before auth is
/// built. Auth holds each of them, so they are handed a slot that is filled a
/// moment later; a call before that answers as an auth that is not there.
#[derive(Default)]
pub(crate) struct LateAuth(OnceLock<Arc<telmoni_auth::AppState>>);

impl LateAuth {
    /// Fill the slot with auth, once built.
    pub(crate) fn fill(&self, auth: Arc<telmoni_auth::AppState>) -> anyhow::Result<()> {
        if self.0.set(auth).is_err() {
            anyhow::bail!("auth was wired twice");
        }
        Ok(())
    }

    fn get(&self) -> Result<&Arc<telmoni_auth::AppState>, TelmoniError> {
        self.0
            .get()
            .ok_or_else(|| AuthError::IdentityUnavailable.into())
    }
}

#[async_trait]
impl Auth for LateAuth {
    async fn resolve(&self, headers: &HeaderMap) -> Result<Acting, TelmoniError> {
        self.get()?.resolve(headers).await
    }

    async fn resolve_again(&self, acting: &Acting) -> Result<Acting, TelmoniError> {
        self.get()?.resolve_again(acting).await
    }

    async fn resolve_api_key(&self, headers: &HeaderMap) -> Result<ActingKey, TelmoniError> {
        self.get()?.resolve_api_key(headers).await
    }

    async fn resolve_person(
        &self,
        user_id: &UserId,
        project_id: &ProjectId,
    ) -> Result<Acting, TelmoniError> {
        self.get()?.resolve_person(user_id, project_id).await
    }

    async fn project_homes(
        &self,
        projects: &[ProjectId],
    ) -> Result<Vec<ProjectHome>, TelmoniError> {
        self.get()?.project_homes(projects).await
    }

    async fn organization_slugs(
        &self,
        organizations: &[OrganizationId],
    ) -> Result<Vec<(OrganizationId, telmoni_shared::slug::Slug)>, TelmoniError> {
        self.get()?.organization_slugs(organizations).await
    }

    async fn readable_projects(
        &self,
        acting: &Acting,
    ) -> Result<Vec<ReadableProject>, TelmoniError> {
        self.get()?.readable_projects(acting).await
    }

    async fn global_flags(&self) -> Result<FlagSet, TelmoniError> {
        self.get()?.global_flags().await
    }

    async fn organization_standing(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError> {
        self.get()?.organization_standing(organization_id).await
    }

    async fn organizations_owned_by(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<OwnedOrganization>, TelmoniError> {
        self.get()?.organizations_owned_by(user_id).await
    }

    async fn mail_owner(&self, organization_id: &OrganizationId, notice: OwnerNotice<'_>) {
        if let Ok(auth) = self.get() {
            auth.mail_owner(organization_id, notice).await;
        }
    }

    async fn founded(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<OwnedOrganization>, TelmoniError> {
        self.get()?.founded(organization_id).await
    }

    async fn owner_of(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<UserId>, TelmoniError> {
        self.get()?.owner_of(organization_id).await
    }

    async fn owners_of_several(&self) -> Result<Vec<UserId>, TelmoniError> {
        self.get()?.owners_of_several().await
    }

    async fn audit_documents(
        &self,
        after: Option<&DocumentCursor>,
        limit: i64,
    ) -> Result<Vec<SourceDocument>, TelmoniError> {
        self.get()?.audit_documents(after, limit).await
    }

    async fn members(&self, acting: &Acting) -> Result<serde_json::Value, TelmoniError> {
        self.get()?.members(acting).await
    }

    async fn audit_events(
        &self,
        acting: &Acting,
        query: &AuditEventsQuery,
    ) -> Result<serde_json::Value, TelmoniError> {
        self.get()?.audit_events(acting, query).await
    }

    async fn mail_receipt(&self, to: &UserId, receipt: Receipt<'_>) {
        if let Ok(auth) = self.get() {
            auth.mail_receipt(to, receipt).await;
        }
    }

    async fn check_founding(
        &self,
        name: &str,
        slug: Option<&telmoni_shared::slug::Slug>,
    ) -> Result<(), TelmoniError> {
        self.get()?.check_founding(name, slug).await
    }

    async fn found_organization(
        &self,
        founding: &OrganizationFounding,
    ) -> Result<FoundedOrganization, TelmoniError> {
        self.get()?.found_organization(founding).await
    }

    async fn check_project(
        &self,
        organization: &OrganizationId,
        user: &UserId,
        name: &str,
    ) -> Result<String, TelmoniError> {
        self.get()?.check_project(organization, user, name).await
    }

    async fn make_ordered_project(
        &self,
        order: &ProjectOrder,
    ) -> Result<OrderedProject, TelmoniError> {
        self.get()?.make_ordered_project(order).await
    }

    async fn close_if_empty(&self, acting: &Acting, code: &str) -> Result<Closing, TelmoniError> {
        self.get()?.close_if_empty(acting, code).await
    }

    async fn move_project(
        &self,
        acting: &Acting,
        project_id: &ProjectId,
        into: &OrganizationId,
    ) -> Result<MovedProject, TelmoniError> {
        self.get()?.move_project(acting, project_id, into).await
    }

    async fn mail_confirmation(
        &self,
        acting: &Acting,
        confirmation: Confirmation<'_>,
    ) -> Result<(), TelmoniError> {
        self.get()?.mail_confirmation(acting, confirmation).await
    }

    async fn spend_confirmation(
        &self,
        acting: &Acting,
        confirmation: Confirmation<'_>,
        code: &str,
    ) -> Result<(), TelmoniError> {
        self.get()?
            .spend_confirmation(acting, confirmation, code)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `fn` names inside the block `opening` starts, comments skipped.
    fn methods_of(source: &str, opening: &str) -> std::collections::BTreeSet<String> {
        let (_, block) = source
            .split_once(opening)
            .expect("the block is in the source");
        let mut depth = 0_usize;
        let mut names = std::collections::BTreeSet::new();
        for line in block.lines() {
            let code = line.split("//").next().unwrap_or_default();
            let words: Vec<&str> = code.split_whitespace().collect();
            if depth == 1
                && let Some(at) = words.iter().position(|w| *w == "fn")
                && let Some(name) = words.get(at + 1)
            {
                names.insert(name.split(['(', '<']).next().unwrap_or(name).to_owned());
            }
            depth += code.matches('{').count();
            depth = depth.saturating_sub(code.matches('}').count());
            if depth == 0 && code.contains('}') {
                break;
            }
        }
        names
    }

    /// Every method of the seam is forwarded: a new one fails here, not in
    /// production, where its default would answer.
    #[test]
    fn late_auth_forwards_every_method_of_the_seam() {
        let declared = methods_of(include_str!("../../shared/src/seam.rs"), "pub trait Auth:");
        let forwarded = methods_of(include_str!("late_auth.rs"), "impl Auth for LateAuth");
        assert!(declared.len() > 20, "{declared:?}");
        assert_eq!(declared, forwarded);
    }

    /// Each call goes to auth, never the seam's default: before auth is built,
    /// every one answers that identity is unavailable.
    #[tokio::test]
    async fn every_call_goes_to_auth() {
        let late = LateAuth::default();
        let acting = Acting {
            user_id: UserId::new(),
            organization_id: OrganizationId::new(),
            organization_role: None,
            project: None,
            session_id: None,
            expires_at: 0,
        };
        let project = ProjectId::new();
        let founding = OrganizationFounding {
            organization_id: OrganizationId::new(),
            founder: UserId::new(),
            name: "Acme".into(),
            slug: None,
        };
        let content = || Confirmation::ContentMode(&project);
        let into = OrganizationId::new();
        let refused = [
            late.mail_confirmation(&acting, content()).await.err(),
            late.spend_confirmation(&acting, content(), "1").await.err(),
            late.check_founding("Acme", None).await.err(),
            late.found_organization(&founding).await.err(),
            late.close_if_empty(&acting, "1").await.err(),
            late.move_project(&acting, &project, &into).await.err(),
            late.resolve_person(&acting.user_id, &project).await.err(),
        ];
        for refused in refused {
            assert!(
                matches!(
                    refused,
                    Some(TelmoniError::Auth(AuthError::IdentityUnavailable))
                ),
                "{refused:?}"
            );
        }
    }
}
