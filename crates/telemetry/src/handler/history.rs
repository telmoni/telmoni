//! `GET /internal/v1/telemetry/history` — what each of the project's agents
//! usually does, for an alert's threshold to start from: plain percentiles,
//! from the hourly totals of the last [`HISTORY_DAYS`] whole UTC days, so the
//! numbers move once a night. Durations are the spans' own percentiles;
//! tokens and spend, which the totals keep only summed, an hour's.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, DurationRound as _, TimeDelta, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{SpanKind, TelmoniError};

use crate::AppState;
use crate::handler::traces::read_failed;
use crate::store::{Projects, seconds};

/// The whole days the percentiles are read over.
pub const HISTORY_DAYS: i64 = 14;

/// The most agents and kinds one answer holds.
const ROWS_MAX: u32 = 4_000;

/// Each agent's spans of each kind, and every agent's together (`every`),
/// by the hour, then the hours' percentiles: durations merged from the
/// totals' digests, and tokens and spend across the hours that held a span.
const HISTORY_SQL: &str = concat!(
    "SELECT agent, every, kind, toUInt64(sum(n)) AS spans, count() AS hours, ",
    "CAST(quantilesTDigestMerge(0.5, 0.9, 0.99)(q) AS Array(Float64)) AS duration_ms, ",
    "CAST(quantiles(0.5, 0.9, 0.99)(t) AS Array(Float64)) AS tokens_hourly, ",
    "CAST(quantiles(0.5, 0.9, 0.99)(t / n) AS Array(Float64)) AS tokens_per_span, ",
    "CAST(quantiles(0.5, 0.9, 0.99)(s) AS Array(Float64)) AS spend_hourly ",
    "FROM (SELECT who.1 AS agent, who.2 AS every, h.kind AS kind, h.hour AS at, ",
    "sum(h.spans) AS n, sum(h.input_tokens) + sum(h.output_tokens) AS t, sum((",
    crate::spend::rollup_spent_of!(),
    ") / h.sample_rate) AS s, ",
    "quantilesTDigestMergeState(0.5, 0.9, 0.99)(h.duration_ms) AS q ",
    "FROM telemetry.spans_hourly AS h ",
    "ARRAY JOIN [(toString(h.agent), toUInt8(0)), ('', toUInt8(1))] AS who ",
    "WHERE h.project_id IN {projects:Array(String)} ",
    "AND h.hour >= {from:DateTime('UTC')} AND h.hour < {to:DateTime('UTC')} ",
    "AND h.kind IN ('run', 'step', 'model_call', 'tool_call') ",
    "GROUP BY agent, every, kind, at) ",
    "WHERE every = 1 OR agent != '' ",
    "GROUP BY agent, every, kind ORDER BY every DESC, agent, kind LIMIT {max:UInt32}"
);

#[derive(Debug, Row, Deserialize)]
struct HistoryRow {
    agent: String,
    every: u8,
    kind: String,
    spans: u64,
    hours: u64,
    duration_ms: Vec<f64>,
    tokens_hourly: Vec<f64>,
    tokens_per_span: Vec<f64>,
    spend_hourly: Vec<f64>,
}

/// The 50th, 90th and 99th percentiles of one measure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Percentiles {
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p99: Option<f64>,
}

impl Percentiles {
    /// Read from `levels` in the SQL's order, none where it gave no number.
    fn of(levels: &[f64]) -> Self {
        let at = |n: usize| levels.get(n).copied().filter(|v| v.is_finite());
        Self {
            p50: at(0),
            p90: at(1),
            p99: at(2),
        }
    }
}

/// What one agent's spans of one kind usually did.
#[derive(Debug, Serialize)]
pub struct AgentHistory {
    /// None for every agent's together.
    pub agent: Option<String>,
    pub kind: SpanKind,
    pub spans: u64,
    /// The hours that held a span, which the hourly percentiles are across.
    pub hours: u64,
    /// Each span's duration, in milliseconds.
    pub duration_ms: Percentiles,
    /// An hour's input and output tokens, summed.
    pub tokens_hourly: Percentiles,
    /// An hour's tokens a span.
    pub tokens_per_span: Percentiles,
    /// What an hour's spans spent, in US dollars, priced as the totals are.
    pub spend_hourly: Percentiles,
}

/// What the lane answers: the days read, and each agent's history.
#[derive(Debug, Serialize)]
pub struct History {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub agents: Vec<AgentHistory>,
}

/// The window read at `now`: the [`HISTORY_DAYS`] whole UTC days before today.
#[must_use]
pub fn window(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let to = now.duration_trunc(TimeDelta::days(1)).unwrap_or(now);
    (to - TimeDelta::days(HISTORY_DAYS), to)
}

/// `GET /internal/v1/telemetry/history` — any member's, as the alerts it
/// serves are.
pub async fn get_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<History>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let (from, to) = window(Utc::now());
    let projects = Projects::one(project_id.clone());
    let book = crate::spend::book_for_rollups(&state, &projects, from)
        .await
        .map_err(|unread| match unread {
            crate::spend::Unread::Store(error) => read_failed(error),
            crate::spend::Unread::Db(error) => error.into(),
        })?;
    let read = state
        .store
        .read(&projects, HISTORY_SQL)
        .param("from", seconds(from))
        .param("to", seconds(to))
        .param("max", ROWS_MAX);
    let rows: Vec<HistoryRow> = crate::spend::priced_by(read, &book)
        .fetch_all()
        .await
        .map_err(read_failed)?;
    let agents = rows
        .into_iter()
        .filter_map(|row| {
            Some(AgentHistory {
                kind: row.kind.parse().ok()?,
                agent: (row.every == 0).then_some(row.agent),
                spans: row.spans,
                hours: row.hours,
                duration_ms: Percentiles::of(&row.duration_ms),
                tokens_hourly: Percentiles::of(&row.tokens_hourly),
                tokens_per_span: Percentiles::of(&row.tokens_per_span),
                spend_hourly: Percentiles::of(&row.spend_hourly),
            })
        })
        .collect();
    Ok(Json(History { from, to, agents }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window is the whole days before today, so it moves at midnight alone.
    #[test]
    fn the_history_is_the_whole_days_before_today() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .expect("an instant")
                .with_timezone(&Utc)
        };
        let (from, to) = window(at("2026-11-04T17:42:10Z"));
        assert_eq!(to, at("2026-11-04T00:00:00Z"));
        assert_eq!(from, at("2026-10-21T00:00:00Z"));
        assert_eq!(window(at("2026-11-04T00:00:01Z")), (from, to));
    }

    #[test]
    fn a_percentile_given_no_number_is_none() {
        let read = Percentiles::of(&[1.0, f64::NAN]);
        assert_eq!(read.p50, Some(1.0));
        assert_eq!((read.p90, read.p99), (None, None));
    }
}
