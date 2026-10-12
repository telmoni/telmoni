//! The console's trace lanes, under `Resource::Trace`, which every role on a
//! project holds: `GET /internal/telemetry/traces`, the list — a row per run
//! in a window of days, filtered and sorted as the URL says — and
//! `GET /internal/telemetry/traces/{run_id}`, one trace's spans in the order
//! they started. Each reads the spans of the project `x-project-id` names and
//! no other: the store names that project on every read, twice.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuthError, SortDirection, SpanKind, SpanStatus, TelemetryError, TelmoniError, TraceSort,
};

use crate::AppState;
use crate::contract::{MODEL_MAX, NAME_MAX, SESSION_ID_MAX, USER_ID_MAX};
use crate::store::{self, Projects, SpanRow};
use crate::traces::{self, DEFAULT_LIMIT, MAX_DAYS, MAX_LIMIT, SPANS_MAX, TraceFilter, TraceRow};

/// The list's query string, every value text until it is checked, so an
/// empty value — a form's untouched field — narrows nothing and a value that
/// is not what its parameter takes is a 400 that echoes none of it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    /// The run's agent.
    #[serde(default)]
    pub agent: String,
    /// The end user, as stored.
    #[serde(default)]
    pub user: String,
    /// The session, by its id.
    #[serde(default)]
    pub session: String,
    /// How the run ended: `ok`, `error` or `cancelled`.
    #[serde(default)]
    pub status: String,
    /// A model one of its calls answered with.
    #[serde(default)]
    pub model: String,
    /// A tool one of its calls ran.
    #[serde(default)]
    pub tool: String,
    /// The column to sort by ([`TraceSort`]).
    #[serde(default)]
    pub sort: String,
    /// Which way ([`SortDirection`]); the column's own default when unset.
    #[serde(default)]
    pub dir: String,
    /// How many days back, 1 to [`MAX_DAYS`].
    #[serde(default)]
    pub days: String,
    /// The most traces to answer, 1 to [`MAX_LIMIT`].
    #[serde(default)]
    pub limit: String,
}

impl ListQuery {
    /// The filter and the sort the query asks for, each value checked.
    ///
    /// # Errors
    ///
    /// A value that is not what its parameter takes, named by the parameter
    /// and never echoed.
    pub fn into_parts(self) -> Result<(TraceFilter, TraceSort, SortDirection), TelmoniError> {
        let text = |name: &str, value: String, max: usize| -> Result<String, TelmoniError> {
            let value = value.trim().to_owned();
            if value.chars().count() > max {
                return Err(TelmoniError::BadRequest(format!(
                    "{name} is longer than {max} characters"
                )));
            }
            Ok(value)
        };
        let status = match self.status.trim() {
            "" => None,
            given => Some(given.parse::<SpanStatus>().map_err(|_| {
                TelmoniError::BadRequest("status is one of ok, error, cancelled".into())
            })?),
        };
        let sort = match self.sort.trim() {
            "" => TraceSort::Started,
            given => given.parse::<TraceSort>().map_err(|_| {
                TelmoniError::BadRequest(
                    "sort is one of started, agent, user, session, status, steps, tokens, \
                     duration"
                        .into(),
                )
            })?,
        };
        let direction = match self.dir.trim() {
            "" => default_direction(sort),
            given => given
                .parse::<SortDirection>()
                .map_err(|_| TelmoniError::BadRequest("dir is asc or desc".into()))?,
        };
        let days = parse_days(&self.days)?;
        let limit = parse_limit(&self.limit)?;
        Ok((
            TraceFilter {
                agent: text("agent", self.agent, NAME_MAX)?,
                user_id: text("user", self.user, USER_ID_MAX)?,
                session_id: text("session", self.session, SESSION_ID_MAX)?,
                status,
                model: text("model", self.model, MODEL_MAX)?,
                tool: text("tool", self.tool, NAME_MAX)?,
                days,
                limit,
            },
            sort,
            direction,
        ))
    }
}

/// The way a column sorts when the URL names none: the newest, the longest
/// and the most first; names in the alphabet's order.
#[must_use]
pub const fn default_direction(sort: TraceSort) -> SortDirection {
    match sort {
        TraceSort::Started | TraceSort::Steps | TraceSort::Tokens | TraceSort::Duration => {
            SortDirection::Desc
        }
        TraceSort::Agent | TraceSort::User | TraceSort::Session | TraceSort::Status => {
            SortDirection::Asc
        }
    }
}

/// `days` as a URL carries it: a whole number from 1 to [`MAX_DAYS`], and
/// the default when empty; every page over the spans reads its window so.
pub(crate) fn parse_days(text: &str) -> Result<u16, TelmoniError> {
    match text.trim() {
        "" => Ok(traces::DEFAULT_DAYS),
        given => given
            .parse::<u16>()
            .ok()
            .filter(|days| (1..=MAX_DAYS).contains(days))
            .ok_or_else(|| {
                TelmoniError::BadRequest(format!("days is a whole number from 1 to {MAX_DAYS}"))
            }),
    }
}

/// `limit` as a URL carries it: a whole number from 1 to [`MAX_LIMIT`], and
/// the default when empty.
pub(crate) fn parse_limit(text: &str) -> Result<u32, TelmoniError> {
    match text.trim() {
        "" => Ok(DEFAULT_LIMIT),
        given => given
            .parse::<u32>()
            .ok()
            .filter(|limit| (1..=MAX_LIMIT).contains(limit))
            .ok_or_else(|| {
                TelmoniError::BadRequest(format!("limit is a whole number from 1 to {MAX_LIMIT}"))
            }),
    }
}

/// What the list answers.
#[derive(Debug, Serialize)]
pub struct TraceList {
    /// The traces, in the order asked.
    pub traces: Vec<Trace>,
    /// How many days back the list read.
    pub days: u16,
    /// The most it would answer.
    pub limit: u32,
    /// Whether the window held more traces than the limit.
    pub more: bool,
    /// The sort it is in.
    pub sort: TraceSort,
    /// And its direction.
    pub dir: SortDirection,
}

/// One trace of the list: a run's spans, summed ([`TraceRow`]).
#[derive(Debug, Serialize)]
pub struct Trace {
    pub run_id: Uuid,
    pub agent: String,
    pub user_id: String,
    pub session_id: String,
    /// Whether the run's own span has arrived.
    pub ended: bool,
    pub status: SpanStatus,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: i64,
    pub spans: u64,
    pub steps: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
    pub errors: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub models: Vec<String>,
    pub tools: Vec<String>,
}

impl From<TraceRow> for Trace {
    fn from(row: TraceRow) -> Self {
        Self {
            run_id: row.run_id,
            agent: row.agent,
            user_id: row.user_id,
            session_id: row.session_id,
            ended: row.ended != 0,
            status: row.status,
            started_at: row.started_at,
            ended_at: row.ended_at,
            duration_ms: row.duration_ms,
            spans: row.spans,
            steps: row.steps,
            model_calls: row.model_calls,
            tool_calls: row.tool_calls,
            errors: row.errors,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_read_input_tokens: row.cache_read_input_tokens,
            cache_write_input_tokens: row.cache_write_input_tokens,
            models: row.models,
            tools: row.tools,
        }
    }
}

/// What one trace's lane answers.
#[derive(Debug, Serialize)]
pub struct TraceDetail {
    pub run_id: Uuid,
    /// Its spans in the order they started, the run's own among them.
    pub spans: Vec<TraceSpan>,
    /// Whether the run holds more spans than the page shows.
    pub truncated: bool,
}

/// One span as the trace page reads it: the row, its maps as maps and its
/// times as text.
#[derive(Debug, Serialize)]
pub struct TraceSpan {
    pub span_id: String,
    pub parent_span_id: String,
    pub trace_id: String,
    pub agent: String,
    pub kind: SpanKind,
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub status: SpanStatus,
    pub error_type: String,
    pub model: String,
    pub provider: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub user_id: String,
    pub session_id: String,
    pub tags: BTreeMap<String, String>,
    pub bound_stall_after_s: Option<u32>,
    pub bound_max_steps: Option<u32>,
    pub bound_max_cost_usd: Option<f64>,
    pub sample_rate: Option<f64>,
    pub attributes: BTreeMap<String, String>,
    pub units: u32,
    pub received_at: DateTime<Utc>,
}

impl From<SpanRow> for TraceSpan {
    fn from(row: SpanRow) -> Self {
        Self {
            span_id: row.span_id,
            parent_span_id: row.parent_span_id,
            trace_id: row.trace_id,
            agent: row.agent,
            kind: row.kind,
            name: row.name,
            started_at: row.started_at,
            ended_at: row.ended_at,
            status: row.status,
            error_type: row.error_type,
            model: row.model,
            provider: row.provider,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_read_input_tokens: row.cache_read_input_tokens,
            cache_write_input_tokens: row.cache_write_input_tokens,
            user_id: row.user_id,
            session_id: row.session_id,
            tags: row.tags.into_iter().collect(),
            bound_stall_after_s: row.bound_stall_after_s,
            bound_max_steps: row.bound_max_steps,
            bound_max_cost_usd: row.bound_max_cost_usd,
            sample_rate: row.sample_rate,
            attributes: row.attributes.into_iter().collect(),
            units: row.units,
            received_at: row.received_at,
        }
    }
}

/// `GET /internal/telemetry/traces` — the project's traces in the window,
/// filtered and sorted as the query says, up to its limit, and whether the
/// window held more.
pub async fn list_traces(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<TraceList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Trace)?
        .project_id
        .clone();
    let (mut filter, sort, dir) = query.into_parts()?;
    // One past the limit, so the answer can say whether there were more.
    let limit = filter.limit;
    filter.limit = limit + 1;
    let rows = traces::list(&state.store, &Projects::one(project_id), &filter, sort, dir)
        .await
        .map_err(read_failed)?;
    let more = rows.len() > limit as usize;
    Ok(Json(TraceList {
        traces: rows
            .into_iter()
            .take(limit as usize)
            .map(Trace::from)
            .collect(),
        days: filter.days,
        limit,
        more,
        sort,
        dir,
    }))
}

/// `GET /internal/telemetry/traces/{run_id}` — one trace's spans, the run's
/// own among them, in the order they started; `404` for a run the project
/// does not hold, which is also what another project's run id gets.
pub async fn get_trace(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<TraceDetail>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Trace)?
        .project_id
        .clone();
    let run_id: Uuid = run_id
        .parse()
        .map_err(|_| TelmoniError::BadRequest("a trace is named by its run's id, a UUID".into()))?;
    let rows = traces::spans(
        &state.store,
        &Projects::one(project_id),
        run_id,
        SPANS_MAX + 1,
    )
    .await
    .map_err(read_failed)?;
    if rows.is_empty() {
        return Err(AuthError::NotFound("no trace with that run id in this project".into()).into());
    }
    let truncated = rows.len() > SPANS_MAX as usize;
    Ok(Json(TraceDetail {
        run_id,
        spans: rows
            .into_iter()
            .take(SPANS_MAX as usize)
            .map(TraceSpan::from)
            .collect(),
        truncated,
    }))
}

/// ClickHouse did not answer a read: an outage the console says so about, or
/// a shape of ours it refused, which retrying will not mend. The users and
/// sessions lanes answer the same way.
pub(crate) fn read_failed(error: clickhouse::error::Error) -> TelmoniError {
    let kind = store::error_kind(&error);
    if kind == "ours" {
        tracing::error!(kind, "traces: ClickHouse refused a read");
        return TelmoniError::Internal("traces: ClickHouse refused a read".into());
    }
    tracing::warn!(kind, "traces: ClickHouse did not answer a read");
    TelemetryError::StoreUnavailable("the span store did not answer; try again".into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(query: ListQuery) -> Result<(TraceFilter, TraceSort, SortDirection), TelmoniError> {
        query.into_parts()
    }

    #[test]
    fn an_empty_query_is_the_default_filter_newest_first() {
        let (filter, sort, dir) = parts(ListQuery::default()).expect("the default");
        assert_eq!(filter, TraceFilter::default());
        assert_eq!(sort, TraceSort::Started);
        assert_eq!(dir, SortDirection::Desc);
    }

    #[test]
    fn every_filter_is_trimmed_and_read_and_a_column_sorts_its_own_way() {
        let (filter, sort, dir) = parts(ListQuery {
            agent: " nightly-report ".into(),
            user: "9f86d081884c7d659a2feaa0c55ad015".into(),
            session: "thread-42".into(),
            status: "error".into(),
            model: "claude-sonnet-5-5".into(),
            tool: "search_docs".into(),
            sort: "user".into(),
            dir: String::new(),
            days: "30".into(),
            limit: "500".into(),
        })
        .expect("a full query");
        assert_eq!(filter.agent, "nightly-report");
        assert_eq!(filter.user_id, "9f86d081884c7d659a2feaa0c55ad015");
        assert_eq!(filter.session_id, "thread-42");
        assert_eq!(filter.status, Some(SpanStatus::Error));
        assert_eq!(filter.model, "claude-sonnet-5-5");
        assert_eq!(filter.tool, "search_docs");
        assert_eq!((filter.days, filter.limit), (30, 500));
        assert_eq!((sort, dir), (TraceSort::User, SortDirection::Asc));

        let (_, sort, dir) = parts(ListQuery {
            sort: "tokens".into(),
            dir: "asc".into(),
            ..ListQuery::default()
        })
        .expect("a sort with its direction");
        assert_eq!((sort, dir), (TraceSort::Tokens, SortDirection::Asc));
        for sort in TraceSort::all() {
            let expected = match sort {
                TraceSort::Started | TraceSort::Steps | TraceSort::Tokens | TraceSort::Duration => {
                    SortDirection::Desc
                }
                _ => SortDirection::Asc,
            };
            assert_eq!(default_direction(sort), expected, "{sort}");
        }
    }

    /// ⚠ A value the parameter does not take is a 400 naming the parameter,
    /// never the value, which a person typed.
    #[test]
    fn a_value_a_parameter_does_not_take_is_refused_without_being_echoed() {
        let bad = [
            ListQuery {
                status: "maybe<script>".into(),
                ..ListQuery::default()
            },
            ListQuery {
                sort: "spend".into(),
                ..ListQuery::default()
            },
            ListQuery {
                dir: "sideways".into(),
                ..ListQuery::default()
            },
            ListQuery {
                days: "0".into(),
                ..ListQuery::default()
            },
            ListQuery {
                days: "361".into(),
                ..ListQuery::default()
            },
            ListQuery {
                days: "7.5".into(),
                ..ListQuery::default()
            },
            ListQuery {
                limit: "0".into(),
                ..ListQuery::default()
            },
            ListQuery {
                limit: "501".into(),
                ..ListQuery::default()
            },
            ListQuery {
                agent: "a".repeat(NAME_MAX + 1),
                ..ListQuery::default()
            },
            ListQuery {
                session: "s".repeat(SESSION_ID_MAX + 1),
                ..ListQuery::default()
            },
        ];
        for query in bad {
            let refused = parts(query).expect_err("taken");
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400);
            let detail = problem.detail.unwrap_or_default();
            assert!(
                !detail.contains("script")
                    && !detail.contains("sideways")
                    && !detail.contains("spend"),
                "the value was echoed: {detail}"
            );
        }
    }
}
