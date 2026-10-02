//! `GET /internal/organization/export` — everything auth holds about one
//! organization, assembled for its owner or an admin to download.
//!
//! ⚠ **THE ORGANIZATION'S, AND ITS OWNER'S AND ADMINS' ALONE.** There is no
//! personal export: the file carries nothing of the person taking it but
//! what the organization's own records hold of them, as the console shows
//! those — their seat on the roster, and the audit chain's events, whose
//! details can name them — and not their account, their sessions, which
//! span every organization they sign in to, nor their conversations with the
//! agent. The roster, the invitations still waiting, key metadata and the
//! audit chain are the owner's and admins' to take away — they read all of
//! it in the console already — and a member, who reads none of it there, is
//! refused. And the queries use explicit column lists, never `SELECT *`:
//! token and invite hashes and `provider_sid` must never land in a file a
//! customer forwards, and a `*` would leak each the day a column is added.
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
use telmoni_shared::{AuditAction, AuthzError, TelmoniError, TelmoniResourceKind};

use crate::{
    AppState,
    handler::{organization_of, organization_role_or_forbidden},
};

/// The most audit rows one export carries.
const AUDIT_LIMIT: i64 = 10_000;

/// An invitation still waiting on its invitee, as the console lists them.
/// One accepted carries the address of the member it seated — the person
/// taking the export, it may be — and the chain records its history anyway.
const LIVE_INVITE: &str = "accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()";

/// Run one query and return its rows as a JSON array. `sql` is always a
/// literal from this file, or a `format!` of this file's constants
/// (`AUDIT_LIMIT`, `LIVE_INVITE`); the value is always bound.
async fn rows(tx: &mut sqlx::PgConnection, sql: &str, bind: &str) -> Result<Value, TelmoniError> {
    let value: Value = sqlx::query_scalar(&format!(
        "SELECT COALESCE(json_agg(row_to_json(t)), '[]'::json) FROM ({sql}) t"
    ))
    .bind(bind)
    .fetch_one(tx)
    .await?;
    Ok(value)
}

/// `GET /internal/organization/export` — the record of the active
/// organization, for its owner or an admin.
pub async fn export_organization(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    // Both keys: the person's to read the caller's own seat, the
    // organization's for its rows.
    let tx = person_scope(&state.db, &actor).await?;
    let mut tx = tx.bind_organization(&organization).await?;
    if !organization_role_or_forbidden(&mut tx, &organization, &actor)
        .await?
        .can_export_organization()
    {
        return Err(AuthzError::Forbidden(
            "only the organization's owner or an admin can export it".into(),
        )
        .into());
    }

    let organization_row = rows(
        &mut tx,
        "SELECT external_id AS organization_id, slug, name, status::text, created_at \
           FROM auth.organizations WHERE external_id = $1",
        organization.as_str(),
    )
    .await?;

    let projects = rows(
        &mut tx,
        "SELECT external_id AS project_id, slug, name, status::text, created_at, updated_at \
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
        &format!(
            "SELECT email, role::text, invited_by, expires_at, created_at \
               FROM auth.organization_invites \
              WHERE organization_id = $1 AND {LIVE_INVITE} ORDER BY created_at"
        ),
        organization.as_str(),
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
            &format!(
                "SELECT email, role::text, invited_by, expires_at, created_at \
                   FROM auth.member_invites \
                  WHERE project_id = $1 AND {LIVE_INVITE} ORDER BY created_at"
            ),
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

    Ok(Json(json!({
        "exported_at": chrono::Utc::now(),
        "organization": organization_row,
        "projects": projects,
        "project_detail": project_detail,
        "organization_members": organization_members,
        "organization_invites": organization_invites,
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
