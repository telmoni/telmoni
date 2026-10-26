//! Each alert, evaluated a minute after its last evaluation by whichever
//! replica leases it first: its value read over the window that ends then —
//! the full hours from `spans_hourly`, and from `spans` the part of an hour
//! at either end, so a window that holds no full hour reads `spans` alone;
//! a run's cost from `spans` alone, each run's model calls as they landed
//! ([`crate::spend`]) — and settled in a transaction of its own with the
//! severity it sets and the incident that moves with it ([`incidents`]),
//! told once that commits. Before a pass evaluates an organization's alerts
//! it holds them to its plan's number, the oldest running and the rest
//! paused by the plan, read at most every few minutes an organization.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, DurationRound as _, SubsecRound as _, TimeDelta, Utc};
use clickhouse::Row;
use serde::Deserialize;
use uuid::Uuid;

use telmoni_shared::AlertCeilings;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::seam;
use telmoni_shared::{OrganizationId, RuleAggregation, RuleMeasure, SpanStatus, TelmoniError};

use crate::AppState;
use crate::db::TelemetryLane;
use crate::rules::db::{self, RuleRow};
use crate::rules::incidents;
use crate::spend::{self, Unread};
use crate::store::Projects;

/// How often each alert is evaluated.
pub const EVERY: Duration = Duration::from_secs(60);

/// The loop's name in `telemetry.heartbeats`.
pub const HEARTBEAT: &str = "alerts";

/// How many alerts a replica leases at once.
const BATCH: i64 = 100;

/// How long a lease holds an alert: past it, an evaluation that died with
/// its replica is another's to take.
const LEASE_SECS: i64 = 120;

/// How long an organization's plan is held to before it is read again.
const HOLD_EVERY: Duration = Duration::from_secs(5 * 60);

/// The levels a percentile is read at, in the order [`Totals::quantiles`]
/// answers them.
const LEVELS: [f64; 4] = [0.5, 0.9, 0.95, 0.99];

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
    "AND (empty({agents:Array(String)}) OR agent IN {agents:Array(String)}) ",
    "AND (empty({models:Array(String)}) OR model IN {models:Array(String)}) ",
    "AND (empty({providers:Array(String)}) OR provider IN {providers:Array(String)}) ",
    "AND (empty({tools:Array(String)}) OR tool IN {tools:Array(String)}) ",
    "AND (empty({statuses:Array(String)}) OR status IN {statuses:Array(String)}) ",
    "AND (empty({environments:Array(String)}) OR environment IN {environments:Array(String)}) ",
    "AND (empty({releases:Array(String)}) OR release IN {releases:Array(String)}) ",
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
    "AND (empty({agents:Array(String)}) OR agent IN {agents:Array(String)}) ",
    "AND (empty({models:Array(String)}) OR model IN {models:Array(String)}) ",
    "AND (empty({providers:Array(String)}) OR provider IN {providers:Array(String)}) ",
    "AND (empty({tools:Array(String)}) OR (kind = 'tool_call' AND name IN {tools:Array(String)})) ",
    "AND (empty({statuses:Array(String)}) OR status IN {statuses:Array(String)}) ",
    "AND (empty({environments:Array(String)}) OR environment IN {environments:Array(String)}) ",
    "AND (empty({releases:Array(String)}) OR release IN {releases:Array(String)})",
    "))"
);

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
        let at = |level: f64| {
            LEVELS
                .iter()
                .position(|l| (*l - level).abs() < f64::EPSILON)
                .and_then(|n| self.quantiles.get(n).copied())
                .filter(|q| q.is_finite())
        };
        match (measure, aggregation) {
            (RuleMeasure::Count, _) => Some(spans),
            (RuleMeasure::Failures, RuleAggregation::Avg) => Some(failed / spans),
            (RuleMeasure::Failures, _) => Some(failed),
            (RuleMeasure::Duration, RuleAggregation::Avg) => Some(self.duration_sum / spans),
            (RuleMeasure::Duration, RuleAggregation::Sum) => Some(self.duration_sum),
            (RuleMeasure::Duration, percentile) => percentile.level().and_then(at),
            (RuleMeasure::Tokens, RuleAggregation::Avg) => Some(tokens / spans),
            (RuleMeasure::Tokens, _) => Some(tokens),
            // Read run by run ([`crate::spend`]), which the totals cannot.
            (RuleMeasure::Cost, _) => None,
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
    /// The `length` of time that ends at `now`, to the microsecond a span's
    /// arrival is kept to.
    #[must_use]
    pub fn ending(now: DateTime<Utc>, length: Duration) -> Self {
        let to = now.trunc_subsecs(6);
        let from = to - TimeDelta::from_std(length).unwrap_or(TimeDelta::zero());
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

/// The alert's totals over its window ending at `now`.
pub async fn totals(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
) -> clickhouse::error::Result<Totals> {
    let window = Window::ending(now, rule.lookback.duration());
    let filters = &rule.filters.0;
    let statuses: Vec<String> = filters.status.iter().map(SpanStatus::to_string).collect();
    state
        .store
        .read(&Projects::one(rule.project_id.clone()), TOTALS_SQL)
        .param("from", micros(window.from))
        .param("to", micros(window.to))
        .param("full_from", seconds(window.full_from))
        .param("full_to", seconds(window.full_to))
        .param("kind", rule.kind.to_string())
        .param("agents", &filters.agent)
        .param("models", &filters.model)
        .param("providers", &filters.provider)
        .param("tools", &filters.tool)
        .param("statuses", statuses)
        .param("environments", &filters.environment)
        .param("releases", &filters.release)
        .fetch_one()
        .await
}

/// An instant as a `DateTime64(6, 'UTC')` parameter reads it.
pub(crate) fn micros(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

/// An instant as a `DateTime('UTC')` parameter reads it.
fn seconds(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// What one replica holds of the organizations' plans: when it last held
/// each to its number.
#[derive(Default)]
pub struct Held(Mutex<HashMap<OrganizationId, Instant>>);

impl Held {
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

/// [`pass`], every minute, forever; a pass that ends beats, and one that
/// panics ends itself alone.
pub async fn run(state: Arc<AppState>) {
    state.heartbeats.runs(HEARTBEAT, EVERY);
    let mut ticker = tokio::time::interval(EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let evaluated = tokio::spawn({
            let state = state.clone();
            async move { pass(&state).await }
        });
        match evaluated.await {
            Ok(Ok(_)) => state.heartbeats.beat(&state.db, HEARTBEAT).await,
            Ok(Err(e)) => tracing::warn!(
                error = %e,
                "telemetry: the alerts' evaluation failed; the next pass retries"
            ),
            Err(e) => tracing::error!(
                error = %e,
                "telemetry: the alerts' evaluation died; the next pass retries"
            ),
        }
    }
}

/// Every alert due, a batch at a time: each organization held to its plan
/// first, then each of its running alerts evaluated and settled. Answers
/// how many were settled. Ends early only when Postgres does not answer: an
/// alert ClickHouse cannot answer for is let go, logged, for the next pass.
pub async fn pass(state: &AppState) -> Result<u64, TelmoniError> {
    let mut settled = 0u64;
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let held_by_plan = db::held_by_plan(&mut tx).await?;
    tx.commit().await?;
    for organization_id in &held_by_plan {
        hold(state, organization_id).await?;
    }
    loop {
        let lease_id = Uuid::new_v4();
        let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
        let leased = db::lease_due(
            &mut tx,
            i64::try_from(EVERY.as_secs()).unwrap_or(60),
            lease_id,
            LEASE_SECS,
            BATCH,
        )
        .await?;
        tx.commit().await?;
        let count = leased.len();
        let mut paused: Vec<Uuid> = Vec::new();
        for organization_id in organizations(&leased) {
            if hold(state, &organization_id).await? > 0 {
                paused.extend(newly_paused(state, &organization_id, &leased).await?);
            }
        }
        for rule in leased {
            if paused.contains(&rule.id) {
                release(state, &rule, lease_id).await?;
                continue;
            }
            if evaluate(state, &rule, lease_id).await? {
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
    let limit = seam::alert_ceilings(state.entitlements.as_ref(), organization_id)
        .await
        .map(|AlertCeilings { alerts, .. }| i64::from(alerts));
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

/// The alert's value over its window ending at `now`: a run's cost from
/// what each run spent, every other measure from the totals.
///
/// # Errors
///
/// ClickHouse, or Postgres for a run's cost, did not answer.
pub async fn value_of(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
) -> Result<Option<f64>, Unread> {
    if rule.measure == RuleMeasure::Cost {
        let window = Window::ending(now, rule.lookback.duration());
        let projects = Projects::one(rule.project_id.clone());
        let spent = spend::runs(state, &projects, window, &rule.filters.0).await?;
        return Ok(spent.value(rule.aggregation));
    }
    Ok(totals(state, rule, now)
        .await?
        .value(rule.measure, rule.aggregation))
}

/// One alert: its value read and settled. Answers whether it was; one
/// ClickHouse could not answer for is let go for the next pass, logged by
/// the error's kind alone, since its text can carry ClickHouse's message.
async fn evaluate(state: &AppState, rule: &RuleRow, lease_id: Uuid) -> Result<bool, TelmoniError> {
    let now = Utc::now();
    let value = match value_of(state, rule, now).await {
        Ok(value) => value,
        Err(Unread::Db(error)) => return Err(error.into()),
        Err(Unread::Store(error)) => {
            tracing::warn!(
                rule_id = %rule.id,
                project_id = %rule.project_id,
                kind = crate::store::error_kind(&error),
                "telemetry: an alert's window could not be read; it is evaluated again at the \
                 next pass"
            );
            release(state, rule, lease_id).await?;
            return Ok(false);
        }
    };
    let (settled, told) = incidents::settle(state, rule, lease_id, value, now).await?;
    crate::notify::tell_incidents(state, rule, value, &told).await;
    Ok(settled)
}

#[cfg(test)]
mod tests {
    use super::*;

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
