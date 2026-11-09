//! A project's agents' changes ([`crate::changes`]): `GET
//! /internal/v1/telemetry/changes`, which anyone on the project reads, as the
//! trace list's charts mark them, and `/v1/changes` behind a key holding
//! `read`; each the newest first over a window held to the plan's days,
//! narrowed by agent and environment as the trace list is.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use serde::Serialize;

use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::{ProjectId, TelmoniError};

use crate::AppState;
use crate::changes::{self, Change};
use crate::contract::{AGENT_MAX, ENVIRONMENT_MAX};
use crate::handler::traces::{parse_days, reader, values};

/// What the list of changes answers.
#[derive(Debug, Serialize)]
pub struct ChangeList {
    /// The days the window reads, held to the plan's.
    pub days: u16,
    /// Whether the window held more than [`changes::CHANGES_MAX`].
    pub more: bool,
    /// The newest first.
    pub changes: Vec<Change>,
}

/// `GET /internal/v1/telemetry/changes` — anyone on the project reads it.
pub async fn list_changes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<ChangeList>, TelmoniError> {
    let reader = reader(&state, &headers).await?;
    Ok(Json(change_list(&state, &reader.project_id, pairs).await?))
}

/// The project's changes, as the console's lane and `/v1` answer them:
/// `days` once, and `agent` and `environment` each as often as wanted.
pub(crate) async fn change_list(
    state: &AppState,
    project_id: &ProjectId,
    pairs: Vec<(String, String)>,
) -> Result<ChangeList, TelmoniError> {
    let (mut days, mut agents, mut environments) = (None, Vec::new(), Vec::new());
    for (name, value) in pairs {
        match name.as_str() {
            "agent" => agents.push(value),
            "environment" => environments.push(value),
            "days" if days.is_none() => days = Some(value),
            "days" => {
                return Err(TelmoniError::BadRequest(
                    "days is given more than once".into(),
                ));
            }
            _ => {
                return Err(TelmoniError::BadRequest(
                    "the lane takes no parameter by that name".into(),
                ));
            }
        }
    }
    let asked = parse_days(days.as_deref())?;
    let agents = values("agent", &agents, AGENT_MAX)?;
    let environments = values("environment", &environments, ENVIRONMENT_MAX)?;
    let days = crate::capacity::visible_days(state, project_id, asked).await;
    let mut tx = project_scope(&state.db, project_id).await?;
    let (changes, more) = changes::list(&mut tx, project_id, days, &agents, &environments).await?;
    tx.commit().await?;
    Ok(ChangeList {
        days,
        more,
        changes,
    })
}
