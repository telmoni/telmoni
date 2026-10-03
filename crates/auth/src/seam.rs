//! What this module answers the modules beside it —
//! [`telmoni_shared::seam::Auth`], implemented on the module's own state.
//!
//! A module beside auth holds no grant on `auth.project_members`, so this is
//! how a person becomes a role for it. The bearer is the caller's proof of
//! the person, resolved here like every person lane's; the headers name the
//! organization and project the request acts on; the answer is what this
//! module derives from its own tables, and nothing the caller asserted.

use async_trait::async_trait;
use axum::http::HeaderMap;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::{Acting, ActingProject};
use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope};
use telmoni_shared::rbac::{Resource, Verb, can};
use telmoni_shared::seam::{
    Audience, AuditEventsQuery, Auth, DocumentCursor, ProjectHome, SourceDocument,
};
use telmoni_shared::{
    AuthError, AuthzError, FlagSet, OrganizationId, OrganizationRole, OrganizationStatus,
    ProjectId, TelmoniError, UserId,
};

use crate::AppState;
use crate::db::audit::{self, AuditQuery};
use crate::db::{AuthLane, access_tokens, flags, members, organizations, projects};
use crate::handler::{
    acting_organization, acting_project, authorize, parse_organization_id, parse_project_id,
};

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

#[async_trait]
impl Auth for AppState {
    async fn resolve(&self, headers: &HeaderMap) -> Result<Acting, TelmoniError> {
        let principal = crate::person::principal_of(self, header(headers, "authorization")).await?;
        let user_id = principal.user_id;
        let organization_id = header(headers, "x-organization-id")
            .ok_or_else(|| AuthError::BadRequest("missing x-organization-id header".into()))?;

        if let Some(project_id) = header(headers, "x-project-id") {
            let project_id = parse_project_id(project_id)?;
            return self
                .on_project(
                    user_id,
                    project_id,
                    Some(principal.session_id),
                    principal.expires_at,
                )
                .await;
        }

        let organization_id = parse_organization_id(organization_id)?;
        let acting = acting_organization(self, &organization_id, &user_id).await?;
        let role = acting.role;
        acting.tx.rollback().await?;
        Ok(Acting {
            user_id,
            organization_id,
            organization_role: Some(role),
            project: None,
            session_id: Some(principal.session_id),
            expires_at: principal.expires_at,
        })
    }

    async fn resolve_again(&self, acting: &Acting) -> Result<Acting, TelmoniError> {
        let project_id = acting.project_or_bad_request()?.project_id.clone();
        // The session too, as the bearer's lookup would have it: a sign-out
        // everywhere, or a deletion asked for, ends the request's reach now.
        if let Some(sid) = acting.session_id.as_deref() {
            let mut tx = maintenance_scope(&self.db, AuthLane).await?;
            let refused = access_tokens::session_refused(&mut tx, &acting.user_id, sid).await?;
            tx.commit().await?;
            if refused {
                return Err(AuthError::Unauthenticated.into());
            }
        }
        self.on_project(
            acting.user_id.clone(),
            project_id,
            acting.session_id.clone(),
            acting.expires_at,
        )
        .await
    }

    async fn project_homes(
        &self,
        projects: &[ProjectId],
    ) -> Result<Vec<ProjectHome>, TelmoniError> {
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let homes = projects::homes(&mut tx, projects).await?;
        tx.commit().await?;
        Ok(homes)
    }

    async fn organization_slugs(
        &self,
        organizations: &[OrganizationId],
    ) -> Result<Vec<(OrganizationId, String)>, TelmoniError> {
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let slugs = organizations::slugs(&mut tx, organizations).await?;
        tx.commit().await?;
        Ok(slugs)
    }

    async fn global_flags(&self) -> Result<FlagSet, TelmoniError> {
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let set = flags::resolve_global(&mut tx).await?;
        tx.commit().await?;
        Ok(set)
    }

    async fn organization_standing(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError> {
        let mut tx = organization_scope(&self.db, organization_id).await?;
        let status = organizations::status(&mut tx, organization_id).await?;
        tx.commit().await?;
        Ok(status)
    }

    async fn audit_documents(
        &self,
        after: Option<&DocumentCursor>,
        limit: i64,
    ) -> Result<Vec<SourceDocument>, TelmoniError> {
        let after = match after {
            Some(cursor) => Some((
                cursor.at,
                Uuid::parse_str(&cursor.id).map_err(|e| {
                    TelmoniError::internal("the audit cursor is not an event id", e)
                })?,
            )),
            None => None,
        };
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let events = audit::after_cursor(&mut tx, after, limit).await?;
        tx.commit().await?;
        Ok(events.into_iter().map(audit_document).collect())
    }

    async fn members(&self, acting: &Acting) -> Result<serde_json::Value, TelmoniError> {
        let project_id = &acting.project_or_bad_request()?.project_id;
        let current = acting_project(self, project_id, &acting.user_id).await?;
        authorize(current.role, Verb::Read, Resource::Member)?;
        let mut current = current.enter_owner_scope().await?;
        let rows = members::list_for_project(&mut current.tx, project_id).await?;
        let current = current.leave_owner_scope().await?;
        current.tx.commit().await?;
        Ok(json!({ "members": rows }))
    }

    async fn audit_events(
        &self,
        acting: &Acting,
        query: &AuditEventsQuery,
    ) -> Result<serde_json::Value, TelmoniError> {
        let project_id = &acting.project_or_bad_request()?.project_id;
        let current = acting_project(self, project_id, &acting.user_id).await?;
        // Read from the tables again rather than trusted from `acting`: the
        // model's tool call can come a minute after the turn was resolved.
        let whole_chain = current
            .organization_role
            .is_some_and(|role| role.can_view_rolled_up_audit());
        if !whole_chain && !can(current.role, Verb::Read, Resource::Audit) {
            current.tx.commit().await?;
            return Err(AuthzError::Forbidden(format!(
                "role {} may not read the audit log",
                current.role
            ))
            .into());
        }
        let organization = current.organization.clone();
        let mut tx = current.tx.bind_organization(&organization).await?;
        let events = audit::list(
            &mut tx,
            &AuditQuery {
                organization_id: &organization,
                in_project: (!whole_chain).then_some(project_id),
                from: query.from,
                to: query.to,
                actor: query.actor.as_deref(),
                action: query.action,
                resource_kind: None,
                cursor: None,
                limit: query.limit.clamp(1, AUDIT_TOOL_MAX),
            },
        )
        .await?;
        tx.commit().await?;
        // Without the addresses an invitation's or an email change's details
        // hold: what the model reads it can quote into an answer, kept past
        // an erasure of the person the address is. The console's audit log
        // page still shows them.
        let events: Vec<serde_json::Value> = events
            .into_iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "created_at": e.created_at,
                    "actor_id": e.actor_id,
                    "action": e.action,
                    "resource_kind": e.resource_kind,
                    "resource_id": e.resource_id,
                    "in_project": e.in_project,
                    "metadata": without_address(e.metadata),
                })
            })
            .collect();
        Ok(json!({
            "scope": if whole_chain { "organization" } else { "project" },
            "events": events,
        }))
    }
}

impl AppState {
    /// The person acting on a project, from the tables: their role on it,
    /// and their row on the roster of the organization that owns it now —
    /// the owner's included, so a module's `require_owner` follows a transfer.
    async fn on_project(
        &self,
        user_id: UserId,
        project_id: ProjectId,
        session_id: Option<String>,
        expires_at: i64,
    ) -> Result<Acting, TelmoniError> {
        let acting = acting_project(self, &project_id, &user_id).await?;
        let organization_id = acting.organization.clone();
        let role = acting.role;
        let organization_role: Option<OrganizationRole> = acting.organization_role;
        acting.tx.rollback().await?;
        Ok(Acting {
            user_id,
            organization_id,
            organization_role,
            project: Some(ActingProject { project_id, role }),
            session_id,
            expires_at,
        })
    }
}

/// The most events one audit tool call returns.
const AUDIT_TOOL_MAX: i64 = 50;

/// An event's details without the address an invitation or an email change
/// records: every one of them is a top-level `email`.
fn without_address(metadata: Option<serde_json::Value>) -> Option<serde_json::Value> {
    metadata.map(|mut details| {
        if let Some(fields) = details.as_object_mut() {
            fields.remove("email");
        }
        details
    })
}

/// An event as the agent indexes it: who did what to which resource, where
/// and when, and the details the audit log page shows. A project's events
/// are read by the roles that read the audit log there; the organization's
/// own by its owner and admins, who read its whole chain.
///
/// ⚠ **No address reaches the index.** An invitation's or an email change's
/// details hold one, and the chain keeps it as long as it keeps the event;
/// the index's copy, read again whenever an erasure ends a page's lease,
/// would put an erased person's address back after the agent scrubbed it.
fn audit_document(e: audit::IndexedEvent) -> SourceDocument {
    let target = match &e.resource_id {
        Some(id) => format!("{} {id}", e.resource_kind),
        None => e.resource_kind.clone(),
    };
    let place = match &e.in_project {
        Some(project) => format!("project {project}"),
        None => "the organization".to_owned(),
    };
    let mut body = format!(
        "Audit event: {action} {target}, by {actor}, in {place}, at {at}.",
        action = e.action,
        actor = e.actor_id,
        at = e.created_at.to_rfc3339(),
    );
    if let Some(metadata) = without_address(e.metadata).filter(|m| !m.is_null()) {
        body.push_str("\nDetails: ");
        body.push_str(&metadata.to_string());
    }
    // By slug, as every link a person is shown: a URL changed on Settings
    // afterwards leaves this citation behind, which is the choice made for
    // every link (console.md, "Paths and slugs"). An event of a project since
    // deleted cites the organization's log, where the event still is.
    let (audience, url) = match (&e.in_project, &e.project_slug) {
        (Some(_), Some(project)) => (
            Audience::Audit,
            format!("/{}/{project}/audit-log", e.organization_slug),
        ),
        (Some(_), None) => (
            Audience::Audit,
            format!("/{}/~/audit-log", e.organization_slug),
        ),
        (None, _) => (
            Audience::OrganizationAdmin,
            format!("/{}/~/audit-log", e.organization_slug),
        ),
    };
    SourceDocument {
        source_id: e.id.to_string(),
        organization_id: e.organization_id,
        project_id: e.in_project,
        subject_user_id: None,
        audience,
        title: format!("{} {}", e.action, e.resource_kind),
        body,
        url,
        created_at: e.created_at,
        cursor: DocumentCursor {
            at: e.created_at,
            id: e.id.to_string(),
        },
    }
}
