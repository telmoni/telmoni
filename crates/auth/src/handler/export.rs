//! `GET /internal/organization/export` — everything auth holds about one
//! organization, and about the person taking it, assembled for them to
//! download.
//!
//! ⚠ **THE OWNER'S ALONE.** The organization's roster, invitations and key
//! metadata are its owner's to take away, and the person half is only ever
//! the caller's own. And the queries use explicit column lists, never
//! `SELECT *`: token and invite hashes and `provider_sid` must never land in a
//! file a customer forwards, and a `*` would leak each the day a column is
//! added.
//!
//! ⚠ **EVERY QUERY NAMES ITS TENANT, NOT JUST RLS.** The policies are
//! permissive and OR together: `auth.projects`' `project_member_read` admits
//! the projects of *other* organizations the caller is a member of, so an
//! export that trusted RLS alone listed them, then walked each and wrote that
//! organization's roster, pending invite addresses and API-key metadata into
//! this one's file. RLS is the floor; the `WHERE` says what the export means.

use std::sync::Arc;

use axum::{extract::State, response::IntoResponse};
use serde_json::{Value, json};

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::person_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{
    AuditAction, AuthzError, OrganizationRole, TelmoniError, TelmoniResourceKind,
};

use crate::{
    AppState,
    handler::{organization_of, organization_role_or_forbidden},
};

/// The most audit rows one export carries.
const AUDIT_LIMIT: i64 = 10_000;

/// Run one query and return its rows as a JSON array. `sql` is always a
/// literal from this file (or a `format!` of constants, which
/// `sql_is_parameterised.rs` scans too); the value is always bound.
async fn rows(tx: &mut sqlx::PgConnection, sql: &str, bind: &str) -> Result<Value, TelmoniError> {
    let value: Value = sqlx::query_scalar(&format!(
        "SELECT COALESCE(json_agg(row_to_json(t)), '[]'::json) FROM ({sql}) t"
    ))
    .bind(bind)
    .fetch_one(tx)
    .await?;
    Ok(value)
}

/// `GET /internal/organization/export` — the owner's record of the active
/// organization, and of themselves.
pub async fn export_organization(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    // Both keys: the person's for their own rows, the organization's for its.
    let tx = person_scope(&state.db, &actor).await?;
    let mut tx = tx.bind_organization(&organization).await?;
    if organization_role_or_forbidden(&mut tx, &organization, &actor).await?
        != OrganizationRole::Owner
    {
        return Err(
            AuthzError::Forbidden("only the organization's owner can export it".into()).into(),
        );
    }

    let organization_row = rows(
        &mut tx,
        "SELECT external_id AS organization_id, name, status::text, created_at \
           FROM auth.organizations WHERE external_id = $1",
        organization.as_str(),
    )
    .await?;
    let person = rows(
        &mut tx,
        "SELECT i.user_id, i.email, i.display_name, \
                COALESCE(a.analytics_opt_in, false) AS analytics_opt_in, i.updated_at \
           FROM auth.identities i LEFT JOIN auth.accounts a ON a.user_id = i.user_id \
          WHERE i.user_id = $1",
        actor.as_str(),
    )
    .await?;

    let projects = rows(
        &mut tx,
        "SELECT external_id AS project_id, name, status::text, created_at, updated_at \
           FROM auth.projects WHERE organization_id = $1 ORDER BY created_at",
        organization.as_str(),
    )
    .await?;
    let organization_members = rows(
        &mut tx,
        "SELECT user_id AS member_id, role::text, added_by, created_at \
           FROM auth.organization_members WHERE organization_id = $1 ORDER BY created_at",
        organization.as_str(),
    )
    .await?;
    let organization_invites = rows(
        &mut tx,
        "SELECT email, role::text, invited_by, expires_at, accepted_at, accepted_by, \
                revoked_at, created_at \
           FROM auth.organization_invites WHERE organization_id = $1 ORDER BY created_at",
        organization.as_str(),
    )
    .await?;
    let sessions = rows(
        &mut tx,
        "SELECT user_agent, created_at, last_seen_at, revoked_at \
           FROM auth.sessions WHERE user_id = $1 ORDER BY created_at",
        actor.as_str(),
    )
    .await?;

    let audit_events = rows(
        &mut tx,
        &format!(
            "SELECT created_at, actor_id, action, resource_kind, resource_id, \
                    in_project, metadata \
               FROM audit.events WHERE organization_id = $1 \
              ORDER BY id DESC LIMIT {AUDIT_LIMIT}"
        ),
        organization.as_str(),
    )
    .await?;
    let audit_truncated = audit_events
        .as_array()
        .is_some_and(|a| i64::try_from(a.len()).unwrap_or(i64::MAX) >= AUDIT_LIMIT);

    let mut project_detail = Vec::new();
    for project in projects.as_array().into_iter().flatten() {
        let Some(project_id) = project.get("project_id").and_then(Value::as_str) else {
            continue;
        };
        let parsed = super::parse_project_id(project_id)?;
        let mut ptx = tx.bind_project(&parsed).await?;

        let members = rows(
            &mut ptx,
            "SELECT user_id AS member_id, role::text, added_by, created_at \
               FROM auth.project_members WHERE project_id = $1 ORDER BY created_at",
            project_id,
        )
        .await?;
        let invites = rows(
            &mut ptx,
            "SELECT email, role::text, invited_by, expires_at, accepted_at, accepted_by, \
                    revoked_at, created_at \
               FROM auth.member_invites WHERE project_id = $1 ORDER BY created_at",
            project_id,
        )
        .await?;
        let api_keys = rows(
            &mut ptx,
            "SELECT name, description, created_by, expires_at, last_used_at, revoked_at, \
                    created_at \
               FROM auth.api_tokens WHERE project_id = $1 ORDER BY created_at",
            project_id,
        )
        .await?;
        tx = ptx.clear_project().await?;

        project_detail.push(json!({
            "project_id": project_id,
            "members": members,
            "invites": invites,
            "api_keys": api_keys,
        }));
    }

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Exported,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "export": true, "audit_truncated": audit_truncated })),
        },
    )
    .await?;

    tx.commit().await?;

    // The owner's own conversations with the agent in this organization.
    // After the commit, so the export's audit row stands whatever the agent
    // answers; an agent that fails fails the export, rather than hand back
    // a file that silently leaves them out.
    let agent_conversations = match state.siblings.agent.as_ref() {
        Some(agent) => agent.export_person(&actor, &organization).await?,
        None => json!([]),
    };

    Ok(Json(json!({
        "exported_at": chrono::Utc::now(),
        "agent_conversations": agent_conversations,
        "organization": organization_row,
        "person": person,
        "projects": projects,
        "project_detail": project_detail,
        "organization_members": organization_members,
        "organization_invites": organization_invites,
        "sessions": sessions,
        "audit_events": audit_events,
        "audit_truncated": audit_truncated,
        "audit_limit": AUDIT_LIMIT,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    /// `rows()` binds one parameter unconditionally. Every query here declares
    /// `$1` today; pinned so a query that stops needing it, or a sqlx upgrade
    /// that tightens Bind/Parse, cannot break the export silently.
    #[sqlx::test]
    async fn a_query_with_no_placeholder_survives_the_unconditional_bind(pool: PgPool) {
        let mut conn = pool.acquire().await.expect("acquire");
        let out = rows(&mut conn, "SELECT 1 AS x", "org_unused").await;
        assert!(out.is_ok(), "surplus bind rejected: {:?}", out.err());
    }
}
