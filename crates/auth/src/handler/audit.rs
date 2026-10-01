//! `GET /internal/audit/…` — the console's two audit lists. The nightly
//! walk of every chain is [`crate::sweep::audit_verify`].

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use axum::http::HeaderMap;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::organization_scope;
use telmoni_shared::error::TelmoniError;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::person_token::Principal;
use telmoni_shared::rbac::{Resource, Verb, can};
use telmoni_shared::{AuditAction, AuthzError, TelmoniResourceKind};

use crate::AppState;
use crate::db::audit::{self, AuditQuery, AuditRow};

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

/// One page of a chain, as both lists answer it.
fn page(events: Vec<AuditRow>, limit: i64) -> Json<serde_json::Value> {
    let next_cursor = events
        .last()
        .filter(|_| i64::try_from(events.len()).is_ok_and(|n| n == limit))
        .map(|r| r.id);
    Json(json!({ "events": events, "next_cursor": next_cursor }))
}
