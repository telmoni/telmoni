//! `GET /internal/auth/sessions` and `…/{id}/revoke` — the Active sessions
//! surface. Rows are written by `/me` (find-or-create on the bearer's `sid`)
//! and touched by the refresh lane; this module only lists and ends them.
//!
//! A session is the PERSON's, not any organization's: both lanes read under
//! `app.user_id` and see only the caller's own rows. Ending one is audited on
//! the organization the console names, after checking the caller is in it.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::person_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{AuditAction, AuthError, TelmoniError, TelmoniResourceKind};

use crate::AppState;

use super::{acting_person_in, log_act_outside_every_organization, recording_organization_of};

/// `GET /internal/auth/sessions` — the caller's live sessions, newest first.
pub async fn list(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let mut tx = person_scope(&state.db, &principal.user_id).await?;
    let sessions = crate::db::sessions::list(&mut tx, &principal.user_id).await?;
    tx.commit().await?;

    Ok(Json(json!({ "sessions": sessions })))
}

/// `POST /internal/auth/sessions/{id}/revoke` — end one of the caller's own
/// sessions.
pub async fn revoke(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = recording_organization_of(&headers)?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| AuthError::BadRequest("session id is not a uuid".into()))?;

    let mut tx = acting_person_in(&state, &principal.user_id, organization.as_ref()).await?;
    let Some(provider_sid) = crate::db::sessions::revoke(&mut tx, &principal.user_id, id).await?
    else {
        tx.commit().await?;
        return Err(AuthError::NotFound("session not found".into()).into());
    };

    let resource_id = id.to_string();
    let metadata = json!({ "revoked_at_provider": provider_sid.is_some() });
    match &organization {
        Some(organization) => {
            emit_audit(
                &mut tx,
                AuditEvent {
                    organization_id: organization,
                    in_project: None,
                    actor: Actor::User(principal.user_id.as_str()),
                    action: AuditAction::Deleted,
                    resource_kind: TelmoniResourceKind::Session,
                    resource_id: Some(&resource_id),
                    request_id: None,
                    ip_address: None,
                    user_agent: None,
                    metadata: Some(metadata),
                },
            )
            .await?;
        }
        None => log_act_outside_every_organization(
            &principal.user_id,
            &format!("session {resource_id} revoked"),
            &metadata,
        ),
    }
    tx.commit().await?;

    if let Some(sid) = provider_sid.as_deref()
        && let Err(e) = state.issuer.revoke_sid(sid).await
    {
        tracing::warn!(error = %e, session = %id,
                "the session's tokens survive its revoke; the row is revoked, which refuses \
                 the device on its next request here");
    }
    Ok(StatusCode::NO_CONTENT)
}
