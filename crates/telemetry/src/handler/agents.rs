//! `GET /internal/v1/telemetry/agents` — each of the project's agents as its
//! Agents page reads it, or the one `agent` names, as its own page does: its
//! traces over the window, those that failed and why, what they cost, its
//! last trace and what that cost against what its ended traces usually
//! cost, the models and prompt versions its traces carried, and its newest
//! release and first tool calls ([`crate::changes`]).

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize, Serializer};

use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::{ChangeKind, ProjectId, SpanStatus, TelmoniError};

use crate::AppState;
use crate::changes::Change;
use crate::contract::AGENT_MAX;
use crate::handler::traces::{parse_days_or, read_failed, reader, window_prices};
use crate::store::{Projects, text};
// The trace's sums by their own names, which `traces!` expands to.
use crate::traces::{
    TraceFilter, filtered, trace_carries, trace_status, trace_sums, traces_by_fault, window,
};

/// The days an agent's traces are read over where the query names none: as
/// many as the history an alert's threshold starts from
/// (`handler/history.rs`).
pub const AGENTS_DAYS: u16 = 14;

/// The most agents one answer holds.
pub const AGENTS_MAX: u32 = 200;

/// An array column's values over an agent's traces, by how many traces
/// carried each, the most first, ties by name.
macro_rules! most_traces_first {
    ($column:literal) => {
        concat!(
            "arrayMap(c -> c.1, arraySort(c -> (-toInt64(c.2), c.1), arrayZip((sumMap(",
            $column,
            ", arrayMap(x -> toUInt64(1), ",
            $column,
            ")) AS ",
            $column,
            "_counts).1, ",
            $column,
            "_counts.2))) AS ",
            $column
        )
    };
}

/// The window's traces, a row each as the trace list sums them, folded by
/// agent, the newest last trace first.
const AGENTS_SQL: &str = concat!(
    "SELECT trace_agent AS agent, count() AS traces, ",
    "countIf(trace_status = 'error') AS failed_traces, ",
    traces_by_fault!(),
    " AS faults, ",
    "sum(spent) AS spend, ",
    "argMax(trace_id, first_started_at) AS last_trace_id, ",
    "max(first_started_at) AS last_started_at, ",
    "argMax(trace_ended, first_started_at) AS last_ended, ",
    "argMax(trace_status, first_started_at) AS last_status, ",
    "argMax(spent, first_started_at) AS last_spend, ",
    "if(countIf(trace_ended) > 0, quantileExactIf(0.5)(spent, trace_ended), NULL) AS usual_spend, ",
    most_traces_first!("models"),
    ", ",
    most_traces_first!("prompt_versions"),
    " FROM (",
    crate::traces::traces!(),
    ") GROUP BY agent ORDER BY last_started_at DESC, agent LIMIT {max:UInt32}"
);

/// A map column's pairs as the JSON object the console reads.
fn as_object<S: Serializer>(pairs: &[(String, u64)], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_map(pairs.iter().map(|(key, count)| (key, count)))
}

/// One agent over the window.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct AgentRow {
    pub agent: String,
    /// Its traces that started in the window, those still running among them.
    pub traces: u64,
    /// Those that ended in error.
    pub failed_traces: u64,
    /// How many of its traces carry each fault, a trace under every one of
    /// its own, whether or not it ended in error.
    #[serde(serialize_with = "as_object")]
    pub faults: Vec<(String, u64)>,
    /// What its traces cost in US dollars, each as the trace list prices it.
    pub spend: f64,
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
    /// The models its traces called, the most traces first: an agent may
    /// switch or route, so no one of them is "its" model.
    pub models: Vec<String>,
    /// The prompt versions its traces named, each `name:version`, the most
    /// traces first.
    pub prompt_versions: Vec<String>,
}

/// The project's agents over the window.
#[derive(Debug, Serialize)]
pub struct Agents {
    pub days: u16,
    pub agents: Vec<AgentRow>,
    /// Each listed agent's newest release and first tool call in the window;
    /// `models` and `prompt_versions` answer the rest.
    pub changes: Vec<Change>,
}

/// What the lane was asked: every agent, or one, over the days named.
#[derive(Debug, PartialEq, Eq)]
struct Asked {
    agent: Option<String>,
    days: u16,
}

/// `given`, once: a parameter the lane takes a single value of.
pub(crate) fn once(
    name: &str,
    slot: &mut Option<String>,
    given: String,
) -> Result<(), TelmoniError> {
    if slot.is_some() {
        return Err(TelmoniError::BadRequest(format!(
            "{name} is given more than once"
        )));
    }
    *slot = Some(given);
    Ok(())
}

/// A name of 1 to `max` characters, as it was sent: a name the answer
/// offered is asked for whole, its spaces among it.
pub(crate) fn named(name: &str, given: &str, max: usize) -> Result<(), TelmoniError> {
    if (1..=max).contains(&given.chars().count()) {
        Ok(())
    } else {
        Err(TelmoniError::BadRequest(format!(
            "{name} is 1 to {max} characters"
        )))
    }
}

/// The query read: at most one `agent` and one `days`.
fn asked(pairs: Vec<(String, String)>) -> Result<Asked, TelmoniError> {
    let (mut agent, mut days) = (None, None);
    for (name, value) in pairs {
        match name.as_str() {
            "agent" => once("agent", &mut agent, value)?,
            "days" => once("days", &mut days, value)?,
            _ => {
                return Err(TelmoniError::BadRequest(
                    "the lane takes no parameter by that name".into(),
                ));
            }
        }
    }
    if let Some(agent) = &agent {
        named("agent", agent, AGENT_MAX)?;
    }
    Ok(Asked {
        agent,
        days: parse_days_or(days.as_deref(), AGENTS_DAYS)?,
    })
}

/// The days an agent's rows read: those asked, held to the plan's and to the
/// project's own retention, since neither store keeps a span past either.
pub(crate) async fn window_days(
    state: &AppState,
    project_id: &ProjectId,
    asked: u16,
) -> Result<u16, TelmoniError> {
    let visible = crate::capacity::visible_days(state, project_id, asked).await;
    let mut tx = project_scope(&state.db, project_id).await?;
    let kept = crate::db::retention_days(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(match kept.map(u16::try_from) {
        Some(Ok(kept)) => visible.min(kept),
        _ => visible,
    })
}

/// `GET /internal/v1/telemetry/agents` — anyone on the project reads it.
pub async fn get_agents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Agents>, TelmoniError> {
    let reader = reader(&state, &headers).await?;
    let asked = asked(pairs)?;
    let days = window_days(&state, &reader.project_id, asked.days).await?;
    let filter = TraceFilter {
        agents: asked.agent.into_iter().collect(),
        days,
        prices: window_prices(&state, &reader.project_id, days).await,
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
    let mut changes = crate::changes::newest(&mut tx, &reader.project_id, days).await?;
    tx.commit().await?;
    changes.retain(|change| {
        matches!(change.kind, ChangeKind::Release | ChangeKind::Tool)
            && agents
                .iter()
                .any(|row: &AgentRow| row.agent == change.agent)
    });
    Ok(Json(Agents {
        days,
        agents,
        changes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn refused(query: &[(&str, &str)]) -> bool {
        matches!(asked(pairs(query)), Err(TelmoniError::BadRequest(_)))
    }

    #[test]
    fn every_agent_over_the_pages_days_where_nothing_is_asked() {
        assert_eq!(
            asked(Vec::new()).expect("the default"),
            Asked {
                agent: None,
                days: AGENTS_DAYS
            }
        );
    }

    #[test]
    fn one_agent_is_asked_for_as_it_is_named() {
        for agent in [".", "..", "a/b", "%", " padded ", "agént-日本"] {
            let read = asked(pairs(&[("agent", agent), ("days", "30")])).expect("any text");
            assert_eq!(read.agent.as_deref(), Some(agent));
            assert_eq!(read.days, 30);
        }
        assert!(asked(pairs(&[("agent", &"é".repeat(AGENT_MAX))])).is_ok());
    }

    #[test]
    fn a_value_the_lane_does_not_take_is_refused() {
        assert!(refused(&[("agent", "")]), "an empty name");
        assert!(refused(&[("agent", &"é".repeat(AGENT_MAX + 1))]));
        assert!(refused(&[("agent", "a"), ("agent", "b")]), "two agents");
        assert!(refused(&[("days", "0")]));
        assert!(refused(&[("days", "361")]));
        assert!(refused(&[("days", "7"), ("days", "7")]));
        assert!(refused(&[("limit", "5")]), "a parameter it has none of");
    }

    /// The faults answer as an object by fault, as the histogram's bars do.
    #[test]
    fn an_agents_faults_answer_as_an_object() {
        let row = AgentRow {
            agent: "support-bot".into(),
            traces: 3,
            failed_traces: 1,
            faults: vec![("rate_limited".into(), 2), ("timeout".into(), 1)],
            spend: 0.5,
            last_trace_id: "ab".repeat(16),
            last_started_at: DateTime::from_timestamp_millis(1_791_721_800_000).expect("a time"),
            last_ended: true,
            last_status: SpanStatus::Ok,
            last_spend: 0.25,
            usual_spend: Some(0.125),
            models: Vec::new(),
            prompt_versions: Vec::new(),
        };
        let json = serde_json::to_value(&row).expect("JSON");
        assert_eq!(
            json["faults"],
            serde_json::json!({ "rate_limited": 2, "timeout": 1 })
        );
    }
}
