//! The trace list and a trace's spans, read through the one query module
//! ([`crate::store`]). A trace is a run's record: its root span and every
//! span under it, found by `run_id`, which every span of a run carries, and
//! never by the wire's trace id, since a run may sit inside an application's
//! own trace.
//!
//! The list is one aggregation over the project's spans in a window of days,
//! a row per run, and its page, its count, its histogram and its download are
//! each one read over those rows; its facets, one more beside them. A filter
//! is a list of values, any of which admits a run, and an empty one binds as
//! `[]` and admits every run, so one literal serves every combination; the
//! sort is a literal chosen in code from the closed set, never text from the
//! request.

use chrono::{DateTime, Utc};
use clickhouse::{Row, RowOwned, RowRead};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::{SortDirection, SpanKind, SpanStatus, TraceSort};

use crate::store::{Projects, Read, SpanRow, Store, text};

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
/// The most units of its spans a trace's page shows, a KiB of their text
/// each: a run of wide spans is cut short before [`SPANS_MAX`], at some
/// 8 MiB of text.
pub const TRACE_UNITS_MAX: u32 = 8_192;
/// The most values one filter admits: more than a person ticks, and few
/// enough that a URL made by hand cannot make a read's query long.
pub const FILTER_VALUES_MAX: usize = 50;
/// The furthest page the list turns to. Every page reads the whole window
/// before it skips to its own, so a page this deep is a read to narrow, not
/// one to turn to.
pub const MAX_PAGE: u32 = 1_000;
/// How many of each facet's values the facets name, the most runs first:
/// those a person picks from, and the ones ticked already.
pub const FACET_VALUES_MAX: usize = 50;
/// The most traces one download holds.
pub const EXPORT_MAX: u32 = 10_000;
/// The most runs a download names by id: the rows chosen on one page, which
/// the console shows a hundred at a time at most.
pub const RUNS_MAX: usize = 100;
/// The most span names a layout pins: the intermediate steps a reviewer
/// checks under each trace, few enough to read without opening it.
pub const PINNED_MAX: usize = 8;
/// The most pinned spans one trace shows: a step a loop ran again and again
/// is read in the trace itself.
pub const PINNED_PER_RUN: u32 = 32;

/// A window of days and a cap on rows: what a session's page is read by, one
/// aggregation over the spans of those days and never the whole table.
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

/// What narrows the list, and its count, facets, histogram and download with
/// it. Each filter admits a run matching any of its values; an empty one
/// admits every run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceFilter {
    /// The runs' agents.
    pub agents: Vec<String>,
    /// Their end users, as stored: hashed, unless the project keeps them
    /// plain.
    pub user_ids: Vec<String>,
    /// Their sessions, by id.
    pub session_ids: Vec<String>,
    /// How they ended.
    pub statuses: Vec<SpanStatus>,
    /// Models one of their calls answered with.
    pub models: Vec<String>,
    /// Tools one of their calls ran.
    pub tools: Vec<String>,
    /// The deployments their spans were sent from.
    pub environments: Vec<String>,
    /// The releases their spans were sent from.
    pub releases: Vec<String>,
    /// Tags, each `key:value`, which a run carries where any of its spans
    /// does.
    pub tags: Vec<String>,
    /// Prompt names (`gen_ai.prompt.name`) a run carries where any of its
    /// spans does.
    pub prompts: Vec<String>,
    /// The runs themselves, by id: the rows a download was chosen from.
    pub runs: Vec<Uuid>,
    /// How many days back, by the day each span arrived.
    pub days: u16,
}

impl Default for TraceFilter {
    fn default() -> Self {
        Self {
            agents: Vec::new(),
            user_ids: Vec::new(),
            session_ids: Vec::new(),
            statuses: Vec::new(),
            models: Vec::new(),
            tools: Vec::new(),
            environments: Vec::new(),
            releases: Vec::new(),
            tags: Vec::new(),
            prompts: Vec::new(),
            runs: Vec::new(),
            days: DEFAULT_DAYS,
        }
    }
}

/// One trace as the list reads it: a run's spans, summed. Its status is the
/// run's own span's once that has arrived, else `error` where any span of
/// it failed and `ok` otherwise; `ended` says which.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct TraceRow {
    /// The run.
    #[serde(deserialize_with = "clickhouse::serde::uuid::deserialize")]
    pub run_id: Uuid,
    /// Its agent, the same on every span of it.
    pub agent: String,
    /// The end user, as stored; empty when none was sent.
    pub user_id: String,
    /// The session's id; empty when none was sent.
    pub session_id: String,
    /// Whether the run's own span has arrived.
    pub ended: bool,
    /// How it ended.
    #[serde(deserialize_with = "text::deserialize")]
    pub status: SpanStatus,
    /// The class of the error its own span ended on, never its message;
    /// empty otherwise, and until that span arrives.
    pub error_type: String,
    /// Its first span's start.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub started_at: DateTime<Utc>,
    /// Its last span's end.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub ended_at: DateTime<Utc>,
    /// From the first start to the last end, in milliseconds.
    pub duration_ms: i64,
    /// Its first model call's time to first token that reported one, in milliseconds.
    pub time_to_first_token_ms: Option<f64>,
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
    /// The models its calls answered with, each once, sorted.
    pub models: Vec<String>,
    /// The tools its calls ran, each once, sorted.
    pub tools: Vec<String>,
    /// The deployments its spans were sent from, each once, sorted: one,
    /// unless the run crossed services deployed apart.
    pub environments: Vec<String>,
    /// The releases its spans were sent from, each once, sorted.
    pub releases: Vec<String>,
}

/// How many rows the filter admits in the window: runs, sessions or users.
#[derive(Debug, Row, Deserialize)]
pub(crate) struct CountRow {
    pub(crate) total: u64,
}

/// The list's filters, which are its facets: each one word, the URL's
/// parameter, a saved view's key and the facets read's first column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub enum Facet {
    Agent,
    User,
    Session,
    Status,
    Model,
    Tool,
    Environment,
    Release,
    Tag,
    Prompt,
}

// In the sidebar's order.
telmoni_shared::wire_enum!(Facet {
    Agent => "agent",
    User => "user",
    Session => "session",
    Status => "status",
    Model => "model",
    Tool => "tool",
    Environment => "environment",
    Release => "release",
    Tag => "tag",
    Prompt => "prompt",
});

impl TryFrom<String> for Facet {
    type Error = telmoni_shared::ParseEnumError;

    fn try_from(word: String) -> Result<Self, Self::Error> {
        word.parse()
    }
}

/// One value of one facet: how many of a list's rows, among those the other
/// filters admit, carry it. A facet the read spells as none of the ten fails
/// the read, never vanishes from the answer.
#[derive(Debug, Clone, PartialEq, Eq, Row, Deserialize)]
pub struct FacetRow {
    pub facet: Facet,
    /// The value, as stored.
    pub value: String,
    /// How many of the list's rows carry it: runs, sessions or users.
    pub listed: u64,
}

/// One span a layout pinned by name, as a session's page shows it under a
/// trace: enough to read without opening the trace.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct PinnedRow {
    /// The run it belongs to, which the page keys it under.
    #[serde(deserialize_with = "clickhouse::serde::uuid::deserialize")]
    pub run_id: Uuid,
    pub span_id: String,
    pub parent_span_id: String,
    #[serde(deserialize_with = "text::deserialize")]
    pub kind: SpanKind,
    pub name: String,
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub started_at: DateTime<Utc>,
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub ended_at: DateTime<Utc>,
    #[serde(deserialize_with = "text::deserialize")]
    pub status: SpanStatus,
    pub error_type: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One bar of the histogram as the read answers it: the runs that started in
/// it, by how they ended, their tokens, and their durations' median and 95th
/// percentile. A bar no run started in is not a row.
#[derive(Debug, Clone, PartialEq, Row, Deserialize)]
pub struct BucketRow {
    /// Where the bar starts, in milliseconds since the epoch.
    pub at_ms: i64,
    /// Its runs that ended well.
    pub ok: u64,
    /// That failed.
    pub error: u64,
    /// That were cancelled.
    pub cancelled: u64,
    /// Their input and output tokens, summed.
    pub token_total: u64,
    /// Their durations' median, in milliseconds.
    pub duration_p50_ms: f64,
    /// And 95th percentile.
    pub duration_p95_ms: f64,
    /// Their times to first token's median, in milliseconds, where any reported one.
    pub time_to_first_token_p50_ms: Option<f64>,
    /// And 95th percentile.
    pub time_to_first_token_p95_ms: Option<f64>,
}

/// One bar of the histogram as the console draws it: a [`BucketRow`], or a
/// bar no run started in, whose durations are none.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bar {
    /// Where the bar starts.
    pub at: DateTime<Utc>,
    /// Its runs that ended well.
    pub ok: u64,
    /// That failed.
    pub error: u64,
    /// That were cancelled.
    pub cancelled: u64,
    /// Their input and output tokens, summed.
    pub tokens: u64,
    /// Their durations' median, in milliseconds, where it had runs.
    pub duration_p50_ms: Option<f64>,
    /// And 95th percentile.
    pub duration_p95_ms: Option<f64>,
    /// Their times to first token's median, in milliseconds, where any reported one.
    pub time_to_first_token_p50_ms: Option<f64>,
    /// And 95th percentile.
    pub time_to_first_token_p95_ms: Option<f64>,
}

/// A run's status: its own span's once that has arrived, else `error` where
/// any span failed and `ok` otherwise.
macro_rules! run_status {
    () => {
        "if(countIf(kind = 'run') > 0, anyIf(status, kind = 'run'), \
         if(countIf(status = 'error') > 0, 'error', 'ok'))"
    };
}

/// Whether the run's sums pass one filter, each admitting every run while
/// it is empty: what the list keeps and the facets flag.
macro_rules! admits {
    (user) => {
        "(empty({users:Array(String)}) OR has({users:Array(String)}, run_user_id))"
    };
    (session) => {
        "(empty({sessions:Array(String)}) OR has({sessions:Array(String)}, run_session_id))"
    };
    (status) => {
        "(empty({statuses:Array(String)}) OR has({statuses:Array(String)}, run_status))"
    };
    (model) => {
        "(empty({models:Array(String)}) OR hasAny(models, {models:Array(String)}))"
    };
    (tool) => {
        "(empty({tools:Array(String)}) OR hasAny(tools, {tools:Array(String)}))"
    };
    (environment) => {
        "(empty({environments:Array(String)}) \
         OR hasAny(environments, {environments:Array(String)}))"
    };
    (release) => {
        "(empty({releases:Array(String)}) OR hasAny(releases, {releases:Array(String)}))"
    };
    (tag) => {
        "(empty({tags:Array(String)}) OR hasAny(tag_pairs, {tags:Array(String)}))"
    };
    (prompt) => {
        "(empty({prompts:Array(String)}) OR hasAny(prompts, {prompts:Array(String)}))"
    };
}

/// What a run carries where any of its spans does, each value once and
/// sorted: the deployments and releases its spans were sent from, its tags
/// as `key:value`, and the prompt names among its attributes. Named apart
/// from the columns, as the sums below are.
macro_rules! run_carries {
    () => {
        concat!(
            "arraySort(groupUniqArrayIf(environment, environment != '')) AS environments, ",
            "arraySort(groupUniqArrayIf(release, release != '')) AS releases, ",
            "arraySort(groupUniqArrayArray(arrayMap((k, v) -> concat(k, ':', v), ",
            "mapKeys(tags), mapValues(tags)))) AS tag_pairs, ",
            "arraySort(groupUniqArrayIf(attributes['gen_ai.prompt.name'], ",
            "mapContains(attributes, 'gen_ai.prompt.name'))) AS prompts "
        )
    };
}

/// A run's spans summed, under names no column has, since ClickHouse
/// resolves a SELECT alias before a column of the same name anywhere in the
/// query: `any(agent) AS agent` would turn the `WHERE` on the span's agent
/// into an aggregate, and `countIf(status = …)` into one inside another.
macro_rules! run_sums {
    () => {
        concat!(
            "run_id, any(agent) AS run_agent, ",
            "anyIf(user_id, user_id != '') AS run_user_id, ",
            "anyIf(session_id, session_id != '') AS run_session_id, ",
            "CAST(countIf(kind = 'run') > 0 AS Bool) AS run_ended, ",
            run_status!(),
            " AS run_status, ",
            "anyIf(error_type, kind = 'run') AS run_error_type, ",
            "min(started_at) AS first_started_at, max(ended_at) AS last_ended_at, ",
            "greatest(dateDiff('millisecond', first_started_at, last_ended_at), 0) AS duration_ms, ",
            "argMinIf(time_to_first_token, started_at, kind = 'model_call') * 1000 AS time_to_first_token_ms, ",
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
            "arraySort(groupUniqArrayIf(name, kind = 'tool_call')) AS tools, ",
            run_carries!()
        )
    };
}

/// The project's spans in the window, of the runs the filter's ids name, if
/// it names any: what every read sums.
macro_rules! window {
    () => {
        concat!(
            "FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
            "AND (empty({runs:Array(UUID)}) OR has({runs:Array(UUID)}, run_id)) "
        )
    };
}

/// The runs the filter admits, a row each: what the list pages through,
/// counts, buckets and downloads. The agent's filter is read off the spans,
/// each of which names its run's agent, so another agent's run is never
/// summed; the rest are read off the sums.
macro_rules! runs {
    () => {
        concat!(
            "SELECT ",
            run_sums!(),
            window!(),
            "AND (empty({agents:Array(String)}) OR has({agents:Array(String)}, agent)) ",
            "GROUP BY run_id ",
            "HAVING ",
            admits!(user),
            " AND ",
            admits!(session),
            " AND ",
            admits!(status),
            " AND ",
            admits!(model),
            " AND ",
            admits!(tool),
            " AND ",
            admits!(environment),
            " AND ",
            admits!(release),
            " AND ",
            admits!(tag),
            " AND ",
            admits!(prompt)
        )
    };
}

/// Every run in the window, a row each, with what the sessions and users
/// lists sum of it and whether the filter admits it (`admitted`), its matches
/// counted rather than kept as the arrays `runs!` shows: some seven times
/// less memory a run, so a long window's runs fit a read.
macro_rules! run_rows {
    () => {
        concat!(
            "SELECT run_id, anyIf(user_id, user_id != '') AS run_user_id, ",
            "anyIf(session_id, session_id != '') AS run_session_id, ",
            "min(started_at) AS first_started_at, max(ended_at) AS last_ended_at, ",
            "countIf(kind = 'step') AS steps, countIf(status = 'error') AS errors, ",
            "sumIf(input_tokens, kind = 'model_call') AS input_total, ",
            "sumIf(output_tokens, kind = 'model_call') AS output_total, ",
            "toUInt8((empty({agents:Array(String)}) OR has({agents:Array(String)}, any(agent))) ",
            "AND (empty({users:Array(String)}) OR has({users:Array(String)}, run_user_id)) ",
            "AND (empty({sessions:Array(String)}) OR has({sessions:Array(String)}, run_session_id)) ",
            "AND (empty({statuses:Array(String)}) OR has({statuses:Array(String)}, ",
            $crate::traces::run_status!(),
            ")) ",
            "AND (empty({models:Array(String)}) ",
            "OR countIf(kind = 'model_call' AND has({models:Array(String)}, model)) > 0) ",
            "AND (empty({tools:Array(String)}) ",
            "OR countIf(kind = 'tool_call' AND has({tools:Array(String)}, name)) > 0) ",
            "AND (empty({environments:Array(String)}) ",
            "OR countIf(has({environments:Array(String)}, environment)) > 0) ",
            "AND (empty({releases:Array(String)}) ",
            "OR countIf(has({releases:Array(String)}, release)) > 0) ",
            "AND (empty({tags:Array(String)}) OR countIf(hasAny(arrayMap((k, v) -> ",
            "concat(k, ':', v), mapKeys(tags), mapValues(tags)), {tags:Array(String)})) > 0) ",
            "AND (empty({prompts:Array(String)}) ",
            "OR countIf(mapContains(attributes, 'gen_ai.prompt.name') ",
            "AND has({prompts:Array(String)}, attributes['gen_ai.prompt.name'])) > 0)) AS admitted ",
            $crate::traces::window!(),
            "GROUP BY run_id"
        )
    };
}

pub(crate) use admits;
pub(crate) use run_carries;
pub(crate) use run_rows;
pub(crate) use run_status;
pub(crate) use window;

/// The list's SQL with one `ORDER BY`, so every (column, direction) is a
/// literal: the query module takes nothing else. The outer SELECT gives the
/// sums the row's names.
macro_rules! list_sql {
    ($order:literal) => {
        concat!(
            "SELECT run_id, run_agent AS agent, run_user_id AS user_id, ",
            "run_session_id AS session_id, run_ended AS ended, run_status AS status, ",
            "run_error_type AS error_type, ",
            "first_started_at AS started_at, last_ended_at AS ended_at, duration_ms, ",
            "time_to_first_token_ms, ",
            "spans, steps, model_calls, tool_calls, errors, ",
            "input_total AS input_tokens, output_total AS output_tokens, ",
            "cache_read_total AS cache_read_input_tokens, ",
            "cache_write_total AS cache_write_input_tokens, models, tools, ",
            "environments, releases ",
            "FROM (",
            runs!(),
            ") ORDER BY ",
            $order,
            ", started_at DESC, run_id ",
            "LIMIT {limit:UInt32} OFFSET {offset:UInt64}"
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
        (TraceSort::Session, Asc) => list_sql!("session_id ASC"),
        (TraceSort::Session, Desc) => list_sql!("session_id DESC"),
        (TraceSort::Status, Asc) => list_sql!("status ASC"),
        (TraceSort::Status, Desc) => list_sql!("status DESC"),
        (TraceSort::Steps, Asc) => list_sql!("steps ASC"),
        (TraceSort::Steps, Desc) => list_sql!("steps DESC"),
        (TraceSort::ModelCalls, Asc) => list_sql!("model_calls ASC"),
        (TraceSort::ModelCalls, Desc) => list_sql!("model_calls DESC"),
        (TraceSort::ToolCalls, Asc) => list_sql!("tool_calls ASC"),
        (TraceSort::ToolCalls, Desc) => list_sql!("tool_calls DESC"),
        (TraceSort::Errors, Asc) => list_sql!("errors ASC"),
        (TraceSort::Errors, Desc) => list_sql!("errors DESC"),
        (TraceSort::InputTokens, Asc) => list_sql!("input_tokens ASC"),
        (TraceSort::InputTokens, Desc) => list_sql!("input_tokens DESC"),
        (TraceSort::OutputTokens, Asc) => list_sql!("output_tokens ASC"),
        (TraceSort::OutputTokens, Desc) => list_sql!("output_tokens DESC"),
        (TraceSort::Tokens, Asc) => list_sql!("tokens ASC"),
        (TraceSort::Tokens, Desc) => list_sql!("tokens DESC"),
        (TraceSort::Duration, Asc) => list_sql!("duration_ms ASC"),
        (TraceSort::Duration, Desc) => list_sql!("duration_ms DESC"),
        (TraceSort::TimeToFirstToken, Asc) => list_sql!("time_to_first_token_ms ASC"),
        (TraceSort::TimeToFirstToken, Desc) => list_sql!("time_to_first_token_ms DESC"),
    }
}

/// How many runs the filter admits: the list's total, a second read beside
/// its page, since a page past the last holds no row to carry it.
const COUNT_SQL: &str = concat!("SELECT count() AS total FROM (", runs!(), ")");

/// The histogram: the runs the filter admits, by the bar their first span
/// started in. A bar is `{bucket}` milliseconds wide and starts where the
/// reader's clock, `{shift}` milliseconds east of UTC, starts an hour or a
/// day; a run that started before the window, its spans arriving late, or
/// after the store's now, its clock ahead, is drawn in the bar at that end.
/// `token_total` is named apart from `tokens`, a sum it adds up, for the
/// reason [`run_sums!`] gives.
const HISTOGRAM_SQL: &str = concat!(
    "SELECT intDiv(toUnixTimestamp64Milli(least(greatest(first_started_at, ",
    "now64(6) - toIntervalDay({days:UInt16})), now64(6))) + {shift:Int64}, {bucket:Int64}) ",
    "* {bucket:Int64} - {shift:Int64} AS at_ms, ",
    "countIf(run_status = 'ok') AS ok, countIf(run_status = 'error') AS error, ",
    "countIf(run_status = 'cancelled') AS cancelled, sum(tokens) AS token_total, ",
    "quantile(0.5)(duration_ms) AS duration_p50_ms, ",
    "quantile(0.95)(duration_ms) AS duration_p95_ms, ",
    "quantile(0.5)(time_to_first_token_ms) AS time_to_first_token_p50_ms, ",
    "quantile(0.95)(time_to_first_token_ms) AS time_to_first_token_p95_ms ",
    "FROM (",
    runs!(),
    ") GROUP BY at_ms ORDER BY at_ms"
);

/// A list's facets: each value of each facet, and how many of the list's
/// rows (`$count`: runs, or the sessions or users `$holds` keeps) carry it
/// among the runs every OTHER filter admits — so a value beside a ticked one
/// still counts what ticking it would add. Every run is summed once, its
/// matches kept as flags, then spread to a row per value it carries; the
/// ticked values first, so none past the first fifty is lost, then the most
/// rows, one more than [`FACET_VALUES_MAX`] each to say there were more.
macro_rules! facets_sql {
    ($count:literal, $holds:literal) => {
        concat!(
            "SELECT tupleElement(facet_value, 1) AS facet, tupleElement(facet_value, 2) AS value, ",
            $count,
            " AS listed ",
            "FROM (SELECT run_id, any(agent) AS run_agent, ",
            "anyIf(user_id, user_id != '') AS run_user_id, ",
            "anyIf(session_id, session_id != '') AS run_session_id, ",
            $crate::traces::run_status!(),
            " AS run_status, ",
            "arraySort(groupUniqArrayIf(model, kind = 'model_call' AND model != '')) AS models, ",
            "arraySort(groupUniqArrayIf(name, kind = 'tool_call')) AS tools, ",
            $crate::traces::run_carries!(),
            ", toUInt8(empty({agents:Array(String)}) OR has({agents:Array(String)}, run_agent)) ",
            "AS m_agent, ",
            "toUInt8(", $crate::traces::admits!(user), ") AS m_user, ",
            "toUInt8(", $crate::traces::admits!(session), ") AS m_session, ",
            "toUInt8(", $crate::traces::admits!(status), ") AS m_status, ",
            "toUInt8(", $crate::traces::admits!(model), ") AS m_model, ",
            "toUInt8(", $crate::traces::admits!(tool), ") AS m_tool, ",
            "toUInt8(", $crate::traces::admits!(environment), ") AS m_environment, ",
            "toUInt8(", $crate::traces::admits!(release), ") AS m_release, ",
            "toUInt8(", $crate::traces::admits!(tag), ") AS m_tag, ",
            "toUInt8(", $crate::traces::admits!(prompt), ") AS m_prompt, ",
            // How many filters the run fails, so a value counts where no
            // filter but its own refuses the run: with its own flag up it
            // needs no miss, with it down that one miss alone.
            "(1 - m_agent) + (1 - m_user) + (1 - m_session) + (1 - m_status) + (1 - m_model) ",
            "+ (1 - m_tool) + (1 - m_environment) + (1 - m_release) + (1 - m_tag) ",
            "+ (1 - m_prompt) AS misses ",
            $crate::traces::window!(),
            "GROUP BY run_id) ",
            "ARRAY JOIN arrayConcat(",
            "[tuple('agent', CAST(run_agent AS String), toUInt8(misses + m_agent = 1))], ",
            "[tuple('user', CAST(run_user_id AS String), toUInt8(misses + m_user = 1))], ",
            "[tuple('session', CAST(run_session_id AS String), toUInt8(misses + m_session = 1))], ",
            "[tuple('status', CAST(run_status AS String), toUInt8(misses + m_status = 1))], ",
            "arrayMap(x -> tuple('model', CAST(x AS String), toUInt8(misses + m_model = 1)), models), ",
            "arrayMap(x -> tuple('tool', CAST(x AS String), toUInt8(misses + m_tool = 1)), tools), ",
            "arrayMap(x -> tuple('environment', CAST(x AS String), ",
            "toUInt8(misses + m_environment = 1)), environments), ",
            "arrayMap(x -> tuple('release', CAST(x AS String), toUInt8(misses + m_release = 1)), ",
            "releases), ",
            "arrayMap(x -> tuple('tag', CAST(x AS String), toUInt8(misses + m_tag = 1)), tag_pairs), ",
            "arrayMap(x -> tuple('prompt', CAST(x AS String), toUInt8(misses + m_prompt = 1)), ",
            "prompts)",
            ") AS facet_value ",
            "WHERE tupleElement(facet_value, 3) = 1 AND tupleElement(facet_value, 2) != '' ",
            $holds,
            "GROUP BY facet, value ",
            "ORDER BY facet, has(multiIf(facet = 'agent', {agents:Array(String)}, ",
            "facet = 'user', {users:Array(String)}, facet = 'session', {sessions:Array(String)}, ",
            "facet = 'status', {statuses:Array(String)}, facet = 'model', {models:Array(String)}, ",
            "facet = 'tool', {tools:Array(String)}, ",
            "facet = 'environment', {environments:Array(String)}, ",
            "facet = 'release', {releases:Array(String)}, facet = 'tag', {tags:Array(String)}, ",
            "facet = 'prompt', {prompts:Array(String)}, emptyArrayString()), value) DESC, ",
            "listed DESC, value ",
            "LIMIT 51 BY facet"
        )
    };
}

pub(crate) use facets_sql;

/// The trace list's facets, each counting the runs that carry a value.
const FACETS_SQL: &str = facets_sql!("count()", "");

/// A trace's spans whole, in the order they started — of two that started
/// together, the one that ended later, which holds the other — up to the one
/// that takes their units past `{units_max}`, summed on the light columns.
const SPANS_SQL: &str = "SELECT project_id, received_at, units, run_id, agent, trace_id, \
     span_id, parent_span_id, kind, name, started_at, ended_at, status, error_type, model, \
     provider, input_tokens, output_tokens, cache_read_input_tokens, cache_write_input_tokens, \
     cost, time_to_first_token, user_id, session_id, environment, release, tags, sample_rate, \
     attributes, sent \
     FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} AND run_id = {run:UUID} \
     AND span_id IN (SELECT span_id FROM (SELECT span_id, sum(units) OVER (\
     ORDER BY started_at, ended_at DESC, span_id \
     ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS units_before \
     FROM (SELECT span_id, started_at, ended_at, units FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} AND run_id = {run:UUID} \
     ORDER BY started_at, ended_at DESC, span_id LIMIT {limit:UInt32})) \
     WHERE units_before <= {units_max:UInt32}) \
     ORDER BY started_at, ended_at DESC, span_id \
     LIMIT {limit:UInt32}";

/// The spans a layout pinned by name under the runs a session's page lists,
/// in the order they started within each run, up to [`PINNED_PER_RUN`] a
/// run; of two that started together, the one that ended later first, as a
/// trace's spans are read.
const PINNED_SQL: &str = "SELECT run_id, span_id, parent_span_id, kind, name, started_at, \
     ended_at, status, error_type, model, input_tokens, output_tokens \
     FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} \
     AND received_at >= now64(6) - toIntervalDay({days:UInt16}) \
     AND has({runs:Array(UUID)}, run_id) AND has({names:Array(String)}, name) \
     ORDER BY run_id, started_at, ended_at DESC, span_id \
     LIMIT {per_run:UInt32} BY run_id";

/// `read` with the filter's every value bound: each read binds them all,
/// since each names them all.
pub(crate) fn filtered(read: Read, filter: &TraceFilter) -> Read {
    let statuses: Vec<String> = filter.statuses.iter().map(ToString::to_string).collect();
    read.param("days", filter.days)
        .param("agents", &filter.agents)
        .param("users", &filter.user_ids)
        .param("sessions", &filter.session_ids)
        .param("statuses", statuses)
        .param("models", &filter.models)
        .param("tools", &filter.tools)
        .param("environments", &filter.environments)
        .param("releases", &filter.releases)
        .param("tags", &filter.tags)
        .param("prompts", &filter.prompts)
        .param("runs", &filter.runs)
}

/// One list over a window's runs — the traces, the sessions or the users —
/// by what it lists: its rows, its sorts, and the SQL of its three reads.
pub(crate) trait Listing {
    type Row: RowOwned + RowRead + serde::de::DeserializeOwned;
    type Sort: Copy + std::str::FromStr + std::fmt::Display + 'static;
    /// Every sort, in the refusal's order.
    const SORTS: &'static [Self::Sort];
    /// The sort when the query names none.
    const FALLBACK: Self::Sort;
    const COUNT_SQL: &'static str;
    const FACETS_SQL: &'static str;
    /// The page's SQL for one sort: a literal per column and direction.
    fn list_sql(sort: Self::Sort, direction: SortDirection) -> &'static str;
    /// The way a column sorts when the URL names none.
    fn first_direction(sort: Self::Sort) -> SortDirection;
}

/// The trace list: a run a row.
pub(crate) struct Traces;

impl Listing for Traces {
    type Row = TraceRow;
    type Sort = TraceSort;
    const SORTS: &'static [TraceSort] = &TraceSort::all();
    const FALLBACK: TraceSort = TraceSort::Started;
    const COUNT_SQL: &'static str = COUNT_SQL;
    const FACETS_SQL: &'static str = FACETS_SQL;

    fn list_sql(sort: TraceSort, direction: SortDirection) -> &'static str {
        list_sql(sort, direction)
    }

    /// The newest, the longest and the most first; names in the alphabet's
    /// order.
    fn first_direction(sort: TraceSort) -> SortDirection {
        match sort {
            TraceSort::Started
            | TraceSort::Steps
            | TraceSort::ModelCalls
            | TraceSort::ToolCalls
            | TraceSort::Errors
            | TraceSort::InputTokens
            | TraceSort::OutputTokens
            | TraceSort::Tokens
            | TraceSort::Duration
            | TraceSort::TimeToFirstToken => SortDirection::Desc,
            TraceSort::Agent | TraceSort::User | TraceSort::Session | TraceSort::Status => {
                SortDirection::Asc
            }
        }
    }
}

/// The rows of `projects` the filter admits, sorted as asked: up to `limit`
/// of them, past the first `offset`.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub(crate) async fn list<L: Listing>(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    (sort, direction): (L::Sort, SortDirection),
    limit: u32,
    offset: u64,
) -> clickhouse::error::Result<Vec<L::Row>> {
    filtered(store.read(projects, L::list_sql(sort, direction)), filter)
        .param("limit", limit)
        .param("offset", offset)
        .fetch_all()
        .await
}

/// How many rows of `projects` the filter admits.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub(crate) async fn count<L: Listing>(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
) -> clickhouse::error::Result<u64> {
    let row: CountRow = filtered(store.read(projects, L::COUNT_SQL), filter)
        .fetch_one()
        .await?;
    Ok(row.total)
}

/// The values of each facet among the rows of `projects`, each counted
/// under every filter but its own.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub(crate) async fn facets<L: Listing>(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
) -> clickhouse::error::Result<Vec<FacetRow>> {
    filtered(store.read(projects, L::FACETS_SQL), filter)
        .fetch_all()
        .await
}

/// The width of the histogram's bars for a window of `days`, in seconds:
/// about two dozen to sixty for a short window, a day each up to four months,
/// a week each beyond — enough to see a pattern, few enough to see each bar.
#[must_use]
pub const fn bucket_seconds(days: u16) -> u32 {
    match days {
        0..=2 => 3_600,
        3..=7 => 3 * 3_600,
        8..=14 => 6 * 3_600,
        15..=120 => 86_400,
        _ => 7 * 86_400,
    }
}

/// The bars of the histogram of `projects`' traces the filter admits, each
/// `bucket_seconds` wide and starting on the reader's clock, `shift_minutes`
/// east of UTC. A bar no run started in is not a row; [`bars`] fills it in.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn histogram(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    bucket_seconds: u32,
    shift_minutes: i16,
) -> clickhouse::error::Result<Vec<BucketRow>> {
    filtered(store.read(projects, HISTOGRAM_SQL), filter)
        .param("bucket", i64::from(bucket_seconds) * 1_000)
        .param("shift", i64::from(shift_minutes) * 60_000)
        .fetch_all()
        .await
}

/// Where the bar holding `ms` starts: as the read's `intDiv`, on the
/// reader's clock, `shift_ms` east of UTC.
const fn bar_start(ms: i64, bucket_ms: i64, shift_ms: i64) -> i64 {
    (ms + shift_ms).div_euclid(bucket_ms) * bucket_ms - shift_ms
}

/// The histogram's every bar, from the one the window opens in to the one
/// `now_ms` falls in, those no run started in drawn empty; a bar the read
/// answered beyond either end, its clock and ours apart, is drawn too.
#[must_use]
pub fn bars(
    rows: Vec<BucketRow>,
    now_ms: i64,
    days: u16,
    bucket_seconds: u32,
    shift_minutes: i16,
) -> Vec<Bar> {
    let bucket_ms = i64::from(bucket_seconds) * 1_000;
    let shift_ms = i64::from(shift_minutes) * 60_000;
    let opened = now_ms - i64::from(days) * 86_400_000;
    let mut first = bar_start(opened, bucket_ms, shift_ms);
    let mut last = bar_start(now_ms, bucket_ms, shift_ms);
    if let (Some(earliest), Some(latest)) = (rows.first(), rows.last()) {
        first = first.min(earliest.at_ms);
        last = last.max(latest.at_ms);
    }
    let mut rows = rows.into_iter().peekable();
    let mut drawn = Vec::new();
    let mut at = first;
    while at <= last {
        // A bar whose start no time can name is not drawn.
        if let Some(start) = DateTime::from_timestamp_millis(at) {
            drawn.push(match rows.next_if(|row| row.at_ms <= at) {
                Some(row) => Bar {
                    at: start,
                    ok: row.ok,
                    error: row.error,
                    cancelled: row.cancelled,
                    tokens: row.token_total,
                    duration_p50_ms: Some(row.duration_p50_ms),
                    duration_p95_ms: Some(row.duration_p95_ms),
                    time_to_first_token_p50_ms: row.time_to_first_token_p50_ms,
                    time_to_first_token_p95_ms: row.time_to_first_token_p95_ms,
                },
                None => Bar {
                    at: start,
                    ok: 0,
                    error: 0,
                    cancelled: 0,
                    tokens: 0,
                    duration_p50_ms: None,
                    duration_p95_ms: None,
                    time_to_first_token_p50_ms: None,
                    time_to_first_token_p95_ms: None,
                },
            });
        }
        at += bucket_ms;
    }
    drawn
}

/// The spans named by `names` under the runs `runs` names, in `projects` and
/// within `days`: what a session's page shows under each trace while it is
/// shut. Nothing for no names, and nothing for no runs.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn pinned(
    store: &Store,
    projects: &Projects,
    runs: &[Uuid],
    names: &[String],
    days: u16,
) -> clickhouse::error::Result<Vec<PinnedRow>> {
    if runs.is_empty() || names.is_empty() {
        return Ok(Vec::new());
    }
    store
        .read(projects, PINNED_SQL)
        .param("days", days)
        .param("runs", runs)
        .param("names", names)
        .param("per_run", PINNED_PER_RUN)
        .fetch_all()
        .await
}

/// The spans of run `run_id` in `projects` in the order they started, up to
/// `limit`, through the one past `units` units; none for a run not theirs.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn spans(
    store: &Store,
    projects: &Projects,
    run_id: Uuid,
    limit: u32,
    units: u32,
) -> clickhouse::error::Result<Vec<SpanRow>> {
    store
        .read(projects, SPANS_SQL)
        .param("run", run_id)
        .param("limit", limit)
        .param("units_max", units)
        .fetch_all()
        .await
}

/// How many spans of a trace, by their units in the order [`spans`] read
/// them, its page shows: up to `limit`, while they total at most `units`.
#[must_use]
pub fn shown(each: impl IntoIterator<Item = u32>, limit: u32, units: u32) -> usize {
    let mut total = 0_u64;
    each.into_iter()
        .take(limit as usize)
        .take_while(|&span| {
            total += u64::from(span);
            total <= u64::from(units)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PROJECTS_PARAM;

    /// Every filter's parameter, which every read but a trace's spans binds.
    const FILTER_PARAMS: [&str; 12] = [
        "{days:UInt16}",
        "{agents:Array(String)}",
        "{users:Array(String)}",
        "{sessions:Array(String)}",
        "{statuses:Array(String)}",
        "{models:Array(String)}",
        "{tools:Array(String)}",
        "{environments:Array(String)}",
        "{releases:Array(String)}",
        "{tags:Array(String)}",
        "{prompts:Array(String)}",
        "{runs:Array(UUID)}",
    ];

    /// Every sort is one literal that names its projects, carries no
    /// settings, orders by its own column first, and binds every filter and
    /// its page.
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
                    TraceSort::Session => "session_id",
                    TraceSort::Status => "status",
                    TraceSort::Steps => "steps",
                    TraceSort::ModelCalls => "model_calls",
                    TraceSort::ToolCalls => "tool_calls",
                    TraceSort::Errors => "errors",
                    TraceSort::InputTokens => "input_tokens",
                    TraceSort::OutputTokens => "output_tokens",
                    TraceSort::Tokens => "tokens",
                    TraceSort::Duration => "duration_ms",
                    TraceSort::TimeToFirstToken => "time_to_first_token_ms",
                };
                let order = format!("ORDER BY {column} {}", direction.to_string().to_uppercase());
                assert!(sql.contains(&order), "{sort} {direction}: {sql}");
                for param in FILTER_PARAMS
                    .iter()
                    .chain(&["{limit:UInt32}", "{offset:UInt64}"])
                {
                    assert!(sql.contains(param), "{sort} {direction} lacks {param}");
                }
            }
        }
        assert!(SPANS_SQL.contains(PROJECTS_PARAM));
        assert!(SPANS_SQL.contains("{run:UUID}"));
        assert!(SPANS_SQL.contains("{units_max:UInt32}"));
    }

    /// A trace shows its spans up to the count and while their units fit, a row of none taking none.
    #[test]
    fn a_trace_shows_its_spans_while_their_count_and_units_fit() {
        assert_eq!(shown([1, 1, 1], 5, 8), 3, "a small run whole");
        assert_eq!(shown([1, 1, 1, 1], 3, 8), 3, "cut at the count");
        assert_eq!(shown([4, 4, 4], 5, 8), 2, "cut where the units pass");
        assert_eq!(shown([4, 4], 5, 8), 2, "units that reach the cap fit");
        assert_eq!(
            shown([0, 0, 8, 0, 1], 5, 8),
            4,
            "a row of no units takes none of the cap"
        );
        assert_eq!(shown([], 5, 8), 0);
    }

    /// What a run carries where any of its spans does — its deployments,
    /// releases, tags and prompts — is summed once, apart from the columns,
    /// and filtered by `hasAny`, so a run crossing two deployments answers
    /// to either.
    #[test]
    fn what_a_run_carries_is_summed_once_and_filtered_by_any_of_it() {
        let sql = list_sql(TraceSort::Started, SortDirection::Desc);
        for (alias, filter) in [
            (
                "AS environments",
                "hasAny(environments, {environments:Array(String)})",
            ),
            ("AS releases", "hasAny(releases, {releases:Array(String)})"),
            ("AS tag_pairs", "hasAny(tag_pairs, {tags:Array(String)})"),
            ("AS prompts", "hasAny(prompts, {prompts:Array(String)})"),
        ] {
            assert_eq!(sql.matches(alias).count(), 1, "{alias}");
            assert!(sql.contains(filter), "{filter}");
            assert!(FACETS_SQL.contains(filter), "the facets: {filter}");
        }
        assert!(
            sql.contains("concat(k, ':', v)"),
            "a tag filters as key:value"
        );
        for facet in ["'environment'", "'release'", "'tag'", "'prompt'"] {
            assert!(FACETS_SQL.contains(facet), "no facet {facet}");
        }
    }

    /// Every facet is spread to its tuple and ranked by its own filter, in
    /// the facets read, under the one spelling of a run's status the list,
    /// the sessions' and users' rows and the facets share.
    #[test]
    fn every_facet_is_counted_and_ranked_and_a_runs_status_is_spelled_once() {
        for facet in Facet::all() {
            let word = facet.as_str();
            assert!(
                FACETS_SQL.contains(&format!("tuple('{word}'")),
                "no tuple for {word}"
            );
            assert!(
                FACETS_SQL.contains(&format!("facet = '{word}'")),
                "no rank for {word}"
            );
        }
        let status = run_status!();
        assert!(list_sql(TraceSort::Started, SortDirection::Desc).contains(status));
        assert!(run_rows!().contains(status));
        assert!(FACETS_SQL.contains(status));
        assert!(FACETS_SQL.contains("AS misses"));
    }

    /// Each row of the list carries the class of error its run ended on,
    /// read off the run's own span and never a call's.
    #[test]
    fn the_list_reads_the_runs_own_error_class() {
        let sql = list_sql(TraceSort::Started, SortDirection::Desc);
        assert!(sql.contains("anyIf(error_type, kind = 'run') AS run_error_type"));
        assert!(sql.contains("run_error_type AS error_type"));
    }

    /// The pinned spans are read under the projects, the window, the runs
    /// listed and the names pinned, a run's capped.
    #[test]
    fn the_pinned_spans_bind_the_runs_and_the_names() {
        assert!(PINNED_SQL.contains(PROJECTS_PARAM));
        for param in [
            "{days:UInt16}",
            "{runs:Array(UUID)}",
            "{names:Array(String)}",
            "{per_run:UInt32} BY run_id",
        ] {
            assert!(PINNED_SQL.contains(param), "lacks {param}");
        }
        assert!(!PINNED_SQL.to_ascii_lowercase().contains(" settings "));
    }

    /// The count, the histogram and the facets read the same window under
    /// the same filters as the list, so the numbers beside a page add up to
    /// it.
    #[test]
    fn every_read_beside_the_list_binds_every_filter() {
        for (name, sql) in [
            ("count", COUNT_SQL),
            ("histogram", HISTOGRAM_SQL),
            ("facets", FACETS_SQL),
        ] {
            assert!(sql.contains(PROJECTS_PARAM), "{name}");
            assert!(!sql.to_ascii_lowercase().contains("settings"), "{name}");
            for param in FILTER_PARAMS {
                assert!(sql.contains(param), "{name} lacks {param}");
            }
        }
        assert!(
            HISTOGRAM_SQL.contains("{bucket:Int64}") && HISTOGRAM_SQL.contains("{shift:Int64}")
        );
    }

    /// The facets answer one past the most they name, so the lane can say
    /// there were more; the literal is the constant's.
    #[test]
    fn the_facets_answer_one_past_the_most_they_name() {
        assert!(FACETS_SQL.ends_with(&format!("LIMIT {} BY facet", FACET_VALUES_MAX + 1)));
    }

    #[test]
    fn the_default_filter_is_a_week_of_every_run() {
        let filter = TraceFilter::default();
        assert_eq!(filter.days, DEFAULT_DAYS);
        assert!(filter.agents.is_empty() && filter.statuses.is_empty() && filter.runs.is_empty());
    }

    /// Every window the list reads is drawn in enough bars to see a pattern
    /// and few enough to see each one.
    #[test]
    fn every_window_is_drawn_in_fifteen_to_a_hundred_and_twenty_bars() {
        for days in 1..=MAX_DAYS {
            let bars = u32::from(days) * 86_400 / bucket_seconds(days);
            assert!((15..=120).contains(&bars), "{days} days: {bars} bars");
        }
        assert_eq!(bucket_seconds(1), 3_600);
        assert_eq!(bucket_seconds(MAX_DAYS), 7 * 86_400);
    }

    fn row(at_ms: i64, ok: u64) -> BucketRow {
        BucketRow {
            at_ms,
            ok,
            error: 1,
            cancelled: 0,
            token_total: 10,
            duration_p50_ms: 5.0,
            duration_p95_ms: 9.0,
            time_to_first_token_p50_ms: None,
            time_to_first_token_p95_ms: None,
        }
    }

    /// Every bar from the window's to now's is drawn, the empty ones with no
    /// durations, each starting where the reader's clock starts an hour: on
    /// the half hour of UTC for a reader five and a half hours east of it.
    #[test]
    fn the_bars_run_from_the_windows_to_nows_the_empty_ones_drawn() {
        let hour = 3_600_000_i64;
        // 2026-10-11 12:30 UTC.
        let now = 1_791_721_800_000_i64;
        let shift_minutes = 330_i16;
        let start = bar_start(now, hour, i64::from(shift_minutes) * 60_000);
        assert_eq!(start.rem_euclid(hour), 30 * 60_000, "on UTC's half hour");
        assert_eq!(start, now, "12:30 UTC is 18:00 there");
        let drawn = bars(
            vec![row(start - hour, 2), row(start, 1)],
            now,
            1,
            3_600,
            shift_minutes,
        );
        assert_eq!(drawn.len(), 25, "the day's hour bars and the one now is in");
        assert_eq!(drawn.last().map(|b| b.at.timestamp_millis()), Some(start));
        assert_eq!(drawn.last().map(|b| b.ok), Some(1));
        assert_eq!(drawn[drawn.len() - 2].ok, 2);
        assert_eq!(drawn[0].ok, 0);
        assert_eq!(drawn[0].duration_p50_ms, None);
        assert!(
            drawn
                .windows(2)
                .all(|w| w[1].at.timestamp_millis() - w[0].at.timestamp_millis() == hour)
        );
    }

    /// A bar the read answered past either end, the store's clock and ours
    /// apart, is drawn rather than dropped.
    #[test]
    fn a_bar_past_either_end_is_drawn_not_dropped() {
        let hour = 3_600_000_i64;
        let now = 1_791_721_800_000_i64;
        let last = bar_start(now, hour, 0);
        let drawn = bars(vec![row(last + hour, 3)], now, 1, 3_600, 0);
        assert_eq!(
            drawn.last().map(|b| (b.at.timestamp_millis(), b.ok)),
            Some((last + hour, 3))
        );
    }
}
