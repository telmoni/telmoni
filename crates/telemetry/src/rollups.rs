//! A project's hourly totals over a window, by the hour or the day and, where
//! asked, by one column: what `/v1/metrics` publishes and the dashboards page
//! draws. Spend is priced as it is read ([`crate::spend`]), never written, and
//! scaled by each row's sample rate, a sum over traces. A tag, an end user and
//! a session are no column of the totals, so a split by one reads the spans,
//! within the plan's retention, its most costly values each and the rest as one.

use chrono::{DateTime, DurationRound as _, TimeDelta, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::types::WireEnum as _;
use telmoni_shared::{MetricsGroup, ProjectId, TelmoniError, TraceSort};

use crate::AppState;
use crate::contract::TAG_KEY_MAX;
use crate::handler::traces::{self, read_failed};
use crate::store::{Projects, seconds};
use crate::traces::ListSort;

/// The most points one read answers.
pub const METRICS_MAX: usize = 10_000;

/// What a read of the totals takes.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsAsked {
    #[serde(default)]
    pub days: Option<String>,
    #[serde(default)]
    pub step: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    /// The tag key a split by `tag` reads.
    #[serde(default)]
    pub tag: Option<String>,
}

/// The most values a split read from the spans names, by what they spent;
/// the rest are summed as one, so a project of many users stays readable.
pub const GROUP_TOP: u32 = 10;

/// Whether `group` is none of the totals' columns, so the spans are read.
#[must_use]
pub const fn from_spans(group: MetricsGroup) -> bool {
    matches!(
        group,
        MetricsGroup::Tag | MetricsGroup::User | MetricsGroup::Session
    )
}

/// The rollup over the window, by the hour or the day and, where asked, by
/// one column: both chosen by a bound value from a closed set, never an
/// identifier.
const METRICS_SQL: &str = concat!(
    // Each column by the table's name: bare, `spans` in `sumIf` reads the sum's alias.
    "SELECT if({daily:UInt8} = 1, toStartOfDay(h.hour), h.hour) AS at, ",
    "toString(multiIf({group:String} = 'agent', h.agent, {group:String} = 'model', h.model, ",
    // `tool` holds a retrieval's source too, which is no tool.
    "{group:String} = 'provider', h.provider, ",
    "{group:String} = 'tool', if(h.kind = 'tool_call', h.tool, ''), ",
    "{group:String} = 'status', h.status, {group:String} = 'environment', h.environment, ",
    "{group:String} = 'release', h.release, '')) AS key, toUInt8(0) AS folded, ",
    "toUInt64(sum(h.spans)) AS spans, toUInt64(sumIf(h.spans, h.status = 'error')) AS failed, ",
    "toUInt64(sumIf(h.spans, h.kind = 'agent')) AS traces, ",
    "toUInt64(sumIf(h.spans, h.kind = 'agent' AND h.status = 'error')) AS failed_traces, ",
    "toUInt64(sum(h.input_tokens)) AS input_tokens, ",
    "toUInt64(sum(h.output_tokens)) AS output_tokens, ",
    "toUInt64(sum(h.cache_read_input_tokens)) AS cache_read_input_tokens, ",
    "toUInt64(sum(h.cache_write_input_tokens)) AS cache_write_input_tokens, ",
    "sum((",
    crate::spend::rollup_spent_of!(),
    ") / h.sample_rate) AS spend, ",
    "sum(h.duration_ms_sum) AS duration_ms_sum, ",
    "CAST(quantilesTDigestMerge(0.5, 0.9, 0.99)(h.duration_ms) AS Array(Float64)) AS quantiles, ",
    "CAST(quantilesTDigestMergeIf(0.5, 0.95, 0.99)(h.duration_ms, h.kind = 'agent') ",
    "AS Array(Float64)) AS trace_quantiles ",
    "FROM telemetry.spans_hourly AS h ",
    // A trace's opening is the agent's, counted by its own span.
    "WHERE h.project_id IN {projects:Array(String)} AND h.hour >= {from:DateTime('UTC')} ",
    "AND h.kind != 'trace_start' ",
    // Newest first, so a window past the limit loses its oldest points.
    "GROUP BY at, key ORDER BY at DESC, key DESC LIMIT {max:UInt32}"
);

/// A span's duration in milliseconds, `s` the spans, as the hourly totals' view keeps it.
macro_rules! span_ms {
    () => {
        "greatest(dateDiff('microsecond', s.started_at, s.ended_at), 0) / 1000"
    };
}

/// The spans over the window, by the hour or the day and by their trace's
/// tag value, end user or session, each summed as the totals are. A trace's
/// value is read off its spans from a day before the window, within what the
/// plan keeps, since a call landing after midnight belongs to a trace that
/// may have started before it; a tag off the agent's own span first, so a
/// trace whose spans disagree counts once.
/// The [`GROUP_TOP`] values that spent most are named, and the rest folded.
const SPANS_METRICS_SQL: &str = concat!(
    "SELECT at, if(folded = 1, '', group_key) AS key, folded, ",
    "toUInt64(sum(n_spans)) AS spans, toUInt64(sum(fails)) AS failed, toUInt64(sum(n_traces)) AS traces, ",
    "toUInt64(sum(n_failed_traces)) AS failed_traces, toUInt64(sum(in_tokens)) AS input_tokens, ",
    "toUInt64(sum(out_tokens)) AS output_tokens, toUInt64(sum(cache_read)) AS cache_read_input_tokens, ",
    "toUInt64(sum(cache_write)) AS cache_write_input_tokens, sum(spent) AS spend, ",
    "sum(ms_sum) AS duration_ms_sum, ",
    "CAST(quantilesTDigestMerge(0.5, 0.9, 0.99)(ms_digest) AS Array(Float64)) AS quantiles, ",
    "CAST(quantilesTDigestMerge(0.5, 0.95, 0.99)(trace_ms_digest) AS Array(Float64)) AS trace_quantiles ",
    // A trace with no value is its own row, never folded, and ranks after every value.
    "FROM (SELECT *, toUInt8(group_key != '' AND dense_rank() OVER ",
    "(ORDER BY group_key = '', key_spend DESC, key_spans DESC, group_key) > {top:UInt32}) AS folded ",
    "FROM (SELECT *, sum(spent) OVER (PARTITION BY group_key) AS key_spend, ",
    "sum(n_spans) OVER (PARTITION BY group_key) AS key_spans FROM (",
    "SELECT if({daily:UInt8} = 1, toStartOfDay(toDateTime(s.received_at, 'UTC')), ",
    "toStartOfHour(toDateTime(s.received_at, 'UTC'))) AS at, k.group_key AS group_key, ",
    "count() AS n_spans, countIf(s.status = 'error') AS fails, countIf(s.kind = 'agent') AS n_traces, ",
    "countIf(s.kind = 'agent' AND s.status = 'error') AS n_failed_traces, sum(s.input_tokens) AS in_tokens, ",
    "sum(s.output_tokens) AS out_tokens, sum(s.cache_read_input_tokens) AS cache_read, ",
    "sum(s.cache_write_input_tokens) AS cache_write, sum((",
    crate::spend::spent_of!(),
    ") / ifNull(s.sample_rate, 1)) AS spent, ",
    "sum(",
    span_ms!(),
    ") AS ms_sum, quantilesTDigestState(0.5, 0.9, 0.99)(",
    span_ms!(),
    ") AS ms_digest, quantilesTDigestStateIf(0.5, 0.95, 0.99)(",
    span_ms!(),
    ", s.kind = 'agent') AS trace_ms_digest ",
    "FROM telemetry.spans AS s LEFT ANY JOIN (",
    "SELECT trace_id, multiIf({group:String} = 'user', anyIf(user_id, user_id != ''), ",
    "{group:String} = 'session', anyIf(session_id, session_id != ''), ",
    "if(countIf(kind = 'agent' AND mapContains(tags, {tag:String})) > 0, ",
    "anyIf(tags[{tag:String}], kind = 'agent' AND mapContains(tags, {tag:String})), ",
    "anyIf(tags[{tag:String}], mapContains(tags, {tag:String})))) AS group_key ",
    "FROM telemetry.spans WHERE project_id IN {projects:Array(String)} ",
    "AND received_at >= {keys_from:DateTime('UTC')} GROUP BY trace_id",
    ") AS k ON s.trace_id = k.trace_id ",
    "WHERE s.project_id IN {projects:Array(String)} AND s.received_at >= {from:DateTime('UTC')} ",
    "AND s.kind != 'trace_start' ",
    "GROUP BY at, group_key))) ",
    "GROUP BY at, folded, key ORDER BY at DESC, folded DESC, key DESC LIMIT {max:UInt32}"
);

/// One point of the rollup as ClickHouse answers it.
#[derive(Debug, Row, Deserialize)]
struct MetricRow {
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime::deserialize")]
    at: DateTime<Utc>,
    key: String,
    folded: u8,
    spans: u64,
    failed: u64,
    traces: u64,
    failed_traces: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_write_input_tokens: u64,
    spend: f64,
    duration_ms_sum: f64,
    quantiles: Vec<f64>,
    trace_quantiles: Vec<f64>,
}

/// What a read of the totals answers.
#[derive(Debug, Serialize)]
pub struct Metrics {
    /// The window read: for a split read from the spans, no more than the
    /// plan keeps of them.
    pub days: u16,
    pub step: &'static str,
    pub group: Option<MetricsGroup>,
    /// The tag key a split by `tag` read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Oldest first, each hour or day only where it holds a span.
    pub points: Vec<MetricPoint>,
    /// Whether the window held more than [`METRICS_MAX`] points, its oldest left out.
    pub truncated: bool,
}

/// One hour's or day's totals, of one group's value where grouped.
#[derive(Debug, Serialize)]
pub struct MetricPoint {
    pub at: DateTime<Utc>,
    /// The group's value; empty where its spans carry none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Whether it sums every value past the [`GROUP_TOP`] that spent most,
    /// its `value` then empty.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub others: bool,
    pub spans: u64,
    pub failed: u64,
    /// The traces that ended in it, and those that failed.
    pub traces: u64,
    pub failed_traces: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    /// What its model calls cost, in US dollars: each as reported, else its
    /// tokens at the project's price or the table's, scaled by its trace's
    /// sample rate.
    pub spend: f64,
    /// Every span's, in milliseconds: the mean and the 50th, 90th and 99th
    /// percentiles.
    pub duration_ms: Durations,
    /// Its traces' durations at the 50th, 95th and 99th percentiles, in
    /// milliseconds; none where no trace ended in it.
    pub trace_duration_ms: TraceDurations,
}

/// A point's durations, in milliseconds.
#[derive(Debug, Serialize)]
pub struct Durations {
    pub avg: f64,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p99: Option<f64>,
}

/// A point's traces' durations, in milliseconds.
#[derive(Debug, Serialize)]
pub struct TraceDurations {
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

/// The project's totals over the window `asked` names: up to 360 days back,
/// 7 when it names none, whatever the plan keeps of the spans, since the
/// totals are kept thirteen months on every plan; by the day, unless by the
/// hour or a day's window; grouped where it names a group, and a split
/// [`from_spans`] no further back than the plan keeps them.
///
/// # Errors
///
/// A window, a step, a group or a tag key it does not take; ClickHouse or
/// Postgres did not answer.
pub async fn read(
    state: &AppState,
    project_id: &ProjectId,
    asked: &MetricsAsked,
) -> Result<Metrics, TelmoniError> {
    let days = traces::parse_days(asked.days.as_deref())?;
    let (step, daily) = match asked.step.as_deref() {
        // A day by the day would be the UTC day so far alone.
        None if days == 1 => ("hour", false),
        None | Some("day") => ("day", true),
        Some("hour") => ("hour", false),
        Some(_) => return Err(TelmoniError::BadRequest("step is `hour` or `day`".into())),
    };
    let group = match asked.group.as_deref() {
        None => None,
        Some(given) => Some(given.parse::<MetricsGroup>().map_err(|_| {
            TelmoniError::BadRequest(format!(
                "group is one of {}",
                MetricsGroup::variants()
                    .iter()
                    .map(|g| g.word())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?),
    };
    let tag = tag_of(group, asked.tag.as_deref())?;
    let spanned = group.is_some_and(from_spans);
    // What the plan keeps of the spans bounds a split's window and the traces it reads.
    let kept = if spanned {
        crate::capacity::visible_days(state, project_id, crate::traces::MAX_DAYS).await
    } else {
        crate::traces::MAX_DAYS
    };
    let days = days.min(kept);
    let unit = if daily {
        TimeDelta::days(1)
    } else {
        TimeDelta::hours(1)
    };
    // Up to the next whole step, so no hour or day reaches past the plan.
    let back = Utc::now() - TimeDelta::days(i64::from(days));
    let from = match back.duration_trunc(unit) {
        Ok(start) if start < back => start + unit,
        _ => back,
    };
    let projects = Projects::one(project_id.clone());
    let book = crate::spend::book_for_rollups(state, &projects, from)
        .await
        .map_err(|unread| match unread {
            crate::spend::Unread::Store(error) => read_failed(error),
            crate::spend::Unread::Db(error) => error.into(),
        })?;
    let read = state
        .store
        .read(
            &projects,
            if spanned {
                SPANS_METRICS_SQL
            } else {
                METRICS_SQL
            },
        )
        .param("daily", u8::from(daily))
        .param("group", group.map_or("", |g| g.word()))
        .param("tag", tag.as_deref().unwrap_or(""))
        .param("top", GROUP_TOP)
        .param("from", seconds(from))
        .param(
            "keys_from",
            seconds((from - TimeDelta::days(1)).max(Utc::now() - TimeDelta::days(i64::from(kept)))),
        )
        .param("max", u32::try_from(METRICS_MAX + 1).unwrap_or(u32::MAX));
    let mut rows: Vec<MetricRow> = crate::spend::priced_by(read, &book)
        .fetch_all()
        .await
        .map_err(read_failed)?;
    let truncated = rows.len() > METRICS_MAX;
    rows.truncate(METRICS_MAX);
    rows.reverse();
    Ok(Metrics {
        days,
        step,
        group,
        tag,
        points: rows
            .into_iter()
            .map(|row| point(row, group.is_some()))
            .collect(),
        truncated,
    })
}

/// The tag key a split by `tag` reads, trimmed: required by it, and refused
/// by every other.
pub(crate) fn tag_of(
    group: Option<MetricsGroup>,
    given: Option<&str>,
) -> Result<Option<String>, TelmoniError> {
    let given = given.map(str::trim).filter(|key| !key.is_empty());
    match (group, given) {
        // A key as a sort by its tag takes one: the list's one rule for a key.
        (Some(MetricsGroup::Tag), Some(key))
            if matches!(
                ListSort::<TraceSort>::parse(&format!("tag:{key}")),
                Some(ListSort::Tag(_))
            ) =>
        {
            Ok(Some(key.to_owned()))
        }
        (Some(MetricsGroup::Tag), _) => Err(TelmoniError::BadRequest(format!(
            "a split by tag names its key in `tag`, 1 to {TAG_KEY_MAX} characters"
        ))),
        (_, Some(_)) => Err(TelmoniError::BadRequest(
            "`tag` is read only with `group=tag`".into(),
        )),
        (_, None) => Ok(None),
    }
}

/// A rollup row as the lanes publish it.
fn point(row: MetricRow, grouped: bool) -> MetricPoint {
    let at = |levels: &[f64], n: usize| levels.get(n).copied().filter(|q| q.is_finite());
    #[expect(
        clippy::cast_precision_loss,
        reason = "a count past 2^53 spans is no hour's"
    )]
    let avg = if row.spans == 0 {
        0.0
    } else {
        row.duration_ms_sum / row.spans as f64
    };
    let ran = |n: usize| {
        if row.traces == 0 {
            None
        } else {
            at(&row.trace_quantiles, n)
        }
    };
    MetricPoint {
        at: row.at,
        value: grouped.then(|| row.key.clone()),
        others: row.folded == 1,
        spans: row.spans,
        failed: row.failed,
        traces: row.traces,
        failed_traces: row.failed_traces,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cache_read_input_tokens: row.cache_read_input_tokens,
        cache_write_input_tokens: row.cache_write_input_tokens,
        spend: row.spend,
        duration_ms: Durations {
            avg,
            p50: at(&row.quantiles, 0),
            p90: at(&row.quantiles, 1),
            p99: at(&row.quantiles, 2),
        },
        trace_duration_ms: TraceDurations {
            p50: ran(0),
            p95: ran(1),
            p99: ran(2),
        },
    }
}
