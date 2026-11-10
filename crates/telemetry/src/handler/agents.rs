//! `GET /internal/v1/telemetry/agents` — each of the project's agents as its
//! Agents page reads it: its traces over the last [`AGENTS_DAYS`] days, those
//! that failed, its last trace, what that trace cost against what its ended
//! traces usually cost, and its newest change of each kind ([`crate::changes`]).

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::{SpanStatus, TelmoniError};

use crate::AppState;
use crate::changes::Change;
use crate::handler::traces::{read_failed, reader, window_prices};
use crate::store::{Projects, text};
// The trace's sums by their own names, which `traces!` expands to.
use crate::traces::{
    TraceFilter, admits, filtered, trace_carries, trace_status, trace_sums, window,
};

/// The days an agent's traces are read over: as many as the history an
/// alert's threshold starts from (`handler/history.rs`).
pub const AGENTS_DAYS: u16 = 14;

/// The most agents one answer holds.
pub const AGENTS_MAX: u32 = 200;

/// The window's traces, a row each as the trace list sums them, folded by
/// agent, the newest last trace first.
const AGENTS_SQL: &str = concat!(
    "SELECT trace_agent AS agent, count() AS traces, ",
    "countIf(trace_status = 'error') AS failed_traces, ",
    "argMax(trace_id, first_started_at) AS last_trace_id, ",
    "max(first_started_at) AS last_started_at, ",
    "argMax(trace_ended, first_started_at) AS last_ended, ",
    "argMax(trace_status, first_started_at) AS last_status, ",
    "argMax(spent, first_started_at) AS last_spend, ",
    "if(countIf(trace_ended) > 0, quantileExactIf(0.5)(spent, trace_ended), NULL) AS usual_spend ",
    "FROM (",
    crate::traces::traces!(),
    ") GROUP BY agent ORDER BY last_started_at DESC, agent LIMIT {max:UInt32}"
);

/// One agent over the window.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct AgentRow {
    pub agent: String,
    /// Its traces that started in the window, those still running among them.
    pub traces: u64,
    /// Those that ended in error.
    pub failed_traces: u64,
    /// The one that started last.
    pub last_trace_id: String,
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub last_started_at: DateTime<Utc>,
    /// Whether the agent's own span has arrived.
    pub last_ended: bool,
    #[serde(deserialize_with = "text::deserialize")]
    pub last_status: SpanStatus,
    /// What it cost in US dollars, as the trace list prices a trace.
    pub last_spend: f64,
    /// The median of what its ended traces cost; none before one has ended.
    pub usual_spend: Option<f64>,
}

/// The project's agents over the window.
#[derive(Debug, Serialize)]
pub struct Agents {
    pub days: u16,
    pub agents: Vec<AgentRow>,
    /// Each listed agent's newest change of each kind in the window.
    pub changes: Vec<Change>,
}

/// `GET /internal/v1/telemetry/agents` — anyone on the project reads it.
pub async fn get_agents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Agents>, TelmoniError> {
    let reader = reader(&state, &headers).await?;
    let filter = TraceFilter {
        days: AGENTS_DAYS,
        prices: window_prices(&state, &reader.project_id, AGENTS_DAYS).await,
        ..TraceFilter::default()
    };
    let agents = filtered(
        state
            .store
            .read(&Projects::one(reader.project_id.clone()), AGENTS_SQL),
        &filter,
    )
    .param("max", AGENTS_MAX)
    .fetch_all()
    .await
    .map_err(read_failed)?;
    let mut tx = project_scope(&state.db, &reader.project_id).await?;
    let mut changes = crate::changes::newest(&mut tx, &reader.project_id, AGENTS_DAYS).await?;
    tx.commit().await?;
    changes.retain(|change| {
        agents
            .iter()
            .any(|row: &AgentRow| row.agent == change.agent)
    });
    Ok(Json(Agents {
        days: AGENTS_DAYS,
        agents,
        changes,
    }))
}
