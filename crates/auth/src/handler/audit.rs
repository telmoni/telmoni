//! `/internal/audit/…` — the console's two audit lists, and the
//! organization's chain to take away, as exports built in the background
//! ([`crate::audit_export`]). The nightly walk of every chain is
//! [`crate::sweep::audit_verify`].

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use axum::http::HeaderMap;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{PersonAndOrganization, Scoped, organization_scope};
use telmoni_shared::error::TelmoniError;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::person_token::Principal;
use telmoni_shared::rbac::{Resource, Verb, can};
use telmoni_shared::{AuditAction, AuthError, AuthzError, OrganizationId, TelmoniResourceKind};

use crate::AppState;
use crate::audit_export::{self, ExportFormat, MAX_KEPT, MAX_UNFINISHED};
use crate::db::audit::{self, AuditQuery, AuditRow};
use crate::db::{audit_exports, locks};

/// The page's default, and the ceiling a caller may ask for: a page that asks
/// for everything times out on exactly the history worth reading.
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 1000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    actor: Option<String>,
    /// ⚠️ **The two closed vocabularies arrive as their enums, not strings.**
    /// A typo matched zero rows, and this endpoint's zero-row answer is
    /// "nothing has happened in your organization". Now it is a 400.
    action: Option<AuditAction>,
    resource_kind: Option<TelmoniResourceKind>,
    cursor: Option<Uuid>,
    limit: Option<i64>,
}

/// `GET /internal/audit/projects/{project_id}` — the events written inside one
/// project, newest first. A project has no chain of its own, so this reads the
/// ORGANIZATION's rows bearing that project.
pub async fn list_project_audit(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Query(params): Query<ListParams>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = super::parse_project_id(&project_id)?;
    let user_id = principal.user_id;

    let acting = super::acting_project(&state, &project_id, &user_id).await?;
    let organization = acting.organization.clone();
    let tx = acting.tx;
    if !can(acting.role, Verb::Read, Resource::Audit) {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(format!(
            "role {} may not read the audit log",
            acting.role
        ))
        .into());
    }

    let mut tx = tx.bind_organization(&organization).await?;

    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let events = audit::list(
        &mut tx,
        &AuditQuery {
            organization_id: &organization,
            in_project: Some(&project_id),
            from: params.from,
            to: params.to,
            actor: params.actor.as_deref(),
            action: params.action,
            resource_kind: params.resource_kind,
            cursor: params.cursor,
            limit,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(page(events, limit))
}

/// `GET /internal/audit/organizations/{organization_id}` — the organization's
/// whole chain, newest first. **Owners and Admins**: it is the
/// record of the whole collaboration.
pub async fn list_organization_audit(
    State(state): State<Arc<AppState>>,
    Path(organization_id): Path<String>,
    Query(params): Query<ListParams>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization_id = super::parse_organization_id(&organization_id)?;
    if organization_id != super::organization_of(&headers)? {
        return Err(AuthzError::Forbidden("not the acting organization".to_string()).into());
    }

    let mut tx = organization_scope(&state.db, &organization_id).await?;
    let role = super::organization_role_or_forbidden(&mut tx, &organization_id, &principal.user_id)
        .await?;
    if !role.can_view_rolled_up_audit() {
        return Err(AuthzError::Forbidden(
            "only an organization owner or admin reads its whole audit log".to_string(),
        )
        .into());
    }
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let events = audit::list(
        &mut tx,
        &AuditQuery {
            organization_id: &organization_id,
            in_project: None,
            from: params.from,
            to: params.to,
            actor: params.actor.as_deref(),
            action: params.action,
            resource_kind: params.resource_kind,
            cursor: params.cursor,
            limit,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(page(events, limit))
}

/// What `POST …/exports` asks for: a format, and a range — `from` absent for
/// the chain's start, `to` absent for now.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartExport {
    format: ExportFormat,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

/// How many of the caller's exports the list answers, newest first.
const EXPORTS_LISTED: i64 = 10;

/// The door the list and the download share: [`export_scope`], and the caller
/// the organization's owner or an admin ([`refuse_unless_exporter`]).
async fn exporter<'a>(
    state: &'a AppState,
    organization_id: &str,
    principal: &Principal,
    headers: &HeaderMap,
) -> Result<(Scoped<'a, PersonAndOrganization>, OrganizationId), TelmoniError> {
    let (mut tx, organization_id) =
        export_scope(state, organization_id, principal, headers).await?;
    refuse_unless_exporter(&mut tx, &organization_id, principal).await?;
    Ok((tx, organization_id))
}

/// The organization the path names, the acting one, with its scope open and
/// the caller bound in it — the only scope the table's policy shows one of
/// their exports to.
async fn export_scope<'a>(
    state: &'a AppState,
    organization_id: &str,
    principal: &Principal,
    headers: &HeaderMap,
) -> Result<(Scoped<'a, PersonAndOrganization>, OrganizationId), TelmoniError> {
    let organization_id = super::parse_organization_id(organization_id)?;
    if organization_id != super::organization_of(headers)? {
        return Err(AuthzError::Forbidden("not the acting organization".to_string()).into());
    }
    let tx = organization_scope(&state.db, &organization_id)
        .await?
        .bind_person(&principal.user_id)
        .await?;
    Ok((tx, organization_id))
}

/// Anybody but the organization's owner or an admin, who read the whole log
/// already, is refused.
async fn refuse_unless_exporter(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    principal: &Principal,
) -> Result<(), TelmoniError> {
    let role =
        super::organization_role_or_forbidden(tx, organization_id, &principal.user_id).await?;
    if !role.can_view_rolled_up_audit() {
        return Err(AuthzError::Forbidden(
            "only an organization owner or admin exports its audit log".to_string(),
        )
        .into());
    }
    Ok(())
}

/// `POST /internal/audit/organizations/{organization_id}/exports` — queue an
/// export of the organization's chain and start building it: 202 with the
/// export as the list shows it. The file holds the stretch of the chain the
/// range touches, oldest first, each row with every field its hash covers and
/// both hashes, so whoever holds it recomputes every `row_hash` and walks
/// every `prev_hash` without trusting us. The range ends at the request at
/// the latest, and a file never holds the record of its own export, which is
/// written once the file is. At most
/// [`MAX_UNFINISHED`] of a person's exports build at once, and a new one
/// deletes their oldest finished one past [`MAX_KEPT`].
pub async fn start_audit_export(
    State(state): State<Arc<AppState>>,
    Path(organization_id): Path<String>,
    principal: Principal,
    headers: HeaderMap,
    Json(body): Json<StartExport>,
) -> Result<impl IntoResponse, TelmoniError> {
    let (mut tx, organization_id) =
        export_scope(&state, &organization_id, &principal, &headers).await?;
    // The person's lock before their role is read, as `locks` asks: an
    // erasure holds it while it takes their seats and their exports, so a
    // start it overlaps reads them gone rather than queueing an export for
    // nobody; and two starts at once cannot both count two building and both
    // go ahead.
    locks::lock_person(&mut tx, &principal.user_id).await?;
    refuse_unless_exporter(&mut tx, &organization_id, &principal).await?;
    let now = Utc::now();
    let to = body.to.map_or(now, |to| to.min(now));
    if body.from.is_some_and(|from| from >= to) {
        return Err(AuthError::BadRequest(
            "an export's range must start before it ends, and before now".into(),
        )
        .into());
    }
    if audit_exports::unfinished_count(&mut tx, &organization_id, &principal.user_id).await?
        >= MAX_UNFINISHED
    {
        return Err(AuthError::Conflict(format!(
            "{MAX_UNFINISHED} of your exports are still being built; one must finish first"
        ))
        .into());
    }
    audit_exports::keep_newest(&mut tx, &organization_id, &principal.user_id, MAX_KEPT - 1).await?;
    let entry = audit_exports::create(
        &mut tx,
        Uuid::now_v7(),
        &organization_id,
        &principal.user_id,
        body.format.as_str(),
        body.from,
        to,
    )
    .await?;
    tx.commit().await?;

    audit_export::spawn_build(
        Arc::clone(&state),
        entry.id,
        organization_id,
        principal.user_id,
    );
    Ok((StatusCode::ACCEPTED, Json(entry)))
}

/// `GET /internal/audit/organizations/{organization_id}/exports` — the
/// caller's latest exports in the organization, newest first, without their
/// files: what the console's bell and export dialog show.
pub async fn list_audit_exports(
    State(state): State<Arc<AppState>>,
    Path(organization_id): Path<String>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let (mut tx, organization_id) =
        exporter(&state, &organization_id, &principal, &headers).await?;
    let exports = audit_exports::list(
        &mut tx,
        &organization_id,
        &principal.user_id,
        EXPORTS_LISTED,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "exports": exports })))
}

/// `GET /internal/audit/organizations/{organization_id}/exports/{export_id}`
/// — a finished file, in the format it was built in: for the person who
/// asked for it alone, while they still hold the role, and while it is kept.
/// The first download is noted, so the bell stops offering it.
pub async fn download_audit_export(
    State(state): State<Arc<AppState>>,
    Path((organization_id, export_id)): Path<(String, Uuid)>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let (mut tx, organization_id) =
        exporter(&state, &organization_id, &principal, &headers).await?;
    let Some(file) =
        audit_exports::file_of(&mut tx, export_id, &organization_id, &principal.user_id).await?
    else {
        return Err(AuthError::NotFound("no such export, or no longer kept".into()).into());
    };
    audit_exports::mark_downloaded(&mut tx, export_id, &organization_id, &principal.user_id)
        .await?;
    tx.commit().await?;

    let format = ExportFormat::parse(&file.format).ok_or_else(|| {
        TelmoniError::Internal(format!("export {export_id} names no known format"))
    })?;
    Ok(([(header::CONTENT_TYPE, format.content_type())], file.file))
}

/// One page of a chain, as both lists answer it.
fn page(events: Vec<AuditRow>, limit: i64) -> Json<serde_json::Value> {
    let next_cursor = events
        .last()
        .filter(|_| i64::try_from(events.len()).is_ok_and(|n| n == limit))
        .map(|r| r.id);
    Json(json!({ "events": events, "next_cursor": next_cursor }))
}
