//! A project's monitors, under [`Resource::Monitor`] — each an agent's
//! contract: when its runs are expected, how long one may go quiet, how many
//! steps it may take and what it may spend. `GET /internal/v1/telemetry/monitors`
//! and `GET /monitors/{slug}`, which every role on the project reads; `POST`,
//! `PUT /monitors/{slug}`, `DELETE`, `POST /monitors/{slug}/pause` and
//! `/resume`, which its owners and admins send; and `POST /monitors/schedule`,
//! which answers what a schedule means and saves nothing, and `GET
//! /timezones`, the zones a monitor may be in. A monitor is made under the
//! organization's chain lock and held to its plan's ceiling, and every
//! change to one is audited as [`TelmoniResourceKind::Monitor`], since a
//! monitor is what pages somebody, or stops paging them.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::{Json, correlation_id};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam;
use telmoni_shared::{
    AuditAction, AuthError, IncidentDetail, IncidentResolution, LaneCeilings, MonitorCeiling,
    MonitorState, ProjectId, TelmoniError, TelmoniResourceKind, slug,
};

use crate::AppState;
use crate::ceilings;
use crate::contract;
use crate::db::{self, IncidentRow, LastRun, MonitorRow, MonitorWrite, OpenRunRow};
use crate::handler::chain;
use crate::incidents;
use crate::schedule::{self, Fault, PERIOD_MAX_S, PREVIEW, Schedule};

/// The longest a monitor's name is, in characters, as the table's CHECK has
/// it.
pub const NAME_MAX: usize = 60;
/// The longest an agent's name is: the contract's, since every run names it.
pub const AGENT_MAX: usize = contract::AGENT_MAX;
/// The longest a zone's name is, as the table's CHECK has it.
pub const TIMEZONE_MAX: usize = 64;
/// The most steps a cap may name.
pub const MAX_STEPS_MAX: i32 = 1_000_000;
/// The most dollars a cap may name.
pub const MAX_COST_USD_MAX: f64 = 1_000_000.0;
/// Words no monitor goes by: the lane beside `/monitors/{slug}` and the page
/// beside the monitor's own, each of which a monitor so named would shadow.
pub const RESERVED: &[&str] = &["new", "schedule"];
/// How many of a monitor's runs still going its page is answered, and how
/// many of its resolved incidents.
pub const OPEN_RUNS_SHOWN: i64 = 10;
pub const RESOLVED_SHOWN: i64 = 10;
/// The unique indexes a second monitor of one slug, or of one agent, runs
/// into, whichever lane writes it ([`refused_as_taken`]).
const SLUG_TAKEN: &str = "monitors_project_slug_idx";
const AGENT_TAKEN: &str = "monitors_project_agent_idx";
/// The zone a monitor is in when none is sent, by its owner or by the ping
/// that made it.
pub(crate) const DEFAULT_TIMEZONE: &str = "UTC";

/// A run's bounds, as a monitor holds its runs to them: none is no bound.
/// The tighter of these and the run's own, sent with it by the SDK, is what
/// the run is held to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bounds {
    /// How long a run may go quiet, in seconds.
    pub stall_after_s: Option<i32>,
    /// How many steps a run may take.
    pub max_steps: Option<i32>,
    /// What a run may spend, in US dollars.
    pub max_cost_usd: Option<f64>,
}

/// What making or changing a monitor sends. A slug left out is made from
/// the name; an agent left out is the slug; a zone left out is UTC; a
/// schedule or an interval left out means the monitor expects no run.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorInput {
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub interval_s: Option<i32>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub grace_s: Option<i32>,
    #[serde(default)]
    pub expected_duration_s: Option<i32>,
    #[serde(default)]
    pub bounds: Bounds,
}

/// What asking what a schedule means sends: the timing fields of a monitor,
/// as they are typed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulePreviewInput {
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub interval_s: Option<i32>,
    #[serde(default)]
    pub timezone: Option<String>,
}

/// What it answers: the next few times a run would be due, in UTC; none for
/// a monitor that expects no run.
#[derive(Debug, Serialize)]
pub struct SchedulePreview {
    pub next: Vec<DateTime<Utc>>,
}

/// A monitor's timing as it is written, each field checked: the line and the
/// zone as typed and trimmed, and what they mean.
#[derive(Debug, Clone)]
struct Timing {
    schedule: Option<String>,
    interval_s: Option<i32>,
    timezone: String,
    read: Schedule,
}

impl Timing {
    fn checked(
        schedule: Option<String>,
        interval_s: Option<i32>,
        timezone: Option<String>,
    ) -> Result<Self, TelmoniError> {
        let schedule = schedule
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty());
        let timezone = timezone
            .map(|zone| zone.trim().to_owned())
            .filter(|zone| !zone.is_empty())
            .unwrap_or_else(|| DEFAULT_TIMEZONE.to_owned());
        if timezone.chars().count() > TIMEZONE_MAX {
            return Err(fault(Fault::Timezone));
        }
        let read = Schedule::read(schedule.as_deref(), interval_s, &timezone).map_err(fault)?;
        Ok(Self {
            schedule,
            interval_s,
            timezone,
            read,
        })
    }
}

/// A monitor as it is written: every field checked and trimmed, the slug
/// made from the name where none was sent, the agent the slug where none
/// was.
#[derive(Debug, Clone)]
struct Checked {
    slug: String,
    name: String,
    agent: String,
    timing: Timing,
    grace_s: Option<i32>,
    expected_duration_s: Option<i32>,
    bounds: Bounds,
}

impl MonitorInput {
    /// Checked for a monitor being made, or, with `fixed`, for one being
    /// changed, whose slug it is and stays.
    fn checked(self, fixed: Option<&str>) -> Result<Checked, TelmoniError> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX || name.chars().any(char::is_control)
        {
            return Err(TelmoniError::BadRequest(format!(
                "a monitor's name is 1 to {NAME_MAX} characters, none of them a control character"
            )));
        }
        let slug = match (fixed, self.slug.as_deref().map(str::trim)) {
            (Some(fixed), Some(sent)) if sent != fixed => {
                return Err(TelmoniError::BadRequest(
                    "a monitor's slug is fixed once it is saved, since every crontab line \
                     carries it"
                        .into(),
                ));
            }
            (Some(fixed), _) => fixed.to_owned(),
            (None, Some(sent)) if !sent.is_empty() => sent.to_owned(),
            (None, _) => slug::slugify(name).ok_or_else(|| {
                TelmoniError::BadRequest(
                    "a monitor's slug could not be made from its name; send one".into(),
                )
            })?,
        };
        if !slug::is_slug(&slug) {
            return Err(TelmoniError::BadRequest(format!(
                "a monitor's slug is lowercase letters and digits joined by single hyphens, up \
                 to {} characters",
                slug::MAX_LEN
            )));
        }
        if slug::offends(&slug) {
            return Err(TelmoniError::BadRequest(
                "a monitor's slug may not hold that word".into(),
            ));
        }
        if RESERVED.contains(&slug.as_str()) {
            return Err(TelmoniError::BadRequest(
                "a monitor may not go by that word; it names a lane or a page beside the \
                 monitors"
                    .into(),
            ));
        }
        let agent = match self.agent.as_deref().map(str::trim) {
            Some(agent) if !agent.is_empty() => {
                if agent.chars().count() > AGENT_MAX || agent.chars().any(char::is_control) {
                    return Err(TelmoniError::BadRequest(format!(
                        "a monitor's agent is 1 to {AGENT_MAX} characters, none of them a \
                         control character: the telmoni.agent its runs carry"
                    )));
                }
                agent.to_owned()
            }
            _ => slug.clone(),
        };
        let timing = Timing::checked(self.schedule, self.interval_s, self.timezone)?;
        let grace_s = match (timing.read.is_scheduled(), self.grace_s) {
            (true, None) => {
                return Err(TelmoniError::BadRequest(
                    "a monitor's grace_s goes with its schedule or interval: how long past its \
                     due time a run may start, in whole seconds"
                        .into(),
                ));
            }
            (true, Some(grace)) => {
                if !(0..=PERIOD_MAX_S).contains(&grace) {
                    return Err(TelmoniError::BadRequest(format!(
                        "a monitor's grace_s is whole seconds, from 0 to a year and a day \
                         ({PERIOD_MAX_S})"
                    )));
                }
                Some(grace)
            }
            (false, Some(_)) => {
                return Err(TelmoniError::BadRequest(
                    "a monitor's grace_s has no meaning without a schedule or an interval".into(),
                ));
            }
            (false, None) => None,
        };
        if let Some(expected) = self.expected_duration_s
            && !(1..=PERIOD_MAX_S).contains(&expected)
        {
            return Err(TelmoniError::BadRequest(format!(
                "a monitor's expected_duration_s is whole seconds, from 1 to a year and a day \
                 ({PERIOD_MAX_S})"
            )));
        }
        let bounds = self.bounds;
        if let Some(stall) = bounds.stall_after_s
            && !(1..=PERIOD_MAX_S).contains(&stall)
        {
            return Err(TelmoniError::BadRequest(format!(
                "a monitor's stall_after_s is whole seconds, from 1 to a year and a day \
                 ({PERIOD_MAX_S})"
            )));
        }
        if let Some(steps) = bounds.max_steps
            && !(1..=MAX_STEPS_MAX).contains(&steps)
        {
            return Err(TelmoniError::BadRequest(format!(
                "a monitor's max_steps is a whole number, from 1 to {MAX_STEPS_MAX}"
            )));
        }
        if let Some(cost) = bounds.max_cost_usd
            && !(cost.is_finite() && cost > 0.0 && cost <= MAX_COST_USD_MAX)
        {
            return Err(TelmoniError::BadRequest(format!(
                "a monitor's max_cost_usd is US dollars, above 0 and up to {MAX_COST_USD_MAX}"
            )));
        }
        Ok(Checked {
            slug,
            name: name.to_owned(),
            agent,
            timing,
            grace_s,
            expected_duration_s: self.expected_duration_s,
            bounds,
        })
    }
}

impl Checked {
    fn write(&self, next_due_at: Option<DateTime<Utc>>) -> MonitorWrite<'_> {
        MonitorWrite {
            slug: &self.slug,
            name: &self.name,
            agent: &self.agent,
            schedule: self.timing.schedule.as_deref(),
            interval_s: self.timing.interval_s,
            timezone: &self.timing.timezone,
            grace_s: self.grace_s,
            expected_duration_s: self.expected_duration_s,
            stall_after_s: self.bounds.stall_after_s,
            max_steps: self.bounds.max_steps,
            max_cost_usd: self.bounds.max_cost_usd,
            next_due_at,
        }
    }

    /// Whether this and `row` time the monitor the same way, so its next due
    /// time stands.
    fn times_as(&self, row: &MonitorRow) -> bool {
        self.timing.schedule == row.schedule
            && self.timing.interval_s == row.interval_s
            && self.timing.timezone == row.timezone
    }

    /// The fields this changes on `row`, by name, for the audit row.
    fn changes_to(&self, row: &MonitorRow) -> Vec<&'static str> {
        let mut changed = Vec::new();
        let mut note = |field: &'static str, differs: bool| {
            if differs {
                changed.push(field);
            }
        };
        note("name", self.name != row.name);
        note("agent", self.agent != row.agent);
        note("schedule", self.timing.schedule != row.schedule);
        note("interval_s", self.timing.interval_s != row.interval_s);
        note("timezone", self.timing.timezone != row.timezone);
        note("grace_s", self.grace_s != row.grace_s);
        note(
            "expected_duration_s",
            self.expected_duration_s != row.expected_duration_s,
        );
        note(
            "stall_after_s",
            self.bounds.stall_after_s != row.stall_after_s,
        );
        note("max_steps", self.bounds.max_steps != row.max_steps);
        note("max_cost_usd", self.bounds.max_cost_usd != row.max_cost_usd);
        changed
    }
}

/// One monitor, as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct Monitor {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub agent: String,
    pub schedule: Option<String>,
    pub interval_s: Option<i32>,
    pub timezone: String,
    pub grace_s: Option<i32>,
    pub expected_duration_s: Option<i32>,
    pub bounds: Bounds,
    pub state: MonitorState,
    pub state_changed_at: DateTime<Utc>,
    pub next_due_at: Option<DateTime<Utc>>,
    /// Its last run that ended, kept as it ended; none until one has.
    pub last_run: Option<LastRun>,
    /// When its last run that ended well ended; none until one has.
    pub last_success_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<MonitorRow> for Monitor {
    fn from(row: MonitorRow) -> Self {
        Self {
            last_run: row.last_run(),
            last_success_at: row.last_success_at,
            id: row.id,
            slug: row.slug,
            name: row.name,
            agent: row.agent,
            schedule: row.schedule,
            interval_s: row.interval_s,
            timezone: row.timezone,
            grace_s: row.grace_s,
            expected_duration_s: row.expected_duration_s,
            bounds: Bounds {
                stall_after_s: row.stall_after_s,
                max_steps: row.max_steps,
                max_cost_usd: row.max_cost_usd,
            },
            state: row.state,
            state_changed_at: row.state_changed_at,
            next_due_at: row.next_due_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// What the list answers.
#[derive(Debug, Serialize)]
pub struct MonitorList {
    /// The project's monitors, the worst state first, then by name.
    pub monitors: Vec<Monitor>,
}

/// A run of the monitor's still going: its counts so far, and the bounds it
/// is held to, each the tighter of the monitor's and the run's own.
#[derive(Debug, Serialize)]
pub struct OpenRun {
    pub run_id: Uuid,
    /// `ping` for a run its pings report, `ingest` for one an SDK sends.
    pub source: String,
    pub started_at: DateTime<Utc>,
    pub last_heard_at: DateTime<Utc>,
    /// When it is over whatever it says, past which the ticker ends it.
    pub ends_by: DateTime<Utc>,
    pub steps: i32,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub bounds: Bounds,
}

impl From<OpenRunRow> for OpenRun {
    fn from(row: OpenRunRow) -> Self {
        Self {
            run_id: row.id,
            source: row.source,
            started_at: row.started_at,
            last_heard_at: row.last_heard_at,
            ends_by: row.ends_by,
            steps: row.steps,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: row.cost_usd,
            bounds: Bounds {
                stall_after_s: row.stall_after_s,
                max_steps: row.max_steps,
                max_cost_usd: row.max_cost_usd,
            },
        }
    }
}

/// One of the monitor's incidents: the state that opened it and why, when,
/// and how it closed.
#[derive(Debug, Serialize)]
pub struct Incident {
    pub id: Uuid,
    pub cause: MonitorState,
    pub detail: IncidentDetail,
    pub opened_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolution: Option<IncidentResolution>,
    pub flapping: bool,
}

impl TryFrom<IncidentRow> for Incident {
    type Error = TelmoniError;

    /// A word the table's CHECK would never take is the database's fault.
    fn try_from(row: IncidentRow) -> Result<Self, Self::Error> {
        let unreadable =
            || TelmoniError::Internal("an incident's row holds a word the lane cannot read".into());
        Ok(Self {
            id: row.id,
            cause: row.cause,
            detail: row.detail.parse().map_err(|_| unreadable())?,
            opened_at: row.opened_at,
            resolved_at: row.resolved_at,
            resolution: row
                .resolution
                .map(|word| word.parse().map_err(|_| unreadable()))
                .transpose()?,
            flapping: row.flapping,
        })
    }
}

/// One monitor as its page reads it: the monitor, its runs still going and
/// its incidents, the open one first.
#[derive(Debug, Serialize)]
pub struct MonitorDetail {
    #[serde(flatten)]
    pub monitor: Monitor,
    pub open_runs: Vec<OpenRun>,
    pub incidents: Vec<Incident>,
}

/// What the timezones lane answers: every zone a monitor may be in.
#[derive(Debug, Serialize)]
pub struct Timezones {
    pub zones: Vec<&'static str>,
}

/// A refusal of a timing, as a 400 naming its field.
fn fault(fault: Fault) -> TelmoniError {
    TelmoniError::BadRequest(fault.detail().to_owned())
}

/// When `monitor` is next due after `after`, under the timing its row keeps:
/// `None` for one that expects no run, and for one whose timing cannot be
/// read now. Every timing is read and searched as it is saved, so only a
/// change of ours makes one unreadable — a `croner` that reads a line more
/// strictly, a zone the database no longer names — and that is the
/// operator's to mend, never a ping or a resume to be refused for. So it is
/// logged and the monitor is due at no time until a read succeeds again: at
/// its next run's end or resume once a release can read the timing, or at a
/// save, which reads the timing as sent and names what it cannot read. No
/// time, rather than one already passed, which would call it late however
/// its runs came.
pub(crate) fn due_after(
    project_id: &ProjectId,
    monitor: &MonitorRow,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    Schedule::read(
        monitor.schedule.as_deref(),
        monitor.interval_s,
        &monitor.timezone,
    )
    .and_then(|schedule| schedule.next_due(after))
    .unwrap_or_else(|fault| {
        tracing::error!(
            project_id = %project_id,
            monitor_id = %monitor.id,
            fault = ?fault,
            "telemetry: a saved monitor's timing cannot be read; it is due at no time"
        );
        None
    })
}

/// A monitor the project does not have.
fn not_found() -> TelmoniError {
    AuthError::NotFound("no monitor by that slug in this project".into()).into()
}

/// A slug from its path: one of a slug's shape, or a monitor the project
/// does not have.
fn monitor_slug(given: &str) -> Result<String, TelmoniError> {
    if slug::is_slug(given) {
        Ok(given.to_owned())
    } else {
        Err(not_found())
    }
}

/// A write a monitor's unique index refused, in the lane's own words
/// (`slug_taken`, `agent_taken`), never in what the request sent; any other
/// failure is the database's.
pub(crate) fn refused_as_taken(
    error: sqlx::Error,
    slug_taken: &'static str,
    agent_taken: &'static str,
) -> TelmoniError {
    let words = match error.as_database_error().and_then(|db| db.constraint()) {
        Some(SLUG_TAKEN) => slug_taken,
        Some(AGENT_TAKEN) => agent_taken,
        _ => return error.into(),
    };
    AuthError::Conflict(words.to_owned()).into()
}

/// A write a unique index refused, in the console's words: a monitor of
/// that slug, or for that agent, is the project's already.
fn taken(error: sqlx::Error) -> TelmoniError {
    refused_as_taken(
        error,
        "this project has a monitor by that slug already",
        "this project has a monitor for that agent already",
    )
}

/// `GET /internal/v1/telemetry/monitors` — the project's monitors, the worst
/// state first.
pub async fn list_monitors(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<MonitorList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Monitor)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = db::monitors(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(MonitorList {
        monitors: rows.into_iter().map(Monitor::from).collect(),
    }))
}

/// `GET /internal/v1/telemetry/monitors/{slug}` — one of the project's
/// monitors, with its runs still going and its incidents: the open one and
/// the last [`RESOLVED_SHOWN`] resolved.
pub async fn get_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<MonitorDetail>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Monitor)?
        .project_id;
    let slug = monitor_slug(&slug)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let Some(row) = db::monitor(&mut tx, project_id, &slug).await? else {
        tx.commit().await?;
        return Err(not_found());
    };
    let open_runs = db::open_runs_of(&mut tx, project_id, row.id, OPEN_RUNS_SHOWN).await?;
    let incidents = db::incidents_of(&mut tx, project_id, row.id, RESOLVED_SHOWN).await?;
    tx.commit().await?;
    Ok(Json(MonitorDetail {
        monitor: Monitor::from(row),
        open_runs: open_runs.into_iter().map(OpenRun::from).collect(),
        incidents: incidents
            .into_iter()
            .map(Incident::try_from)
            .collect::<Result<_, _>>()?,
    }))
}

/// `GET /internal/v1/telemetry/timezones` — every zone a monitor may be in,
/// for its form's picker; every role on the project may ask, as the
/// schedule's preview does.
pub async fn list_timezones(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Timezones>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    acting.require_project(Verb::Read, Resource::Monitor)?;
    Ok(Json(Timezones {
        zones: schedule::zones(),
    }))
}

/// `POST /internal/v1/telemetry/monitors/schedule` — what a schedule means: the
/// next few times a run would be due. Saves nothing, so every role on the
/// project may ask.
pub async fn preview_schedule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<SchedulePreviewInput>,
) -> Result<Json<SchedulePreview>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    acting.require_project(Verb::Read, Resource::Monitor)?;
    let timing = Timing::checked(input.schedule, input.interval_s, input.timezone)?;
    let next = timing
        .read
        .next_due_times(Utc::now(), PREVIEW)
        .map_err(fault)?;
    Ok(Json(SchedulePreview { next }))
}

/// `POST /internal/v1/telemetry/monitors` — a new monitor of the project, made
/// under the organization's chain lock, as a view is: the project's settings
/// row is made with it where the project has none, and the lock puts two
/// monitors made at once one after the other, so neither counts past the
/// plan's ceiling.
pub async fn create_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<MonitorInput>,
) -> Result<(StatusCode, Json<Monitor>), TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    // Refused before any lock is taken, so nobody who may not make one waits
    // on the organization's chain to be told no.
    let project_id = acting
        .require_project(Verb::Create, Resource::Monitor)?
        .project_id
        .clone();
    let monitor = input.checked(None)?;
    seam::admit_new(state.entitlements.as_ref(), &acting.organization_id).await?;
    // The plan's ceilings, read before a transaction is opened and the lock
    // taken: only the counts under the lock need them.
    let ceiling = chain::ceiling(state.entitlements.as_ref(), &acting.organization_id).await;
    let lanes = state
        .ceilings
        .of(state.entitlements.as_ref(), &acting.organization_id)
        .await;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match make_monitor(
        &state,
        &acting,
        &project_id,
        &mut tx,
        &headers,
        &monitor,
        (ceiling.as_ref(), lanes.as_ref()),
    )
    .await
    {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "monitor");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, monitor_id = %row.id, "telemetry: a monitor made");
    Ok((StatusCode::CREATED, Json(Monitor::from(row))))
}

/// The making itself, inside the transaction [`create_monitor`] opened:
/// under the organization's chain lock, the role checked again on auth's
/// second answer, the project's settings row made or re-filed, the
/// organization's count held to the plan's monitor `ceiling` and its
/// schedules to the pings its `lanes` hold, both read before the lock, and
/// the monitor written and recorded.
async fn make_monitor(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    headers: &HeaderMap,
    monitor: &Checked,
    (ceiling, lanes): (Option<&MonitorCeiling>, Option<&LaneCeilings>),
) -> Result<MonitorRow, TelmoniError> {
    let organization_id = &acting.organization_id;
    let now = chain::hold(&*state.auth, acting, project_id, tx).await?;
    now.require_project(Verb::Create, Resource::Monitor)?;
    db::ensure_settings(tx, project_id, organization_id).await?;
    if let Some(ceiling) = ceiling {
        let held = db::monitor_count(tx, organization_id).await?;
        // A negative count, which no table gives, fails closed.
        ceiling.admit(u64::try_from(held).unwrap_or(u64::MAX))?;
    }
    ceilings::admit_schedules(tx, organization_id, lanes, None, &monitor.timing.read).await?;
    let next_due_at = monitor.timing.read.next_due(Utc::now()).map_err(fault)?;
    let row = db::insert_monitor(
        tx,
        project_id,
        organization_id,
        Uuid::new_v4(),
        MonitorState::Healthy,
        monitor.write(next_due_at),
    )
    .await
    .map_err(taken)?;
    emit_audit(
        tx,
        AuditEvent {
            organization_id,
            in_project: Some(project_id),
            actor: Actor::User(acting.user_id.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Monitor,
            resource_id: Some(&row.id.to_string()),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "slug": row.slug, "agent": row.agent })),
        },
    )
    .await?;
    Ok(row)
}

/// The monitor a change is about, read under the organization's chain lock,
/// which the change holds to its commit, with the role checked again on
/// auth's second answer, the one that stands: a transfer of the project
/// committed between the two answers refuses the change rather than filing
/// it under an organization that no longer holds the project. Read plainly:
/// a ping takes no chain lock, so what a ping moves is read again from the
/// row as the change writes it.
async fn held(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    verb: Verb,
    slug: &str,
) -> Result<MonitorRow, TelmoniError> {
    let now = chain::hold(&*state.auth, acting, project_id, tx).await?;
    now.require_project(verb, Resource::Monitor)?;
    db::monitor(tx, project_id, slug)
        .await?
        .ok_or_else(not_found)
}

/// `PUT /internal/v1/telemetry/monitors/{slug}` — one of the project's
/// monitors, saved over: everything but its slug. A monitor its first ping
/// or run made, `new`, is set up by this and starts; a paused one stays
/// paused. Its next due time stands while its timing does, but for one a
/// timing that could not be read left it without; a timing that never comes
/// is refused whenever it is sent, as at a make.
pub async fn update_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Json(input): Json<MonitorInput>,
) -> Result<Json<Monitor>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Monitor)?
        .project_id
        .clone();
    let slug = monitor_slug(&slug)?;
    let monitor = input.checked(Some(&slug))?;
    let lanes = state
        .ceilings
        .of(state.entitlements.as_ref(), &acting.organization_id)
        .await;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match change_monitor(
        &state,
        &acting,
        &project_id,
        &mut tx,
        &headers,
        (&slug, &monitor),
        lanes.as_ref(),
    )
    .await
    {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "monitor");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, monitor_id = %row.id, "telemetry: a monitor saved over");
    Ok(Json(Monitor::from(row)))
}

/// The change itself, inside the transaction [`update_monitor`] opened:
/// under the organization's chain lock, its schedules held to the pings
/// the plan's `lanes` hold, written over the row as it stands, and recorded
/// where it changes something.
async fn change_monitor(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    headers: &HeaderMap,
    (slug, monitor): (&str, &Checked),
    lanes: Option<&LaneCeilings>,
) -> Result<MonitorRow, TelmoniError> {
    let before = held(state, acting, project_id, tx, Verb::Update, slug).await?;
    ceilings::admit_schedules(
        tx,
        &acting.organization_id,
        lanes,
        Some(before.id),
        &monitor.timing.read,
    )
    .await?;
    let starts = before.state == MonitorState::New;
    // Searched whenever it is sent, changed or not and paused or not, so a
    // line that never comes is refused as a make refuses it, and no timing is
    // saved that a ping or a resume could not read.
    let found = monitor.timing.read.next_due(Utc::now()).map_err(fault)?;
    // No ping pauses a monitor or moves a timing, so the read decides these;
    // only a kept due time is read again, as the row is written.
    let (next_due_at, keep_due) = if before.state == MonitorState::Paused {
        (None, false)
    } else if monitor.times_as(&before) && !starts {
        // The timing stands, so its due time does — but where it has none,
        // which a timing that could not be read leaves (`due_after`), the one
        // just found.
        (found, true)
    } else {
        (found, false)
    };
    let state_after = starts.then_some(MonitorState::Healthy);
    let changed = monitor.changes_to(&before);
    let row = db::update_monitor(
        tx,
        project_id,
        slug,
        monitor.write(next_due_at),
        keep_due,
        state_after,
    )
    .await
    .map_err(taken)?
    .ok_or_else(not_found)?;
    if changed.is_empty() && !starts {
        return Ok(row);
    }
    let mut metadata = serde_json::Map::new();
    metadata.insert("slug".into(), json!(row.slug));
    metadata.insert("changed".into(), json!(changed));
    if starts {
        metadata.insert(
            "state".into(),
            json!({ "from": MonitorState::New, "to": MonitorState::Healthy }),
        );
    }
    emit_audit(
        tx,
        AuditEvent {
            organization_id: &acting.organization_id,
            in_project: Some(project_id),
            actor: Actor::User(acting.user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Monitor,
            resource_id: Some(&row.id.to_string()),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(serde_json::Value::Object(metadata)),
        },
    )
    .await?;
    Ok(row)
}

/// `POST /internal/v1/telemetry/monitors/{slug}/pause` — the monitor paused:
/// never late, its runs still kept. One paused already is answered as it is,
/// and so is one its first ping or run made, paused until its owner saves it.
pub async fn pause_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<Monitor>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Monitor)?
        .project_id
        .clone();
    let slug = monitor_slug(&slug)?;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match halt_monitor(&state, &acting, &project_id, &mut tx, &headers, &slug).await {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "monitor");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, monitor_id = %row.id, "telemetry: a monitor paused");
    Ok(Json(Monitor::from(row)))
}

/// The pause itself, inside the transaction [`pause_monitor`] opened: under
/// the organization's chain lock, a monitor not paused already put in
/// `paused` with no next due time, and recorded with the state it wrote
/// over; one paused already, or `new` — paused until its owner saves its
/// settings, which a pause must not let a resume skip — is answered as it
/// is, and nothing is recorded.
async fn halt_monitor(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    headers: &HeaderMap,
    slug: &str,
) -> Result<MonitorRow, TelmoniError> {
    let read = held(state, acting, project_id, tx, Verb::Update, slug).await?;
    // Only a change under the chain lock pauses or resumes, so the plain
    // read says whether to write; the state recorded is read again, locked,
    // since a ping may have moved it.
    if matches!(read.state, MonitorState::Paused | MonitorState::New) {
        return Ok(read);
    }
    let before = db::monitor_to_change(tx, project_id, slug)
        .await?
        .ok_or_else(not_found)?;
    let row = db::set_monitor_state(tx, project_id, slug, MonitorState::Paused, None)
        .await?
        .ok_or_else(not_found)?;
    // The incident its trouble opened, if one, closed as paused.
    incidents::on_change(
        tx,
        project_id,
        &acting.organization_id,
        &row,
        before.state,
        None,
        Utc::now(),
    )
    .await?;
    emit_audit(
        tx,
        AuditEvent {
            organization_id: &acting.organization_id,
            in_project: Some(project_id),
            actor: Actor::User(acting.user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Monitor,
            resource_id: Some(&row.id.to_string()),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "slug": row.slug,
                "state": { "from": before.state, "to": MonitorState::Paused },
            })),
        },
    )
    .await?;
    Ok(row)
}

/// `POST /internal/v1/telemetry/monitors/{slug}/resume` — a paused monitor
/// watched again, its next run due from now. One not paused is answered as
/// it is, but for one its first ping or run made, which its settings start.
pub async fn resume_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Json<Monitor>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Update, Resource::Monitor)?
        .project_id
        .clone();
    let slug = monitor_slug(&slug)?;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match restart_monitor(&state, &acting, &project_id, &mut tx, &headers, &slug).await {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "monitor");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, monitor_id = %row.id, "telemetry: a monitor resumed");
    Ok(Json(Monitor::from(row)))
}

/// The resumption itself, inside the transaction [`resume_monitor`] opened:
/// under the organization's chain lock, a paused monitor put back in
/// `healthy`, its next run due from now under its own timing — at no time
/// where that timing cannot be read now ([`due_after`]) — and recorded.
async fn restart_monitor(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    headers: &HeaderMap,
    slug: &str,
) -> Result<MonitorRow, TelmoniError> {
    let before = held(state, acting, project_id, tx, Verb::Update, slug).await?;
    match before.state {
        MonitorState::Paused => {}
        MonitorState::New => {
            return Err(AuthError::Conflict(
                "this monitor was made by its first ping or run; save its settings to start it"
                    .into(),
            )
            .into());
        }
        _ => return Ok(before),
    }
    let next_due_at = due_after(project_id, &before, Utc::now());
    let row = db::set_monitor_state(tx, project_id, slug, MonitorState::Healthy, next_due_at)
        .await?
        .ok_or_else(not_found)?;
    emit_audit(
        tx,
        AuditEvent {
            organization_id: &acting.organization_id,
            in_project: Some(project_id),
            actor: Actor::User(acting.user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Monitor,
            resource_id: Some(&row.id.to_string()),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "slug": row.slug,
                "state": { "from": MonitorState::Paused, "to": MonitorState::Healthy },
            })),
        },
    )
    .await?;
    Ok(row)
}

/// `DELETE /internal/v1/telemetry/monitors/{slug}` — one of the project's
/// monitors, gone; its finished runs stay, as traces of its agent, and a
/// run still open goes with it.
pub async fn delete_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Delete, Resource::Monitor)?
        .project_id
        .clone();
    let slug = monitor_slug(&slug)?;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match remove_monitor(&state, &acting, &project_id, &mut tx, &headers, &slug).await {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "monitor");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, monitor_id = %row.id, "telemetry: a monitor deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// The removal itself, inside the transaction [`delete_monitor`] opened:
/// under the organization's chain lock, the row gone, its open runs with it
/// ([`db::delete_monitor`]), and recorded.
async fn remove_monitor(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    headers: &HeaderMap,
    slug: &str,
) -> Result<MonitorRow, TelmoniError> {
    let row = held(state, acting, project_id, tx, Verb::Delete, slug).await?;
    if !db::delete_monitor(tx, project_id, row.id).await? {
        return Err(not_found());
    }
    emit_audit(
        tx,
        AuditEvent {
            organization_id: &acting.organization_id,
            in_project: Some(project_id),
            actor: Actor::User(acting.user_id.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Monitor,
            resource_id: Some(&row.id.to_string()),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "slug": row.slug, "agent": row.agent })),
        },
    )
    .await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(body: serde_json::Value) -> MonitorInput {
        serde_json::from_value(body).expect("a monitor's body")
    }

    /// A monitor is kept as sent, trimmed: its slug from its name and its
    /// agent from its slug where neither was sent, UTC where no zone was.
    #[test]
    fn a_monitor_is_filled_in_from_its_name() {
        let checked = input(json!({ "name": "  Nightly report " }))
            .checked(None)
            .expect("a monitor");
        assert_eq!(checked.name, "Nightly report");
        assert_eq!(checked.slug, "nightly-report");
        assert_eq!(checked.agent, "nightly-report");
        assert_eq!(checked.timing.timezone, "UTC");
        assert!(!checked.timing.read.is_scheduled());
        assert_eq!(checked.grace_s, None);
        assert_eq!(checked.bounds, Bounds::default());

        let checked = input(json!({
            "name": "Nightly report",
            "slug": "report",
            "agent": " report-bot ",
            "schedule": " 0 2 * * * ",
            "timezone": "Europe/London",
            "grace_s": 600,
            "expected_duration_s": 300,
            "bounds": { "stall_after_s": 120, "max_steps": 40, "max_cost_usd": 2.5 },
        }))
        .checked(None)
        .expect("a monitor");
        assert_eq!(checked.slug, "report");
        assert_eq!(checked.agent, "report-bot");
        assert_eq!(checked.timing.schedule.as_deref(), Some("0 2 * * *"));
        assert_eq!(checked.timing.timezone, "Europe/London");
        assert!(checked.timing.read.is_scheduled());
        assert_eq!(checked.grace_s, Some(600));
        assert_eq!(checked.bounds.max_steps, Some(40));
    }

    /// Changed, a monitor keeps its slug: one sent that is not its own is
    /// refused, and none sent is fine.
    #[test]
    fn a_changed_monitor_keeps_its_slug() {
        let kept = input(json!({ "name": "Report" }))
            .checked(Some("nightly"))
            .expect("a monitor");
        assert_eq!(kept.slug, "nightly");
        let same = input(json!({ "name": "Report", "slug": "nightly" }))
            .checked(Some("nightly"))
            .expect("its own slug");
        assert_eq!(same.slug, "nightly");
        let moved = input(json!({ "name": "Report", "slug": "weekly" }))
            .checked(Some("nightly"))
            .expect_err("another slug");
        assert_eq!(moved.to_problem_details().status, 400);
    }

    /// ⚠ A field the lane cannot take is a 400 that names the field and
    /// echoes none of its value.
    #[test]
    fn a_monitor_the_lane_cannot_take_is_refused_naming_the_field() {
        for (body, field) in [
            (json!({ "name": "" }), "name"),
            (json!({ "name": "x".repeat(NAME_MAX + 1) }), "name"),
            (json!({ "name": "two\nlines<script>" }), "name"),
            (
                json!({ "name": "Report", "slug": "Nightly<script>" }),
                "slug",
            ),
            (json!({ "name": "Report", "slug": "new" }), "word"),
            (json!({ "name": "Report", "slug": "schedule" }), "word"),
            (json!({ "name": "Report", "slug": "shit-2" }), "word"),
            (json!({ "name": "日報" }), "slug"),
            (
                json!({ "name": "Report", "agent": "a".repeat(AGENT_MAX + 1) }),
                "agent",
            ),
            (
                json!({ "name": "Report", "schedule": "0 0 2 * * *<script>", "grace_s": 0 }),
                "schedule",
            ),
            (
                json!({ "name": "Report", "schedule": "0 2 * * *", "interval_s": 60, "grace_s": 0 }),
                "schedule",
            ),
            (
                json!({ "name": "Report", "interval_s": 59, "grace_s": 0 }),
                "interval_s",
            ),
            (
                json!({ "name": "Report", "interval_s": 60, "grace_s": 0, "timezone": "Mars/Olympus<script>" }),
                "timezone",
            ),
            (
                json!({ "name": "Report", "schedule": "0 2 * * *" }),
                "grace_s",
            ),
            (
                json!({ "name": "Report", "schedule": "0 2 * * *", "grace_s": -1 }),
                "grace_s",
            ),
            (json!({ "name": "Report", "grace_s": 60 }), "grace_s"),
            (
                json!({ "name": "Report", "expected_duration_s": 0 }),
                "expected_duration_s",
            ),
            (
                json!({ "name": "Report", "bounds": { "stall_after_s": 0 } }),
                "stall_after_s",
            ),
            (
                json!({ "name": "Report", "bounds": { "max_steps": 0 } }),
                "max_steps",
            ),
            (
                json!({ "name": "Report", "bounds": { "max_cost_usd": 0 } }),
                "max_cost_usd",
            ),
            (
                json!({ "name": "Report", "bounds": { "max_cost_usd": -1.5 } }),
                "max_cost_usd",
            ),
        ] {
            let refused = input(body.clone())
                .checked(None)
                .expect_err(&body.to_string());
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400, "{body}");
            let detail = problem.detail.unwrap_or_default();
            assert!(detail.contains(field), "{body}: {detail}");
            assert!(!detail.contains("script"), "{body} was echoed: {detail}");
        }
        for body in [
            json!({ "name": "Report", "spend": 1 }),
            json!({ "name": "Report", "bounds": { "max_tokens": 1 } }),
            json!({ "name": "Report", "interval_s": 1.5 }),
            json!({ "slug": "report" }),
        ] {
            assert!(
                serde_json::from_value::<MonitorInput>(body.clone()).is_err(),
                "{body}"
            );
        }
    }
}
