//! An agent's changes: the first trace of an agent, in one environment, to
//! carry a release, a model or a prompt's version its traces there had not
//! carried in the [`NEW_FOR`] before, each a row of `telemetry.changes`. Each
//! minute the replica that wins the lock reads the spans landed since the
//! last pass, which `telemetry.reads` keeps: those received since, and the
//! traces another exporter's held spans landed in since ([`crate::foreign`]).
//! An agent past [`DAY_SHOWN`] of one kind in a UTC day is marked no further
//! that day, its last shown one saying so; what it carried past them is
//! still remembered, `hidden`, so it is not new the next day. The trace
//! list's charts, the Agents page and `/v1/changes` read them
//! ([`crate::handler::changes`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{HasProject, Scoped, maintenance_scope};
use telmoni_shared::digest::sha256_hex;
use telmoni_shared::{ChangeKind, ProjectId, TelmoniError};

use crate::AppState;
use crate::db::{self, TelemetryLane};
use crate::rules::RuleFilters;
use crate::store::{Projects, micros};

/// The loop's name in `telemetry.heartbeats` and `telemetry.reads`.
pub const HEARTBEAT: &str = "changes";

/// How often the loop reads.
pub const EVERY: Duration = Duration::from_secs(60);

/// How long a value its traces carried is remembered as theirs.
pub const NEW_FOR: TimeDelta = TimeDelta::days(30);

/// The changes of one kind an agent's UTC day shows.
pub const DAY_SHOWN: i64 = 24;

/// The rows a project's UTC day keeps, shown or not, so agents named after
/// each request write no more.
pub const DAY_ROWS_MAX: i64 = 1_000;

/// The most changes a list answers.
pub const CHANGES_MAX: i64 = 500;

/// How far each read reaches back past the last: rows ClickHouse was still
/// flushing (`store::ASYNC_INSERT_WAIT`).
const OVERLAP: TimeDelta = TimeDelta::minutes(5);

/// The most one tick reads, so the loop catches up after an outage an hour
/// a tick.
const STEP: TimeDelta = TimeDelta::hours(1);

/// The furthest back a tick reads: a held span keeps its landed trace a day.
const CATCH_UP: TimeDelta = TimeDelta::days(1);

/// How far past its receipt a span's start is believed, as the monitors'.
const SKEW_MAX_SECS: u32 = 300;

/// How often a value still carried has its `seen_at` moved.
const SEEN_EVERY: TimeDelta = TimeDelta::hours(1);

/// How long one tick may run: inside the minute.
const BUDGET: Duration = Duration::from_secs(50);

/// How many projects one read names.
const PAGE: i64 = 500;

/// What a row is: shown, shown and the last its agent's day showed of its
/// kind, or remembered alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Drawn and listed.
    Shown,
    /// Drawn and listed, saying its agent's later changes of its kind that
    /// day were not.
    Capped,
    /// Neither: past its day's cap, or its trace's day past retention.
    Hidden,
}

telmoni_shared::wire_enum!(Mark {
    Shown => "shown",
    Capped => "capped",
    Hidden => "hidden",
});

/// A change as the lanes answer it: `prompt` on a prompt's alone, its
/// `value` the version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Change {
    pub agent: String,
    /// Empty where its spans named none.
    pub environment: String,
    pub kind: ChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub value: String,
    /// When the trace that first carried it started, held to five minutes
    /// past its receipt.
    pub changed_at: DateTime<Utc>,
    /// The trace that first carried it.
    pub trace_id: String,
    /// Its agent's later changes of its kind that UTC day were not marked.
    pub capped: bool,
}

const CHANGE_COLUMNS: &str = "agent, environment, kind, prompt, value, changed_at, trace_id, \
     mark = 'capped' AS capped";

/// How long before an incident opened a change is named on it.
pub const BEFORE_INCIDENT: TimeDelta = TimeDelta::hours(24);

/// The most changes an incident's answer names, the newest.
pub const INCIDENT_CHANGES_MAX: usize = 20;

/// Whose changes an incident names: the agents and environments its alert
/// or monitor watches, each empty for every one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub agents: Vec<String>,
    pub environments: Vec<String>,
}

impl Scope {
    /// An alert's: its filters' agents and environments, an objective's one
    /// agent among them.
    #[must_use]
    pub fn of_alert(filters: &RuleFilters) -> Self {
        Self {
            agents: filters.agent.clone(),
            environments: filters.environment.clone(),
        }
    }

    /// A monitor's: its agent, wherever it runs.
    #[must_use]
    pub fn of_monitor(agent: &str) -> Self {
        Self {
            agents: vec![agent.to_owned()],
            environments: Vec::new(),
        }
    }

    /// Whether a change must name its agent to be told from another's.
    #[must_use]
    pub const fn several(&self) -> bool {
        self.agents.len() != 1
    }
}

/// The project's changes over the last `days`, the newest first, narrowed to
/// `agents` and `environments` where given, [`CHANGES_MAX`] at most, and
/// whether the window held more.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn list<S: HasProject>(
    tx: &mut Scoped<'_, S>,
    project_id: &ProjectId,
    days: u16,
    agents: &[String],
    environments: &[String],
) -> sqlx::Result<(Vec<Change>, bool)> {
    let mut changes: Vec<Change> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CHANGE_COLUMNS} FROM telemetry.changes
          WHERE project_id = $1 AND mark <> 'hidden'
            AND changed_at >= now() - make_interval(days => $2)
            AND (cardinality($3::text[]) = 0 OR agent = ANY($3))
            AND (cardinality($4::text[]) = 0 OR environment = ANY($4))
          ORDER BY changed_at DESC, id
          LIMIT $5"
    )))
    .bind(project_id)
    .bind(i32::from(days))
    .bind(agents)
    .bind(environments)
    .bind(CHANGES_MAX + 1)
    .fetch_all(tx.conn())
    .await?;
    let more = changes.len() > usize::try_from(CHANGES_MAX).unwrap_or(usize::MAX);
    changes.truncate(usize::try_from(CHANGES_MAX).unwrap_or(usize::MAX));
    Ok((changes, more))
}

/// An incident whose changes are asked for: when it opened, and whose.
#[derive(Debug, Clone)]
pub struct Opened {
    pub incident_id: Uuid,
    pub opened_at: DateTime<Utc>,
    pub scope: Scope,
}

/// One incident's change, as the read of many answers it.
#[derive(Debug, sqlx::FromRow)]
struct Around {
    incident_id: Uuid,
    #[sqlx(flatten)]
    change: Change,
}

/// Each incident's newest [`INCIDENT_CHANGES_MAX`] changes in the
/// [`BEFORE_INCIDENT`] before it opened, and whether it had more: one read for all.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn around_incidents<S: HasProject>(
    tx: &mut Scoped<'_, S>,
    project_id: &ProjectId,
    incidents: &[Opened],
) -> sqlx::Result<HashMap<Uuid, (Vec<Change>, bool)>> {
    if incidents.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<Uuid> = incidents.iter().map(|one| one.incident_id).collect();
    let opened: Vec<DateTime<Utc>> = incidents.iter().map(|one| one.opened_at).collect();
    let agents: Vec<serde_json::Value> = incidents
        .iter()
        .map(|one| serde_json::json!(one.scope.agents))
        .collect();
    let environments: Vec<serde_json::Value> = incidents
        .iter()
        .map(|one| serde_json::json!(one.scope.environments))
        .collect();
    let most = i64::try_from(INCIDENT_CHANGES_MAX).unwrap_or(i64::MAX);
    let rows: Vec<Around> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT asked.incident_id, change.*
           FROM unnest($2::uuid[], $3::timestamptz[], $4::jsonb[], $5::jsonb[])
                AS asked(incident_id, opened_at, agents, environments)
          CROSS JOIN LATERAL (
                SELECT {CHANGE_COLUMNS} FROM telemetry.changes
                 WHERE project_id = $1 AND mark <> 'hidden'
                   AND changed_at > asked.opened_at - make_interval(hours => $6)
                   AND changed_at <= asked.opened_at
                   AND (jsonb_array_length(asked.agents) = 0 OR asked.agents ? agent)
                   AND (jsonb_array_length(asked.environments) = 0
                        OR asked.environments ? environment)
                 ORDER BY changed_at DESC, id
                 LIMIT $7) AS change"
    )))
    .bind(project_id)
    .bind(ids)
    .bind(opened)
    .bind(agents)
    .bind(environments)
    .bind(i32::try_from(BEFORE_INCIDENT.num_hours()).unwrap_or(24))
    .bind(most + 1)
    .fetch_all(tx.conn())
    .await?;
    let mut around: HashMap<Uuid, (Vec<Change>, bool)> = HashMap::new();
    for row in rows {
        let (changes, more) = around.entry(row.incident_id).or_default();
        if changes.len() < INCIDENT_CHANGES_MAX {
            changes.push(row.change);
        } else {
            *more = true;
        }
    }
    Ok(around)
}

/// Each agent's newest change of each kind over the last `days`.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn newest<S: HasProject>(
    tx: &mut Scoped<'_, S>,
    project_id: &ProjectId,
    days: u16,
) -> sqlx::Result<Vec<Change>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT DISTINCT ON (agent, kind) {CHANGE_COLUMNS} FROM telemetry.changes
          WHERE project_id = $1 AND mark <> 'hidden'
            AND changed_at >= now() - make_interval(days => $2)
          ORDER BY agent, kind, changed_at DESC, id"
    )))
    .bind(project_id)
    .bind(i32::from(days))
    .fetch_all(tx.conn())
    .await
}

/// The changes past `cutoff`, of `projects` or of every project: one whose
/// trace's day it passed is hidden, that trace's spans gone, and one no span
/// since carried goes. Answers how many went.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn purge_before(
    conn: &mut PgConnection,
    projects: Option<&[ProjectId]>,
    cutoff: DateTime<Utc>,
) -> sqlx::Result<u64> {
    // Every project's, or `projects`' alone where `$2` names them.
    let hide = "UPDATE telemetry.changes SET mark = 'hidden'
                 WHERE changed_at < $1 AND mark <> 'hidden'
                   AND ($2::text[] IS NULL OR project_id = ANY($2))";
    let delete = "DELETE FROM telemetry.changes
                   WHERE seen_at < $1 AND ($2::text[] IS NULL OR project_id = ANY($2))";
    sqlx::query(hide)
        .bind(cutoff)
        .bind(projects)
        .execute(&mut *conn)
        .await?;
    Ok(sqlx::query(delete)
        .bind(cutoff)
        .bind(projects)
        .execute(&mut *conn)
        .await?
        .rows_affected())
}

/// The loop: every [`EVERY`], on whichever replica wins the lock.
pub async fn run(state: Arc<AppState>) {
    state
        .heartbeats
        .every(&state.db, HEARTBEAT, Duration::ZERO, EVERY, || {
            let state = state.clone();
            async move { tick_at(&state, Utc::now()).await }
        })
        .await;
}

/// One tick at `now`, a clock a suite may set: run by the replica that wins
/// the loop's lock, held in its own transaction while each project's changes
/// are written in theirs; another replica's tick skips.
///
/// # Errors
///
/// A store did not answer, a project's read or write failed, which the next
/// tick's overlap reads again, or the tick ran past its budget.
pub async fn tick_at(state: &AppState, now: DateTime<Utc>) -> Result<(), TelmoniError> {
    let mut leader = maintenance_scope(&state.db, TelemetryLane).await?;
    let ours: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtextextended('telemetry:changes', 0))",
    )
    .fetch_one(leader.conn())
    .await?;
    if !ours {
        leader.rollback().await?;
        return Ok(());
    }
    let read = tokio::time::timeout(BUDGET, read_all(state, now)).await;
    // The lock goes with the transaction; a rollback that fails means the
    // connection, and the lock with it, is gone.
    if let Err(error) = leader.rollback().await {
        tracing::warn!(error = %error, "telemetry: the changes' lock did not roll back cleanly");
    }
    read.unwrap_or_else(|_| {
        Err(TelmoniError::Internal(
            "the changes' tick ran past its budget; the next one reads its window again".into(),
        ))
    })
}

/// Every project's spans landed in the tick's window read, its changes
/// written, and the window's end kept as the next tick's start.
async fn read_all(state: &AppState, now: DateTime<Utc>) -> Result<(), TelmoniError> {
    let (from, to) = window(state, now).await?;
    let landed = landed(state, from, to).await?;
    let (mut after, mut failed, mut read_any, mut unread) = (String::new(), 0u32, false, false);
    loop {
        let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
        let page = db::settings_after(&mut tx, &after, PAGE).await?;
        tx.commit().await?;
        let Some((last, _)) = page.last() else {
            break;
        };
        after = last.as_str().to_owned();
        let projects: Vec<ProjectId> = page.iter().map(|(project, _)| project.clone()).collect();
        let seen = match seen(state, &projects, &landed, from, to).await {
            Ok(seen) => {
                read_any = true;
                seen
            }
            // One project's spans may be what failed the page: each is read alone.
            Err(_) => {
                let mut seen = Vec::new();
                for project in &projects {
                    match seen_of(state, project, &landed, from, to).await {
                        Ok(one) => {
                            read_any = true;
                            seen.extend(one);
                        }
                        Err(error) => {
                            tracing::warn!(project_id = %project, error = %error,
                                "telemetry: a project's spans could not be read for its changes");
                            (failed, unread) = (failed + 1, true);
                        }
                    }
                }
                seen
            }
        };
        let mut by_project: HashMap<ProjectId, Vec<Candidate>> = HashMap::new();
        for (project, candidate) in seen {
            by_project.entry(project).or_default().push(candidate);
        }
        for (project, candidates) in by_project {
            if let Err(error) = write(state, &project, candidates, now).await {
                tracing::warn!(project_id = %project, error = %error,
                    "telemetry: a project's changes could not be written; the next tick reads them again");
                failed += 1;
            }
        }
        if i64::try_from(page.len()).unwrap_or(i64::MAX) < PAGE {
            break;
        }
    }
    // With ClickHouse answering no read, the window is kept for the next tick.
    if unread && !read_any {
        return Err(TelmoniError::Internal(
            "the changes' spans could not be read; the next tick reads the same window".into(),
        ));
    }
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    sqlx::query(
        "INSERT INTO telemetry.reads (name, read_to) VALUES ($1, $2)
         ON CONFLICT (name) DO UPDATE SET read_to = EXCLUDED.read_to",
    )
    .bind(HEARTBEAT)
    .bind(to)
    .execute(tx.conn())
    .await?;
    tx.commit().await?;
    if failed > 0 {
        return Err(TelmoniError::Internal(format!(
            "{failed} projects' changes were not read or written this tick"
        )));
    }
    Ok(())
}

/// The tick's window: from [`OVERLAP`] before the last pass's end, never
/// past [`CATCH_UP`] back, or [`STEP`] back where none has passed, to at
/// most [`STEP`] later.
async fn window(
    state: &AppState,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let read_to: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT read_to FROM telemetry.reads WHERE name = $1")
            .bind(HEARTBEAT)
            .fetch_optional(tx.conn())
            .await?;
    tx.commit().await?;
    Ok(span_of(read_to, now))
}

/// [`window`]'s arithmetic, for a last pass's end and the tick's clock.
fn span_of(read_to: Option<DateTime<Utc>>, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let from = read_to.map_or(now - STEP, |last| {
        (last - OVERLAP).clamp(now - CATCH_UP, now - OVERLAP)
    });
    (from, (from + STEP).min(now))
}

/// The traces another exporter's held spans landed in within the window, by
/// project.
async fn landed(
    state: &AppState,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<HashMap<ProjectId, Vec<String>>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let rows: Vec<(ProjectId, String)> = sqlx::query_as(
        "SELECT DISTINCT project_id, trace_id FROM telemetry.held_spans
          WHERE landed_at >= $1 AND landed_at < $2",
    )
    .bind(from)
    .bind(to)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    let mut by_project: HashMap<ProjectId, Vec<String>> = HashMap::new();
    for (project, trace) in rows {
        by_project.entry(project).or_default().push(trace);
    }
    Ok(by_project)
}

/// What a read finds of one value an agent carried in one environment.
#[derive(Debug, Clone, Row, Deserialize)]
struct SeenRow {
    project_id: String,
    agent: String,
    environment: String,
    what: String,
    named: String,
    carried: String,
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    first_at: DateTime<Utc>,
    first_trace: String,
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    first_received: DateTime<Utc>,
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    last_received: DateTime<Utc>,
}

/// Each release, model call's model and prompt version with its name the
/// window's spans carried, by agent and environment: when it first started,
/// believed no further than [`SKEW_MAX_SECS`] past its receipt, and the
/// trace that did, taken together so the time is that trace's; and when it
/// first and last arrived. Aliased apart from the columns, which ClickHouse
/// resolves after an alias of the same name.
const SEEN_SQL: &str = concat!(
    "SELECT project_id, agent, environment, ",
    "tupleElement(c, 1) AS what, tupleElement(c, 2) AS named, tupleElement(c, 3) AS carried, ",
    "min(least(started_at, received_at + toIntervalSecond({skew:UInt32}))) AS first_at, ",
    "argMin(trace_id, tuple(least(started_at, received_at + toIntervalSecond({skew:UInt32})), ",
    "trace_id)) AS first_trace, ",
    "min(received_at) AS first_received, max(received_at) AS last_received ",
    "FROM telemetry.spans ",
    "ARRAY JOIN arrayFilter(x -> tupleElement(x, 3) != '', [",
    "tuple('release', '', release), ",
    "tuple('model', '', if(kind = 'model_call', model, '')), ",
    "tuple('prompt', attributes['gen_ai.prompt.name'], ",
    "if(attributes['gen_ai.prompt.name'] != '', attributes['gen_ai.prompt.version'], ''))",
    "]) AS c ",
    "WHERE project_id IN {projects:Array(String)} AND agent != '' AND (",
    "(received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')}) ",
    "OR (has({traces:Array(String)}, trace_id) AND received_at >= {held_from:DateTime64(6, 'UTC')})) ",
    "GROUP BY project_id, agent, environment, what, named, carried"
);

/// What `projects`' spans landed in the window carried.
async fn seen(
    state: &AppState,
    projects: &[ProjectId],
    landed: &HashMap<ProjectId, Vec<String>>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<(ProjectId, Candidate)>, TelmoniError> {
    let Some(read) = Projects::of(projects.iter().cloned()) else {
        return Ok(Vec::new());
    };
    let traces: Vec<&str> = projects
        .iter()
        .filter_map(|project| landed.get(project))
        .flatten()
        .map(String::as_str)
        .collect();
    let rows: Vec<SeenRow> = state
        .store
        .read(&read, SEEN_SQL)
        .param("skew", SKEW_MAX_SECS)
        .param("from", micros(from))
        .param("to", micros(to))
        .param("traces", traces)
        .param("held_from", micros(from - CATCH_UP))
        .fetch_all()
        .await
        .map_err(|error| {
            crate::store::failed(&error, "the changes' spans", "the next tick reads them")
        })?;
    Ok(rows.into_iter().filter_map(SeenRow::candidate).collect())
}

/// [`seen`] for one project.
async fn seen_of(
    state: &AppState,
    project: &ProjectId,
    landed: &HashMap<ProjectId, Vec<String>>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<(ProjectId, Candidate)>, TelmoniError> {
    seen(state, std::slice::from_ref(project), landed, from, to).await
}

impl SeenRow {
    fn candidate(self) -> Option<(ProjectId, Candidate)> {
        let kind = self.what.parse::<ChangeKind>().ok()?;
        let prompt = (kind == ChangeKind::Prompt).then_some(self.named);
        Some((
            ProjectId::from_trusted(&self.project_id),
            Candidate {
                agent: self.agent,
                environment: self.environment,
                kind,
                prompt,
                value: self.carried,
                first_at: self.first_at,
                first_trace: self.first_trace,
                first_received: self.first_received,
                last_received: self.last_received,
            },
        ))
    }
}

/// One project's candidates settled and written in one transaction: the
/// project held in place, its settings row there, and no other replica
/// writing its changes at once. A project a move or a purge holds, or one
/// gone, is left to the next tick.
async fn write(
    state: &AppState,
    project_id: &ProjectId,
    candidates: Vec<Candidate>,
    now: DateTime<Utc>,
) -> Result<(), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    if !project_move::try_hold_in_place(tx.conn(), project_id).await? {
        tx.rollback().await?;
        return Ok(());
    }
    let kept: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM telemetry.project_settings WHERE project_id = $1)",
    )
    .bind(project_id)
    .fetch_one(tx.conn())
    .await?;
    if !kept {
        tx.rollback().await?;
        return Ok(());
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('changes:' || $1, 0))")
        .bind(project_id)
        .execute(tx.conn())
        .await?;
    let identities: Vec<String> = candidates.iter().map(Candidate::identity).collect();
    let remembered: HashMap<String, Remembered> =
        sqlx::query_as::<_, (String, Uuid, DateTime<Utc>)>(
            "SELECT DISTINCT ON (identity) identity, id, seen_at FROM telemetry.changes
          WHERE project_id = $1 AND identity = ANY($2)
          ORDER BY identity, seen_at DESC",
        )
        .bind(project_id)
        .bind(&identities)
        .fetch_all(tx.conn())
        .await?
        .into_iter()
        .map(|(identity, id, seen_at)| (identity, Remembered { id, seen_at }))
        .collect();
    let counts = counts(&mut tx, project_id, &candidates, now).await?;
    let decided = settle(candidates, &remembered, counts, now);
    apply(&mut tx, project_id, decided).await?;
    tx.commit().await?;
    Ok(())
}

/// What the table remembers of one value: its newest row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Remembered {
    pub(crate) id: Uuid,
    pub(crate) seen_at: DateTime<Utc>,
}

/// An agent's changes of one kind on one UTC day.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Day {
    /// Those shown, the capped one among them.
    pub(crate) shown: i64,
    pub(crate) capped: bool,
    /// The newest shown, which says so once the day is past its cap.
    pub(crate) newest_shown: Option<(Uuid, DateTime<Utc>)>,
}

/// The days a tick's candidates fall on: each agent's by kind, and the
/// project's rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Counts {
    pub(crate) days: HashMap<(String, ChangeKind, NaiveDate), Day>,
    pub(crate) rows: HashMap<NaiveDate, i64>,
}

/// What the table holds of the UTC days `candidates` could be changes on.
async fn counts(
    tx: &mut Scoped<'_, telmoni_shared::db::tenant_session::Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
    candidates: &[Candidate],
    now: DateTime<Utc>,
) -> Result<Counts, TelmoniError> {
    let times = candidates.iter().map(|c| c.first_at.min(now));
    let (Some(first), Some(last)) = (times.clone().min(), times.max()) else {
        return Ok(Counts::default());
    };
    let start = first
        .date_naive()
        .and_time(chrono::NaiveTime::MIN)
        .and_utc();
    let end = last.date_naive().and_time(chrono::NaiveTime::MIN).and_utc() + TimeDelta::days(1);
    type DayRow = (
        String,
        ChangeKind,
        NaiveDate,
        i64,
        i64,
        bool,
        Option<Uuid>,
        Option<DateTime<Utc>>,
    );
    let rows: Vec<DayRow> = sqlx::query_as(
        "SELECT agent, kind, (changed_at AT TIME ZONE 'UTC')::date AS day, count(*) AS rows,
                count(*) FILTER (WHERE mark <> 'hidden') AS shown,
                coalesce(bool_or(mark = 'capped'), false) AS capped,
                (array_agg(id ORDER BY changed_at DESC, id DESC)
                    FILTER (WHERE mark = 'shown'))[1] AS newest_shown,
                max(changed_at) FILTER (WHERE mark = 'shown') AS newest_shown_at
           FROM telemetry.changes
          WHERE project_id = $1 AND changed_at >= $2 AND changed_at < $3
          GROUP BY agent, kind, day",
    )
    .bind(project_id)
    .bind(start)
    .bind(end)
    .fetch_all(tx.conn())
    .await?;
    let mut counts = Counts::default();
    for (agent, kind, day, rows, shown, capped, newest, newest_at) in rows {
        *counts.rows.entry(day).or_default() += rows;
        counts.days.insert(
            (agent, kind, day),
            Day {
                shown,
                capped,
                newest_shown: newest.zip(newest_at),
            },
        );
    }
    Ok(counts)
}

/// A value an agent carried in one environment, as one tick read it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) agent: String,
    pub(crate) environment: String,
    pub(crate) kind: ChangeKind,
    pub(crate) prompt: Option<String>,
    pub(crate) value: String,
    pub(crate) first_at: DateTime<Utc>,
    pub(crate) first_trace: String,
    pub(crate) first_received: DateTime<Utc>,
    pub(crate) last_received: DateTime<Utc>,
}

impl Candidate {
    /// The five that name it, as one digest the table keys on.
    pub(crate) fn identity(&self) -> String {
        let named = [
            self.agent.as_str(),
            self.environment.as_str(),
            self.kind.as_str(),
            self.prompt.as_deref().unwrap_or(""),
            self.value.as_str(),
        ];
        sha256_hex(serde_json::to_string(&named).unwrap_or_default().as_bytes())
    }
}

/// A row to write.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct New {
    pub(crate) identity: String,
    pub(crate) candidate: Candidate,
    pub(crate) changed_at: DateTime<Utc>,
    pub(crate) mark: Mark,
}

/// What one project's tick writes.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Decided {
    pub(crate) new: Vec<New>,
    /// Rows still carried, and their last arrival.
    pub(crate) seen: Vec<(Uuid, DateTime<Utc>)>,
    /// Stored rows that now say their day was capped.
    pub(crate) capped: Vec<Uuid>,
}

/// The decision, with no store: in the order the candidates started, one
/// the table remembers carried within [`NEW_FOR`] of its first arrival is
/// no change, its `seen_at` moved at most each [`SEEN_EVERY`]; any other is,
/// at its start held to `now`, shown until its agent's day has
/// [`DAY_SHOWN`] of its kind, then hidden, the day's newest shown marked
/// `capped` once; none past the project's [`DAY_ROWS_MAX`] for the day.
pub(crate) fn settle(
    candidates: Vec<Candidate>,
    remembered: &HashMap<String, Remembered>,
    mut counts: Counts,
    now: DateTime<Utc>,
) -> Decided {
    let mut candidates: Vec<(String, Candidate)> = candidates
        .into_iter()
        .map(|candidate| (candidate.identity(), candidate))
        .collect();
    candidates.sort_by(|(a_identity, a), (b_identity, b)| {
        a.first_at
            .cmp(&b.first_at)
            .then_with(|| a_identity.cmp(b_identity))
    });
    let mut decided = Decided::default();
    // Each day's newest shown row this tick writes, by its place in `new`.
    let mut newest_new: HashMap<(String, ChangeKind, NaiveDate), usize> = HashMap::new();
    for (identity, candidate) in candidates {
        if let Some(row) = remembered.get(&identity)
            && row.seen_at >= candidate.first_received - NEW_FOR
        {
            if candidate.last_received >= row.seen_at + SEEN_EVERY {
                decided.seen.push((row.id, candidate.last_received));
            }
            continue;
        }
        let changed_at = candidate.first_at.min(now);
        let date = changed_at.date_naive();
        let rows = counts.rows.entry(date).or_default();
        if *rows >= DAY_ROWS_MAX {
            continue;
        }
        *rows += 1;
        let key = (candidate.agent.clone(), candidate.kind, date);
        let day = counts.days.entry(key.clone()).or_default();
        let mark = if day.shown < DAY_SHOWN {
            day.shown += 1;
            Mark::Shown
        } else {
            if !day.capped {
                day.capped = true;
                let stored = day.newest_shown;
                let in_tick = newest_new
                    .get(&key)
                    .and_then(|&at| decided.new.get_mut(at))
                    .filter(|new| stored.is_none_or(|(_, stored_at)| new.changed_at >= stored_at));
                match (in_tick, stored) {
                    (Some(new), _) => new.mark = Mark::Capped,
                    (None, Some((id, _))) => decided.capped.push(id),
                    (None, None) => {}
                }
            }
            Mark::Hidden
        };
        if mark == Mark::Shown {
            newest_new.insert(key, decided.new.len());
        }
        decided.new.push(New {
            identity,
            candidate,
            changed_at,
            mark,
        });
    }
    decided
}

/// `decided` written: the new rows, each once, the moved `seen_at`s and the
/// days now capped.
async fn apply(
    tx: &mut Scoped<'_, telmoni_shared::db::tenant_session::Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
    decided: Decided,
) -> sqlx::Result<()> {
    if !decided.new.is_empty() {
        let new = &decided.new;
        let column = |of: fn(&New) -> &str| new.iter().map(of).collect::<Vec<&str>>();
        let ids: Vec<Uuid> = new.iter().map(|_| Uuid::new_v4()).collect();
        let prompts: Vec<Option<&str>> =
            new.iter().map(|n| n.candidate.prompt.as_deref()).collect();
        let changed: Vec<DateTime<Utc>> = new.iter().map(|n| n.changed_at).collect();
        let traces: Vec<&str> = new
            .iter()
            .map(|n| n.candidate.first_trace.as_str())
            .collect();
        let seen: Vec<DateTime<Utc>> = new.iter().map(|n| n.candidate.last_received).collect();
        sqlx::query(
            "INSERT INTO telemetry.changes
                 (id, project_id, agent, environment, kind, prompt, value, identity, changed_at,
                  trace_id, seen_at, mark)
             SELECT u.id, $1, u.agent, u.environment, u.kind, u.prompt, u.value, u.identity,
                    u.changed_at, u.trace_id, u.seen_at, u.mark
               FROM UNNEST($2::uuid[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[],
                           $8::text[], $9::timestamptz[], $10::text[], $11::timestamptz[],
                           $12::text[])
                 AS u(id, agent, environment, kind, prompt, value, identity, changed_at, trace_id,
                      seen_at, mark)
             ON CONFLICT (project_id, identity, trace_id) DO NOTHING",
        )
        .bind(project_id)
        .bind(&ids)
        .bind(column(|n| &n.candidate.agent))
        .bind(column(|n| &n.candidate.environment))
        .bind(column(|n| n.candidate.kind.as_str()))
        .bind(&prompts)
        .bind(column(|n| &n.candidate.value))
        .bind(column(|n| &n.identity))
        .bind(&changed)
        .bind(&traces)
        .bind(&seen)
        .bind(column(|n| n.mark.as_str()))
        .execute(tx.conn())
        .await?;
    }
    if !decided.seen.is_empty() {
        let (ids, seen): (Vec<Uuid>, Vec<DateTime<Utc>>) = decided.seen.into_iter().unzip();
        sqlx::query(
            "UPDATE telemetry.changes AS c SET seen_at = u.seen_at
               FROM UNNEST($2::uuid[], $3::timestamptz[]) AS u(id, seen_at)
              WHERE c.project_id = $1 AND c.id = u.id AND c.seen_at < u.seen_at",
        )
        .bind(project_id)
        .bind(&ids)
        .bind(&seen)
        .execute(tx.conn())
        .await?;
    }
    if !decided.capped.is_empty() {
        sqlx::query(
            "UPDATE telemetry.changes SET mark = 'capped'
              WHERE project_id = $1 AND id = ANY($2) AND mark = 'shown'",
        )
        .bind(project_id)
        .bind(&decided.capped)
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("a time")
            .with_timezone(&Utc)
    }

    /// A release `value` of `checkout` in production, first started and
    /// arrived at `when`.
    fn release(value: &str, when: &str) -> Candidate {
        Candidate {
            agent: "checkout".into(),
            environment: "production".into(),
            kind: ChangeKind::Release,
            prompt: None,
            value: value.into(),
            first_at: at(when),
            first_trace: "1".repeat(32),
            first_received: at(when),
            last_received: at(when),
        }
    }

    fn day(when: &str) -> NaiveDate {
        at(when).date_naive()
    }

    const NOW: &str = "2026-11-08T12:00:00Z";

    /// A value new to the table is shown, at its start, by its first trace.
    #[test]
    fn a_value_never_carried_is_a_change() {
        let decided = settle(
            vec![release("v2", "2026-11-08T10:00:00Z")],
            &HashMap::new(),
            Counts::default(),
            at(NOW),
        );
        assert_eq!(decided.new.len(), 1);
        assert_eq!(decided.new[0].mark, Mark::Shown);
        assert_eq!(decided.new[0].changed_at, at("2026-11-08T10:00:00Z"));
        assert!(decided.seen.is_empty() && decided.capped.is_empty());
    }

    /// One the table saw within its 30 days is none, its last arrival moved
    /// once an hour has passed; one last seen past them is a change again.
    #[test]
    fn a_value_carried_in_the_thirty_days_before_is_none() {
        let candidate = release("v2", "2026-11-08T10:00:00Z");
        let id = Uuid::from_u128(7);
        let seen_20_days_ago = HashMap::from([(
            candidate.identity(),
            Remembered {
                id,
                seen_at: at("2026-10-19T10:00:00Z"),
            },
        )]);
        let decided = settle(
            vec![candidate.clone()],
            &seen_20_days_ago,
            Counts::default(),
            at(NOW),
        );
        assert!(decided.new.is_empty());
        assert_eq!(decided.seen, [(id, at("2026-11-08T10:00:00Z"))]);

        let seen_this_hour = HashMap::from([(
            candidate.identity(),
            Remembered {
                id,
                seen_at: at("2026-11-08T09:30:00Z"),
            },
        )]);
        let decided = settle(
            vec![candidate.clone()],
            &seen_this_hour,
            Counts::default(),
            at(NOW),
        );
        assert!(decided.new.is_empty() && decided.seen.is_empty());

        let seen_31_days_ago = HashMap::from([(
            candidate.identity(),
            Remembered {
                id,
                seen_at: at("2026-10-08T09:00:00Z"),
            },
        )]);
        let decided = settle(
            vec![candidate],
            &seen_31_days_ago,
            Counts::default(),
            at(NOW),
        );
        assert_eq!(decided.new.len(), 1);
    }

    /// Twenty-five new on one day: the first 24 shown, by when they started
    /// whatever the order read, the 24th capped, the 25th remembered hidden.
    #[test]
    fn a_day_past_its_cap_marks_no_further_and_says_so() {
        let mut candidates: Vec<Candidate> = (0..25)
            .map(|n| release(&format!("v{n:02}"), &format!("2026-11-08T10:{n:02}:00Z")))
            .collect();
        candidates.reverse();
        let decided = settle(candidates, &HashMap::new(), Counts::default(), at(NOW));
        let marks: Vec<Mark> = decided.new.iter().map(|new| new.mark).collect();
        assert_eq!(marks.len(), 25);
        assert!(marks[..23].iter().all(|mark| *mark == Mark::Shown));
        assert_eq!(marks[23], Mark::Capped);
        assert_eq!(marks[24], Mark::Hidden);
        assert_eq!(decided.new[24].candidate.value, "v24");
    }

    /// A day the table already holds 24 shown of: the next is hidden and the
    /// stored newest says so, once; a day already capped says nothing more.
    #[test]
    fn a_stored_days_cap_is_marked_on_its_newest_shown_change() {
        let newest = Uuid::from_u128(24);
        let full = |capped| Counts {
            days: HashMap::from([(
                ("checkout".to_owned(), ChangeKind::Release, day(NOW)),
                Day {
                    shown: DAY_SHOWN,
                    capped,
                    newest_shown: Some((newest, at("2026-11-08T09:00:00Z"))),
                },
            )]),
            rows: HashMap::from([(day(NOW), DAY_SHOWN)]),
        };
        let candidates = vec![
            release("v25", "2026-11-08T10:00:00Z"),
            release("v26", "2026-11-08T10:01:00Z"),
        ];
        let decided = settle(candidates.clone(), &HashMap::new(), full(false), at(NOW));
        assert!(decided.new.iter().all(|new| new.mark == Mark::Hidden));
        assert_eq!(decided.capped, [newest]);
        let decided = settle(candidates, &HashMap::new(), full(true), at(NOW));
        assert!(decided.capped.is_empty());
    }

    /// A value the table remembers is still carried on a capped day: its
    /// `seen_at` moves, so it is not taken for new once it ages.
    #[test]
    fn a_capped_day_still_moves_what_its_values_last_arrived() {
        let candidate = release("v1", "2026-11-08T10:00:00Z");
        let id = Uuid::from_u128(1);
        let remembered = HashMap::from([(
            candidate.identity(),
            Remembered {
                id,
                seen_at: at("2026-11-07T10:00:00Z"),
            },
        )]);
        let capped = Counts {
            days: HashMap::from([(
                ("checkout".to_owned(), ChangeKind::Release, day(NOW)),
                Day {
                    shown: DAY_SHOWN,
                    capped: true,
                    newest_shown: None,
                },
            )]),
            rows: HashMap::new(),
        };
        let decided = settle(vec![candidate], &remembered, capped, at(NOW));
        assert_eq!(decided.seen, [(id, at("2026-11-08T10:00:00Z"))]);
    }

    /// Past the project's rows for the day, nothing more is written.
    #[test]
    fn a_projects_day_writes_no_more_than_its_rows() {
        let full = Counts {
            days: HashMap::new(),
            rows: HashMap::from([(day(NOW), DAY_ROWS_MAX)]),
        };
        let decided = settle(
            vec![release("v1", "2026-11-08T10:00:00Z")],
            &HashMap::new(),
            full,
            at(NOW),
        );
        assert!(decided.new.is_empty());
    }

    /// A start a clock ahead put past the tick is held to the tick's clock.
    #[test]
    fn a_change_is_never_marked_after_the_clock_that_found_it() {
        let decided = settle(
            vec![release("v1", "2026-11-08T12:04:00Z")],
            &HashMap::new(),
            Counts::default(),
            at(NOW),
        );
        assert_eq!(decided.new[0].changed_at, at(NOW));
    }

    /// The identity tells each of the five apart, and a prompt's name from
    /// its version, wherever a colon falls.
    #[test]
    fn the_identity_names_each_value_apart() {
        let prompt = |name: &str, version: &str| Candidate {
            kind: ChangeKind::Prompt,
            prompt: Some(name.into()),
            value: version.into(),
            ..release("", NOW)
        };
        assert_ne!(
            prompt("triage:blue", "v2").identity(),
            prompt("triage", "blue:v2").identity()
        );
        let mut staging = release("v2", NOW);
        staging.environment = "staging".into();
        assert_ne!(staging.identity(), release("v2", NOW).identity());
        assert_eq!(release("v2", NOW).identity().len(), 64);
    }

    /// The window starts five minutes before the last pass ended, never past
    /// a day back, and reads an hour at most; with none, the last hour.
    #[test]
    fn the_window_goes_on_from_the_last_pass() {
        let now = at(NOW);
        assert_eq!(span_of(None, now), (now - STEP, now));
        let last = now - TimeDelta::minutes(1);
        assert_eq!(span_of(Some(last), now), (last - OVERLAP, now));
        let three_hours = now - TimeDelta::hours(3);
        assert_eq!(
            span_of(Some(three_hours), now),
            (three_hours - OVERLAP, three_hours - OVERLAP + STEP)
        );
        let a_week = now - TimeDelta::days(7);
        assert_eq!(
            span_of(Some(a_week), now),
            (now - CATCH_UP, now - CATCH_UP + STEP)
        );
        let ahead = now + TimeDelta::hours(1);
        assert_eq!(span_of(Some(ahead), now), (now - OVERLAP, now));
    }
}
