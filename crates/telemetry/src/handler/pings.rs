//! The public ping lane: `/ping/{ping_key}/{slug}`, `GET` or `POST` with no
//! header, so a crontab line needs only `curl`. A bare ping is success; its
//! `/start`, `/checkpoint` and `/fail` are the rest of a run, each held on
//! the run's open row until the run ends, when the run becomes a row of
//! `spans` through the outbox ([`crate::outbox`]), as every finished span
//! does. The key rides in the URL and is resolved outside any tenant, as
//! auth resolves an API key; the request then runs in the project's scope.
//! A slug no monitor has makes its monitor, paused as `new`, and the owner
//! is told once; past the plan's limit, or past the monitors nobody has set
//! up that one project may hold, nothing is made and the ping is refused
//! with a problem document that says what lifts it, which `curl -f` turns
//! into cron's mail. A ping's body is content: read no further than its
//! cap, never parsed whatever its media type, and kept nowhere until 4.1
//! keeps one, the answer saying so; a run's progress rides in the query
//! alone, numbers alone. Nothing here is audited but the monitor a first
//! ping makes: a ping is traffic, and its run is the lane's own record.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SubsecRound as _, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{
    ProjectAndOrganization, Scoped, maintenance_scope, project_scope,
};
use telmoni_shared::digest::sha256_hex;
use telmoni_shared::extract::correlation_id;
use telmoni_shared::middleware::problem;
use telmoni_shared::seam;
use telmoni_shared::types::{PING_KEY_PREFIX, PING_KEY_RANDOM_LEN};
use telmoni_shared::{
    AuditAction, AuthError, MonitorState, ProjectId, SpanStatus, TelmoniError, TelmoniResourceKind,
    slug,
};

use crate::AppState;
use crate::contract::PING_BODY_MAX_BYTES;
use crate::db::{
    self, MonitorRow, MonitorWrite, OpenRunRow, OpenRunWrite, ResolvedPingKey, TelemetryLane,
};
use crate::handler::chain;
use crate::handler::monitors::{MAX_COST_USD_MAX, MAX_STEPS_MAX, RESERVED};
use crate::notify;
use crate::outbox::{self, FinishedRun};
use crate::schedule::Schedule;

/// The methods the lane takes, for a `405`.
const ALLOW: &str = "GET, HEAD, POST";
/// What an open run's `source` says of a run reported by pings.
const SOURCE: &str = "ping";
/// The zone a monitor a ping makes is in, until its owner sets one.
const DEFAULT_TIMEZONE: &str = "UTC";
/// The unique indexes a monitor a ping makes can run into: its slug, which
/// no monitor has or the ping would have found it, and its agent, which a
/// monitor of another slug may carry.
const SLUG_TAKEN: &str = "monitors_project_slug_idx";
const AGENT_TAKEN: &str = "monitors_project_agent_idx";
/// The primary key a run opened under a `rid` the project has open already
/// runs into: the key is the project's and the id's, so the run it finds is
/// one of the project's own, under another of its monitors.
const RUN_TAKEN: &str = "open_runs_pkey";
/// The most monitors one project holds that its pings made and nobody has
/// set up — `new` — before a ping that would make one more is refused: room
/// for a crontab's worth of first pings at once, and a bound on what a slug
/// that carries a date can fill a project with. The plan's ceiling counts
/// every monitor of the organization; this counts the unset ones of one
/// project, so it holds on a binary that sells nothing too.
pub const NEW_MONITORS_MAX: i64 = 50;
/// The stall timeout a monitor its first ping made is given, in seconds:
/// an hour, so its runs are among the ones the ticker visits for silence
/// and a job that crashes after its `/start` is seen, until its owner sets
/// the real one. The monitor is held to nothing until then, as `new` is.
pub const DISCOVERED_STALL_AFTER_S: i32 = 60 * 60;
/// How long a run may stay open whatever it says: its monitor's expected
/// duration [`RUN_CEILING_FACTOR`] times over, or [`RUN_CEILING_MIN_S`]
/// where that is shorter or the monitor expects nothing. Past it the ticker
/// ends the run as `error`, since a run that never ends is one that crashed
/// before its end ping: the backstop under the stall timeout, which the
/// owner tunes and a run without one has none of, so no row is left for
/// good.
pub const RUN_CEILING_FACTOR: i32 = 4;
pub const RUN_CEILING_MIN_S: i64 = 24 * 60 * 60;

/// When a run opened at `started_at` is over whatever it says
/// ([`RUN_CEILING_FACTOR`]), for its row's `ends_by`.
#[must_use]
pub fn ends_by(started_at: DateTime<Utc>, expected_duration_s: Option<i32>) -> DateTime<Utc> {
    let ceiling = expected_duration_s
        .map_or(RUN_CEILING_MIN_S, |expected| {
            i64::from(expected) * i64::from(RUN_CEILING_FACTOR)
        })
        .max(RUN_CEILING_MIN_S);
    started_at + chrono::Duration::seconds(ceiling)
}

/// Which ping came, and so what it means of its run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    /// A bare ping: the open run ends `ok`, or a run of no duration is
    /// recorded.
    Success,
    /// A run opens, and its stall timeout starts.
    Start,
    /// Progress: steps and spend so far; the stall timeout starts again.
    Checkpoint,
    /// The open run ends `error`, with its exit code; or a run of no
    /// duration ended `error` is recorded.
    Fail,
}

/// What a ping's query says, as sent: each value a number alone, read by
/// `Progress::read`. A key the lane does not know is let by, so a line
/// that carries one of its own is not refused for it.
#[derive(Debug, Default, Deserialize)]
pub struct PingQuery {
    #[serde(default)]
    rid: Option<String>,
    #[serde(default)]
    steps: Option<String>,
    #[serde(default)]
    cost: Option<String>,
    #[serde(default)]
    exit_code: Option<String>,
}

/// A ping's query, read: the run it names, and the progress or the exit
/// code it reports.
#[derive(Debug, Default, Clone, Copy)]
struct Progress {
    rid: Option<Uuid>,
    steps: Option<i32>,
    cost_usd: Option<f64>,
    exit_code: Option<i32>,
}

impl Progress {
    /// Each value a number alone, or a 400 that names the field and echoes
    /// none of its value. A query that could not be read at all — a field
    /// sent twice — is a 400 too, so a wrong line is found at its first
    /// run rather than recorded without what it meant to say.
    fn read(query: Result<Query<PingQuery>, QueryRejection>) -> Result<Self, TelmoniError> {
        let Query(query) = query.map_err(|_| {
            TelmoniError::BadRequest(
                "a ping's query holds rid, steps, cost and exit_code, each once and each a \
                 number alone"
                    .into(),
            )
        })?;
        let sent = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        let rid = sent(&query.rid)
            .map(|v| {
                Uuid::parse_str(&v).map_err(|_| {
                    TelmoniError::BadRequest("a ping's rid is a UUID: the run it names".into())
                })
            })
            .transpose()?;
        let steps = sent(&query.steps)
            .map(|v| {
                v.parse::<i32>()
                    .ok()
                    .filter(|steps| (0..=MAX_STEPS_MAX).contains(steps))
                    .ok_or_else(|| {
                        TelmoniError::BadRequest(format!(
                            "a ping's steps is a whole number, from 0 to {MAX_STEPS_MAX}"
                        ))
                    })
            })
            .transpose()?;
        let cost_usd = sent(&query.cost)
            .map(|v| {
                v.parse::<f64>()
                    .ok()
                    .filter(|cost| cost.is_finite() && (0.0..=MAX_COST_USD_MAX).contains(cost))
                    .ok_or_else(|| {
                        TelmoniError::BadRequest(format!(
                            "a ping's cost is US dollars, a number from 0 to {MAX_COST_USD_MAX}"
                        ))
                    })
            })
            .transpose()?;
        let exit_code = sent(&query.exit_code)
            .map(|v| {
                v.parse::<i32>().map_err(|_| {
                    TelmoniError::BadRequest(
                        "a ping's exit_code is a whole number: the command's".into(),
                    )
                })
            })
            .transpose()?;
        Ok(Self {
            rid,
            steps,
            cost_usd,
            exit_code,
        })
    }
}

/// What a ping did, for the answer and the log.
#[derive(Debug, Clone, Copy)]
struct Heard {
    /// The request carried a body, which was dropped.
    body_dropped: bool,
    run_id: Uuid,
    monitor_id: Uuid,
}

/// `GET` or `POST /ping/{ping_key}/{slug}` — success: the open run ends `ok`,
/// or a run of no duration is recorded.
pub async fn success(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((key, slug)): Path<(String, String)>,
    query: Result<Query<PingQuery>, QueryRejection>,
    body: Body,
) -> Response {
    answer(handle(&state, &headers, &key, &slug, query, body, Report::Success).await)
}

/// `GET` or `POST /ping/{ping_key}/{slug}/start` — a run opens, and its
/// stall timeout starts.
pub async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((key, slug)): Path<(String, String)>,
    query: Result<Query<PingQuery>, QueryRejection>,
    body: Body,
) -> Response {
    answer(handle(&state, &headers, &key, &slug, query, body, Report::Start).await)
}

/// `GET` or `POST /ping/{ping_key}/{slug}/checkpoint?steps=4&cost=0.08` —
/// progress: steps and spend so far, numbers alone; the stall timeout
/// starts again. With no run open, it opens the run it reports on, since
/// the run's `/start` was lost on the way.
pub async fn checkpoint(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((key, slug)): Path<(String, String)>,
    query: Result<Query<PingQuery>, QueryRejection>,
    body: Body,
) -> Response {
    answer(
        handle(
            &state,
            &headers,
            &key,
            &slug,
            query,
            body,
            Report::Checkpoint,
        )
        .await,
    )
}

/// `GET` or `POST /ping/{ping_key}/{slug}/fail?exit_code=1` — the open run
/// ends `error`, with its exit code; with no run open, a run of no duration
/// ended `error` is recorded.
pub async fn fail(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((key, slug)): Path<(String, String)>,
    query: Result<Query<PingQuery>, QueryRejection>,
    body: Body,
) -> Response {
    answer(handle(&state, &headers, &key, &slug, query, body, Report::Fail).await)
}

/// A path under `/ping` no lane serves: a `404` problem document, as the
/// other machine lanes answer one, in place of axum's bare one.
pub async fn not_found() -> Response {
    problem::not_found("no such lane under /ping")
}

/// A method no lane takes: `405` with `Allow`, as a problem document.
pub async fn method_not_allowed() -> Response {
    problem::method_not_allowed(ALLOW)
}

/// What a ping is answered: `200` with `OK` — or `OK; body dropped` — in
/// plain text, since a crontab reads nothing else; a refusal as a problem
/// document, which `curl -f` turns into an exit code and a line in cron's
/// mail.
fn answer(outcome: Result<Heard, TelmoniError>) -> Response {
    match outcome {
        Ok(heard) => {
            let text = if heard.body_dropped {
                "OK; body dropped"
            } else {
                "OK"
            };
            (
                StatusCode::OK,
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                )],
                text,
            )
                .into_response()
        }
        Err(error) => {
            let status = error.to_problem_details().status;
            if status >= 500 {
                tracing::error!(error = %error, status, "ping: refused on our side");
            }
            error.into_response()
        }
    }
}

/// The lane's order: the key, then the slug, then the query, then the
/// organization's standing, then the body, and only then the project's
/// scope. A key the table does not hold, or holds retired, is a 404 whatever
/// else the request says, so a scanner learns nothing from the rest.
async fn handle(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    key: &str,
    slug: &str,
    query: Result<Query<PingQuery>, QueryRejection>,
    body: Body,
    report: Report,
) -> Result<Heard, TelmoniError> {
    let key = resolve_key(state, key).await?;
    let slug = checked_slug(slug)?;
    let progress = Progress::read(query)?;
    // A paused organization keeps what it holds and takes in nothing new: a
    // 402, as ingest answers one.
    seam::admit_new(state.entitlements.as_ref(), &key.organization_id).await?;
    let body_dropped = dropped(headers, body).await;
    let now = Utc::now().trunc_subsecs(6);

    let mut tx = project_scope(&state.db, &key.project_id)
        .await?
        .bind_organization(&key.organization_id)
        .await?;
    let heard = match ping(state, headers, &key, &slug, progress, report, now, &mut tx).await {
        Ok(heard) => heard,
        Err(refused) => {
            rolled_back(tx.rollback().await, &key.project_id);
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::debug!(
        project_id = %key.project_id,
        ping_key_id = %key.id,
        monitor_id = %heard.monitor_id,
        run_id = %heard.run_id,
        report = ?report,
        body_dropped,
        request_id = ?correlation_id(headers),
        "ping: heard"
    );
    Ok(Heard {
        body_dropped,
        ..heard
    })
}

/// The key a ping presents, resolved outside any tenant as auth resolves an
/// API key: the live key with that hash, or a 404 — never a 401, so a
/// scanner learns nothing from the difference. A key of another shape is a
/// 404 without a read.
async fn resolve_key(state: &AppState, key: &str) -> Result<ResolvedPingKey, TelmoniError> {
    if !well_formed(key) {
        return Err(unknown_key());
    }
    let hash = sha256_hex(key.as_bytes());
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let found = db::resolve_ping_key(&mut tx, &hash).await?;
    tx.commit().await?;
    found.ok_or_else(unknown_key)
}

/// Whether `key` is spelled as a ping key is minted — the prefix and its
/// letters and digits — so nothing else costs a hash and a read. The door
/// asks the same of the key's segment as it came, before the path is
/// decoded, so an encoding of a key never passes as one.
#[must_use]
pub fn well_formed(key: &str) -> bool {
    key.strip_prefix(PING_KEY_PREFIX).is_some_and(|rest| {
        rest.len() == PING_KEY_RANDOM_LEN && rest.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// A key the table does not hold, or holds retired — and, at the door, one
/// spelled otherwise than minted. ⚠ Never the key itself.
#[must_use]
pub fn unknown_key() -> TelmoniError {
    AuthError::NotFound("no ping key by that value".into()).into()
}

/// A slug of the core's shape and no word a monitor may not go by, else a
/// 400 that echoes none of it. The words beside `/monitors/{slug}` are
/// refused as a monitor's slug is, so a ping can never name a monitor the
/// console could not.
fn checked_slug(given: &str) -> Result<String, TelmoniError> {
    if !slug::is_slug(given) {
        return Err(TelmoniError::BadRequest(format!(
            "a monitor's slug is lowercase letters and digits joined by single hyphens, up to \
             {} characters",
            slug::MAX_LEN
        )));
    }
    if slug::offends(given) || RESERVED.contains(&given) {
        return Err(TelmoniError::BadRequest(
            "a monitor may not go by that word".into(),
        ));
    }
    Ok(given.to_owned())
}

/// Whether the request carried a body, which is dropped. A ping's body is
/// content — kept by 4.1 on an `on` project, and by nothing until then — and
/// is never parsed whatever its media type: a run's progress rides in the
/// query alone, so no form field is ever read as one. It is read no further
/// than its cap, so a log attached to a crontab's `curl` costs a bounded
/// read, and one announced past the cap is not read at all.
async fn dropped(headers: &HeaderMap, body: Body) -> bool {
    let announced = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<usize>().ok());
    if announced.is_some_and(|length| length > PING_BODY_MAX_BYTES) {
        return true;
    }
    match to_bytes(body, PING_BODY_MAX_BYTES).await {
        Ok(bytes) => !bytes.is_empty(),
        Err(_) => true,
    }
}

/// A refused ping let go of: nothing was written, and the chain lock, where
/// one was taken, goes with the transaction rather than whenever the pool
/// next sees the connection.
fn rolled_back(result: sqlx::Result<()>, project_id: &ProjectId) {
    if let Err(e) = result {
        tracing::warn!(
            project_id = %project_id,
            error = %e,
            "ping: a refused ping could not be rolled back; its connection drops it"
        );
    }
}

/// The ping itself, inside the transaction [`handle`] opened: the key
/// touched, the monitor found or made, the run recorded and the monitor
/// settled.
#[expect(
    clippy::too_many_arguments,
    reason = "a ping is these eight things; a struct built once for one call would say the same in more lines"
)]
async fn ping(
    state: &AppState,
    headers: &HeaderMap,
    key: &ResolvedPingKey,
    slug: &str,
    progress: Progress,
    report: Report,
    now: DateTime<Utc>,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<Heard, TelmoniError> {
    let project_id = &key.project_id;
    db::touch_ping_key(tx, project_id, key.id).await?;
    let monitor = match db::monitor(tx, project_id, slug).await? {
        Some(monitor) => monitor,
        None => discover_monitor(state, headers, key, slug, tx).await?,
    };
    let (run_id, event) = record(key, &monitor, progress, report, now, tx).await?;
    settle(project_id, &monitor, event, now, tx).await?;
    Ok(Heard {
        body_dropped: false,
        run_id,
        monitor_id: monitor.id,
    })
}

/// A monitor made by its first ping, as the owner's Decide has it: in the
/// state `new` — no schedule, so never late; its pings and runs kept and
/// counted; paused for its owner to set up — under the organization's chain
/// lock, held to the monitors nobody has set up that the project may hold
/// and to the plan's ceiling, recorded on the chain as made by the key, and
/// the owner told once in the feed. Past either bound nothing is made and
/// the ping is refused with the bound's own words — a 409 that says to set
/// up or delete what the project holds, the ceiling's 402 that names what
/// lifts it — since a notice a slug would cost more than the plan, and a
/// refusal is what `curl -f` turns into cron's mail at the first run.
async fn discover_monitor(
    state: &AppState,
    headers: &HeaderMap,
    key: &ResolvedPingKey,
    slug: &str,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<MonitorRow, TelmoniError> {
    let (project_id, organization_id) = (&key.project_id, &key.organization_id);
    chain::lock(project_id, organization_id, tx).await?;
    // Another ping may have made it while this one waited for the lock.
    if let Some(monitor) = db::monitor(tx, project_id, slug).await? {
        return Ok(monitor);
    }
    db::ensure_settings(tx, project_id, organization_id).await?;
    let unset = db::new_monitor_count(tx, project_id).await?;
    if unset >= NEW_MONITORS_MAX {
        tracing::debug!(
            project_id = %project_id,
            ping_key_id = %key.id,
            unset,
            "ping: a monitor was not made; the project holds every unset one it may"
        );
        return Err(AuthError::Conflict(format!(
            "this project holds {NEW_MONITORS_MAX} monitors its pings made and nobody has set \
             up, the most it may: set them up or delete them before a ping makes another"
        ))
        .into());
    }
    if let Some(ceiling) = seam::monitor_ceiling(state.entitlements.as_ref(), organization_id).await
    {
        let held = db::monitor_count(tx, organization_id).await?;
        if let Err(refused) = ceiling.admit(u64::try_from(held).unwrap_or(u64::MAX)) {
            tracing::debug!(
                project_id = %project_id,
                ping_key_id = %key.id,
                limit = ceiling.limit,
                "ping: a monitor was not made, for the plan's limit; the ping is refused"
            );
            return Err(refused);
        }
    }
    let monitor = db::insert_monitor(
        tx,
        project_id,
        organization_id,
        Uuid::new_v4(),
        MonitorState::New,
        MonitorWrite {
            slug,
            name: slug,
            agent: slug,
            schedule: None,
            interval_s: None,
            timezone: DEFAULT_TIMEZONE,
            grace_s: None,
            expected_duration_s: None,
            stall_after_s: Some(DISCOVERED_STALL_AFTER_S),
            max_steps: None,
            max_cost_usd: None,
            next_due_at: None,
        },
    )
    .await
    .map_err(taken)?;
    emit_audit(
        tx,
        AuditEvent {
            organization_id,
            in_project: Some(project_id),
            actor: Actor::External("ping-key"),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Monitor,
            resource_id: Some(&monitor.id.to_string()),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "slug": monitor.slug,
                "agent": monitor.agent,
                "ping_key_id": key.id,
                "ping_key_prefix": key.prefix,
            })),
        },
    )
    .await?;
    outbox::enqueue_notice(
        tx,
        project_id,
        organization_id,
        &notify::monitor_discovered(&monitor, key),
    )
    .await?;
    tracing::info!(
        project_id = %project_id,
        monitor_id = %monitor.id,
        ping_key_id = %key.id,
        "ping: a monitor made by its first ping, as new"
    );
    Ok(monitor)
}

/// A write a unique index refused: a monitor of that slug was made beside
/// this ping, which the lock should have put after it, or a monitor of
/// another slug carries the agent this one would. Any other failure is the
/// database's.
fn taken(error: sqlx::Error) -> TelmoniError {
    match error.as_database_error().and_then(|db| db.constraint()) {
        Some(SLUG_TAKEN) => {
            AuthError::Conflict("a monitor by that slug was made just now; ping again".into())
                .into()
        }
        Some(AGENT_TAKEN) => AuthError::Conflict(
            "this project has a monitor for that agent under another slug; ping that slug".into(),
        )
        .into(),
        _ => error.into(),
    }
}

/// What a ping did to its run, which decides the monitor's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunEvent {
    Opened,
    Heard,
    OverBound,
    EndedOk,
    EndedError,
    /// Ended beside this ping, by another: nothing of the monitor's to change.
    Gone,
}

/// The run recorded: opened, heard, or ended and handed to the outbox as a
/// finished span, once — an end that finds the run ended beside it hands
/// over nothing. A run that never said it ended — a job that crashed after
/// its `/start`, a `/checkpoint` that came after its run's end — is ended
/// `error` by the next ping that finds it ([`abandon`]): a `/start` naming
/// no run ends what its monitor still has open, and an end ping naming none
/// ends the newest with its verdict and the rest the same way; a ping
/// naming a run leaves the others be, since runs that overlap name
/// themselves. Answers the run's id and what happened to it.
async fn record(
    key: &ResolvedPingKey,
    monitor: &MonitorRow,
    progress: Progress,
    report: Report,
    now: DateTime<Utc>,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<(Uuid, RunEvent), TelmoniError> {
    let project_id = &key.project_id;
    // The run the ping names, or the monitor's newest open one.
    let open = match progress.rid {
        Some(rid) => db::open_run(tx, project_id, rid)
            .await?
            .filter(|run| run.monitor_id == monitor.id),
        None => db::newest_open_run(tx, project_id, monitor.id).await?,
    };
    match report {
        Report::Start => match (open, progress.rid) {
            // A `/start` sent again for the run it named — a retried curl —
            // is heard, not a second run.
            (Some(run), Some(_)) => {
                db::hear_open_run(tx, project_id, run.id, now, None, None).await?;
                Ok((run.id, RunEvent::Heard))
            }
            (_, rid) => {
                // The monitor's next run: whatever it still has open never
                // said it ended.
                if rid.is_none() {
                    let stale = db::close_open_runs(tx, project_id, monitor.id).await?;
                    abandon(key, monitor, stale, now, tx).await?;
                }
                let id = rid.unwrap_or_else(Uuid::new_v4);
                let run = open_run(key, monitor, id, now, 0, 0.0, tx).await?;
                Ok((run.id, RunEvent::Opened))
            }
        },
        Report::Checkpoint => {
            let run = match open {
                Some(run) => db::hear_open_run(
                    tx,
                    project_id,
                    run.id,
                    now,
                    progress.steps,
                    progress.cost_usd,
                )
                .await?
                .ok_or_else(|| {
                    TelmoniError::Internal("ping: an open run went while it was heard".into())
                })?,
                // Its `/start` was lost on the way: the checkpoint opens the
                // run it reports on.
                None => {
                    let id = progress.rid.unwrap_or_else(Uuid::new_v4);
                    open_run(
                        key,
                        monitor,
                        id,
                        now,
                        progress.steps.unwrap_or(0),
                        progress.cost_usd.unwrap_or(0.0),
                        tx,
                    )
                    .await?
                }
            };
            let event = if over_bound(&run) {
                RunEvent::OverBound
            } else {
                RunEvent::Heard
            };
            Ok((run.id, event))
        }
        Report::Success | Report::Fail => {
            let status = if report == Report::Success {
                SpanStatus::Ok
            } else {
                SpanStatus::Error
            };
            let (run_id, started_at, steps, cost_usd) = match (open, progress.rid) {
                // The row's delete is the one record of the run's end: a
                // second end ping racing this one finds the row gone, files
                // no second run and leaves the monitor as the first left it.
                (Some(run), Some(_)) => match db::close_open_run(tx, project_id, run.id).await? {
                    Some(run) => ended(run),
                    None => return Ok((run.id, RunEvent::Gone)),
                },
                // The newest is the run this ping means; the rest never said
                // they ended.
                (Some(run), None) => {
                    let mut closed = db::close_open_runs(tx, project_id, monitor.id).await?;
                    closed.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(a.id.cmp(&b.id)));
                    if closed.is_empty() {
                        return Ok((run.id, RunEvent::Gone));
                    }
                    let newest = closed.remove(0);
                    abandon(key, monitor, closed, now, tx).await?;
                    ended(newest)
                }
                // No run open: one of no duration is recorded, under the
                // `rid` given, so a retried end ping whose first copy got
                // through files a second run of the same id — which the
                // outbox drops before it reaches `spans`.
                (None, rid) => (rid.unwrap_or_else(Uuid::new_v4), now, None, None),
            };
            let finished = FinishedRun {
                run_id,
                agent: monitor.agent.clone(),
                started_at,
                ended_at: now,
                received_at: now,
                status,
                exit_code: progress.exit_code.filter(|_| report == Report::Fail),
                steps,
                cost_usd,
            };
            outbox::enqueue_run(tx, project_id, &key.organization_id, &finished).await?;
            let event = if status == SpanStatus::Ok {
                RunEvent::EndedOk
            } else {
                RunEvent::EndedError
            };
            Ok((run_id, event))
        }
    }
}

/// What a run's row gives its finished span: its id, its start, and its
/// steps and spend where it reported any.
fn ended(run: OpenRunRow) -> (Uuid, DateTime<Utc>, Option<i32>, Option<f64>) {
    (
        run.id,
        run.started_at,
        (run.steps > 0).then_some(run.steps),
        (run.cost_usd > 0.0).then_some(run.cost_usd),
    )
}

/// Runs that never said they ended — a job that crashed after its `/start`,
/// a `/checkpoint` that came after its run's end — ended `error` with no
/// exit code, each handed to the outbox as a finished span that ends when
/// the run was last heard, oldest first; their rows are gone already.
/// Nothing else in the lane would end them: the ticker's scans, for
/// silence and for a run past its ceiling ([`ends_by`]), are the backstop.
async fn abandon(
    key: &ResolvedPingKey,
    monitor: &MonitorRow,
    mut runs: Vec<OpenRunRow>,
    now: DateTime<Utc>,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<(), TelmoniError> {
    runs.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
    for run in runs {
        let ended_at = run.last_heard_at;
        let (run_id, started_at, steps, cost_usd) = ended(run);
        let finished = FinishedRun {
            run_id,
            agent: monitor.agent.clone(),
            started_at,
            ended_at,
            received_at: now,
            status: SpanStatus::Error,
            exit_code: None,
            steps,
            cost_usd,
        };
        outbox::enqueue_run(tx, &key.project_id, &key.organization_id, &finished).await?;
        tracing::info!(
            project_id = %key.project_id,
            monitor_id = %monitor.id,
            run_id = %run_id,
            "ping: a run that never said it ended is ended as error by the ping after it"
        );
    }
    Ok(())
}

/// A run opened under `id`, held to its monitor's bounds — a `curl` run
/// declares none of its own — with what the ping reported so far.
async fn open_run(
    key: &ResolvedPingKey,
    monitor: &MonitorRow,
    id: Uuid,
    now: DateTime<Utc>,
    steps: i32,
    cost_usd: f64,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<OpenRunRow, TelmoniError> {
    db::insert_open_run(
        tx,
        &key.project_id,
        &key.organization_id,
        id,
        OpenRunWrite {
            monitor_id: monitor.id,
            agent: &monitor.agent,
            source: SOURCE,
            ping_key_id: Some(key.id),
            started_at: now,
            ends_by: ends_by(now, monitor.expected_duration_s),
            steps,
            cost_usd,
            stall_after_s: monitor.stall_after_s,
            max_steps: monitor.max_steps,
            max_cost_usd: monitor.max_cost_usd,
        },
    )
    .await
    .map_err(
        |error| match error.as_database_error().and_then(|db| db.constraint()) {
            Some(RUN_TAKEN) => AuthError::Conflict(
                "a run by that rid is open already, under another monitor".into(),
            )
            .into(),
            _ => error.into(),
        },
    )
}

/// Whether a run is past its step cap. Its spend is held to its cost cap
/// from 3.9, which prices a call; a `/checkpoint`'s cost is kept meanwhile.
fn over_bound(run: &OpenRunRow) -> bool {
    run.max_steps.is_some_and(|cap| run.steps > cap)
}

/// The monitor after its run was heard: its state from what the run did —
/// unless its state is its owner's to set, which one its first ping made,
/// `new`, and one paused keep, their runs still kept — and its next due
/// time found again once a run ended, so a run that came is not late.
async fn settle(
    project_id: &ProjectId,
    monitor: &MonitorRow,
    event: RunEvent,
    now: DateTime<Utc>,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<(), TelmoniError> {
    let state = state_after(monitor.state, event);
    let next_due_at = match (state, event) {
        (Some(_), RunEvent::EndedOk | RunEvent::EndedError) => Schedule::read(
            monitor.schedule.as_deref(),
            monitor.interval_s,
            &monitor.timezone,
        )
        .and_then(|schedule| schedule.next_due(now))
        .map_err(|fault| {
            TelmoniError::Internal(format!(
                "ping: a saved monitor's timing cannot be read: {}",
                fault.detail()
            ))
        })?,
        _ => monitor.next_due_at,
    };
    if state.is_none_or(|state| state == monitor.state) && next_due_at == monitor.next_due_at {
        return Ok(());
    }
    db::settle_monitor_run(tx, project_id, monitor.id, state, next_due_at).await?;
    Ok(())
}

/// The state a monitor is put in by what its run did, or `None` where its
/// state is its owner's to set, or where the run ended beside this ping and
/// the other decided it. Past a bound, a monitor stays past it while the run
/// goes on, however many checkpoints follow.
fn state_after(current: MonitorState, event: RunEvent) -> Option<MonitorState> {
    let state = match (current, event) {
        (MonitorState::New | MonitorState::Paused, _) | (_, RunEvent::Gone) => return None,
        (MonitorState::OverBound, RunEvent::Opened | RunEvent::Heard) => MonitorState::OverBound,
        (_, RunEvent::Opened | RunEvent::Heard) => MonitorState::Running,
        (_, RunEvent::OverBound) => MonitorState::OverBound,
        (_, RunEvent::EndedOk) => MonitorState::Healthy,
        (_, RunEvent::EndedError) => MonitorState::Failed,
    };
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run's ceiling is a day, or four times what its monitor expects
    /// where that is longer.
    #[test]
    fn a_runs_ceiling_is_a_day_or_four_times_what_is_expected() {
        let now = Utc::now();
        let day = chrono::Duration::hours(24);
        assert_eq!(ends_by(now, None), now + day);
        assert_eq!(ends_by(now, Some(60)), now + day);
        assert_eq!(ends_by(now, Some(6 * 60 * 60)), now + day);
        assert_eq!(
            ends_by(now, Some(8 * 60 * 60)),
            now + chrono::Duration::hours(32)
        );
    }

    fn query(pairs: &[(&str, &str)]) -> Query<PingQuery> {
        let mut query = PingQuery::default();
        for (name, value) in pairs {
            let slot = match *name {
                "rid" => &mut query.rid,
                "steps" => &mut query.steps,
                "cost" => &mut query.cost,
                "exit_code" => &mut query.exit_code,
                other => panic!("no such field: {other}"),
            };
            *slot = Some((*value).to_owned());
        }
        Query(query)
    }

    /// Each value is a number alone, trimmed; one that is not is a 400 that
    /// names its field and echoes none of it.
    #[test]
    fn a_pings_query_is_numbers_alone() {
        let read = Progress::read(Ok(query(&[
            ("rid", " 8f3c6d2a-1b4e-4c5d-9e7f-0a1b2c3d4e5f "),
            ("steps", "4"),
            ("cost", "0.08"),
            ("exit_code", "-1"),
        ])))
        .expect("a query of numbers");
        assert_eq!(
            read.rid.map(|rid| rid.to_string()).as_deref(),
            Some("8f3c6d2a-1b4e-4c5d-9e7f-0a1b2c3d4e5f")
        );
        assert_eq!(read.steps, Some(4));
        assert_eq!(read.cost_usd, Some(0.08));
        assert_eq!(read.exit_code, Some(-1));

        let empty = Progress::read(Ok(query(&[("steps", " ")]))).expect("nothing sent");
        assert_eq!(empty.steps, None);

        for (pairs, field) in [
            (vec![("rid", "run-1<script>")], "rid"),
            (vec![("steps", "four<script>")], "steps"),
            (vec![("steps", "-1")], "steps"),
            (vec![("steps", "1.5")], "steps"),
            (vec![("cost", "$0.08<script>")], "cost"),
            (vec![("cost", "-0.5")], "cost"),
            (vec![("cost", "NaN")], "cost"),
            (vec![("exit_code", "one<script>")], "exit_code"),
        ] {
            let refused = Progress::read(Ok(query(&pairs))).expect_err(field);
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400, "{field}");
            let detail = problem.detail.unwrap_or_default();
            assert!(detail.contains(field), "{field}: {detail}");
            assert!(!detail.contains("script"), "{field} was echoed: {detail}");
        }
    }

    /// A key is read for its shape before anything costs a hash.
    #[test]
    fn a_key_of_another_shape_opens_nothing() {
        let key = format!("{PING_KEY_PREFIX}{}", "a".repeat(PING_KEY_RANDOM_LEN));
        assert!(well_formed(&key));
        for other in [
            "",
            "telmoniping_",
            &format!("telmoni_{}", "a".repeat(PING_KEY_RANDOM_LEN)),
            &format!("{PING_KEY_PREFIX}{}", "a".repeat(PING_KEY_RANDOM_LEN - 1)),
            &format!("{PING_KEY_PREFIX}{}", "a".repeat(PING_KEY_RANDOM_LEN + 1)),
            &format!("{PING_KEY_PREFIX}{}/", "a".repeat(PING_KEY_RANDOM_LEN - 1)),
            &format!("{PING_KEY_PREFIX}{}é", "a".repeat(PING_KEY_RANDOM_LEN - 2)),
        ] {
            assert!(!well_formed(other), "{other:?}");
        }
        let refused = unknown_key().to_problem_details();
        assert_eq!(refused.status, 404, "never a 401");
    }

    /// A slug the console could not save is one a ping cannot name.
    #[test]
    fn a_slug_is_held_to_the_cores_rules() {
        assert_eq!(
            checked_slug("nightly-report").expect("a slug"),
            "nightly-report"
        );
        for refused in ["Nightly", "nightly report", "new", "schedule", "shit-2", ""] {
            let problem = checked_slug(refused)
                .expect_err(refused)
                .to_problem_details();
            assert_eq!(problem.status, 400, "{refused}");
            assert!(
                !problem.detail.unwrap_or_default().contains(refused) || refused.is_empty(),
                "{refused} was echoed"
            );
        }
    }

    /// A monitor's state follows its run, but for one its owner has yet to
    /// set up or has paused, whose runs are kept and whose state stands.
    #[test]
    fn a_monitors_state_follows_its_run() {
        use MonitorState::{Failed, Healthy, Late, New, OverBound, Paused, Running};
        use RunEvent::{EndedError, EndedOk, Heard, Opened};
        assert_eq!(state_after(Healthy, Opened), Some(Running));
        assert_eq!(state_after(Late, Opened), Some(Running));
        assert_eq!(state_after(Failed, Heard), Some(Running));
        assert_eq!(state_after(Running, RunEvent::OverBound), Some(OverBound));
        assert_eq!(
            state_after(OverBound, Heard),
            Some(OverBound),
            "past a bound stays past it while the run goes on"
        );
        assert_eq!(state_after(OverBound, EndedOk), Some(Healthy));
        assert_eq!(state_after(Running, EndedError), Some(Failed));
        assert_eq!(state_after(Failed, EndedOk), Some(Healthy));
        for current in [Healthy, Running, OverBound, Failed, Late] {
            assert_eq!(
                state_after(current, RunEvent::Gone),
                None,
                "{current:?}: the run ended beside this ping, and the other decided it"
            );
        }
        for owners in [New, Paused] {
            for event in [
                Opened,
                Heard,
                RunEvent::OverBound,
                EndedOk,
                EndedError,
                RunEvent::Gone,
            ] {
                assert_eq!(state_after(owners, event), None, "{owners:?} {event:?}");
            }
        }
    }
}
