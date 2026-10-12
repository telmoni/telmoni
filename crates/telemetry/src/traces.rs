//! The trace list and a trace's spans, read through the one query module
//! ([`crate::store`]). A trace is a run's record: its root span and every
//! span under it, found by `run_id`, which every span of a run carries, and
//! never by the wire's trace id, since a run may sit inside an application's
//! own trace.
//!
//! The list is one aggregation over the project's spans in a window of days,
//! a row per run: a filter that is empty binds as `''` and matches every row,
//! so one literal serves every combination; the sort is a literal chosen in
//! code from the closed set, never text from the request.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::{SortDirection, SpanStatus, TraceSort};

use crate::store::{Projects, SpanRow, Store, text};

/// How many days back the list reads when the request names none.
pub const DEFAULT_DAYS: u16 = 7;
/// The furthest back the list reads: the longest retention sold.
pub const MAX_DAYS: u16 = 360;
/// How many traces the list answers when the request names no limit.
pub const DEFAULT_LIMIT: u32 = 100;
/// The most traces one answer holds.
pub const MAX_LIMIT: u32 = 500;
/// The most spans a trace's page shows; a run past it is shown cut short.
pub const SPANS_MAX: u32 = 5_000;

/// A window of days and a cap on rows: what the users and conversations pages are
/// read by, each one aggregation over the spans of those days and never the
/// whole table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// How many days back, by the day each span arrived.
    pub days: u16,
    /// The most rows to answer.
    pub limit: u32,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            days: DEFAULT_DAYS,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// What narrows the list. A text filter left empty matches every trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceFilter {
    /// The run's agent.
    pub agent: String,
    /// The end user, as stored: hashed, unless the project keeps it plain.
    pub user_id: String,
    /// The conversation.
    pub conversation_id: String,
    /// How the run ended.
    pub status: Option<SpanStatus>,
    /// A model one of its calls answered with.
    pub model: String,
    /// A tool one of its calls ran.
    pub tool: String,
    /// How many days back, by the day each span arrived.
    pub days: u16,
    /// The most traces to answer.
    pub limit: u32,
}

impl Default for TraceFilter {
    fn default() -> Self {
        Self {
            agent: String::new(),
            user_id: String::new(),
            conversation_id: String::new(),
            status: None,
            model: String::new(),
            tool: String::new(),
            days: DEFAULT_DAYS,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// One trace as the list reads it: a run's spans, summed. Its status is the
/// run's own span's once that has arrived, else `error` where any span of
/// it failed and `ok` otherwise; `ended` says which.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct TraceRow {
    /// The run.
    #[serde(with = "clickhouse::serde::uuid")]
    pub run_id: Uuid,
    /// Its agent, the same on every span of it.
    pub agent: String,
    /// The end user, as stored; empty when none was sent.
    pub user_id: String,
    /// The conversation; empty when none was sent.
    pub conversation_id: String,
    /// 1 once the run's own span has arrived, 0 while it has not.
    pub ended: u8,
    /// How it ended.
    #[serde(with = "text")]
    pub status: SpanStatus,
    /// Its first span's start.
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub started_at: DateTime<Utc>,
    /// Its last span's end.
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub ended_at: DateTime<Utc>,
    /// From the first start to the last end, in milliseconds.
    pub duration_ms: i64,
    /// Every span of it.
    pub spans: u64,
    /// Its steps.
    pub steps: u64,
    /// Its model calls.
    pub model_calls: u64,
    /// Its tool calls.
    pub tool_calls: u64,
    /// Its spans that ended in error.
    pub errors: u64,
    /// Its model calls' input tokens, summed.
    pub input_tokens: u64,
    /// Its model calls' output tokens, summed.
    pub output_tokens: u64,
    /// Its model calls' cache reads, summed.
    pub cache_read_input_tokens: u64,
    /// Its model calls' cache writes, summed.
    pub cache_write_input_tokens: u64,
    /// Input and output, summed: what the list sorts by.
    pub tokens: u64,
    /// The models its calls answered with, each once, sorted.
    pub models: Vec<String>,
    /// The tools its calls ran, each once, sorted.
    pub tools: Vec<String>,
}

/// The list's SQL with one `ORDER BY`, so every (column, direction) is a
/// literal: the query module takes nothing else.
///
/// Two levels, because ClickHouse resolves a SELECT alias before a column of
/// the same name anywhere in the query: `any(agent) AS agent` would turn the
/// `WHERE` on the span's agent into an aggregate, and `countIf(status = …)`
/// into one inside another. So the sums take names no column has, and the
/// outer SELECT gives them the row's.
macro_rules! list_sql {
    ($order:literal) => {
        concat!(
            "SELECT run_id, run_agent AS agent, run_user_id AS user_id, ",
            "run_conversation_id AS conversation_id, run_ended AS ended, run_status AS status, ",
            "first_started_at AS started_at, last_ended_at AS ended_at, duration_ms, ",
            "spans, steps, model_calls, tool_calls, errors, ",
            "input_total AS input_tokens, output_total AS output_tokens, ",
            "cache_read_total AS cache_read_input_tokens, ",
            "cache_write_total AS cache_write_input_tokens, tokens, models, tools ",
            "FROM (SELECT run_id, any(agent) AS run_agent, ",
            "anyIf(user_id, user_id != '') AS run_user_id, ",
            "anyIf(conversation_id, conversation_id != '') AS run_conversation_id, ",
            "toUInt8(countIf(kind = 'run') > 0) AS run_ended, ",
            "if(countIf(kind = 'run') > 0, anyIf(status, kind = 'run'), ",
            "if(countIf(status = 'error') > 0, 'error', 'ok')) AS run_status, ",
            "min(started_at) AS first_started_at, max(ended_at) AS last_ended_at, ",
            "greatest(dateDiff('millisecond', first_started_at, last_ended_at), 0) AS duration_ms, ",
            "count() AS spans, countIf(kind = 'step') AS steps, ",
            "countIf(kind = 'model_call') AS model_calls, ",
            "countIf(kind = 'tool_call') AS tool_calls, ",
            "countIf(status = 'error') AS errors, ",
            "sumIf(input_tokens, kind = 'model_call') AS input_total, ",
            "sumIf(output_tokens, kind = 'model_call') AS output_total, ",
            "sumIf(cache_read_input_tokens, kind = 'model_call') AS cache_read_total, ",
            "sumIf(cache_write_input_tokens, kind = 'model_call') AS cache_write_total, ",
            "input_total + output_total AS tokens, ",
            "arraySort(groupUniqArrayIf(model, kind = 'model_call' AND model != '')) AS models, ",
            "arraySort(groupUniqArrayIf(name, kind = 'tool_call')) AS tools ",
            "FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
            "AND ({agent:String} = '' OR agent = {agent:String}) ",
            "GROUP BY run_id ",
            "HAVING ({user:String} = '' OR run_user_id = {user:String}) ",
            "AND ({conversation:String} = '' OR run_conversation_id = {conversation:String}) ",
            "AND ({status:String} = '' OR run_status = {status:String}) ",
            "AND ({model:String} = '' OR has(models, {model:String})) ",
            "AND ({tool:String} = '' OR has(tools, {tool:String}))) ",
            "ORDER BY ",
            $order,
            ", started_at DESC, run_id ",
            "LIMIT {limit:UInt32}"
        )
    };
}

/// The list's SQL for one sort: a literal per column and direction.
const fn list_sql(sort: TraceSort, direction: SortDirection) -> &'static str {
    use SortDirection::{Asc, Desc};
    match (sort, direction) {
        (TraceSort::Started, Asc) => list_sql!("started_at ASC"),
        (TraceSort::Started, Desc) => list_sql!("started_at DESC"),
        (TraceSort::Agent, Asc) => list_sql!("agent ASC"),
        (TraceSort::Agent, Desc) => list_sql!("agent DESC"),
        (TraceSort::User, Asc) => list_sql!("user_id ASC"),
        (TraceSort::User, Desc) => list_sql!("user_id DESC"),
        (TraceSort::Conversation, Asc) => list_sql!("conversation_id ASC"),
        (TraceSort::Conversation, Desc) => list_sql!("conversation_id DESC"),
        (TraceSort::Status, Asc) => list_sql!("status ASC"),
        (TraceSort::Status, Desc) => list_sql!("status DESC"),
        (TraceSort::Steps, Asc) => list_sql!("steps ASC"),
        (TraceSort::Steps, Desc) => list_sql!("steps DESC"),
        (TraceSort::Tokens, Asc) => list_sql!("tokens ASC"),
        (TraceSort::Tokens, Desc) => list_sql!("tokens DESC"),
        (TraceSort::Duration, Asc) => list_sql!("duration_ms ASC"),
        (TraceSort::Duration, Desc) => list_sql!("duration_ms DESC"),
    }
}

/// A trace's spans, every column of the row, in the order they started; of
/// two that started together, the one that ended later first, since that is
/// the one holding the other, as a run holds its first step.
const SPANS_SQL: &str = "SELECT project_id, received_at, units, run_id, agent, trace_id, \
     span_id, parent_span_id, kind, name, started_at, ended_at, status, error_type, model, \
     provider, input_tokens, output_tokens, cache_read_input_tokens, cache_write_input_tokens, \
     user_id, conversation_id, tags, bound_stall_after_s, bound_max_steps, \
     bound_max_cost_usd, sample_rate, attributes \
     FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} AND run_id = {run:UUID} \
     ORDER BY started_at, ended_at DESC, span_id \
     LIMIT {limit:UInt32}";

/// The traces of `projects` in `filter`'s window, sorted as asked: up to
/// `filter.limit` of them.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn list(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    sort: TraceSort,
    direction: SortDirection,
) -> clickhouse::error::Result<Vec<TraceRow>> {
    store
        .read(projects, list_sql(sort, direction))
        .param("days", filter.days)
        .param("agent", &filter.agent)
        .param("user", &filter.user_id)
        .param("conversation", &filter.conversation_id)
        .param(
            "status",
            filter.status.map(|s| s.to_string()).unwrap_or_default(),
        )
        .param("model", &filter.model)
        .param("tool", &filter.tool)
        .param("limit", filter.limit)
        .fetch_all()
        .await
}

/// The spans of run `run_id` in `projects`, in the order they started: up
/// to `limit` of them, and none for a run the projects do not hold.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn spans(
    store: &Store,
    projects: &Projects,
    run_id: Uuid,
    limit: u32,
) -> clickhouse::error::Result<Vec<SpanRow>> {
    store
        .read(projects, SPANS_SQL)
        .param("run", run_id)
        .param("limit", limit)
        .fetch_all()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PROJECTS_PARAM;

    /// Every sort is one literal that names its projects, carries no
    /// settings, orders by its own column first, and binds every filter.
    #[test]
    fn every_sort_is_a_literal_naming_its_projects_and_its_column() {
        for sort in TraceSort::all() {
            for direction in SortDirection::all() {
                let sql = list_sql(sort, direction);
                assert!(sql.contains(PROJECTS_PARAM), "{sort} {direction}");
                assert!(!sql.to_ascii_lowercase().contains(" settings "));
                let column = match sort {
                    TraceSort::Started => "started_at",
                    TraceSort::Agent => "agent",
                    TraceSort::User => "user_id",
                    TraceSort::Conversation => "conversation_id",
                    TraceSort::Status => "status",
                    TraceSort::Steps => "steps",
                    TraceSort::Tokens => "tokens",
                    TraceSort::Duration => "duration_ms",
                };
                let order = format!("ORDER BY {column} {}", direction.to_string().to_uppercase());
                assert!(sql.contains(&order), "{sort} {direction}: {sql}");
                for param in [
                    "{days:UInt16}",
                    "{agent:String}",
                    "{user:String}",
                    "{conversation:String}",
                    "{status:String}",
                    "{model:String}",
                    "{tool:String}",
                    "{limit:UInt32}",
                ] {
                    assert!(sql.contains(param), "{sort} {direction} lacks {param}");
                }
            }
        }
        assert!(SPANS_SQL.contains(PROJECTS_PARAM));
        assert!(SPANS_SQL.contains("{run:UUID}"));
    }

    #[test]
    fn the_default_filter_is_a_week_of_a_hundred() {
        let filter = TraceFilter::default();
        assert_eq!(filter.days, DEFAULT_DAYS);
        assert_eq!(filter.limit, DEFAULT_LIMIT);
        assert!(filter.agent.is_empty() && filter.status.is_none());
    }
}
