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
use telmoni_shared::seam::{Audience, AuditEventsQuery, Auth, DocumentCursor, SourceDocument};
use telmoni_shared::{
    AuthError, AuthzError, FlagSet, OrganizationId, OrganizationRole, OrganizationStatus,
    TelmoniError,
};

use crate::AppState;
use crate::db::audit::{self, AuditQuery};
use crate::db::{AuthLane, flags, members, organizations};
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
            let acting = acting_project(self, &project_id, &user_id).await?;
            let organization = acting.organization.clone();
            let role = acting.role;
            // The caller's row in the owning organization's roster, which
            // `acting_project` has already read — the owner's included, so a
            // module's `require_owner` follows a transfer.
            let organization_role: Option<OrganizationRole> = acting.organization_role;
            acting.tx.rollback().await?;
            return Ok(Acting {
                user_id,
                organization_id: organization,
                organization_role,
                project: Some(ActingProject { project_id, role }),
                session_id: Some(principal.session_id),
                expires_at: principal.expires_at,
            });
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
        let whole_chain = current.organization_role == Some(OrganizationRole::Owner);
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
                    "metadata": e.metadata,
                })
            })
            .collect();
        Ok(json!({
            "scope": if whole_chain { "organization" } else { "project" },
            "events": events,
        }))
    }
}

/// The most events one audit tool call returns.
const AUDIT_TOOL_MAX: i64 = 50;

/// An event as the agent indexes it: who did what to which resource, where
/// and when, and the details the audit log page shows. A project's events
/// are read by the roles that read the audit log there; the organization's
/// own by its owner, the only person who reads its whole chain.
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
    if let Some(metadata) = e.metadata.as_ref().filter(|m| !m.is_null()) {
        body.push_str("\nDetails: ");
        body.push_str(&metadata.to_string());
    }
    let (audience, url) = match &e.in_project {
        Some(project) => (Audience::Audit, format!("/{project}/audit-log")),
        None => (Audience::Owner, "/organization/audit-log".to_owned()),
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
