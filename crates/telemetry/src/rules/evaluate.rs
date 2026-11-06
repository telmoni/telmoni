//! Each alert, evaluated a minute after its last evaluation, a starter five
//! ([`crate::starters`]), by whichever replica leases it first, a starter's
//! threshold set from its project's history first once it is due: its value
//! read over the window that ends then —
//! the full hours from `spans_hourly`, and from `spans` the part of an hour
//! at either end; a run's cost ([`crate::spend`]), steps, repeats and stalls
//! from `spans` run by run, and an objective's burn from its agent's runs —
//! and settled in a transaction of its own with the severity it sets and the
//! incident that moves with it ([`incidents`]), told once that commits.
//! Before a pass evaluates an organization's alerts it holds them to its
//! plan's number, read at most every few minutes an organization, then reads
//! its batch together ([`Together`]): one query a group of a project's alerts
//! that share what they read, each alert's value the one it reads alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike as _, DurationRound as _, SubsecRound as _, TimeDelta, Utc};
use clickhouse::Row;
use serde::Deserialize;
use uuid::Uuid;

use telmoni_shared::AlertCeilings;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::{
    MissingData, OrganizationId, ProjectId, RuleAggregation, RuleBaseline, RuleMeasure, RuleWindow,
    Severity, SpanKind, TelmoniError,
};

use crate::AppState;
use crate::db::TelemetryLane;
use crate::handler::chain;
use crate::rules::db::{self, RuleRow};
use crate::rules::{Definition, Objective, SLOW_WINDOW, incidents};
use crate::spend::{self, Unread};
use crate::store::{Projects, Read, micros, seconds};

/// How often each alert is evaluated.
pub const EVERY: Duration = Duration::from_secs(60);

/// How often a starter alert is evaluated: watching from the first run, it
/// need not cost a query a minute ([`crate::starters`]).
pub const STARTER_EVERY: Duration = Duration::from_secs(5 * 60);

/// How often a pass looks for the alerts due: well inside [`EVERY`], since an
/// alert comes due a minute after it was read, partway into its pass.
pub const TICK: Duration = Duration::from_secs(10);

/// The loop's name in `telemetry.heartbeats`.
pub const HEARTBEAT: &str = "alerts";

/// How many alerts a replica leases at once.
const BATCH: i64 = 100;

/// How long a lease holds an alert: past it, an evaluation that died with
/// its replica is another's to take.
const LEASE_SECS: i64 = 120;

/// How long a replica works a leased batch: inside its lease, with room for
/// the alert in hand to be read and settled.
const LEASE_WORK: Duration = Duration::from_secs(90);

/// How long an organization's plan is held to before it is read again.
const HOLD_EVERY: Duration = Duration::from_secs(5 * 60);

/// The percentiles [`Totals::quantiles`] answers, in the order `TOTALS_SQL`
/// lists their levels.
const LEVELS: [RuleAggregation; 4] = [
    RuleAggregation::P50,
    RuleAggregation::P90,
    RuleAggregation::P95,
    RuleAggregation::P99,
];

/// An alert's window, as two sources read it: the full hours from the
/// totals, and the rest from the spans, each ignoring what the other reads.
const TOTALS_SQL: &str = concat!(
    "SELECT sum(n) AS spans, sum(f) AS failed, sum(d) AS duration_sum, sum(t) AS tokens, ",
    "CAST(quantilesTDigestMerge(0.5, 0.9, 0.95, 0.99)(q) AS Array(Float64)) AS quantiles ",
    "FROM (",
    "SELECT sum(spans) AS n, sumIf(spans, status = 'error') AS f, sum(duration_ms_sum) AS d, ",
    "sum(input_tokens) + sum(output_tokens) AS t, ",
    "quantilesTDigestMergeState(0.5, 0.9, 0.99)(duration_ms) AS q ",
    "FROM telemetry.spans_hourly ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND hour >= {full_from:DateTime('UTC')} AND hour < {full_to:DateTime('UTC')} ",
    "AND kind = {kind:String} ",
    crate::rules::calls_admitted!(),
    // The rollup keeps a tool call's name as its own column.
    "AND (empty({tools:Array(String)}) OR tool IN {tools:Array(String)}) ",
    "AND (empty({statuses:Array(String)}) OR status IN {statuses:Array(String)}) ",
    "UNION ALL ",
    "SELECT count() AS n, countIf(status = 'error') AS f, sum(duration) AS d, ",
    "sum(input_tokens + output_tokens) AS t, quantilesTDigestState(0.5, 0.9, 0.99)(duration) AS q ",
    "FROM (SELECT status, input_tokens, output_tokens, ",
    "greatest(dateDiff('microsecond', started_at, ended_at), 0) / 1000 AS duration ",
    "FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
    "AND (received_at < {full_from:DateTime('UTC')} OR received_at >= {full_to:DateTime('UTC')}) ",
    "AND kind = {kind:String} ",
    crate::rules::spans_admitted!(),
    "))"
);

/// The filters a run's calls take, each alert's by its place `i` in the
/// `Array(Array(String))` parameters `bind_together` binds: what
/// [`crate::rules::calls_admitted!`] says of one alert, `has` for `IN`.
macro_rules! calls_together {
    () => {
        concat!(
            "(empty({agents:Array(Array(String))}[i + 1]) OR has({agents:Array(Array(String))}[i + 1], agent)) ",
            "AND (empty({models:Array(Array(String))}[i + 1]) OR has({models:Array(Array(String))}[i + 1], model)) ",
            "AND (empty({providers:Array(Array(String))}[i + 1]) OR has({providers:Array(Array(String))}[i + 1], provider)) ",
            "AND (empty({environments:Array(Array(String))}[i + 1]) OR has({environments:Array(Array(String))}[i + 1], environment)) ",
            "AND (empty({releases:Array(Array(String))}[i + 1]) OR has({releases:Array(Array(String))}[i + 1], release)) ",
        )
    };
}

/// [`calls_together!`] and the rest an alert on the totals takes.
macro_rules! admitted_together {
    ($tool:literal) => {
        concat!(
            calls_together!(),
            "AND (empty({tools:Array(Array(String))}[i + 1]) OR ", $tool, ") ",
            "AND (empty({statuses:Array(Array(String))}[i + 1]) OR has({statuses:Array(Array(String))}[i + 1], status)) ",
        )
    };
}

/// Each alert's place in a group read together.
macro_rules! places {
    () => {
        "range(toUInt32(length({agents:Array(Array(String))})))"
    };
}

/// [`TOTALS_SQL`] for every alert of a group at once, one project, kind and
/// window: each row read once and counted for each alert it passes, `i` the
/// alert's place; one whose window held nothing is left out.
const TOTALS_TOGETHER_SQL: &str = concat!(
    "SELECT i, sum(n) AS spans, sum(f) AS failed, sum(d) AS duration_sum, sum(t) AS tokens, ",
    "CAST(quantilesTDigestMerge(0.5, 0.9, 0.95, 0.99)(q) AS Array(Float64)) AS quantiles ",
    "FROM (",
    "SELECT i, sum(spans) AS n, sumIf(spans, status = 'error') AS f, sum(duration_ms_sum) AS d, ",
    "sum(input_tokens) + sum(output_tokens) AS t, ",
    "quantilesTDigestMergeState(0.5, 0.9, 0.99)(duration_ms) AS q ",
    "FROM telemetry.spans_hourly ARRAY JOIN ",
    places!(),
    " AS i ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND hour >= {full_from:DateTime('UTC')} AND hour < {full_to:DateTime('UTC')} ",
    "AND kind = {kind:String} AND ",
    admitted_together!("has({tools:Array(Array(String))}[i + 1], tool)"),
    "GROUP BY i ",
    "UNION ALL ",
    "SELECT i, count() AS n, countIf(status = 'error') AS f, sum(duration) AS d, ",
    "sum(input_tokens + output_tokens) AS t, quantilesTDigestState(0.5, 0.9, 0.99)(duration) AS q ",
    "FROM (SELECT i, status, input_tokens, output_tokens, ",
    "greatest(dateDiff('microsecond', started_at, ended_at), 0) / 1000 AS duration ",
    "FROM telemetry.spans ARRAY JOIN ",
    places!(),
    " AS i ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
    "AND (received_at < {full_from:DateTime('UTC')} OR received_at >= {full_to:DateTime('UTC')}) ",
    "AND kind = {kind:String} AND ",
    admitted_together!("(kind = 'tool_call' AND has({tools:Array(Array(String))}[i + 1], name))"),
    ") GROUP BY i",
    ") GROUP BY i"
);

/// A score alert's window: each score of its name once, as its last row in
/// the window has it, a number's value or a yes or no's 1 or 0, taken back
/// ones left out. Scores are kept by their id, so the window's are found by
/// a scan of the project's: bounded by retention and the plan's alerts.
const SCORES_SQL: &str = concat!(
    "SELECT count() AS count, avg(v) AS mean, max(v) AS top, ",
    "CAST(quantiles(0.5, 0.9, 0.95, 0.99)(v) AS Array(Float64)) AS quantiles ",
    "FROM (SELECT argMax(value, received_at) AS v, argMax(is_deleted, received_at) AS gone ",
    "FROM telemetry.scores ",
    "WHERE project_id IN {projects:Array(String)} AND name = {name:String} ",
    "AND data_type IN ('number', 'boolean') ",
    "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
    "GROUP BY score_id) ",
    "WHERE gone = 0"
);

/// The runs any of whose spans landed in the window, as the alert's filters
/// admit them: agent, environment and release, which all of a run's spans
/// carry; given `""`, every run of the window, as a group read together
/// admits each alert's itself.
macro_rules! window_runs {
    () => {
        $crate::rules::evaluate::window_runs!($crate::rules::calls_admitted!())
    };
    ($($admitted:tt)+) => {
        concat!(
            "run_id IN (SELECT run_id FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= {from:DateTime64(6, 'UTC')} ",
            "AND received_at < {to:DateTime64(6, 'UTC')} ",
            $($admitted)+,
            ") "
        )
    };
}

/// The spans of the window's runs, from a day before it, so a run that
/// began before the window is read whole.
macro_rules! runs_whole {
    () => {
        $crate::rules::evaluate::runs_whole!($crate::rules::calls_admitted!())
    };
    ($($admitted:tt)+) => {
        concat!(
            "FROM telemetry.spans WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= {from:DateTime64(6, 'UTC')} - INTERVAL 1 DAY ",
            "AND received_at < {to:DateTime64(6, 'UTC')} AND ",
            $crate::rules::evaluate::window_runs!($($admitted)+),
            "GROUP BY run_id"
        )
    };
}

/// Each run of the window, `n` its steps.
macro_rules! run_steps {
    () => {
        $crate::rules::evaluate::run_steps!($crate::rules::calls_admitted!())
    };
    ($($admitted:tt)+) => {
        concat!(
            "SELECT run_id, countIf(kind = 'step') AS n, max(received_at) AS last ",
            $crate::rules::evaluate::runs_whole!($($admitted)+)
        )
    };
}

/// Each run of the window, `n` the most times in a row it called one tool,
/// `tool`: its tool calls in the order they started, what lies between them
/// breaking no stretch.
macro_rules! run_repeats {
    () => {
        $crate::rules::evaluate::run_repeats!($crate::rules::calls_admitted!())
    };
    ($($admitted:tt)+) => {
        concat!(
            "SELECT run_id, any(agent) AS agent, max(received_at) AS last, ",
            "arrayMap(c -> c.3, arraySort(groupArrayIf((started_at, span_id, name), ",
            "kind = 'tool_call'))) AS calls, ",
            "arraySplit((c, p) -> c != p, calls, arrayPopBack(arrayPushFront(calls, ''))) ",
            "AS stretches, arrayMap(s -> length(s), stretches) AS lengths, ",
            "arrayMax(lengths) AS n, stretches[indexOf(lengths, n)][1] AS tool ",
            $crate::rules::evaluate::runs_whole!($($admitted)+)
        )
    };
}

/// Each run with spans in the window and no span of its own by `{now}`, quiet
/// since the window ended: `n` how long, in seconds.
macro_rules! stalled_runs {
    () => {
        $crate::rules::evaluate::stalled_runs!($crate::rules::calls_admitted!())
    };
    ($($admitted:tt)+) => {
        concat!(
            "SELECT run_id, max(received_at) AS last, ",
            "toUInt64(greatest(dateDiff('second', last, {now:DateTime64(6, 'UTC')}), 0)) AS n ",
            "FROM telemetry.spans WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= {from:DateTime64(6, 'UTC')} - INTERVAL 1 DAY ",
            "AND received_at < {now:DateTime64(6, 'UTC')} AND ",
            $crate::rules::evaluate::window_runs!($($admitted)+),
            "GROUP BY run_id ",
            "HAVING countIf(kind = 'run') = 0 AND last < {to:DateTime64(6, 'UTC')}"
        )
    };
}

pub(crate) use {run_repeats, run_steps, runs_whole, stalled_runs, window_runs};

/// A read run by run folded over its runs, `n` each one's measure.
macro_rules! folded_runs {
    () => {
        concat!(
            "SELECT count() AS count, avg(n) AS mean, toFloat64(max(n)) AS top, ",
            "CAST(quantiles(0.5, 0.9, 0.95, 0.99)(n) AS Array(Float64)) AS quantiles FROM ("
        )
    };
}

const STEPS_SQL: &str = concat!(folded_runs!(), run_steps!(), ")");
const REPEATS_SQL: &str = concat!(folded_runs!(), run_repeats!(), ")");
const STALLED_SQL: &str = concat!(folded_runs!(), stalled_runs!(), ")");

/// A read run by run for every alert of a group at once, one project,
/// measure and window: each run of the window read once, then counted for
/// each alert one of its spans in the window passes, `i` the alert's place.
macro_rules! folded_together {
    ($runs:expr) => {
        concat!(
            "SELECT i, count() AS count, avg(n) AS mean, toFloat64(max(n)) AS top, ",
            "CAST(quantiles(0.5, 0.9, 0.95, 0.99)(n) AS Array(Float64)) AS quantiles ",
            "FROM (SELECT runs.n AS n, admitted.alerts AS alerts FROM (",
            $runs,
            ") AS runs INNER JOIN (SELECT run_id, groupUniqArrayArray(arrayFilter(i -> ",
            calls_together!(),
            ", ", places!(), ")) AS alerts FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
            "GROUP BY run_id) AS admitted ON runs.run_id = admitted.run_id) ",
            "ARRAY JOIN alerts AS i GROUP BY i"
        )
    };
}

const STEPS_TOGETHER_SQL: &str = folded_together!(run_steps!(""));
const REPEATS_TOGETHER_SQL: &str = folded_together!(run_repeats!(""));
const STALLED_TOGETHER_SQL: &str = folded_together!(stalled_runs!(""));

/// One agent's runs over a window, and those that missed its objective:
/// failed, or for a latency objective, ran past it.
const MISSED_SQL: &str = concat!(
    "SELECT count() AS runs, countIf(if({latency_ms:UInt32} = 0, status = 'error', ",
    "dateDiff('microsecond', started_at, ended_at) > toInt64({latency_ms:UInt32}) * 1000)) ",
    "AS missed FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
    "AND kind = 'run' AND agent = {agent:String}"
);

/// An objective's runs over a window, and those that missed it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Row, Deserialize)]
pub struct Missed {
    pub runs: u64,
    pub missed: u64,
}

/// `agent`'s runs over `window` that missed `objective`.
///
/// # Errors
///
/// ClickHouse did not answer.
pub async fn missed(
    state: &AppState,
    project_id: &ProjectId,
    agent: &str,
    objective: &Objective,
    window: Window,
) -> clickhouse::error::Result<Missed> {
    state
        .store
        .read(&Projects::one(project_id.clone()), MISSED_SQL)
        .param("latency_ms", objective.latency_ms.unwrap_or(0))
        .param("agent", agent)
        .param("from", micros(window.from))
        .param("to", micros(window.to))
        .fetch_one()
        .await
}

/// An objective's burn over its last hour and its last six hours, at `now`.
///
/// # Errors
///
/// ClickHouse did not answer.
pub async fn burns(
    state: &AppState,
    rule: &RuleRow,
    objective: &Objective,
    now: DateTime<Utc>,
) -> clickhouse::error::Result<(f64, f64)> {
    let agent = rule
        .definition
        .filters
        .agent
        .first()
        .map_or("", String::as_str);
    let mut burned = [0.0; 2];
    for (burn, length) in burned
        .iter_mut()
        .zip([RuleWindow::Hour.duration(), SLOW_WINDOW])
    {
        let Missed { runs, missed } = missed(
            state,
            &rule.project_id,
            agent,
            objective,
            Window::ending(now, length),
        )
        .await?;
        *burn = objective.burn(runs, missed);
    }
    Ok((burned[0], burned[1]))
}

/// The severity an objective's burns set, and the value told with it: the
/// hour's at its alert threshold pages, else the six hours' at its warning
/// one warns; the value is the burn that set it, the hour's while neither does.
#[must_use]
pub fn burned(definition: &Definition, fast: f64, slow: f64) -> (Option<f64>, Severity) {
    if fast >= definition.alert_threshold {
        (Some(fast), Severity::Alert)
    } else if definition.warning_threshold.is_some_and(|at| slow >= at) {
        (Some(slow), Severity::Warning)
    } else {
        (Some(fast), Severity::Ok)
    }
}

/// What a window holds of a score alert's scores, or of its runs read run by
/// run: how many, their mean, the largest, and their 50th, 90th, 95th and
/// 99th percentiles.
#[derive(Debug, Clone, Default, PartialEq, Row, Deserialize)]
pub struct Folded {
    pub count: u64,
    pub mean: f64,
    pub top: f64,
    pub quantiles: Vec<f64>,
}

impl Folded {
    /// The alert's value over these, none where there are none.
    #[must_use]
    pub fn value(&self, aggregation: RuleAggregation) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let value = match aggregation {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a count past 2^53 is no alert's"
            )]
            RuleAggregation::Count => self.count as f64,
            RuleAggregation::Avg => self.mean,
            RuleAggregation::Max | RuleAggregation::Sum => self.top,
            percentile => quantile(&self.quantiles, percentile)?,
        };
        value.is_finite().then_some(value)
    }
}

/// `quantiles`' value at `percentile`, read by its place in [`LEVELS`].
fn quantile(quantiles: &[f64], percentile: RuleAggregation) -> Option<f64> {
    LEVELS
        .iter()
        .position(|level| *level == percentile)
        .and_then(|n| quantiles.get(n).copied())
        .filter(|q| q.is_finite())
}

/// What an alert's window holds: its spans, those that failed, their
/// durations summed, their tokens, and their durations at the 50th, 90th,
/// 95th and 99th percentiles.
#[derive(Debug, Clone, Default, PartialEq, Row, Deserialize)]
pub struct Totals {
    pub spans: u64,
    pub failed: u64,
    pub duration_sum: f64,
    pub tokens: u64,
    pub quantiles: Vec<f64>,
}

impl Totals {
    /// The alert's value over these totals, none where they hold no span.
    #[must_use]
    pub fn value(&self, measure: RuleMeasure, aggregation: RuleAggregation) -> Option<f64> {
        if self.spans == 0 {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a count past 2^53 spans is no alert's"
        )]
        let (spans, failed, tokens) = (self.spans as f64, self.failed as f64, self.tokens as f64);
        let at = |percentile: RuleAggregation| quantile(&self.quantiles, percentile);
        match (measure, aggregation) {
            (RuleMeasure::Count, _) => Some(spans),
            (RuleMeasure::Failures, RuleAggregation::Avg) => Some(failed / spans),
            (RuleMeasure::Failures, _) => Some(failed),
            (RuleMeasure::Duration, RuleAggregation::Avg) => Some(self.duration_sum / spans),
            (RuleMeasure::Duration, RuleAggregation::Sum) => Some(self.duration_sum),
            (RuleMeasure::Duration, percentile) => at(percentile),
            (RuleMeasure::Tokens, RuleAggregation::Avg) => Some(tokens / spans),
            (RuleMeasure::Tokens, _) => Some(tokens),
            // Read run by run, from the scores, and from an agent's runs
            // ([`burns`]), which the totals cannot.
            (
                RuleMeasure::Cost
                | RuleMeasure::Score
                | RuleMeasure::Burn
                | RuleMeasure::Steps
                | RuleMeasure::Repeats
                | RuleMeasure::Stalled,
                _,
            ) => None,
        }
    }
}

/// The window ending at `now`, as an alert's read takes it: its start, and the
/// full hours inside it, none where it holds no full hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub full_from: DateTime<Utc>,
    pub full_to: DateTime<Utc>,
}

impl Window {
    /// An alert's window at `now`: the last hour, day or week, or the month
    /// so far from its first at midnight UTC.
    #[must_use]
    pub fn of(now: DateTime<Utc>, window: RuleWindow) -> Self {
        match window {
            RuleWindow::Month => {
                let to = now.trunc_subsecs(6);
                let first = to
                    .date_naive()
                    .with_day(1)
                    .and_then(|day| day.and_hms_opt(0, 0, 0))
                    .map_or(to, |at| at.and_utc());
                Self::between(first, to)
            }
            trailing => Self::ending(now, trailing.duration()),
        }
    }

    /// The `length` of time that ends at `now`, to the microsecond a span's
    /// arrival is kept to.
    #[must_use]
    pub fn ending(now: DateTime<Utc>, length: Duration) -> Self {
        let to = now.trunc_subsecs(6);
        let from = to - TimeDelta::from_std(length).unwrap_or(TimeDelta::zero());
        Self::between(from, to)
    }

    /// From `from` to `to`, its full hours between apart.
    #[must_use]
    pub fn between(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        let hour = TimeDelta::hours(1);
        let floor = |at: DateTime<Utc>| at.duration_trunc(hour).unwrap_or(at);
        let full_from = if floor(from) == from {
            from
        } else {
            floor(from) + hour
        };
        let full_to = floor(to);
        Self {
            from,
            to,
            full_from,
            full_to: full_to.max(full_from),
        }
    }
}

/// `sql`, a read run by run, bound for `rule` at `now`: over its window, or
/// for a stall the window before it, whose runs have been quiet since.
pub(crate) fn by_run(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
    sql: &'static str,
) -> Read {
    let window = Window::of(now, rule.definition.window);
    let runs = if rule.definition.measure == RuleMeasure::Stalled {
        Window::ending(window.from, rule.definition.window.duration())
    } else {
        window
    };
    let read = state
        .store
        .read(&Projects::one(rule.project_id.clone()), sql)
        .param("from", micros(runs.from))
        .param("to", micros(runs.to))
        .param("now", micros(window.to));
    rule.definition.filters.bind_calls(read)
}

/// The alert's totals over its window ending at `now`.
pub async fn totals(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
) -> clickhouse::error::Result<Totals> {
    let window = Window::of(now, rule.definition.window);
    let read = state
        .store
        .read(&Projects::one(rule.project_id.clone()), TOTALS_SQL)
        .param("from", micros(window.from))
        .param("to", micros(window.to))
        .param("full_from", seconds(window.full_from))
        .param("full_to", seconds(window.full_to))
        .param("kind", rule.definition.kind.to_string());
    rule.definition.filters.bind(read).fetch_one().await
}

/// What a pass read together for its batch before evaluating it: each
/// alert's value at each time its reading asks for, by the alert and the time.
#[derive(Debug, Default)]
pub struct Together(HashMap<(Uuid, DateTime<Utc>), Option<f64>>);

/// Alerts one query reads together: those of a project on the totals that
/// share a kind and a window, or read run by run that share a measure and a
/// window, at one time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Group {
    Totals(ProjectId, SpanKind, RuleWindow, DateTime<Utc>),
    Runs(ProjectId, RuleMeasure, RuleWindow, DateTime<Utc>),
}

/// [`Totals`] of the alert at place `i`.
#[derive(Debug, Row, Deserialize)]
struct TotalsAt {
    i: u32,
    spans: u64,
    failed: u64,
    duration_sum: f64,
    tokens: u64,
    quantiles: Vec<f64>,
}

/// [`Folded`] of the alert at place `i`.
#[derive(Debug, Row, Deserialize)]
struct FoldedAt {
    i: u32,
    count: u64,
    mean: f64,
    top: f64,
    quantiles: Vec<f64>,
}

impl Together {
    /// `rules`' values at `now`, read together where they can be: one query a
    /// group, however many alerts it holds, each alert's filters by its place.
    /// A score, a cost and an objective are read alone, as is every alert of
    /// a group ClickHouse does not answer, logged.
    pub async fn read(state: &AppState, rules: &[&RuleRow], now: DateTime<Utc>) -> Self {
        let mut groups: HashMap<Group, Vec<&RuleRow>> = HashMap::new();
        for rule in rules {
            let definition = &rule.definition;
            if definition.objective.is_some() {
                continue;
            }
            let mut times = vec![now];
            if definition.baseline == RuleBaseline::Previous {
                times.push(
                    now - TimeDelta::from_std(definition.window.duration()).unwrap_or_default(),
                );
            }
            for at in times {
                let project = rule.project_id.clone();
                let group = match definition.measure {
                    RuleMeasure::Count
                    | RuleMeasure::Failures
                    | RuleMeasure::Duration
                    | RuleMeasure::Tokens => {
                        Group::Totals(project, definition.kind, definition.window, at)
                    }
                    RuleMeasure::Steps | RuleMeasure::Repeats | RuleMeasure::Stalled => {
                        Group::Runs(project, definition.measure, definition.window, at)
                    }
                    RuleMeasure::Cost | RuleMeasure::Score | RuleMeasure::Burn => continue,
                };
                groups.entry(group).or_default().push(rule);
            }
        }
        let mut together = Self::default();
        for (group, members) in groups {
            let at = match &group {
                Group::Totals(.., at) | Group::Runs(.., at) => *at,
            };
            match group_values(state, &group, &members).await {
                Ok(values) => {
                    for (rule, value) in members.iter().zip(values) {
                        together.0.insert((rule.id, at), value);
                    }
                }
                Err(error) => tracing::warn!(
                    alerts = members.len(),
                    kind = crate::store::Fault::of(&error).as_str(),
                    "telemetry: alerts read together could not be; each is read alone"
                ),
            }
        }
        together
    }

    /// The value read together for `rule` at `at`, if it was.
    #[must_use]
    pub fn of(&self, rule: &RuleRow, at: DateTime<Utc>) -> Option<Option<f64>> {
        self.0.get(&(rule.id, at)).copied()
    }
}

/// `read` bound to each of `members`' filters by its place.
fn bind_together(read: Read, members: &[&RuleRow]) -> Read {
    let field = |of: fn(&crate::rules::RuleFilters) -> Vec<String>| -> Vec<Vec<String>> {
        members
            .iter()
            .map(|rule| of(&rule.definition.filters))
            .collect()
    };
    read.param("agents", field(|filters| filters.agent.clone()))
        .param("models", field(|filters| filters.model.clone()))
        .param("providers", field(|filters| filters.provider.clone()))
        .param("environments", field(|filters| filters.environment.clone()))
        .param("releases", field(|filters| filters.release.clone()))
        .param("tools", field(|filters| filters.tool.clone()))
        .param(
            "statuses",
            field(|filters| filters.status.iter().map(ToString::to_string).collect()),
        )
}

/// Each of `members`' values over `group`'s window, in their order: as
/// [`totals`] and [`by_run`] read one alone, each alert's own filters and
/// aggregation.
async fn group_values(
    state: &AppState,
    group: &Group,
    members: &[&RuleRow],
) -> clickhouse::error::Result<Vec<Option<f64>>> {
    match group {
        Group::Totals(project, kind, window, at) => {
            let window = Window::of(*at, *window);
            let read = state
                .store
                .read(&Projects::one(project.clone()), TOTALS_TOGETHER_SQL)
                .param("from", micros(window.from))
                .param("to", micros(window.to))
                .param("full_from", seconds(window.full_from))
                .param("full_to", seconds(window.full_to))
                .param("kind", kind.to_string());
            let rows: Vec<TotalsAt> = bind_together(read, members).fetch_all().await?;
            let mut totals = vec![Totals::default(); members.len()];
            for row in rows {
                if let Some(slot) = usize::try_from(row.i).ok().and_then(|i| totals.get_mut(i)) {
                    *slot = Totals {
                        spans: row.spans,
                        failed: row.failed,
                        duration_sum: row.duration_sum,
                        tokens: row.tokens,
                        quantiles: row.quantiles,
                    };
                }
            }
            Ok(members
                .iter()
                .zip(totals)
                .map(|(rule, totals)| {
                    totals.value(rule.definition.measure, rule.definition.aggregation)
                })
                .collect())
        }
        Group::Runs(project, measure, window_kind, at) => {
            let sql = match measure {
                RuleMeasure::Steps => STEPS_TOGETHER_SQL,
                RuleMeasure::Repeats => REPEATS_TOGETHER_SQL,
                _ => STALLED_TOGETHER_SQL,
            };
            let window = Window::of(*at, *window_kind);
            let runs = if *measure == RuleMeasure::Stalled {
                Window::ending(window.from, window_kind.duration())
            } else {
                window
            };
            let read = state
                .store
                .read(&Projects::one(project.clone()), sql)
                .param("from", micros(runs.from))
                .param("to", micros(runs.to))
                .param("now", micros(window.to));
            let rows: Vec<FoldedAt> = bind_together(read, members).fetch_all().await?;
            let mut folded = vec![Folded::default(); members.len()];
            for row in rows {
                if let Some(slot) = usize::try_from(row.i).ok().and_then(|i| folded.get_mut(i)) {
                    *slot = Folded {
                        count: row.count,
                        mean: row.mean,
                        top: row.top,
                        quantiles: row.quantiles,
                    };
                }
            }
            Ok(members
                .iter()
                .zip(folded)
                .map(|(rule, folded)| folded.value(rule.definition.aggregation))
                .collect())
        }
    }
}

/// What one replica holds of the organizations' plans: when it last held
/// each to its number.
#[derive(Default)]
pub struct Held(Mutex<HashMap<OrganizationId, Instant>>);

/// Whether this replica is stopping, and the pass it is in: a pass stops
/// between alerts once asked, and the process waits for it, so no incident
/// a pass settled goes untold.
#[derive(Default)]
pub struct Stopping {
    asked: AtomicBool,
    pub(crate) pass: tokio::sync::Mutex<()>,
}

impl Stopping {
    /// Ask every pass to stop between alerts.
    pub fn ask(&self) {
        self.asked.store(true, Ordering::SeqCst);
    }

    pub(crate) fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }

    /// Ask, then wait up to `wait` for the pass in hand to end.
    pub async fn stop(&self, wait: Duration) {
        self.ask();
        let _ = tokio::time::timeout(wait, self.pass.lock()).await;
    }
}

impl Held {
    /// Held again at the next pass, its plan's answer not had.
    fn forget(&self, organization_id: &OrganizationId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(organization_id);
    }

    /// Whether `organization_id` is due to be held again, marking it held if so.
    fn due(&self, organization_id: &OrganizationId) -> bool {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if held
            .get(organization_id)
            .is_some_and(|at| at.elapsed() < HOLD_EVERY)
        {
            return false;
        }
        held.retain(|_, at| at.elapsed() < HOLD_EVERY);
        held.insert(organization_id.clone(), Instant::now());
        true
    }
}

/// [`pass`], every [`TICK`] until the replica stops.
pub async fn run(state: Arc<AppState>) {
    state
        .heartbeats
        .every(&state.db, HEARTBEAT, Duration::ZERO, TICK, || {
            let state = state.clone();
            async move {
                if state.alerts_stopping.asked() {
                    // Never ends: the process is ending, and a loop that ended
                    // would read to it as one that died.
                    std::future::pending::<()>().await;
                }
                pass(&state).await.map(drop)
            }
        })
        .await;
}

/// Every alert due, a batch at a time: each organization held to its plan
/// first, then each of its running alerts evaluated and settled. Answers
/// how many were settled. Ends early when Postgres does not answer, and an
/// alert ClickHouse cannot answer for waits for the next pass.
pub async fn pass(state: &AppState) -> Result<u64, TelmoniError> {
    pass_within(state, LEASE_WORK).await
}

/// [`pass`], working each leased batch for at most `work`: what is left past
/// it, or once the replica is stopping, is let go for another to take.
pub async fn pass_within(state: &AppState, work: Duration) -> Result<u64, TelmoniError> {
    let _pass = state.alerts_stopping.pass.lock().await;
    let mut settled = 0u64;
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let held_by_plan = db::held_by_plan(&mut tx).await?;
    tx.commit().await?;
    for organization_id in &held_by_plan {
        hold(state, organization_id).await?;
    }
    loop {
        if state.alerts_stopping.asked() {
            return Ok(settled);
        }
        let lease_id = Uuid::new_v4();
        let leased_at = Instant::now();
        let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
        let leased = db::lease_due(
            &mut tx,
            (
                i64::try_from(EVERY.as_secs()).unwrap_or(60),
                i64::try_from(STARTER_EVERY.as_secs()).unwrap_or(300),
            ),
            lease_id,
            LEASE_SECS,
            BATCH,
        )
        .await?;
        tx.commit().await?;
        let count = leased.len();
        let mut paused: Vec<Uuid> = Vec::new();
        let mut closing: Vec<OrganizationId> = Vec::new();
        for organization_id in organizations(&leased) {
            // One being erased is told nothing, its feed and connectors about to go.
            if !chain::open(state.auth.as_ref(), &organization_id).await {
                closing.push(organization_id);
                continue;
            }
            if hold(state, &organization_id).await? > 0 {
                paused.extend(newly_paused(state, &organization_id, &leased).await?);
            }
        }
        // One moment for the batch, so what its alerts read together is what
        // each would read alone.
        let now = Utc::now();
        let due: Vec<&RuleRow> = leased
            .iter()
            .filter(|rule| !paused.contains(&rule.id) && !closing.contains(&rule.organization_id))
            .collect();
        let together = Together::read(state, &due, now).await;
        let mut rules = leased.into_iter();
        while let Some(rule) = rules.next() {
            if state.alerts_stopping.asked() || leased_at.elapsed() >= work {
                release(state, &rule, lease_id).await?;
                for rest in rules {
                    release(state, &rest, lease_id).await?;
                }
                return Ok(settled);
            }
            if paused.contains(&rule.id) {
                release(state, &rule, lease_id).await?;
                continue;
            }
            if closing.contains(&rule.organization_id) {
                defer(state, &rule, lease_id).await?;
                continue;
            }
            let rule = crate::starters::tuned(state, rule, now).await;
            if evaluate(state, &rule, lease_id, (now, &together)).await? {
                settled += 1;
            }
        }
        if i64::try_from(count).unwrap_or(i64::MAX) < BATCH {
            return Ok(settled);
        }
    }
}

/// The organizations of `leased`, each once.
fn organizations(leased: &[RuleRow]) -> Vec<OrganizationId> {
    let mut organizations: Vec<OrganizationId> = Vec::new();
    for rule in leased {
        if !organizations.contains(&rule.organization_id) {
            organizations.push(rule.organization_id.clone());
        }
    }
    organizations
}

/// The organization held to its plan's number, where this replica has not
/// held it for [`HOLD_EVERY`]: answers how many of its alerts changed. An
/// answer the plan cannot give holds nothing, logged, as every ceiling's does.
async fn hold(state: &AppState, organization_id: &OrganizationId) -> Result<u64, TelmoniError> {
    if !state.alerts_held.due(organization_id) {
        return Ok(0);
    }
    // An answer not had changes nothing: lifting the plan's pauses would
    // run, and tell, alerts the plan holds back until it answers again.
    let limit = match state.entitlements.alert_ceilings(organization_id).await {
        Ok(ceilings) => ceilings.map(|AlertCeilings { alerts, .. }| i64::from(alerts)),
        Err(e) => {
            tracing::warn!(organization_id = %organization_id, error = %e,
                "telemetry: the plan's alerts could not be read; its alerts are held as they stand");
            state.alerts_held.forget(organization_id);
            return Ok(0);
        }
    };
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let changed = db::hold_to(&mut tx, organization_id, limit).await?;
    tx.commit().await?;
    if changed > 0 {
        tracing::info!(
            organization_id = %organization_id,
            changed,
            "telemetry: an organization's alerts held to its plan"
        );
    }
    Ok(changed)
}

/// Which of `leased`, of the organization, its plan has paused now.
async fn newly_paused(
    state: &AppState,
    organization_id: &OrganizationId,
    leased: &[RuleRow],
) -> Result<Vec<Uuid>, TelmoniError> {
    let ids: Vec<Uuid> = leased
        .iter()
        .filter(|rule| rule.organization_id == *organization_id)
        .map(|rule| rule.id)
        .collect();
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let paused: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM telemetry.rules WHERE id = ANY($1) AND paused IS NOT NULL",
    )
    .bind(&ids)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(paused)
}

/// The rule's lease let go, unsettled.
async fn release(state: &AppState, rule: &RuleRow, lease_id: Uuid) -> Result<(), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    db::release(&mut tx, rule.id, lease_id).await?;
    tx.commit().await?;
    Ok(())
}

/// The rule's lease held for [`EVERY`], unsettled: let go, it would be leased
/// again at once, and every alert like it.
async fn defer(state: &AppState, rule: &RuleRow, lease_id: Uuid) -> Result<(), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let every = i64::try_from(EVERY.as_secs()).unwrap_or(60);
    db::defer(&mut tx, rule.id, lease_id, every).await?;
    tx.commit().await?;
    Ok(())
}

/// The alert's value at `now`: over its window, or, read against the
/// window before, its change from it in percent, none where that window held
/// nothing or 0.
///
/// # Errors
///
/// ClickHouse, or Postgres for a run's cost, did not answer.
pub async fn value_of(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
) -> Result<Option<f64>, Unread> {
    value_with(state, rule, now, &Together::default()).await
}

/// [`value_of`], from what was read `together` where it was.
///
/// # Errors
///
/// As [`value_of`].
pub async fn value_with(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
    together: &Together,
) -> Result<Option<f64>, Unread> {
    let value = value_at(state, rule, now, together).await?;
    if rule.definition.baseline == RuleBaseline::None {
        return Ok(value);
    }
    // A count of an empty window is 0, so traffic that stops reads as -100%.
    let counted = |value: Option<f64>| match counts(&rule.definition) {
        true => value.or(Some(0.0)),
        false => value,
    };
    let Some(value) = counted(value) else {
        return Ok(None);
    };
    let length = TimeDelta::from_std(rule.definition.window.duration()).unwrap_or_default();
    let previous = counted(value_at(state, rule, now - length, together).await?);
    Ok(change(value, previous))
}

/// Whether the alert counts, so an empty window is a 0 rather than nothing.
fn counts(definition: &crate::rules::Definition) -> bool {
    matches!(
        definition.measure,
        RuleMeasure::Count | RuleMeasure::Stalled
    ) || (definition.aggregation == RuleAggregation::Count
        && matches!(
            definition.measure,
            RuleMeasure::Failures | RuleMeasure::Score
        ))
}

/// `value`'s change from `previous`, in percent of it: below 0 for a drop.
fn change(value: f64, previous: Option<f64>) -> Option<f64> {
    previous
        .filter(|previous| *previous != 0.0 && previous.is_finite())
        .map(|previous| (value - previous) / previous.abs() * 100.0)
}

/// The alert's value over its window ending at `now`: a run's cost summed
/// with no tag as the totals are read, the full hours from the hourly totals;
/// folded any other way, or narrowed by a tag, from what each run spent; a
/// score's from the scores; a run's steps, repeats and stalls run by run;
/// every other measure from the totals.
async fn value_at(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
    together: &Together,
) -> Result<Option<f64>, Unread> {
    if let Some(value) = together.of(rule, now) {
        return Ok(value);
    }
    let by_runs = match rule.definition.measure {
        RuleMeasure::Steps => Some(STEPS_SQL),
        RuleMeasure::Repeats => Some(REPEATS_SQL),
        RuleMeasure::Stalled => Some(STALLED_SQL),
        _ => None,
    };
    if let Some(sql) = by_runs {
        let runs: Folded = by_run(state, rule, now, sql).fetch_one().await?;
        return Ok(runs.value(rule.definition.aggregation));
    }
    if rule.definition.measure == RuleMeasure::Score {
        let window = Window::of(now, rule.definition.window);
        let scores: Folded = state
            .store
            .read(&Projects::one(rule.project_id.clone()), SCORES_SQL)
            .param(
                "name",
                rule.definition.score_name.as_deref().unwrap_or_default(),
            )
            .param("from", micros(window.from))
            .param("to", micros(window.to))
            .fetch_one()
            .await?;
        return Ok(scores.value(rule.definition.aggregation));
    }
    if rule.definition.measure == RuleMeasure::Cost {
        let window = Window::of(now, rule.definition.window);
        let projects = Projects::one(rule.project_id.clone());
        let filters = &rule.definition.filters;
        // The totals keep no tag, nor a run's own spend to fold another way.
        if rule.definition.aggregation == RuleAggregation::Sum && filters.tag.is_empty() {
            return spend::summed(state, &projects, window, filters).await;
        }
        let spent = spend::runs(state, &projects, window, filters).await?;
        return Ok(spent.value(rule.definition.aggregation));
    }
    Ok(totals(state, rule, now)
        .await?
        .value(rule.definition.measure, rule.definition.aggregation))
}

/// What an evaluation at `now` reads: the alert's value, and the severity
/// it sets.
async fn reading(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
    together: &Together,
) -> Result<(Option<f64>, Severity), Unread> {
    if let Some(objective) = &rule.definition.objective {
        let (fast, slow) = burns(state, rule, objective, now).await?;
        return Ok(burned(&rule.definition, fast, slow));
    }
    let value = value_with(state, rule, now, together).await?;
    // A window holding no span, read as 0, is kept and told as the 0 it crossed at.
    let value = value.or((rule.definition.missing_data == MissingData::Zero).then_some(0.0));
    Ok((value, incidents::next_severity(rule, value)))
}

/// One alert: its value read and settled. Answers whether it was; one
/// ClickHouse could not answer for waits for the next pass, logged by the
/// error's kind alone, since its text can carry ClickHouse's message.
async fn evaluate(
    state: &AppState,
    rule: &RuleRow,
    lease_id: Uuid,
    (now, together): (DateTime<Utc>, &Together),
) -> Result<bool, TelmoniError> {
    let (value, next) = match reading(state, rule, now, together).await {
        Ok(read) => read,
        Err(Unread::Db(error)) => return Err(error.into()),
        Err(Unread::Store(error)) => {
            tracing::warn!(
                rule_id = %rule.id,
                project_id = %rule.project_id,
                kind = crate::store::Fault::of(&error).as_str(),
                "telemetry: an alert's window could not be read; it is evaluated again in a \
                 minute"
            );
            defer(state, rule, lease_id).await?;
            return Ok(false);
        }
    };
    let (settled, told) = incidents::settle(state, rule, lease_id, value, next, now).await?;
    crate::notify::tell_incidents(state, rule, value, &told, now).await;
    // An incident left standing reaches the connectors a hold kept it from.
    let standing = rule.severity.is_breach() || rule.severity == Severity::NoData;
    if settled
        && standing
        && told
            .iter()
            .all(|one| one.telling == incidents::Telling::Silent)
    {
        crate::notify::tell_held(state, rule, value).await;
    }
    Ok(settled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answer's quantiles are read by position: the SQL must list the
    /// levels in `LEVELS`'s order.
    #[test]
    fn the_totals_read_answers_the_levels_in_order() {
        let listed: Vec<String> = LEVELS
            .iter()
            .filter_map(|percentile| percentile.level())
            .map(|level| level.to_string())
            .collect();
        assert_eq!(listed.len(), LEVELS.len());
        let merge = format!("quantilesTDigestMerge({})", listed.join(", "));
        assert!(TOTALS_SQL.contains(&merge), "{merge} not in {TOTALS_SQL}");
        let levels = format!("quantiles({})", listed.join(", "));
        for sql in [SCORES_SQL, STEPS_SQL, REPEATS_SQL, STALLED_SQL] {
            assert!(sql.contains(&levels), "{levels} not in {sql}");
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("an instant")
            .with_timezone(&Utc)
    }

    /// ⚠ The full hours are the totals', the rest the spans', and neither
    /// reads what the other does.
    #[test]
    fn a_window_reads_its_full_hours_from_the_totals() {
        let day = Window::ending(at("2026-10-26T10:23:45.6Z"), Duration::from_secs(86_400));
        assert_eq!(day.from, at("2026-10-25T10:23:45.6Z"));
        assert_eq!(day.to, at("2026-10-26T10:23:45.6Z"));
        assert_eq!(day.full_from, at("2026-10-25T11:00:00Z"));
        assert_eq!(day.full_to, at("2026-10-26T10:00:00Z"));

        let hour = Window::ending(at("2026-10-26T10:23:45Z"), Duration::from_secs(3_600));
        assert_eq!(
            hour.full_from, hour.full_to,
            "an hour that holds no full hour is read from the spans alone"
        );

        let aligned = Window::ending(at("2026-10-26T11:00:00Z"), Duration::from_secs(3_600));
        assert_eq!(
            (aligned.full_from, aligned.full_to),
            (at("2026-10-26T10:00:00Z"), at("2026-10-26T11:00:00Z")),
            "an hour on the hour is the totals' whole"
        );
    }

    /// Scores and runs fold as their aggregation says, none where there are none.
    #[test]
    fn a_fold_is_its_aggregation_and_none_without_any() {
        let scores = Folded {
            count: 4,
            mean: 0.6,
            top: 1.0,
            quantiles: vec![0.5, 0.9, 0.95, 0.99],
        };
        assert_eq!(scores.value(RuleAggregation::Count), Some(4.0));
        assert_eq!(scores.value(RuleAggregation::Avg), Some(0.6));
        assert_eq!(scores.value(RuleAggregation::Max), Some(1.0));
        assert_eq!(scores.value(RuleAggregation::P90), Some(0.9));
        assert_eq!(Folded::default().value(RuleAggregation::Avg), None);
        let empty_mean = Folded {
            mean: f64::NAN,
            ..scores
        };
        assert_eq!(
            empty_mean.value(RuleAggregation::Avg),
            None,
            "no NaN is a value"
        );
    }

    /// A change from the window before is in percent of it, below 0 for a
    /// drop; none against a window that held nothing or 0.
    #[test]
    fn a_change_is_in_percent_of_the_window_before() {
        let near = |got: Option<f64>, want: f64| got.is_some_and(|got| (got - want).abs() < 1e-9);
        assert!(near(change(0.6, Some(0.8)), -25.0));
        assert!(near(change(1.2, Some(0.8)), 50.0));
        assert!(
            near(change(-1.0, Some(-2.0)), 50.0),
            "of the before's size, whatever its sign"
        );
        assert_eq!(change(0.6, None), None);
        assert_eq!(change(0.6, Some(0.0)), None);
    }

    /// ⚠ The hour pages at its threshold, else the six hours warn at theirs,
    /// each told with its own burn.
    #[test]
    fn an_objectives_hour_pages_and_its_six_hours_warn() {
        let definition = crate::rules::RuleInput {
            name: "Support succeeds".into(),
            kind: telmoni_shared::SpanKind::Run,
            measure: RuleMeasure::Burn,
            aggregation: RuleAggregation::Avg,
            filters: crate::rules::RuleFilters {
                agent: vec!["support-bot".into()],
                ..crate::rules::RuleFilters::default()
            },
            window: RuleWindow::Hour,
            operator: telmoni_shared::RuleOperator::Gte,
            alert_threshold: 0.0,
            warning_threshold: None,
            missing_data: MissingData::Zero,
            renotify_minutes: None,
            score_name: None,
            baseline: RuleBaseline::None,
            objective: Some(Objective {
                target: 0.99,
                latency_ms: None,
                days: 30,
            }),
            connectors: Vec::new(),
        }
        .checked()
        .expect("an objective")
        .definition;
        assert_eq!(
            burned(&definition, 20.0, 1.0),
            (Some(20.0), Severity::Alert)
        );
        assert_eq!(
            burned(&definition, 14.4, 1.0),
            (Some(14.4), Severity::Alert)
        );
        assert_eq!(
            burned(&definition, 10.0, 7.0),
            (Some(7.0), Severity::Warning)
        );
        assert_eq!(burned(&definition, 3.0, 2.0), (Some(3.0), Severity::Ok));
    }

    #[test]
    fn a_value_is_its_measure_folded_and_none_without_spans() {
        let totals = Totals {
            spans: 4,
            failed: 1,
            duration_sum: 1_000.0,
            tokens: 400,
            quantiles: vec![100.0, 400.0, 450.0, 490.0],
        };
        let value = |m, a| totals.value(m, a);
        assert_eq!(value(RuleMeasure::Count, RuleAggregation::Count), Some(4.0));
        assert_eq!(
            value(RuleMeasure::Failures, RuleAggregation::Count),
            Some(1.0)
        );
        assert_eq!(
            value(RuleMeasure::Failures, RuleAggregation::Avg),
            Some(0.25)
        );
        assert_eq!(
            value(RuleMeasure::Duration, RuleAggregation::Avg),
            Some(250.0)
        );
        assert_eq!(
            value(RuleMeasure::Duration, RuleAggregation::Sum),
            Some(1_000.0)
        );
        assert_eq!(
            value(RuleMeasure::Duration, RuleAggregation::P95),
            Some(450.0)
        );
        assert_eq!(
            value(RuleMeasure::Duration, RuleAggregation::P99),
            Some(490.0)
        );
        assert_eq!(
            value(RuleMeasure::Tokens, RuleAggregation::Avg),
            Some(100.0)
        );
        assert_eq!(
            value(RuleMeasure::Tokens, RuleAggregation::Sum),
            Some(400.0)
        );
        assert_eq!(
            value(RuleMeasure::Cost, RuleAggregation::Max),
            None,
            "a run's cost is read run by run, never from the totals"
        );
        assert_eq!(
            Totals::default().value(RuleMeasure::Count, RuleAggregation::Count),
            None
        );
    }
}
