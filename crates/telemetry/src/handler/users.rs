//! The console's end-user lane: `POST /internal/v1/telemetry/users/erase`
//! erases one, a project admin's, from the spans of the project
//! `x-project-id` names and no other.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuditAction, ProjectId, Redacted, TelmoniError, TelmoniResourceKind};

use crate::AppState;
use crate::contract::USER_ID_MAX;
use crate::erasure::{self, Erase};
use crate::handler::chain;
use crate::hashing;

/// An end user to erase, by the id the project's SDK sent: never logged,
/// never kept; hashed as ingest hashed it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Erasure {
    /// The id as sent, byte for byte.
    pub user_id: Redacted,
}

/// What an erasure answers: how much of the end user went.
#[derive(Debug, Serialize)]
pub struct Erased {
    /// Their traces, marked deleted.
    pub traces: u64,
    /// Those traces' spans.
    pub spans: u64,
    /// The content fields those spans kept, deleted.
    pub fields: u64,
}

/// `POST /internal/v1/telemetry/users/erase` — one end user erased by a
/// project admin: every trace their id is on, as [`super::traces::delete_trace`]
/// deletes one, the request filed first under the hash, never the id, and
/// the receipt mailed after. An id on no trace is filed and answered too.
pub async fn erase_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(erasure): Json<Erasure>,
) -> Result<Json<Erased>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Delete, Resource::Trace)?
        .project_id;
    let raw = erasure.user_id.expose();
    if raw.is_empty() || raw.chars().count() > USER_ID_MAX {
        return Err(TelmoniError::BadRequest(format!(
            "an end user's id is 1 to {USER_ID_MAX} characters, as their spans carry it"
        )));
    }
    let hash = hashing::storage_for_project(&state, project_id)
        .await?
        .store(raw);
    let found = erasure::find(&state, project_id, Erase::EndUser(&hash)).await?;
    let traces = found.traces.len() as u64;
    chain::write(
        &state.db,
        &acting,
        project_id,
        "end user erasure",
        async |tx| file_erasure(&state, &acting, project_id, tx, &hash, traces).await,
    )
    .await?;
    let spans = found.spans;
    let erase = Erase::EndUser(&hash);
    let fields = erasure::carry_out(&state, project_id, erase, found, &acting.user_id).await?;
    tracing::info!(project_id = %project_id, traces, spans, fields, "telemetry: an end user erased");
    Ok(Json(Erased {
        traces,
        spans,
        fields,
    }))
}

/// The erasure's request, filed under the organization's chain lock by the
/// hash alone, the role checked again on auth's second answer.
async fn file_erasure(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    hash: &str,
    traces: u64,
) -> Result<(), TelmoniError> {
    chain::held(
        state.auth.as_ref(),
        acting,
        tx,
        Verb::Delete,
        Resource::Trace,
    )
    .await?;
    emit_audit(
        tx,
        AuditEvent::new(
            &acting.organization_id,
            Actor::User(acting.user_id.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::EndUser,
        )
        .in_project(project_id)
        .on(hash)
        .details(json!({ "traces": traces })),
    )
    .await?;
    Ok(())
}
