//! Monitors: each one agent's expected schedule ([`crate::schedule`]) and
//! the grace a trace due has to start in, kept in `telemetry.monitors`, made,
//! edited, paused and deleted by any member of the project
//! ([`crate::handler::monitors`]). Each minute the replica that wins the
//! lock reads every running monitor's agent's latest trace start and end from
//! the spans, and judges it ([`judge`]): a trace not started a minute past its
//! due time is `late`, and past its grace `down` (`missed`); a trace that ended
//! in error makes it `down` (`failed`). Each move opens, raises or resolves
//! its incident as an alert's does, told after its commit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{HasProject, Maintenance, Scoped, maintenance_scope};
use telmoni_shared::{
    MissingData, MonitorCause, MonitorState, OrganizationId, ProjectId, Severity, SpanStatus,
    TelmoniError,
};

use crate::AppState;
use crate::contract::AGENT_MAX;
use crate::db::TelemetryLane;
use crate::handler::chain;
use crate::rules::incidents::{self, Filed, Owner, Standing, Telling, Told};
use crate::rules::{Link, checked_links};
use crate::schedule::{self, Fault, Schedule};
use crate::store::{Projects, micros};

/// The loop's name in `telemetry.heartbeats`.
pub const HEARTBEAT: &str = "monitors";

/// How often the loop judges every monitor.
pub const EVERY: Duration = Duration::from_secs(60);

/// How long past its due time a trace's start may take to arrive before the
/// monitor is `late`: one tick, so a trace started on the minute never is.
pub const LATE_AFTER: TimeDelta = TimeDelta::minutes(1);

/// How far back each read reaches past the last: an insert's flush, and
/// another exporter's spans held before they land, keep their receipt time.
const OVERLAP: TimeDelta = TimeDelta::minutes(10);

/// The furthest a read reaches back, on a replica that has read none.
const CATCH_UP: TimeDelta = TimeDelta::hours(1);

/// How far past its receipt a trace's start is believed, so a clock ahead
/// cannot push its monitor's next due time out.
const SKEW_MAX_SECS: u32 = 300;

/// How long one tick may run: inside the minute, so a read that hangs never
/// holds the lock past the next.
const BUDGET: Duration = Duration::from_secs(50);

/// How many monitors one read judges.
const BATCH: i64 = 500;

/// A monitor as a person writes it: one agent, a period or a cron line in a
/// zone, a grace, and the connectors its notices go to.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorInput {
    pub agent: String,
    #[serde(default)]
    pub every_seconds: Option<i32>,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub time_zone: Option<String>,
    pub grace_seconds: i32,
    #[serde(default)]
    pub connectors: Vec<Link>,
}

/// A monitor checked whole, as the table keeps it.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub agent: String,
    pub schedule: Schedule,
    pub spelled: Spelled,
    pub grace_seconds: i32,
    pub connectors: Vec<Link>,
}

/// A schedule as its columns keep it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spelled {
    Every(i32),
    Cron { line: String, zone: String },
}

impl Spelled {
    /// Its three columns: `every_seconds`, `cron` and `time_zone`.
    fn columns(&self) -> (Option<i32>, Option<&str>, Option<&str>) {
        match self {
            Self::Every(seconds) => (Some(*seconds), None, None),
            Self::Cron { line, zone } => (None, Some(line), Some(zone)),
        }
    }
}

impl MonitorInput {
    /// The monitor, or why it cannot be one: one agent of 1 to [`AGENT_MAX`]
    /// characters; a period, or a cron line and its zone, never both; a grace
    /// from a minute to 31 days; and its links as an alert's are checked.
    pub fn checked(self) -> Result<Monitor, TelmoniError> {
        let bad = |why: &str| TelmoniError::BadRequest(why.to_owned());
        let agent = self.agent.trim();
        if agent.is_empty() || agent.chars().count() > AGENT_MAX {
            return Err(bad(&format!(
                "a monitor's agent is 1 to {AGENT_MAX} characters"
            )));
        }
        let fault = |fault: Fault| TelmoniError::BadRequest(fault.detail().to_owned());
        let (schedule, spelled) = match (self.every_seconds, self.cron, self.time_zone) {
            (Some(seconds), None, None) => (
                Schedule::every(seconds).map_err(fault)?,
                Spelled::Every(seconds),
            ),
            (None, Some(line), Some(zone)) => {
                let line = line.trim().to_owned();
                (
                    Schedule::cron(&line, &zone).map_err(fault)?,
                    Spelled::Cron { line, zone },
                )
            }
            _ => {
                return Err(bad(
                    "a monitor's schedule is every_seconds, or cron with its time_zone, never both",
                ));
            }
        };
        schedule::grace(self.grace_seconds).map_err(fault)?;
        Ok(Monitor {
            agent: agent.to_owned(),
            schedule,
            spelled,
            grace_seconds: self.grace_seconds,
            connectors: checked_links(self.connectors, "a monitor")?,
        })
    }
}

/// A monitor as its row keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct MonitorRow {
    pub id: Uuid,
    #[serde(skip)]
    pub project_id: ProjectId,
    #[serde(skip)]
    pub organization_id: OrganizationId,
    pub agent: String,
    pub every_seconds: Option<i32>,
    pub cron: Option<String>,
    pub time_zone: Option<String>,
    pub grace_seconds: i32,
    pub state: MonitorState,
    pub cause: Option<MonitorCause>,
    pub state_since: DateTime<Utc>,
    /// When its next trace is due; none while it is `new` or paused.
    pub next_due_at: Option<DateTime<Utc>>,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_ended_at: Option<DateTime<Utc>>,
    pub last_status: Option<SpanStatus>,
    pub last_trace_id: Option<String>,
    #[serde(skip)]
    pub told_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub told_to: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MonitorRow {
    /// Its schedule, read back from its columns.
    pub fn schedule(&self) -> Result<Schedule, Fault> {
        match (self.every_seconds, &self.cron, &self.time_zone) {
            (Some(seconds), None, None) => Schedule::every(seconds),
            (None, Some(line), Some(zone)) => Schedule::cron(line, zone),
            _ => Err(Fault::Cron),
        }
    }

    /// Its grace.
    #[must_use]
    pub fn grace(&self) -> TimeDelta {
        TimeDelta::seconds(i64::from(self.grace_seconds))
    }
}

/// The columns a [`MonitorRow`] is read from, in its order.
const MONITOR_COLUMNS: &str = "id, project_id, organization_id, agent, every_seconds, cron, \
    time_zone, grace_seconds, state, cause, state_since, next_due_at, last_started_at, \
    last_ended_at, last_status, last_trace_id, told_at, told_to, created_at, updated_at";

/// The project's monitors, by agent.
pub async fn monitors<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<MonitorRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {MONITOR_COLUMNS} FROM telemetry.monitors WHERE project_id = $1 ORDER BY agent, id"
    )))
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// One of the project's monitors.
pub async fn monitor<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<Option<MonitorRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {MONITOR_COLUMNS} FROM telemetry.monitors WHERE project_id = $1 AND id = $2"
    )))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
}

/// A new monitor of the project, `new` until its agent's first run.
pub async fn insert<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    monitor: &Monitor,
) -> sqlx::Result<MonitorRow> {
    let (every, cron, zone) = monitor.spelled.columns();
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO telemetry.monitors
            (id, project_id, organization_id, agent, every_seconds, cron, time_zone, grace_seconds)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING {MONITOR_COLUMNS}"
    )))
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(organization_id)
    .bind(&monitor.agent)
    .bind(every)
    .bind(cron)
    .bind(zone)
    .bind(monitor.grace_seconds)
    .fetch_one(tx.conn())
    .await
}

/// When a monitor saved over is next due: found again from its last start
/// under its new schedule, which the loop judges again within a minute.
pub fn due_after_edit(
    before: &MonitorRow,
    monitor: &Monitor,
) -> Result<Option<DateTime<Utc>>, Fault> {
    match (before.state, before.last_started_at) {
        (MonitorState::New | MonitorState::Paused, _) | (_, None) => Ok(before.next_due_at),
        (_, Some(started)) => monitor.schedule.next_after(started).map(Some),
    }
}

/// One of the project's monitors saved over, due next at `next_due_at`.
pub async fn update<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    monitor: &Monitor,
    next_due_at: Option<DateTime<Utc>>,
) -> sqlx::Result<Option<MonitorRow>> {
    let (every, cron, zone) = monitor.spelled.columns();
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE telemetry.monitors
            SET agent = $3, every_seconds = $4, cron = $5, time_zone = $6, grace_seconds = $7,
                next_due_at = $8, updated_at = now()
          WHERE project_id = $1 AND id = $2
         RETURNING {MONITOR_COLUMNS}"
    )))
    .bind(project_id)
    .bind(id)
    .bind(&monitor.agent)
    .bind(every)
    .bind(cron)
    .bind(zone)
    .bind(monitor.grace_seconds)
    .bind(next_due_at)
    .fetch_optional(tx.conn())
    .await
}

/// One of the project's monitors, gone with its incidents, links and
/// windows. Answers whether there was one.
pub async fn delete<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
) -> sqlx::Result<bool> {
    Ok(
        sqlx::query("DELETE FROM telemetry.monitors WHERE project_id = $1 AND id = $2")
            .bind(project_id)
            .bind(id)
            .execute(tx.conn())
            .await?
            .rows_affected()
            > 0,
    )
}

/// One of the project's monitors paused, judged no more; or, `resume`d,
/// `new` again until its agent's next run. Its open incident stands either way.
pub async fn pause<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    resume: bool,
) -> sqlx::Result<Option<MonitorRow>> {
    let sql = if resume {
        "UPDATE telemetry.monitors
            SET state = 'new', cause = NULL, next_due_at = NULL, state_since = now(),
                updated_at = now()
          WHERE project_id = $1 AND id = $2 AND state = 'paused'"
    } else {
        "UPDATE telemetry.monitors
            SET state = 'paused', cause = NULL, next_due_at = NULL, state_since = now(),
                updated_at = now()
          WHERE project_id = $1 AND id = $2 AND state <> 'paused'"
    };
    sqlx::query(sql)
        .bind(project_id)
        .bind(id)
        .execute(tx.conn())
        .await?;
    monitor(tx, project_id, id).await
}

/// The connectors the project's monitors link, those of `ids` alone.
pub async fn links<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    ids: &[Uuid],
) -> sqlx::Result<Vec<(Uuid, Link)>> {
    let rows: Vec<MonitorLinkRow> = sqlx::query_as(
        "SELECT monitor_id, connection_id, level, hours FROM telemetry.monitor_connectors
          WHERE project_id = $1 AND monitor_id = ANY($2)
          ORDER BY monitor_id, connection_id",
    )
    .bind(project_id)
    .bind(ids)
    .fetch_all(tx.conn())
    .await?;
    Ok(rows.into_iter().map(MonitorLinkRow::link).collect())
}

/// The connectors one monitor links, read in the module's lane by the loop.
pub async fn links_of(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    id: Uuid,
) -> sqlx::Result<Vec<Link>> {
    let rows: Vec<MonitorLinkRow> = sqlx::query_as(
        "SELECT monitor_id, connection_id, level, hours FROM telemetry.monitor_connectors
          WHERE monitor_id = $1 ORDER BY connection_id",
    )
    .bind(id)
    .fetch_all(tx.conn())
    .await?;
    Ok(rows.into_iter().map(|row| row.link().1).collect())
}

/// The monitor's connectors, written whole in place of those it linked.
pub async fn write_links<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    links: &[Link],
) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM telemetry.monitor_connectors WHERE project_id = $1 AND monitor_id = $2",
    )
    .bind(project_id)
    .bind(id)
    .execute(tx.conn())
    .await?;
    for link in links {
        sqlx::query(
            "INSERT INTO telemetry.monitor_connectors
                (monitor_id, connection_id, project_id, level, hours)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(link.connection_id)
        .bind(project_id)
        .bind(link.level)
        .bind(link.hours.as_ref().map(sqlx::types::Json))
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// A connector a monitor links, as its row keeps it.
#[derive(Debug, Clone, sqlx::FromRow)]
struct MonitorLinkRow {
    monitor_id: Uuid,
    connection_id: Uuid,
    level: telmoni_shared::RuleLevel,
    hours: Option<sqlx::types::Json<crate::rules::Hours>>,
}

impl MonitorLinkRow {
    fn link(self) -> (Uuid, Link) {
        (
            self.monitor_id,
            Link {
                connection_id: self.connection_id,
                level: self.level,
                hours: self.hours.map(|hours| hours.0),
            },
        )
    }
}

/// What one read found of an agent's traces: the latest start of a trace,
/// and the trace that ended last.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fact {
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<Ended>,
}

/// A trace that ended: when, how, and which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub at: DateTime<Utc>,
    pub status: SpanStatus,
    pub trace_id: String,
}

/// Where a monitor stands after a judgement, and the last trace it knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    pub state: MonitorState,
    pub cause: Option<MonitorCause>,
    pub next_due_at: Option<DateTime<Utc>>,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_ended: Option<Ended>,
}

impl Judged {
    /// Where `row` stood before.
    fn of(row: &MonitorRow) -> Self {
        let last_ended = match (row.last_ended_at, row.last_status, &row.last_trace_id) {
            (Some(at), Some(status), Some(trace_id)) => Some(Ended {
                at,
                status,
                trace_id: trace_id.clone(),
            }),
            _ => None,
        };
        Self {
            state: row.state,
            cause: row.cause,
            next_due_at: row.next_due_at,
            last_started_at: row.last_started_at,
            last_ended,
        }
    }
}

/// A running monitor judged at `now` from what the latest read found of its
/// agent's runs. A start newer than the last makes the next run due after it
/// and ends a `missed`; an end newer than the last, in error, makes it `down`
/// (`failed`), and one that ended well ends a `failed`; a cancelled run is
/// neither. Then the clock: past its due time and [`LATE_AFTER`] it is
/// `late`, past its grace `down`, and back inside them, its schedule edited,
/// `up` again. A `failed` stands until a trace ends well.
pub fn judge(
    row: &MonitorRow,
    schedule: &Schedule,
    fact: &Fact,
    now: DateTime<Utc>,
) -> Result<Judged, Fault> {
    let mut judged = Judged::of(row);
    if row.state == MonitorState::Paused {
        return Ok(judged);
    }
    if let Some(started) = fact
        .started
        .filter(|started| row.last_started_at.is_none_or(|last| *started > last))
    {
        judged.last_started_at = Some(started);
        judged.next_due_at = Some(schedule.next_after(started)?);
        if judged.cause != Some(MonitorCause::Failed) {
            (judged.state, judged.cause) = (MonitorState::Up, None);
        }
    }
    if let Some(ended) = fact.ended.as_ref().filter(|ended| {
        judged
            .last_ended
            .as_ref()
            .is_none_or(|last| ended.at > last.at)
    }) {
        match ended.status {
            SpanStatus::Error => {
                (judged.state, judged.cause) = (MonitorState::Down, Some(MonitorCause::Failed));
            }
            SpanStatus::Ok if judged.cause == Some(MonitorCause::Failed) => {
                (judged.state, judged.cause) = (MonitorState::Up, None);
            }
            SpanStatus::Ok | SpanStatus::Cancelled => {}
        }
        judged.last_ended = Some(ended.clone());
    }
    if let (Some(due), true) = (
        judged.next_due_at,
        judged.cause != Some(MonitorCause::Failed) && judged.state != MonitorState::New,
    ) {
        (judged.state, judged.cause) = if now >= due + row.grace() {
            (MonitorState::Down, Some(MonitorCause::Missed))
        } else if now >= due + LATE_AFTER {
            (MonitorState::Late, Some(MonitorCause::Missed))
        } else {
            (MonitorState::Up, None)
        };
    }
    Ok(judged)
}

/// The loop: every [`EVERY`], on whichever replica wins the lock.
pub async fn run(state: Arc<AppState>) {
    state
        .heartbeats
        .every(&state.db, HEARTBEAT, Duration::ZERO, EVERY, || {
            let state = state.clone();
            async move {
                if state.monitors_stopping.asked() {
                    // Never ends: the process is ending, and a loop that ended
                    // would read to it as one that died.
                    std::future::pending::<()>().await;
                }
                tick_at(&state, Utc::now()).await
            }
        })
        .await;
}

/// The last read this replica made, as [`AppState`] keeps it.
#[derive(Debug, Default)]
pub struct Read(Mutex<Option<DateTime<Utc>>>);

impl Read {
    /// Where the next read starts: [`OVERLAP`] before the last, never past
    /// [`CATCH_UP`] back.
    fn from(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        let last = *self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let floor = now - CATCH_UP;
        last.map_or(floor, |last| (last - OVERLAP).max(floor))
    }

    fn done(&self, at: DateTime<Utc>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(at);
    }
}

/// One tick at `now`, a clock a suite may set: run by the replica that wins
/// the loop's lock, held in its own transaction while the monitors are judged
/// in theirs, and let go within fifty seconds; another replica's tick skips.
pub async fn tick_at(state: &AppState, now: DateTime<Utc>) -> Result<(), TelmoniError> {
    let _pass = state.monitors_stopping.pass.lock().await;
    let mut leader = maintenance_scope(&state.db, TelemetryLane).await?;
    let ours: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtextextended('telemetry:monitors', 0))",
    )
    .fetch_one(leader.conn())
    .await?;
    if !ours {
        leader.rollback().await?;
        return Ok(());
    }
    let judged = tokio::time::timeout(BUDGET, judge_all(state, now)).await;
    // The lock goes with the transaction; a rollback that fails means the
    // connection, and the lock with it, is gone.
    if let Err(error) = leader.rollback().await {
        tracing::warn!(error = %error, "telemetry: the monitors' lock did not roll back cleanly");
    }
    judged.unwrap_or_else(|_| {
        Err(TelmoniError::Internal(
            "the monitors' tick ran past its budget; the next one carries on".into(),
        ))
    })
}

/// Every running monitor judged, a batch at a time: one read of its agents'
/// runs, then each that moved settled and told.
async fn judge_all(state: &AppState, now: DateTime<Utc>) -> Result<(), TelmoniError> {
    let from = state.monitors_read.from(now);
    let mut after = Uuid::nil();
    let mut closing: HashMap<OrganizationId, bool> = HashMap::new();
    loop {
        if state.monitors_stopping.asked() {
            return Ok(());
        }
        let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
        let page: Vec<MonitorRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {MONITOR_COLUMNS} FROM telemetry.monitors
              WHERE state <> 'paused' AND id > $1 ORDER BY id LIMIT $2"
        )))
        .bind(after)
        .bind(BATCH)
        .fetch_all(tx.conn())
        .await?;
        tx.commit().await?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.id;
        let facts = facts(state, &page, from).await?;
        for row in &page {
            if state.monitors_stopping.asked() {
                return Ok(());
            }
            let erased = match closing.get(&row.organization_id) {
                Some(erased) => *erased,
                None => {
                    let erased = !chain::open(state.auth.as_ref(), &row.organization_id).await;
                    closing.insert(row.organization_id.clone(), erased);
                    erased
                }
            };
            // One being erased is told nothing, its feed and connectors about to go.
            if erased {
                continue;
            }
            let fact = facts
                .get(&(row.project_id.clone(), row.agent.clone()))
                .cloned()
                .unwrap_or_default();
            one(state, row, &fact, now).await?;
        }
        if i64::try_from(page.len()).unwrap_or(i64::MAX) < BATCH {
            break;
        }
    }
    state.monitors_read.done(now);
    Ok(())
}

/// One monitor judged, and where it moved, settled and told.
async fn one(
    state: &AppState,
    row: &MonitorRow,
    fact: &Fact,
    now: DateTime<Utc>,
) -> Result<(), TelmoniError> {
    let judged = match row
        .schedule()
        .and_then(|schedule| judge(row, &schedule, fact, now))
    {
        Ok(judged) => judged,
        Err(fault) => {
            tracing::warn!(
                monitor_id = %row.id,
                fault = fault.detail(),
                "telemetry: a monitor's schedule could not be read; it is not judged"
            );
            return Ok(());
        }
    };
    if judged == Judged::of(row) {
        return Ok(());
    }
    let told = settle(state, row, &judged, now).await?;
    let Some(told) = told else {
        return Ok(());
    };
    crate::notify::tell_monitor(state, row, &judged, &told, now).await;
    Ok(())
}

/// A judgement written where the monitor stands as it was read, its incident
/// moved in the same transaction: a cause ended resolves it, a new cause
/// opens another, and a severity crossed opens, raises or follows it as an
/// alert's. Answers what is to be told, none where a member's edit came first.
async fn settle(
    state: &AppState,
    row: &MonitorRow,
    judged: &Judged,
    now: DateTime<Utc>,
) -> Result<Option<Vec<Told>>, TelmoniError> {
    let owner = Owner::Monitor(row.id);
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let standing_open = incidents::open_of(&mut tx, owner).await?;
    let mut open = standing_open.as_ref();
    let filed = |cause| Filed {
        owner,
        project_id: &row.project_id,
        organization_id: &row.organization_id,
        cause,
    };
    let standing = |severity| Standing {
        severity,
        told_at: row.told_at,
        missing_data: MissingData::Zero,
        renotify_minutes: None,
    };
    let mut severity = row.state.severity().unwrap_or(Severity::Unknown);
    let next = judged.state.severity().filter(|next| next.is_breach());
    // Each move beside the cause it files under and the incident it moves.
    let mut moves = Vec::new();
    if let Some(incident) = open.filter(|incident| next.is_none() || incident.cause != judged.cause)
    {
        for one in incidents::moves(standing(severity), Severity::Ok, now, Some(incident), now) {
            moves.push((one, incident.cause, Some(incident)));
        }
        open = None;
        severity = Severity::Ok;
    }
    if let Some(next) = next {
        for one in incidents::moves(standing(severity), next, now, open, now) {
            moves.push((one, judged.cause, open));
        }
    }
    let told_at = if moves.iter().any(|(one, ..)| one.telling != Telling::Silent) {
        Some(now)
    } else {
        row.told_at
    };
    let state_since = if judged.state == row.state {
        row.state_since
    } else {
        now
    };
    let ended = judged.last_ended.as_ref();
    let settled = sqlx::query(
        "UPDATE telemetry.monitors
            SET state = $4, cause = $5, state_since = $6, next_due_at = $7,
                last_started_at = $8, last_ended_at = $9, last_status = $10, last_trace_id = $11,
                told_at = $12
          WHERE id = $1 AND updated_at = $2 AND organization_id = $3 AND state <> 'paused'",
    )
    .bind(row.id)
    .bind(row.updated_at)
    .bind(&row.organization_id)
    .bind(judged.state)
    .bind(judged.cause)
    .bind(state_since)
    .bind(judged.next_due_at)
    .bind(judged.last_started_at)
    .bind(ended.map(|ended| ended.at))
    .bind(ended.map(|ended| ended.status))
    .bind(ended.map(|ended| ended.trace_id.as_str()))
    .bind(told_at)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0;
    if !settled {
        tx.rollback().await?;
        return Ok(None);
    }
    let mut written = Vec::with_capacity(moves.len());
    for (one, cause, incident) in moves {
        written.push(incidents::write(&mut tx, filed(cause), incident, None, now, one).await?);
    }
    tx.commit().await?;
    Ok(Some(written))
}

/// What a read finds of one agent's traces.
#[derive(Debug, Clone, Row, Deserialize)]
struct FactRow {
    project_id: String,
    agent: String,
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    started: DateTime<Utc>,
    #[serde(
        deserialize_with = "clickhouse::serde::chrono::datetime64::micros::option::deserialize"
    )]
    ended: Option<DateTime<Utc>>,
    status: String,
    trace_id: Option<String>,
}

/// The latest start of a trace of each agent read, believed no further than
/// [`SKEW_MAX_SECS`] past its receipt, and the trace that ended last.
const FACTS_SQL: &str = concat!(
    "SELECT project_id, agent, ",
    "max(least(started_at, received_at + toIntervalSecond({skew:UInt32}))) AS started, ",
    "if(countIf(kind = 'agent') > 0, maxIf(ended_at, kind = 'agent'), NULL) AS ended, ",
    "argMaxIf(status, ended_at, kind = 'agent') AS status, ",
    "if(countIf(kind = 'agent') > 0, argMaxIf(trace_id, ended_at, kind = 'agent'), NULL) AS trace_id ",
    "FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} AND agent IN {agents:Array(String)} ",
    "AND kind IN ('trace_start', 'agent') AND received_at >= {from:DateTime64(6, 'UTC')} ",
    "GROUP BY project_id, agent"
);

/// What the spans received since `from` say of the agents of `page`. One
/// ClickHouse cannot answer for fails the tick, which judges nothing on
/// missing facts.
async fn facts(
    state: &AppState,
    page: &[MonitorRow],
    from: DateTime<Utc>,
) -> Result<HashMap<(ProjectId, String), Fact>, TelmoniError> {
    let Some(projects) = Projects::of(page.iter().map(|row| row.project_id.clone())) else {
        return Ok(HashMap::new());
    };
    let mut agents: Vec<&str> = page.iter().map(|row| row.agent.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();
    let rows: Vec<FactRow> = state
        .store
        .read(&projects, FACTS_SQL)
        .param("skew", SKEW_MAX_SECS)
        .param("agents", agents)
        .param("from", micros(from))
        .fetch_all()
        .await
        .map_err(|error| {
            crate::store::failed(&error, "the monitors' traces", "the next tick reads them")
        })?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let ended = match (row.ended, row.status.parse::<SpanStatus>(), row.trace_id) {
                (Some(at), Ok(status), Some(trace_id)) => Some(Ended {
                    at,
                    status,
                    trace_id,
                }),
                _ => None,
            };
            (
                (ProjectId::from_trusted(&row.project_id), row.agent),
                Fact {
                    started: Some(row.started),
                    ended,
                },
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("a time")
            .with_timezone(&Utc)
    }

    /// A nightly monitor at 02:00 UTC with a grace of half an hour, `up`,
    /// its last run started the night before.
    fn nightly() -> (MonitorRow, Schedule) {
        let row = MonitorRow {
            id: Uuid::nil(),
            project_id: ProjectId::from_trusted("project_a"),
            organization_id: OrganizationId::from_trusted("org_a"),
            agent: "nightly-triage".into(),
            every_seconds: None,
            cron: Some("0 2 * * *".into()),
            time_zone: Some("UTC".into()),
            grace_seconds: 1_800,
            state: MonitorState::Up,
            cause: None,
            state_since: at("2026-11-05T02:00:03Z"),
            next_due_at: Some(at("2026-11-06T02:00:00Z")),
            last_started_at: Some(at("2026-11-05T02:00:03Z")),
            last_ended_at: None,
            last_status: None,
            last_trace_id: None,
            told_at: None,
            told_to: Vec::new(),
            created_at: at("2026-11-01T00:00:00Z"),
            updated_at: at("2026-11-01T00:00:00Z"),
        };
        let schedule = row.schedule().expect("a schedule");
        (row, schedule)
    }

    fn started(when: &str) -> Fact {
        Fact {
            started: Some(at(when)),
            ended: None,
        }
    }

    fn ended(when: &str, status: SpanStatus) -> Fact {
        Fact {
            started: Some(at(when)),
            ended: Some(Ended {
                at: at(when),
                status,
                trace_id: "07".repeat(16),
            }),
        }
    }

    /// A run on time keeps it up and moves its due time a day on; none
    /// started a minute past due is late, and past its grace down.
    #[test]
    fn a_run_on_time_late_and_down() {
        let (row, schedule) = nightly();
        let on_time = judge(
            &row,
            &schedule,
            &started("2026-11-06T02:00:02Z"),
            at("2026-11-06T02:00:30Z"),
        )
        .expect("judged");
        assert_eq!(on_time.state, MonitorState::Up);
        assert_eq!(on_time.next_due_at, Some(at("2026-11-07T02:00:00Z")));

        let none = Fact::default();
        let seconds_in = judge(&row, &schedule, &none, at("2026-11-06T02:00:30Z")).expect("judged");
        assert_eq!(
            seconds_in.state,
            MonitorState::Up,
            "a start may still be on its way"
        );
        let late = judge(&row, &schedule, &none, at("2026-11-06T02:01:00Z")).expect("judged");
        assert_eq!(
            (late.state, late.cause),
            (MonitorState::Late, Some(MonitorCause::Missed))
        );
        let down = judge(&row, &schedule, &none, at("2026-11-06T02:30:00Z")).expect("judged");
        assert_eq!(
            (down.state, down.cause),
            (MonitorState::Down, Some(MonitorCause::Missed))
        );
    }

    /// A start ends a missed, late or down; a failed run makes it down until
    /// one ends well, and a start alone does not end a failure.
    #[test]
    fn a_start_ends_a_miss_and_a_good_end_a_failure() {
        let (mut row, schedule) = nightly();
        (row.state, row.cause) = (MonitorState::Down, Some(MonitorCause::Missed));
        let back = judge(
            &row,
            &schedule,
            &started("2026-11-06T03:10:00Z"),
            at("2026-11-06T03:10:30Z"),
        )
        .expect("judged");
        assert_eq!((back.state, back.cause), (MonitorState::Up, None));

        let (row, schedule) = nightly();
        let failed = judge(
            &row,
            &schedule,
            &ended("2026-11-06T02:05:00Z", SpanStatus::Error),
            at("2026-11-06T02:05:30Z"),
        )
        .expect("judged");
        assert_eq!(
            (failed.state, failed.cause),
            (MonitorState::Down, Some(MonitorCause::Failed))
        );
        let row = MonitorRow {
            state: failed.state,
            cause: failed.cause,
            next_due_at: failed.next_due_at,
            last_started_at: failed.last_started_at,
            last_ended_at: failed.last_ended.as_ref().map(|ended| ended.at),
            last_status: failed.last_ended.as_ref().map(|ended| ended.status),
            last_trace_id: failed
                .last_ended
                .as_ref()
                .map(|ended| ended.trace_id.clone()),
            ..row
        };
        let still = judge(
            &row,
            &schedule,
            &started("2026-11-07T02:00:01Z"),
            at("2026-11-07T02:00:30Z"),
        )
        .expect("judged");
        assert_eq!(
            still.state,
            MonitorState::Down,
            "a start alone ends no failure"
        );
        let well = judge(
            &row,
            &schedule,
            &ended("2026-11-07T02:04:00Z", SpanStatus::Ok),
            at("2026-11-07T02:04:30Z"),
        )
        .expect("judged");
        assert_eq!((well.state, well.cause), (MonitorState::Up, None));
    }

    /// A cancelled run is neither a failure nor a recovery; an older end
    /// landing late changes nothing.
    #[test]
    fn a_cancelled_run_and_an_old_end_change_nothing() {
        let (row, schedule) = nightly();
        let cancelled = judge(
            &row,
            &schedule,
            &ended("2026-11-06T02:05:00Z", SpanStatus::Cancelled),
            at("2026-11-06T02:05:30Z"),
        )
        .expect("judged");
        assert_eq!(cancelled.state, MonitorState::Up);
        let row = MonitorRow {
            last_ended_at: Some(at("2026-11-06T02:10:00Z")),
            last_status: Some(SpanStatus::Ok),
            last_trace_id: Some("08".repeat(16)),
            ..row
        };
        let old = judge(
            &row,
            &schedule,
            &ended("2026-11-06T02:05:00Z", SpanStatus::Error),
            at("2026-11-06T02:11:00Z"),
        )
        .expect("judged");
        assert_eq!(old.state, MonitorState::Up);
    }

    /// A new monitor waits for its first run, never late; a paused one is
    /// not judged.
    #[test]
    fn new_waits_and_paused_is_not_judged() {
        let (mut row, schedule) = nightly();
        (row.state, row.next_due_at, row.last_started_at) = (MonitorState::New, None, None);
        let waiting = judge(
            &row,
            &schedule,
            &Fact::default(),
            at("2026-11-09T12:00:00Z"),
        )
        .expect("judged");
        assert_eq!(waiting.state, MonitorState::New);
        let first = judge(
            &row,
            &schedule,
            &started("2026-11-09T12:00:00Z"),
            at("2026-11-09T12:00:10Z"),
        )
        .expect("judged");
        assert_eq!(
            (first.state, first.next_due_at),
            (MonitorState::Up, Some(at("2026-11-10T02:00:00Z")))
        );

        row.state = MonitorState::Paused;
        let paused = judge(
            &row,
            &schedule,
            &ended("2026-11-09T12:00:00Z", SpanStatus::Error),
            at("2026-11-12T00:00:00Z"),
        )
        .expect("judged");
        assert_eq!(paused, Judged::of(&row));
    }

    /// A schedule moved past now while late brings it back up.
    #[test]
    fn a_due_time_moved_later_is_up_again() {
        let (mut row, schedule) = nightly();
        (row.state, row.cause, row.next_due_at) = (
            MonitorState::Late,
            Some(MonitorCause::Missed),
            Some(at("2026-11-06T04:00:00Z")),
        );
        let up = judge(
            &row,
            &schedule,
            &Fact::default(),
            at("2026-11-06T02:10:00Z"),
        )
        .expect("judged");
        assert_eq!((up.state, up.cause), (MonitorState::Up, None));
    }
}
