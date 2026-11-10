//! The trace list and a trace's spans, read through the one query module
//! ([`crate::store`]). A trace is one agent's execution: the agent's own span
//! and every span under it, found by OpenTelemetry's trace id, which every
//! span of it carries.
//!
//! The list is one aggregation over the project's spans in a window of days,
//! a row per trace, and its page, its count, its histogram and its download
//! are each one read over those rows; its facets, one more beside them. A
//! filter is a list of values, any of which admits a trace, and an empty one
//! binds as `[]` and admits every trace, so one literal serves every
//! combination; the sort is a literal chosen in code from the closed set,
//! never text from the request.

use chrono::{DateTime, Utc};
use clickhouse::{Row, RowOwned, RowRead};
use serde::{Deserialize, Serialize};

use telmoni_shared::{SortDirection, SpanKind, SpanStatus, TraceSort};

use crate::contract::TAG_KEY_MAX;
use crate::store::{Projects, Read, SpanRow, Store, text};

/// How many days back the list reads when the request names none.
pub const DEFAULT_DAYS: u16 = 7;
/// The furthest back the list reads: the longest retention sold.
pub const MAX_DAYS: u16 = 360;
/// How many traces the list answers when the request names no limit.
pub const DEFAULT_LIMIT: u32 = 100;
/// The most traces one answer holds.
pub const MAX_LIMIT: u32 = 500;
/// The most spans a trace's page shows; a trace past it is shown cut short.
pub const SPANS_MAX: u32 = 5_000;
/// The most units of its spans a trace's page shows, a KiB of their text
/// each: a trace of wide spans is cut short before [`SPANS_MAX`], at some
/// 8 MiB of text.
pub const TRACE_UNITS_MAX: u32 = 8_192;
/// The most values one filter admits: more than a person ticks, and few
/// enough that a URL made by hand cannot make a read's query long.
pub const FILTER_VALUES_MAX: usize = 50;
/// The furthest page the list turns to. Every page reads the whole window
/// before it skips to its own, so a page this deep is a read to narrow, not
/// one to turn to.
pub const MAX_PAGE: u32 = 1_000;
/// How many of each facet's values the facets name, the most traces first:
/// those a person picks from, and the ones ticked already.
pub const FACET_VALUES_MAX: usize = 50;
/// The most traces one download holds.
pub const EXPORT_MAX: u32 = 10_000;
/// What narrows the list, and its count, facets, histogram and download with
/// it. Each filter admits a trace matching any of its values; an empty one
/// admits every trace.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceFilter {
    /// The traces' agents.
    pub agents: Vec<String>,
    /// Their end users, as stored: hashed, unless the project keeps them
    /// plain.
    pub user_ids: Vec<String>,
    /// Their sessions, by id.
    pub session_ids: Vec<String>,
    /// How they ended.
    pub statuses: Vec<SpanStatus>,
    pub faults: Vec<telmoni_shared::SpanFault>,
    /// Models one of their calls answered with.
    pub models: Vec<String>,
    /// Tools one of their calls ran.
    pub tools: Vec<String>,
    /// The deployments their spans were sent from.
    pub environments: Vec<String>,
    /// The releases their spans were sent from.
    pub releases: Vec<String>,
    /// Tags, each `key:value`, which a trace carries where any of its spans
    /// does.
    pub tags: Vec<String>,
    /// Prompt names (`gen_ai.prompt.name`) a trace carries where any of its
    /// spans does.
    pub prompts: Vec<String>,
    /// Prompts by name and version, each `name:version`, which a trace
    /// carries where any of its spans carries both.
    pub prompt_versions: Vec<String>,
    /// How many days back, by the day each span arrived.
    pub days: u16,
    /// The window's prices for the spend the rows show and sort by; none prices
    /// only what calls reported.
    pub prices: crate::prices::PriceBook,
    /// A score the traces carry, within its bounds.
    pub score: Option<ScoreFilter>,
}

/// A trace admitted by a score of `name` it carries, `min` to `max` where given.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreFilter {
    pub name: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Default for TraceFilter {
    fn default() -> Self {
        Self {
            agents: Vec::new(),
            user_ids: Vec::new(),
            session_ids: Vec::new(),
            statuses: Vec::new(),
            faults: Vec::new(),
            models: Vec::new(),
            tools: Vec::new(),
            environments: Vec::new(),
            releases: Vec::new(),
            tags: Vec::new(),
            prompts: Vec::new(),
            prompt_versions: Vec::new(),
            days: DEFAULT_DAYS,
            prices: crate::prices::PriceBook::default(),
            score: None,
        }
    }
}

/// One trace as the list reads it: its spans, summed. Its status is the
/// agent's own span's once that has arrived, else `error` where any span of
/// it failed and `ok` otherwise; `ended` says which.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct TraceRow {
    /// The trace.
    pub trace_id: String,
    /// Its agent, the same on every span of it.
    pub agent: String,
    /// The end user, as stored; empty when none was sent.
    pub user_id: String,
    /// The session's id; empty when none was sent.
    pub session_id: String,
    /// Whether the agent's own span has arrived.
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
    /// Its retrievals.
    pub retrievals: u64,
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
    /// Its model calls' reasoning tokens, included in output tokens.
    pub reasoning_output_tokens: u64,
    /// What its model calls cost in US dollars, each as reported, else at
    /// its price ([`crate::spend`]); its own, never scaled by a sample rate.
    pub spend: f64,
    /// The models its calls answered with, each once, sorted.
    pub models: Vec<String>,
    /// The tools its calls ran, each once, sorted.
    pub tools: Vec<String>,
    /// The deployments its spans were sent from, each once, sorted: one,
    /// unless the trace crossed services deployed apart.
    pub environments: Vec<String>,
    /// The releases its spans were sent from, each once, sorted.
    pub releases: Vec<String>,
    /// Its tags as `key:value`, any of its spans carrying them, each once,
    /// sorted.
    pub tags: Vec<String>,
}

/// How many traces the filter admits in the window.
#[derive(Debug, Row, Deserialize)]
pub(crate) struct CountRow {
    pub(crate) total: u64,
}

/// The list's filters, which are its facets: each one word, the URL's
/// parameter and the facets read's first column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub enum Facet {
    Agent,
    User,
    Session,
    Status,
    Fault,
    Model,
    Tool,
    Environment,
    Release,
    Tag,
    Prompt,
    PromptVersion,
}

// In the sidebar's order.
telmoni_shared::wire_enum!(Facet {
    Agent => "agent",
    User => "user",
    Session => "session",
    Status => "status",
    Fault => "fault",
    Model => "model",
    Tool => "tool",
    Environment => "environment",
    Release => "release",
    Tag => "tag",
    Prompt => "prompt",
    PromptVersion => "prompt_version",
});

impl TryFrom<String> for Facet {
    type Error = telmoni_shared::ParseEnumError;

    fn try_from(word: String) -> Result<Self, Self::Error> {
        word.parse()
    }
}

/// One value of one facet: how many of a list's rows, among those the other
/// filters admit, carry it. A facet the read spells as none of the eleven fails
/// the read, never vanishes from the answer.
#[derive(Debug, Clone, PartialEq, Eq, Row, Deserialize)]
pub struct FacetRow {
    pub facet: Facet,
    /// The value, as stored.
    pub value: String,
    /// How many of the list's traces carry it.
    pub listed: u64,
}

/// One bar of the histogram as the read answers it: the traces that started
/// in it, by how they ended, their tokens, and their durations' median and
/// 95th percentile. A bar no trace started in is not a row.
#[derive(Debug, Clone, PartialEq, Row, Deserialize)]
pub struct BucketRow {
    /// Where the bar starts, in milliseconds since the epoch.
    pub at_ms: i64,
    /// Its traces that ended well.
    pub ok: u64,
    /// That failed.
    pub error: u64,
    /// That were cancelled.
    pub cancelled: u64,
    pub faults: Vec<(String, u64)>,
    /// Their input and output tokens, summed.
    pub token_total: u64,
    /// What their model calls cost, in US dollars.
    pub spend_total: f64,
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
/// bar no trace started in, whose durations are none.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bar {
    /// Where the bar starts.
    pub at: DateTime<Utc>,
    /// Its traces that ended well.
    pub ok: u64,
    /// That failed.
    pub error: u64,
    /// That were cancelled.
    pub cancelled: u64,
    /// Traces carrying each fault; categories may overlap on one trace.
    pub faults: std::collections::BTreeMap<String, u64>,
    /// Their input and output tokens, summed.
    pub tokens: u64,
    /// Inclusive counts of each model's calls, under the whole-trace filters.
    pub tokens_by_model: std::collections::BTreeMap<String, TokenCounts>,
    /// Eligible model calls' output tokens per second after their first token.
    pub output_speed_by_model: std::collections::BTreeMap<String, OutputSpeed>,
    /// What their model calls cost, in US dollars, each trace its own.
    pub spend: f64,
    /// Their durations' median, in milliseconds, where it had traces.
    pub duration_p50_ms: Option<f64>,
    /// And 95th percentile.
    pub duration_p95_ms: Option<f64>,
    /// Their times to first token's median, in milliseconds, where any reported one.
    pub time_to_first_token_p50_ms: Option<f64>,
    /// And 95th percentile.
    pub time_to_first_token_p95_ms: Option<f64>,
    /// The mean of the scores given in it of the score the list is narrowed
    /// to, on the traces it admits; none where it holds none, or the list is
    /// narrowed to no score.
    pub score_mean: Option<f64>,
}

/// A trace's status: the agent's own span's once that has arrived, else
/// `error` where any span failed and `ok` otherwise.
macro_rules! trace_status {
    () => {
        "if(countIf(kind = 'agent') > 0, anyIf(status, kind = 'agent'), \
         if(countIf(status = 'error') > 0, 'error', 'ok'))"
    };
}

/// Whether the trace's sums pass one filter, each admitting every trace
/// while it is empty: what the list keeps and the facets flag.
macro_rules! admits {
    (agent) => {
        "(empty({agents:Array(String)}) OR has({agents:Array(String)}, trace_agent))"
    };
    (user) => {
        "(empty({users:Array(String)}) OR has({users:Array(String)}, trace_user_id))"
    };
    (session) => {
        "(empty({sessions:Array(String)}) OR has({sessions:Array(String)}, trace_session_id))"
    };
    (status) => {
        "(empty({statuses:Array(String)}) OR has({statuses:Array(String)}, trace_status))"
    };
    (fault) => {
        "(empty({faults:Array(String)}) OR hasAny(trace_faults, {faults:Array(String)}))"
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
    (prompt_version) => {
        "(empty({prompt_versions:Array(String)}) \
         OR hasAny(prompt_versions, {prompt_versions:Array(String)}))"
    };
}

/// What a trace carries where any of its spans does, each value once and
/// sorted: the deployments and releases its spans were sent from, its tags
/// as `key:value`, the prompt names among its attributes, and each prompt by
/// name and version, `name:version`, where one span carries both. Named
/// apart from the columns, as the sums below are.
macro_rules! trace_carries {
    () => {
        concat!(
            "arraySort(groupUniqArrayIf(fault, fault != '')) AS trace_faults, ",
            "arraySort(groupUniqArrayIf(environment, environment != '')) AS environments, ",
            "arraySort(groupUniqArrayIf(release, release != '')) AS releases, ",
            "arraySort(groupUniqArrayArray(arrayMap((k, v) -> concat(k, ':', v), ",
            "mapKeys(tags), mapValues(tags)))) AS tag_pairs, ",
            "arraySort(groupUniqArrayIf(attributes['gen_ai.prompt.name'], ",
            "mapContains(attributes, 'gen_ai.prompt.name'))) AS prompts, ",
            "arraySort(groupUniqArrayIf(concat(attributes['gen_ai.prompt.name'], ':', ",
            "attributes['gen_ai.prompt.version']), attributes['gen_ai.prompt.name'] != '' ",
            "AND attributes['gen_ai.prompt.version'] != '')) AS prompt_versions "
        )
    };
}

/// A trace's spans summed, under names no column has, since ClickHouse
/// resolves a SELECT alias before a column of the same name anywhere in the
/// query: `any(agent) AS agent` would turn the `WHERE` on the span's agent
/// into an aggregate, and `countIf(status = …)` into one inside another.
/// The trace's agent is the agent's own span's, the topmost; any span of
/// it names that agent too but a nested agent's of another exporter's.
macro_rules! trace_sums {
    () => {
        concat!(
            "trace_id, ",
            "if(countIf(kind = 'agent') > 0, anyIf(agent, kind = 'agent'), any(agent)) AS trace_agent, ",
            "anyIf(user_id, user_id != '') AS trace_user_id, ",
            "anyIf(session_id, session_id != '') AS trace_session_id, ",
            "CAST(countIf(kind = 'agent') > 0 AS Bool) AS trace_ended, ",
            trace_status!(),
            " AS trace_status, ",
            "anyIf(error_type, kind = 'agent') AS trace_error_type, ",
            "min(started_at) AS first_started_at, max(ended_at) AS last_ended_at, ",
            "greatest(dateDiff('millisecond', first_started_at, last_ended_at), 0) AS duration_ms, ",
            "argMinIf(time_to_first_token, started_at, kind = 'model_call') * 1000 AS time_to_first_token_ms, ",
            "countIf(kind != 'trace_start') AS spans, countIf(kind = 'step') AS steps, ",
            "countIf(kind = 'model_call') AS model_calls, ",
            "countIf(kind = 'tool_call') AS tool_calls, ",
            "countIf(kind = 'retrieval') AS retrievals, ",
            "countIf(status = 'error') AS errors, ",
            "sumIf(input_tokens, kind = 'model_call') AS input_total, ",
            "sumIf(output_tokens, kind = 'model_call') AS output_total, ",
            "sumIf(cache_read_input_tokens, kind = 'model_call') AS cache_read_total, ",
            "sumIf(cache_write_input_tokens, kind = 'model_call') AS cache_write_total, ",
            "sumIf(reasoning_output_tokens, kind = 'model_call') AS reasoning_total, ",
            "input_total + output_total AS tokens, ",
            "sumIf(",
            $crate::spend::spent_of!(),
            ", kind = 'model_call') AS spent, ",
            "arraySort(groupUniqArrayIf(model, kind = 'model_call' AND model != '')) AS models, ",
            "arraySort(groupUniqArrayIf(name, kind = 'tool_call')) AS tools, ",
            trace_carries!()
        )
    };
}

/// The project's spans in the window: what every read sums.
macro_rules! window {
    () => {
        concat!(
            "FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
            // A trace carrying a score of that name, within its bounds, on itself or a span.
            "AND ({score:String} = '' OR trace_id IN (SELECT trace_id FROM telemetry.scores FINAL ",
            "WHERE project_id IN {projects:Array(String)} AND name = {score:String} ",
            "AND value >= {score_min:Float64} AND value <= {score_max:Float64})) "
        )
    };
}

/// The traces the filter admits, a row each: what the list pages through,
/// counts, buckets and downloads. Every filter is read off the sums, the
/// agent's too: a trace holding another exporter's nested agent is summed
/// whole, under its own agent, as the facets count it.
macro_rules! traces {
    () => {
        concat!(
            "SELECT ",
            trace_sums!(),
            window!(),
            "GROUP BY trace_id ",
            "HAVING ",
            admits!(agent),
            " AND ",
            admits!(user),
            " AND ",
            admits!(session),
            " AND ",
            admits!(status),
            " AND ",
            admits!(fault),
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
            admits!(prompt),
            " AND ",
            admits!(prompt_version)
        )
    };
}

pub(crate) use admits;
pub(crate) use trace_carries;
pub(crate) use trace_status;
pub(crate) use trace_sums;
pub(crate) use traces;
pub(crate) use window;

/// The list's SQL with one `ORDER BY`, so every (column, direction) is a
/// literal: the query module takes nothing else. The outer SELECT gives the
/// sums the row's names.
macro_rules! list_sql {
    ($order:expr) => {
        concat!(
            "SELECT trace_id, trace_agent AS agent, trace_user_id AS user_id, ",
            "trace_session_id AS session_id, trace_ended AS ended, trace_status AS status, ",
            "trace_error_type AS error_type, ",
            "first_started_at AS started_at, last_ended_at AS ended_at, duration_ms, ",
            "time_to_first_token_ms, ",
            "spans, steps, model_calls, tool_calls, retrievals, errors, ",
            "input_total AS input_tokens, output_total AS output_tokens, ",
            "cache_read_total AS cache_read_input_tokens, ",
            "cache_write_total AS cache_write_input_tokens, reasoning_total AS reasoning_output_tokens, ",
            "spent AS spend, models, tools, ",
            "environments, releases, tag_pairs AS tags ",
            "FROM (",
            traces!(),
            ") ORDER BY ",
            $order,
            ", started_at DESC, trace_id ",
            "LIMIT {limit:UInt32} OFFSET {offset:UInt64}"
        )
    };
}

/// A list's order by a tag, its key bound as `sort_tag`: each row's least
/// value of it among `$pairs`, its `key:value` pairs sorted, and a row
/// without it last either way.
macro_rules! by_tag {
    ($pairs:literal, $direction:literal) => {
        concat!(
            "empty(arrayFilter(p -> startsWith(p, concat({sort_tag:String}, ':')), ",
            $pairs,
            ")) ASC, arrayFilter(p -> startsWith(p, concat({sort_tag:String}, ':')), ",
            $pairs,
            ")[1] ",
            $direction
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
        (TraceSort::Spend, Asc) => list_sql!("spend ASC"),
        (TraceSort::Spend, Desc) => list_sql!("spend DESC"),
        (TraceSort::Duration, Asc) => list_sql!("duration_ms ASC"),
        (TraceSort::Duration, Desc) => list_sql!("duration_ms DESC"),
        (TraceSort::TimeToFirstToken, Asc) => list_sql!("time_to_first_token_ms ASC"),
        (TraceSort::TimeToFirstToken, Desc) => list_sql!("time_to_first_token_ms DESC"),
    }
}

/// How many traces the filter admits: the list's total, a second read beside
/// its page, since a page past the last holds no row to carry it.
const COUNT_SQL: &str = concat!("SELECT count() AS total FROM (", traces!(), ")");

/// The histogram: the traces the filter admits, by the bar their first span
/// started in. A bar is `{bucket}` milliseconds wide and starts where the
/// reader's clock, `{shift}` milliseconds east of UTC, starts an hour or a
/// day; a trace that started before the window, its spans arriving late, or
/// after the store's now, its clock ahead, is drawn in the bar at that end.
/// `token_total` is named apart from `tokens`, a sum it adds up, for the
/// reason [`trace_sums!`] gives.
const HISTOGRAM_SQL: &str = concat!(
    "SELECT intDiv(toUnixTimestamp64Milli(least(greatest(first_started_at, ",
    "now64(6) - toIntervalDay({days:UInt16})), now64(6))) + {shift:Int64}, {bucket:Int64}) ",
    "* {bucket:Int64} - {shift:Int64} AS at_ms, ",
    "countIf(trace_status = 'ok') AS ok, countIf(trace_status = 'error') AS error, ",
    "countIf(trace_status = 'cancelled') AS cancelled, ",
    "sumMap(mapFromArrays(trace_faults, arrayMap(x -> toUInt64(1), trace_faults))) AS faults, ",
    "sum(tokens) AS token_total, ",
    "sum(spent) AS spend_total, ",
    "quantile(0.5)(duration_ms) AS duration_p50_ms, ",
    "quantile(0.95)(duration_ms) AS duration_p95_ms, ",
    "quantile(0.5)(time_to_first_token_ms) AS time_to_first_token_p50_ms, ",
    "quantile(0.95)(time_to_first_token_ms) AS time_to_first_token_p95_ms ",
    "FROM (",
    traces!(),
    ") GROUP BY at_ms ORDER BY at_ms"
);

/// A list's facets: each value of each facet, and how many of the list's
/// rows (`$count`, of those `$holds` keeps) carry it among the traces every
/// OTHER filter admits — so a value beside a ticked one
/// still counts what ticking it would add. Every trace is summed once, its
/// matches kept as flags, then spread to a row per value it carries; the
/// ticked values first, so none past the first fifty is lost, then the most
/// rows, one more than [`FACET_VALUES_MAX`] each to say there were more.
macro_rules! facets_sql {
    ($count:literal, $holds:literal) => {
        concat!(
            "SELECT tupleElement(facet_value, 1) AS facet, tupleElement(facet_value, 2) AS value, ",
            $count,
            " AS listed ",
            "FROM (SELECT trace_id, ",
            "if(countIf(kind = 'agent') > 0, anyIf(agent, kind = 'agent'), any(agent)) AS trace_agent, ",
            "anyIf(user_id, user_id != '') AS trace_user_id, ",
            "anyIf(session_id, session_id != '') AS trace_session_id, ",
            $crate::traces::trace_status!(),
            " AS trace_status, ",
            "arraySort(groupUniqArrayIf(model, kind = 'model_call' AND model != '')) AS models, ",
            "arraySort(groupUniqArrayIf(name, kind = 'tool_call')) AS tools, ",
            $crate::traces::trace_carries!(),
            ", toUInt8(", $crate::traces::admits!(agent), ") AS m_agent, ",
            "toUInt8(", $crate::traces::admits!(user), ") AS m_user, ",
            "toUInt8(", $crate::traces::admits!(session), ") AS m_session, ",
            "toUInt8(", $crate::traces::admits!(status), ") AS m_status, ",
            "toUInt8(", $crate::traces::admits!(fault), ") AS m_fault, ",
            "toUInt8(", $crate::traces::admits!(model), ") AS m_model, ",
            "toUInt8(", $crate::traces::admits!(tool), ") AS m_tool, ",
            "toUInt8(", $crate::traces::admits!(environment), ") AS m_environment, ",
            "toUInt8(", $crate::traces::admits!(release), ") AS m_release, ",
            "toUInt8(", $crate::traces::admits!(tag), ") AS m_tag, ",
            "toUInt8(", $crate::traces::admits!(prompt), ") AS m_prompt, ",
            "toUInt8(", $crate::traces::admits!(prompt_version), ") AS m_prompt_version, ",
            // How many filters the trace fails, so a value counts where no
            // filter but its own refuses the trace: with its own flag up it
            // needs no miss, with it down that one miss alone.
            "(1 - m_agent) + (1 - m_user) + (1 - m_session) + (1 - m_status) + (1 - m_model) ",
            "+ (1 - m_tool) + (1 - m_environment) + (1 - m_release) + (1 - m_tag) ",
            "+ (1 - m_prompt) + (1 - m_prompt_version) + (1 - m_fault) AS misses ",
            $crate::traces::window!(),
            "GROUP BY trace_id) ",
            "ARRAY JOIN arrayConcat(",
            "[tuple('agent', CAST(trace_agent AS String), toUInt8(misses + m_agent = 1))], ",
            "[tuple('user', CAST(trace_user_id AS String), toUInt8(misses + m_user = 1))], ",
            "[tuple('session', CAST(trace_session_id AS String), toUInt8(misses + m_session = 1))], ",
            "[tuple('status', CAST(trace_status AS String), toUInt8(misses + m_status = 1))], ",
            "arrayMap(x -> tuple('fault', CAST(x AS String), toUInt8(misses + m_fault = 1)), trace_faults), ",
            "arrayMap(x -> tuple('model', CAST(x AS String), toUInt8(misses + m_model = 1)), models), ",
            "arrayMap(x -> tuple('tool', CAST(x AS String), toUInt8(misses + m_tool = 1)), tools), ",
            "arrayMap(x -> tuple('environment', CAST(x AS String), ",
            "toUInt8(misses + m_environment = 1)), environments), ",
            "arrayMap(x -> tuple('release', CAST(x AS String), toUInt8(misses + m_release = 1)), ",
            "releases), ",
            "arrayMap(x -> tuple('tag', CAST(x AS String), toUInt8(misses + m_tag = 1)), tag_pairs), ",
            "arrayMap(x -> tuple('prompt', CAST(x AS String), toUInt8(misses + m_prompt = 1)), ",
            "prompts), ",
            "arrayMap(x -> tuple('prompt_version', CAST(x AS String), ",
            "toUInt8(misses + m_prompt_version = 1)), prompt_versions)",
            ") AS facet_value ",
            "WHERE tupleElement(facet_value, 3) = 1 AND tupleElement(facet_value, 2) != '' ",
            $holds,
            "GROUP BY facet, value ",
            "ORDER BY facet, has(multiIf(facet = 'agent', {agents:Array(String)}, ",
            "facet = 'user', {users:Array(String)}, facet = 'session', {sessions:Array(String)}, ",
            "facet = 'status', {statuses:Array(String)}, facet = 'model', {models:Array(String)}, ",
            "facet = 'fault', {faults:Array(String)}, ",
            "facet = 'tool', {tools:Array(String)}, ",
            "facet = 'environment', {environments:Array(String)}, ",
            "facet = 'release', {releases:Array(String)}, facet = 'tag', {tags:Array(String)}, ",
            "facet = 'prompt', {prompts:Array(String)}, ",
            "facet = 'prompt_version', {prompt_versions:Array(String)}, emptyArrayString()), value) DESC, ",
            "listed DESC, value ",
            "LIMIT 51 BY facet"
        )
    };
}

/// The trace list's facets, each counting the traces that carry a value.
const FACETS_SQL: &str = facets_sql!("count()", "");

/// A trace's spans whole, in the order they started — of two that started
/// together, the one that ended later, which holds the other — up to the one
/// that takes their units past `{units_max}`, summed on the light columns.
const SPANS_SQL: &str = "SELECT project_id, received_at, units, agent, trace_id, \
     span_id, parent_span_id, kind, name, started_at, ended_at, status, error_type, fault, model, \
     provider, input_tokens, output_tokens, cache_read_input_tokens, cache_write_input_tokens, \
     reasoning_output_tokens, \
     cost, time_to_first_token, user_id, session_id, environment, release, tags, sample_rate, \
     attributes, sent \
     FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} AND trace_id = {trace:String} \
     AND span_id IN (SELECT span_id FROM (SELECT span_id, sum(units) OVER (\
     ORDER BY started_at, ended_at DESC, span_id \
     ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS units_before \
     FROM (SELECT span_id, started_at, ended_at, units FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} AND trace_id = {trace:String} \
     ORDER BY started_at, ended_at DESC, span_id LIMIT {limit:UInt32})) \
     WHERE units_before <= {units_max:UInt32}) \
     ORDER BY started_at, ended_at DESC, span_id \
     LIMIT {limit:UInt32}";

/// `read` with the filter's every value bound: each read binds them all,
/// since each names them all.
pub(crate) fn filtered(read: Read, filter: &TraceFilter) -> Read {
    let statuses: Vec<String> = filter.statuses.iter().map(ToString::to_string).collect();
    crate::spend::priced_by(read, &filter.prices)
        .param("days", filter.days)
        .param("agents", &filter.agents)
        .param("users", &filter.user_ids)
        .param("sessions", &filter.session_ids)
        .param("statuses", statuses)
        .param(
            "faults",
            filter
                .faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .param("models", &filter.models)
        .param("tools", &filter.tools)
        .param("environments", &filter.environments)
        .param("releases", &filter.releases)
        .param("tags", &filter.tags)
        .param("prompts", &filter.prompts)
        .param("prompt_versions", &filter.prompt_versions)
        .param(
            "score",
            filter.score.as_ref().map_or("", |s| s.name.as_str()),
        )
        .param(
            "score_min",
            filter
                .score
                .as_ref()
                .and_then(|s| s.min)
                .unwrap_or(f64::MIN),
        )
        .param(
            "score_max",
            filter
                .score
                .as_ref()
                .and_then(|s| s.max)
                .unwrap_or(f64::MAX),
        )
}

/// One list over a window's traces, by what it lists: its rows, its sorts, and the SQL of its three reads.
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
    /// The page's SQL sorted by a tag, `{sort_tag:String}` its key; none for
    /// a list no tag sorts.
    fn tag_sql(_direction: SortDirection) -> Option<&'static str> {
        None
    }
    /// The way a column sorts when the URL names none.
    fn first_direction(sort: Self::Sort) -> SortDirection;
}

/// A list's sort: one of its own columns, or `tag:<key>`, its rows by their
/// value of that tag, a key of 1 to [`TAG_KEY_MAX`] characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListSort<S> {
    Column(S),
    Tag(String),
}

impl<S: std::str::FromStr> ListSort<S> {
    /// `given` as a sort: `tag:` and a key, no character of it a control
    /// one, else one of the list's own words.
    pub fn parse(given: &str) -> Option<Self> {
        match given.strip_prefix("tag:") {
            Some(key) => ((1..=TAG_KEY_MAX).contains(&key.chars().count())
                && !key.chars().any(char::is_control))
            .then(|| Self::Tag(key.to_owned())),
            None => given.parse().ok().map(Self::Column),
        }
    }
}

impl<S: std::fmt::Display> std::fmt::Display for ListSort<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Column(column) => column.fmt(f),
            Self::Tag(key) => write!(f, "tag:{key}"),
        }
    }
}

impl<S: std::fmt::Display> Serialize for ListSort<S> {
    fn serialize<Z: serde::Serializer>(&self, serializer: Z) -> Result<Z::Ok, Z::Error> {
        serializer.collect_str(self)
    }
}

/// The trace list: a trace a row.
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

    fn tag_sql(direction: SortDirection) -> Option<&'static str> {
        Some(match direction {
            SortDirection::Asc => list_sql!(by_tag!("tag_pairs", "ASC")),
            SortDirection::Desc => list_sql!(by_tag!("tag_pairs", "DESC")),
        })
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
            | TraceSort::Spend
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
    (sort, direction): (&ListSort<L::Sort>, SortDirection),
    limit: u32,
    offset: u64,
) -> clickhouse::error::Result<Vec<L::Row>> {
    // A tag sort is refused before a list no tag sorts is read.
    let (sql, tag) = match sort {
        ListSort::Column(column) => (L::list_sql(*column, direction), ""),
        ListSort::Tag(key) => match L::tag_sql(direction) {
            Some(sql) => (sql, key.as_str()),
            None => (L::list_sql(L::FALLBACK, direction), ""),
        },
    };
    filtered(store.read(projects, sql), filter)
        .param("sort_tag", tag)
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
/// east of UTC. A bar no trace started in is not a row; [`bars`] fills it in.
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

/// The mean of the narrowing score given in each bar, on the traces the
/// filter admits, a number's value and a yes or no's 1 or 0, by when it was
/// given.
const SCORE_MEANS_SQL: &str = concat!(
    "SELECT intDiv(toUnixTimestamp64Milli(received_at) + {shift:Int64}, {bucket:Int64}) ",
    "* {bucket:Int64} - {shift:Int64} AS at_ms, avg(value) AS mean ",
    "FROM telemetry.scores FINAL ",
    "WHERE project_id IN {projects:Array(String)} AND name = {score:String} AND is_deleted = 0 ",
    "AND data_type IN ('number', 'boolean') ",
    "AND received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
    "AND trace_id IN (SELECT trace_id FROM (",
    traces!(),
    ")) GROUP BY at_ms ORDER BY at_ms"
);

/// One bar's mean of the narrowing score.
#[derive(Debug, Row, Deserialize)]
pub struct ScoreMeanRow {
    pub at_ms: i64,
    pub mean: f64,
}

/// `SCORE_MEANS_SQL`'s bars, none where the filter names no score.
///
/// # Errors
///
/// ClickHouse did not answer.
pub async fn score_means(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    bucket_seconds: u32,
    shift_minutes: i16,
) -> clickhouse::error::Result<Vec<ScoreMeanRow>> {
    if filter.score.is_none() {
        return Ok(Vec::new());
    }
    filtered(store.read(projects, SCORE_MEANS_SQL), filter)
        .param("bucket", i64::from(bucket_seconds) * 1_000)
        .param("shift", i64::from(shift_minutes) * 60_000)
        .fetch_all()
        .await
}

/// Inclusive input and output counts and the subsets displayed beside them.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct TokenCounts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub reasoning_output_tokens: u64,
}

/// A measured call's output tokens per second after its first token.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "rates use Float64 on both readers; subtract counts before conversion"
)]
pub fn output_speed(
    kind: SpanKind,
    output_tokens: u64,
    duration: chrono::TimeDelta,
    first: Option<f64>,
) -> Option<f64> {
    if kind != SpanKind::ModelCall || output_tokens < 2 {
        return None;
    }
    let first = first?;
    let duration = duration.num_microseconds()? as f64 / 1_000_000.0;
    if !first.is_finite() || first < 0.0 || duration <= first {
        return None;
    }
    let speed = (output_tokens - 1) as f64 / (duration - first);
    (speed.is_finite() && speed > 0.0).then_some(speed)
}

/// The slow tail and median of eligible calls; none without observations.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct OutputSpeed {
    pub p50: Option<f64>,
    pub p05: Option<f64>,
}

macro_rules! call_speed {
    () => {
        "(toFloat64(greatest(toInt128(s.output_tokens), toInt128(1)) - toInt128(1)) / \
         nullIf((toUnixTimestamp64Micro(s.ended_at) - toUnixTimestamp64Micro(s.started_at)) \
         / 1000000.0 - s.time_to_first_token, 0))"
    };
    (valid) => {
        concat!(
            "(s.output_tokens >= 2 AND isFinite(s.time_to_first_token) ",
            "AND s.time_to_first_token >= 0 AND isFinite(",
            call_speed!(),
            ") AND ",
            call_speed!(),
            " > 0)"
        )
    };
}

const TOKENS_BY_MODEL_SQL: &str = concat!(
    "SELECT intDiv(toUnixTimestamp64Milli(least(greatest(t.first_started_at, ",
    "now64(6) - toIntervalDay({days:UInt16})), now64(6))) + {shift:Int64}, {bucket:Int64}) ",
    "* {bucket:Int64} - {shift:Int64} AS at_ms, s.model AS model, ",
    "sum(s.input_tokens) AS input_tokens, sum(s.output_tokens) AS output_tokens, ",
    "sum(s.cache_read_input_tokens) AS cache_read_input_tokens, ",
    "sum(s.cache_write_input_tokens) AS cache_write_input_tokens, ",
    "sum(s.reasoning_output_tokens) AS reasoning_output_tokens, ",
    "countIf(",
    call_speed!(valid),
    ") AS speed_calls, ",
    "quantilesIf(0.05, 0.5)(ifNull(",
    call_speed!(),
    ", 0), ",
    call_speed!(valid),
    ") AS speed_quantiles ",
    "FROM telemetry.spans AS s INNER JOIN (",
    traces!(),
    ") AS t ON s.trace_id = t.trace_id ",
    "WHERE s.project_id IN {projects:Array(String)} AND s.kind = 'model_call' ",
    "AND s.received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
    "GROUP BY at_ms, model ORDER BY at_ms, model"
);

/// One model's counts and speed observations in a trace-start bucket.
#[derive(Debug, Row, Deserialize)]
pub struct ModelTokensRow {
    pub at_ms: i64,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub speed_calls: u64,
    pub speed_quantiles: Vec<f64>,
}

/// Counts and speeds by model under whole-trace admission and its clock.
///
/// # Errors
///
/// ClickHouse did not answer or stopped the read at its time or memory cap.
pub async fn tokens_by_model(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    bucket_seconds: u32,
    shift_minutes: i16,
) -> clickhouse::error::Result<Vec<ModelTokensRow>> {
    filtered(store.read(projects, TOKENS_BY_MODEL_SQL), filter)
        .param("bucket", i64::from(bucket_seconds) * 1_000)
        .param("shift", i64::from(shift_minutes) * 60_000)
        .fetch_all()
        .await
}

/// Merge model metrics without changing the histogram's buckets.
pub fn with_model_metrics(bars: &mut [Bar], rows: Vec<ModelTokensRow>) {
    let mut counts = std::collections::BTreeMap::new();
    let mut speeds = std::collections::BTreeMap::new();
    for row in rows {
        let quantile = |place: usize| -> Option<f64> {
            (row.speed_calls > 0)
                .then(|| row.speed_quantiles.get(place).copied())
                .flatten()
                .filter(|value| value.is_finite() && *value > 0.0)
        };
        speeds
            .entry(row.at_ms)
            .or_insert_with(std::collections::BTreeMap::new)
            .insert(
                row.model.clone(),
                OutputSpeed {
                    p05: quantile(0),
                    p50: quantile(1),
                },
            );
        counts
            .entry(row.at_ms)
            .or_insert_with(std::collections::BTreeMap::new)
            .insert(
                row.model,
                TokenCounts {
                    input_tokens: row.input_tokens,
                    output_tokens: row.output_tokens,
                    cache_read_input_tokens: row.cache_read_input_tokens,
                    cache_write_input_tokens: row.cache_write_input_tokens,
                    reasoning_output_tokens: row.reasoning_output_tokens,
                },
            );
    }
    for bar in bars {
        bar.output_speed_by_model = speeds
            .remove(&bar.at.timestamp_millis())
            .unwrap_or_default();
        bar.tokens_by_model = counts
            .remove(&bar.at.timestamp_millis())
            .unwrap_or_default();
    }
}

/// Each bar's mean of the narrowing score set from `means`, by its start.
pub fn with_score_means(bars: &mut [Bar], means: &[ScoreMeanRow]) {
    for bar in bars {
        let at = bar.at.timestamp_millis();
        bar.score_mean = means
            .iter()
            .find(|row| row.at_ms == at)
            .map(|row| row.mean)
            .filter(|mean| mean.is_finite());
    }
}

/// Where the bar holding `ms` starts: as the read's `intDiv`, on the
/// reader's clock, `shift_ms` east of UTC.
const fn bar_start(ms: i64, bucket_ms: i64, shift_ms: i64) -> i64 {
    (ms + shift_ms).div_euclid(bucket_ms) * bucket_ms - shift_ms
}

/// The histogram's every bar, from the one the window opens in to the one
/// `now_ms` falls in, those no trace started in drawn empty; a bar the read
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
                    faults: row.faults.into_iter().collect(),
                    tokens: row.token_total,
                    tokens_by_model: std::collections::BTreeMap::new(),
                    output_speed_by_model: std::collections::BTreeMap::new(),
                    spend: row.spend_total,
                    duration_p50_ms: Some(row.duration_p50_ms),
                    duration_p95_ms: Some(row.duration_p95_ms),
                    time_to_first_token_p50_ms: row.time_to_first_token_p50_ms,
                    time_to_first_token_p95_ms: row.time_to_first_token_p95_ms,
                    score_mean: None,
                },
                None => Bar {
                    at: start,
                    ok: 0,
                    error: 0,
                    cancelled: 0,
                    faults: std::collections::BTreeMap::new(),
                    tokens: 0,
                    tokens_by_model: std::collections::BTreeMap::new(),
                    output_speed_by_model: std::collections::BTreeMap::new(),
                    spend: 0.0,
                    duration_p50_ms: None,
                    duration_p95_ms: None,
                    time_to_first_token_p50_ms: None,
                    time_to_first_token_p95_ms: None,
                    score_mean: None,
                },
            });
        }
        at += bucket_ms;
    }
    drawn
}

/// The spans of trace `trace_id` in `projects` in the order they started, up
/// to `limit`, through the one past `units` units; none for a trace not
/// theirs.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn spans(
    store: &Store,
    projects: &Projects,
    trace_id: &str,
    limit: u32,
    units: u32,
) -> clickhouse::error::Result<Vec<SpanRow>> {
    store
        .read(projects, SPANS_SQL)
        .param("trace", trace_id)
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

    #[test]
    fn output_speed_uses_measured_microseconds_and_refuses_unknowns() {
        use chrono::TimeDelta;
        for (tokens, micros, first, expected) in [
            (101, 3_000_000, Some(1.0), Some(50.0)),
            (11, 3_000_000, Some(1.0), Some(5.0)),
            (2, 500_000, Some(0.0), Some(2.0)),
            (0, 3_000_000, Some(1.0), None),
            (1, 3_000_000, Some(1.0), None),
            (101, 3_000_000, None, None),
            (101, 3_000_000, Some(3.0), None),
            (101, 3_000_000, Some(4.0), None),
            (2, 0, Some(0.0), None),
            (2, -1, Some(0.0), None),
            (2, 3, Some(0.000001), Some(500_000.0)),
            (2, 3_000_000, Some(-1.0), None),
            (2, 3_000_000, Some(f64::NAN), None),
            (2, 3_000_000, Some(f64::INFINITY), None),
            (
                u64::MAX,
                2_000_000,
                Some(1.0),
                Some(18_446_744_073_709_551_616.0),
            ),
        ] {
            let got = output_speed(
                SpanKind::ModelCall,
                tokens,
                TimeDelta::microseconds(micros),
                first,
            );
            match (got, expected) {
                (Some(got), Some(expected)) => assert!(
                    (got - expected).abs() <= expected * 1e-12,
                    "{got} != {expected}"
                ),
                _ => assert_eq!(got, expected),
            }
        }
        for kind in [SpanKind::Agent, SpanKind::ToolCall] {
            assert_eq!(
                output_speed(kind, 101, TimeDelta::seconds(3), Some(1.0)),
                None
            );
        }
    }

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
        "{prompt_versions:Array(String)}",
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
                    TraceSort::Spend => "spend",
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
        assert!(SPANS_SQL.contains("{trace:String}"));
        assert!(SPANS_SQL.contains("{units_max:UInt32}"));
    }

    /// A trace shows its spans up to the count and while their units fit, a row of none taking none.
    #[test]
    fn a_trace_shows_its_spans_while_their_count_and_units_fit() {
        assert_eq!(shown([1, 1, 1], 5, 8), 3, "a small trace whole");
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

    /// What a trace carries where any of its spans does — its deployments,
    /// releases, tags, prompts and their versions — is summed once, apart
    /// from the columns, and filtered by `hasAny`, so a trace crossing two
    /// deployments answers to either.
    #[test]
    fn what_a_trace_carries_is_summed_once_and_filtered_by_any_of_it() {
        let sql = list_sql(TraceSort::Started, SortDirection::Desc);
        for (alias, filter) in [
            (
                "AS environments",
                "hasAny(environments, {environments:Array(String)})",
            ),
            ("AS releases", "hasAny(releases, {releases:Array(String)})"),
            ("AS tag_pairs", "hasAny(tag_pairs, {tags:Array(String)})"),
            ("AS prompts", "hasAny(prompts, {prompts:Array(String)})"),
            (
                "AS prompt_versions",
                "hasAny(prompt_versions, {prompt_versions:Array(String)})",
            ),
        ] {
            assert_eq!(sql.matches(alias).count(), 1, "{alias}");
            assert!(sql.contains(filter), "{filter}");
            assert!(FACETS_SQL.contains(filter), "the facets: {filter}");
        }
        assert!(
            sql.contains("concat(k, ':', v)"),
            "a tag filters as key:value"
        );
        for facet in [
            "'environment'",
            "'release'",
            "'tag'",
            "'prompt'",
            "'prompt_version'",
        ] {
            assert!(FACETS_SQL.contains(facet), "no facet {facet}");
        }
    }

    /// Every facet is spread to its tuple and ranked by its own filter, in
    /// the facets read, under the one spelling of a trace's status the list
    /// and the facets share.
    #[test]
    fn every_facet_is_counted_and_ranked_and_a_traces_status_is_spelled_once() {
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
        let status = trace_status!();
        assert!(list_sql(TraceSort::Started, SortDirection::Desc).contains(status));
        assert!(FACETS_SQL.contains(status));
        assert!(FACETS_SQL.contains("AS misses"));
    }

    /// Each row of the list carries the class of error its agent's span
    /// ended on, read off that span and never a call's; and its agent is
    /// that span's, not a nested agent's.
    #[test]
    fn the_list_reads_the_agents_own_error_class_and_name() {
        let sql = list_sql(TraceSort::Started, SortDirection::Desc);
        assert!(sql.contains("anyIf(error_type, kind = 'agent') AS trace_error_type"));
        assert!(sql.contains("trace_error_type AS error_type"));
        assert!(sql.contains("anyIf(agent, kind = 'agent'), any(agent)) AS trace_agent"));
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
    fn the_default_filter_is_a_week_of_every_trace() {
        let filter = TraceFilter::default();
        assert_eq!(filter.days, DEFAULT_DAYS);
        assert!(filter.agents.is_empty() && filter.statuses.is_empty());
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
            faults: Vec::new(),
            at_ms,
            ok,
            error: 1,
            cancelled: 0,
            token_total: 10,
            spend_total: 0.0,
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
