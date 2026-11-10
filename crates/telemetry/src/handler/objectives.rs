//! `GET /internal/v1/telemetry/objectives` — where each of the project's
//! objectives stands: its burn over the last hour and six hours, as its
//! alert reads them ([`crate::rules::evaluate::burns`]), and its error budget
//! over its days, or as many as the plan keeps of the spans. An objective is
//! made, edited and deleted as the alert it is, on the alerts' lanes.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::TelmoniError;
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};

use crate::AppState;
use crate::handler::traces::read_failed;
use crate::rules::db as rules;
use crate::rules::evaluate::{self, Missed, Window};

/// One objective's standing.
#[derive(Debug, Serialize)]
pub struct Standing {
    /// Its alert's id.
    pub rule_id: Uuid,
    pub hour_burn: f64,
    pub six_hour_burn: f64,
    /// The days its budget is read over: its own, or fewer where the plan
    /// keeps fewer of the spans.
    pub days: u16,
    pub traces: u64,
    /// Its traces that missed it: failed, or ran past its latency.
    pub missed: u64,
    /// The share of its budget left, below 0 once spent past it.
    pub budget_left: f64,
}

/// What the lane answers.
#[derive(Debug, Serialize)]
pub struct Objectives {
    pub objectives: Vec<Standing>,
}

/// `GET /internal/v1/telemetry/objectives` — any member's, as its alerts are.
pub async fn list_objectives(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Objectives>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = rules::rules(&mut tx, project_id).await?;
    tx.commit().await?;
    let longest = crate::rules::OBJECTIVE_DAYS
        .iter()
        .copied()
        .max()
        .unwrap_or(30);
    let kept = crate::capacity::visible_days(&state, project_id, longest).await;
    let now = Utc::now();
    let mut objectives = Vec::new();
    for row in rows {
        let Some(objective) = row.definition.objective else {
            continue;
        };
        let (hour_burn, six_hour_burn) = evaluate::burns(&state, &row, &objective, now)
            .await
            .map_err(read_failed)?;
        let days = objective.days.min(kept);
        let agent = row
            .definition
            .filters
            .agent
            .first()
            .map_or("", String::as_str);
        let period = Window::ending(now, Duration::from_secs(u64::from(days) * 86_400));
        let Missed { traces, missed } =
            evaluate::missed(&state, project_id, agent, &objective, period)
                .await
                .map_err(read_failed)?;
        objectives.push(Standing {
            rule_id: row.id,
            hour_burn,
            six_hour_burn,
            days,
            traces,
            missed,
            // A burn of 1 over the whole period spends exactly its budget.
            budget_left: 1.0 - objective.burn(traces, missed),
        });
    }
    Ok(Json(Objectives { objectives }))
}
