//! A project's hourly totals over a window, by the hour or the day and, where
//! asked, by one column: what `/v1/metrics` publishes and the dashboards page
//! draws. Spend is priced as it is read ([`crate::spend`]), never written.

use chrono::{DateTime, DurationRound as _, TimeDelta, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::{ProjectId, TelmoniError};

use crate::AppState;
use crate::handler::traces::{self, read_failed};
use crate::store::{Projects, seconds};

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
}

/// The columns the totals may be grouped by, each as the rollup names it.
pub const GROUPS: [&str; 7] = [
    "agent",
    "model",
    "provider",
    "tool",
    "status",
    "environment",
    "release",
];

/// The rollup over the window, by the hour or the day and, where asked, by
/// one column: both chosen by a bound value from a closed set, never an
/// identifier.
const METRICS_SQL: &str = concat!(
    // Each column by the table's name: bare, `spans` in `sumIf` reads the sum's alias.
    "SELECT if({daily:UInt8} = 1, toStartOfDay(h.hour), h.hour) AS at, ",
    "toString(multiIf({group:String} = 'agent', h.agent, {group:String} = 'model', h.model, ",
    "{group:String} = 'provider', h.provider, {group:String} = 'tool', h.tool, ",
    "{group:String} = 'status', h.status, {group:String} = 'environment', h.environment, ",
    "{group:String} = 'release', h.release, '')) AS key, ",
    "toUInt64(sum(h.spans)) AS spans, toUInt64(sumIf(h.spans, h.status = 'error')) AS failed, ",
    "toUInt64(sumIf(h.spans, h.kind = 'run')) AS runs, ",
    "toUInt64(sumIf(h.spans, h.kind = 'run' AND h.status = 'error')) AS failed_runs, ",
    "toUInt64(sum(h.input_tokens)) AS input_tokens, ",
    "toUInt64(sum(h.output_tokens)) AS output_tokens, ",
    "toUInt64(sum(h.cache_read_input_tokens)) AS cache_read_input_tokens, ",
    "toUInt64(sum(h.cache_write_input_tokens)) AS cache_write_input_tokens, ",
    "sum(",
    crate::spend::rollup_spent_of!(),
    ") AS spend, ",
    "sum(h.duration_ms_sum) AS duration_ms_sum, ",
    "CAST(quantilesTDigestMerge(0.5, 0.9, 0.99)(h.duration_ms) AS Array(Float64)) AS quantiles, ",
    "CAST(quantilesTDigestMergeIf(0.5, 0.95, 0.99)(h.duration_ms, h.kind = 'run') ",
    "AS Array(Float64)) AS run_quantiles ",
    "FROM telemetry.spans_hourly AS h ",
    "WHERE h.project_id IN {projects:Array(String)} AND h.hour >= {from:DateTime('UTC')} ",
    "GROUP BY at, key ORDER BY at, key LIMIT {max:UInt32}"
);

/// One point of the rollup as ClickHouse answers it.
#[derive(Debug, Row, Deserialize)]
struct MetricRow {
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime::deserialize")]
    at: DateTime<Utc>,
    key: String,
    spans: u64,
    failed: u64,
    runs: u64,
    failed_runs: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_write_input_tokens: u64,
    spend: f64,
    duration_ms_sum: f64,
    quantiles: Vec<f64>,
    run_quantiles: Vec<f64>,
}

/// What a read of the totals answers.
#[derive(Debug, Serialize)]
pub struct Metrics {
    pub days: u16,
    pub step: &'static str,
    pub group: Option<&'static str>,
    /// Oldest first, each hour or day only where it holds a span.
    pub points: Vec<MetricPoint>,
    /// Whether the window held more than [`METRICS_MAX`] points.
    pub truncated: bool,
}

/// One hour's or day's totals, of one group's value where grouped.
#[derive(Debug, Serialize)]
pub struct MetricPoint {
    pub at: DateTime<Utc>,
    /// The group's value; empty where its spans carry none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub spans: u64,
    pub failed: u64,
    /// The runs that ended in it, and those that failed.
    pub runs: u64,
    pub failed_runs: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    /// What its model calls cost, in US dollars: each as reported, else its
    /// tokens at the project's price or the table's.
    pub spend: f64,
    /// Every span's, in milliseconds: the mean and the 50th, 90th and 99th
    /// percentiles.
    pub duration_ms: Durations,
    /// Its runs' durations at the 50th, 95th and 99th percentiles, in
    /// milliseconds; none where no run ended in it.
    pub run_duration_ms: RunDurations,
}

/// A point's durations, in milliseconds.
#[derive(Debug, Serialize)]
pub struct Durations {
    pub avg: f64,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p99: Option<f64>,
}

/// A point's runs' durations, in milliseconds.
#[derive(Debug, Serialize)]
pub struct RunDurations {
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

/// The project's totals over the window `asked` names: up to 360 days back,
/// 7 when it names none and no further than the plan keeps spans; by the
/// day, unless by the hour; grouped where it names a group.
///
/// # Errors
///
/// A window, a step or a group it does not take; ClickHouse or Postgres did
/// not answer.
pub async fn read(
    state: &AppState,
    project_id: &ProjectId,
    asked: &MetricsAsked,
) -> Result<Metrics, TelmoniError> {
    let days = traces::parse_days(asked.days.as_deref())?;
    let days = crate::capacity::visible_days(state, project_id, days).await;
    let (step, daily) = match asked.step.as_deref() {
        None | Some("day") => ("day", true),
        Some("hour") => ("hour", false),
        Some(_) => return Err(TelmoniError::BadRequest("step is `hour` or `day`".into())),
    };
    let group = match asked.group.as_deref() {
        None => None,
        Some(given) => Some(GROUPS.into_iter().find(|g| *g == given).ok_or_else(|| {
            TelmoniError::BadRequest(format!("group is one of {}", GROUPS.join(", ")))
        })?),
    };
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
        .read(&projects, METRICS_SQL)
        .param("daily", u8::from(daily))
        .param("group", group.unwrap_or(""))
        .param("from", seconds(from))
        .param("max", u32::try_from(METRICS_MAX + 1).unwrap_or(u32::MAX));
    let mut rows: Vec<MetricRow> = crate::spend::priced_by(read, &book)
        .fetch_all()
        .await
        .map_err(read_failed)?;
    let truncated = rows.len() > METRICS_MAX;
    rows.truncate(METRICS_MAX);
    Ok(Metrics {
        days,
        step,
        group,
        points: rows
            .into_iter()
            .map(|row| point(row, group.is_some()))
            .collect(),
        truncated,
    })
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
        if row.runs == 0 {
            None
        } else {
            at(&row.run_quantiles, n)
        }
    };
    MetricPoint {
        at: row.at,
        value: grouped.then(|| row.key.clone()),
        spans: row.spans,
        failed: row.failed,
        runs: row.runs,
        failed_runs: row.failed_runs,
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
        run_duration_ms: RunDurations {
            p50: ran(0),
            p95: ran(1),
            p99: ran(2),
        },
    }
}
