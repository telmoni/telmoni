//! The runs an export reports on, kept open in Postgres while they go, in the
//! row a ping's run is kept in (`telemetry.open_runs`): opened by the run's
//! `telmoni.run.start`, or by its first span to land where none came; each
//! export's steps and tokens added, and its bounds checked, as it lands; and
//! ended by the run's own span, its totals left on its monitor, as a ping's
//! end leaves them. A run names its monitor by its agent, and an agent no
//! monitor has makes one, `new`, as a ping's unknown slug does. Settled once
//! the spans are kept and never in their way: a run that cannot be settled
//! is logged and its spans answered as kept, and the ticker's scans end what
//! nothing else would.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, SubsecRound as _, Utc};
use redis::AsyncCommands;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::ActingKey;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::{AuditAction, MonitorState, SpanKind, SpanStatus, TelmoniResourceKind, slug};
use telmoni_shared::{AuthError, ProjectId, TelmoniError};

use crate::AppState;
use crate::ceilings;
use crate::db::{self, LastRun, MonitorRow, MonitorWrite, OpenRunWrite, RunAdds};
use crate::handler::chain;
use crate::handler::monitors::{DEFAULT_TIMEZONE, NAME_MAX, RESERVED, refused_as_taken};
use crate::handler::pings::{
    self, DISCOVERED_STALL_AFTER_S, NEW_MONITORS_MAX, Recorded, RunEvent, ends_by, hearing,
};
use crate::ingest::batch::Batch;
use crate::notify;
use crate::outbox;

/// What an open run's `source` says of a run an SDK reports.
const SOURCE: &str = "ingest";

/// How long a run its own span ended is remembered, so spans of it that come
/// later — another service's, exported after the run's own — never open it
/// again, nor a retried export end it twice.
const ENDED_KEPT: Duration = Duration::from_secs(60 * 60);

/// How many ended runs this replica remembers while Redis does not answer.
const ENDED_KNOWN_MAX: usize = 10_000;

/// How long an export's runs are settled before the rest are left to their
/// next exports and the ticker: the exporter waits on it.
const SETTLE_BUDGET: Duration = Duration::from_secs(2);

/// The runs this replica ended lately, by project and id, while Redis does
/// not answer: when each was ended.
#[derive(Default)]
pub struct EndedLately(Mutex<HashMap<(ProjectId, Uuid), Instant>>);

fn ended_key(project_id: &ProjectId, run_id: Uuid) -> String {
    format!("telemetry:run-ended:{project_id}:{run_id}")
}

/// Whether the run's own span ended it within [`ENDED_KEPT`].
async fn ended_lately(state: &AppState, project_id: &ProjectId, run_id: Uuid) -> bool {
    if let Some(mut connection) = state.redis.connection().await {
        let found: Result<bool, redis::RedisError> =
            connection.exists(ended_key(project_id, run_id)).await;
        match found {
            Ok(found) => return found,
            Err(error) => state.redis.stood_in("a run's end", &error),
        }
    }
    state
        .ended_runs
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&(project_id.clone(), run_id))
        .is_some_and(|at| at.elapsed() < ENDED_KEPT)
}

/// The run remembered as ended, once its end has committed.
async fn remember_ended(state: &AppState, project_id: &ProjectId, run_id: Uuid) {
    if let Some(mut connection) = state.redis.connection().await {
        let kept: Result<(), redis::RedisError> = connection
            .set_ex(ended_key(project_id, run_id), 1, ENDED_KEPT.as_secs())
            .await;
        match kept {
            Ok(()) => return,
            Err(error) => state.redis.stood_in("a run's end", &error),
        }
    }
    let mut ended = state
        .ended_runs
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if ended.len() >= ENDED_KNOWN_MAX {
        ended.retain(|_, at| at.elapsed() < ENDED_KEPT);
    }
    if ended.len() < ENDED_KNOWN_MAX {
        ended.insert((project_id.clone(), run_id), Instant::now());
    }
}

/// A run as one export reports it.
#[derive(Debug, Clone, PartialEq)]
struct Reported {
    agent: String,
    /// The earliest any of its spans in the export began, its start among them.
    first_heard: DateTime<Utc>,
    /// Its own span, where the export carries it: the run ended.
    ended: Option<Ended>,
    /// Its steps and tokens in this export, and the bounds it declares.
    adds: RunAdds,
}

/// How a run's own span says it ended.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ended {
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    status: SpanStatus,
}

/// The tighter of two bounds, either side's where the other set none.
fn tighter<T: PartialOrd + Copy>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b < a { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// A bound as the open run holds it: a whole number a column takes.
fn held(bound: Option<u32>) -> Option<i32> {
    bound.map(|n| i32::try_from(n).unwrap_or(i32::MAX))
}

/// Every run the export reports on, by its id: what it adds, and whether it
/// ended. A step is a span our SDKs opened that is no call; tokens are
/// summed off every span but the run's own, which may carry the run's sum.
fn reported(batch: &Batch) -> BTreeMap<Uuid, Reported> {
    let mut runs: BTreeMap<Uuid, Reported> = BTreeMap::new();
    for start in &batch.starts {
        let run = runs.entry(start.run_id).or_insert_with(|| Reported {
            agent: start.agent.clone(),
            first_heard: start.started_at,
            ended: None,
            adds: RunAdds::default(),
        });
        run.first_heard = run.first_heard.min(start.started_at);
        run.adds.stall_after_s = tighter(run.adds.stall_after_s, held(start.bound_stall_after_s));
        run.adds.max_steps = tighter(run.adds.max_steps, held(start.bound_max_steps));
        run.adds.max_cost_usd = tighter(run.adds.max_cost_usd, start.bound_max_cost_usd);
    }
    for row in &batch.rows {
        let run = runs.entry(row.run_id).or_insert_with(|| Reported {
            agent: row.agent.clone(),
            first_heard: row.started_at,
            ended: None,
            adds: RunAdds::default(),
        });
        run.first_heard = run.first_heard.min(row.started_at);
        match row.kind {
            SpanKind::Run => {
                run.ended = Some(Ended {
                    started_at: row.started_at,
                    ended_at: row.ended_at,
                    status: row.status,
                });
                run.adds.stall_after_s =
                    tighter(run.adds.stall_after_s, held(row.bound_stall_after_s));
                run.adds.max_steps = tighter(run.adds.max_steps, held(row.bound_max_steps));
                run.adds.max_cost_usd = tighter(run.adds.max_cost_usd, row.bound_max_cost_usd);
                continue;
            }
            SpanKind::Step => run.adds.steps = run.adds.steps.saturating_add(1),
            SpanKind::ModelCall | SpanKind::ToolCall | SpanKind::Span => {}
        }
        let tokens = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        run.adds.input_tokens = run
            .adds
            .input_tokens
            .saturating_add(tokens(row.input_tokens));
        run.adds.output_tokens = run
            .adds
            .output_tokens
            .saturating_add(tokens(row.output_tokens));
    }
    runs
}

/// What settling an export's runs came to: a line for each run no monitor
/// watches, for the export's answer, and whether a monitor moved.
#[derive(Debug, Default)]
pub struct Settled {
    pub notes: Vec<String>,
    pub moved: bool,
}

/// The runs in the order their monitors should hear of them: those that
/// ended, by when, then those still going, so a monitor ends as its newest
/// run stands.
fn in_order(runs: BTreeMap<Uuid, Reported>) -> Vec<(Uuid, Reported)> {
    let mut runs: Vec<(Uuid, Reported)> = runs.into_iter().collect();
    runs.sort_by_key(|(run_id, run)| match run.ended {
        Some(ended) => (false, ended.ended_at, *run_id),
        None => (true, run.first_heard, *run_id),
    });
    runs
}

/// Every run `batch` reports on, settled, each in a transaction of its own
/// so one that fails leaves the others settled; a failure is logged. An
/// agent no monitor can watch is tried once an export, and what is left past
/// `SETTLE_BUDGET`, or once the database stops answering, is left.
pub async fn settle(state: &AppState, key: &ActingKey, batch: &Batch) -> Settled {
    let mut settled = Settled::default();
    let now = Utc::now().trunc_subsecs(6);
    let began = Instant::now();
    let runs = in_order(reported(batch));
    let total = runs.len();
    let mut untracked: HashSet<String> = HashSet::new();
    for (done, (run_id, run)) in runs.into_iter().enumerate() {
        if began.elapsed() >= SETTLE_BUDGET {
            tracing::warn!(
                project_id = %key.project_id,
                left = total - done,
                "ingest: an export's runs outlasted the time they are settled in; the rest wait \
                 for their next exports, and the ticker"
            );
            break;
        }
        if untracked.contains(&run.agent) {
            continue;
        }
        match settle_one(state, key, run_id, &run, now).await {
            Ok(Outcome::Settled) => settled.moved = true,
            Ok(Outcome::Untracked { note, counted }) => {
                if let Some(slug) = &counted {
                    ceilings::refused(
                        state,
                        &key.organization_id,
                        ceilings::Lane::Monitors,
                        Some(slug),
                    )
                    .await;
                }
                settled.notes.push(note);
                untracked.insert(run.agent);
            }
            Ok(Outcome::Skipped) => {}
            Err(error) if error.is_database_unanswered() => {
                tracing::warn!(
                    project_id = %key.project_id,
                    left = total - done,
                    error = %error,
                    "ingest: the database stopped answering while an export's runs were \
                     settled; the rest wait for their next exports, and the ticker"
                );
                break;
            }
            Err(error) => tracing::warn!(
                project_id = %key.project_id,
                run_id = %run_id,
                error = %error,
                "ingest: a run's spans were kept but its open run was not settled; the ticker \
                 ends what nothing else does"
            ),
        }
    }
    settled
}

/// What one run's settling came to.
enum Outcome {
    Settled,
    /// No monitor watches it, and the export is told why; `counted` is the
    /// slug refused for the plan's monitors, counted once the lock is let go.
    Untracked {
        note: String,
        counted: Option<String>,
    },
    /// Nothing to do: its project moved or is moving, or its id is another
    /// monitor's.
    Skipped,
}

async fn settle_one(
    state: &AppState,
    key: &ActingKey,
    run_id: Uuid,
    run: &Reported,
    now: DateTime<Utc>,
) -> Result<Outcome, TelmoniError> {
    let (project_id, organization_id) = (&key.project_id, &key.organization_id);
    if ended_lately(state, project_id, run_id).await {
        return Ok(Outcome::Skipped);
    }
    let mut tx = project_scope(&state.db, project_id).await?;
    // A project being moved or purged, or moved since the export's key was
    // resolved, is left: its runs settle with its next export, or by the
    // ticker, never filed under the organization it left.
    let stands = project_move::try_hold_in_place(tx.conn(), project_id).await?
        && db::settings_organization(&mut tx, project_id)
            .await?
            .as_ref()
            == Some(organization_id);
    if !stands {
        chain::rolled_back(tx.rollback().await, project_id, "run");
        tracing::debug!(
            project_id = %project_id,
            run_id = %run_id,
            "ingest: a run's project is moving, or moved since its key was resolved; its next \
             export settles it"
        );
        return Ok(Outcome::Skipped);
    }
    let mut tx = tx.bind_organization(organization_id).await?;
    let outcome = match record(state, key, run_id, run, now, &mut tx).await {
        Ok(outcome) => outcome,
        Err(error) => {
            chain::rolled_back(tx.rollback().await, project_id, "run");
            return Err(error);
        }
    };
    tx.commit().await?;
    if run.ended.is_some() && matches!(outcome, Outcome::Settled) {
        remember_ended(state, project_id, run_id).await;
    }
    Ok(outcome)
}

/// The run recorded on its monitor's open run, and the monitor settled.
async fn record(
    state: &AppState,
    key: &ActingKey,
    run_id: Uuid,
    run: &Reported,
    now: DateTime<Utc>,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<Outcome, TelmoniError> {
    let (project_id, organization_id) = (&key.project_id, &key.organization_id);
    let monitor = match db::hold_monitor_by_agent(tx, project_id, &run.agent).await? {
        Some(monitor) => monitor,
        None => match discover(state, key, &run.agent, tx).await? {
            Discovered::Made(monitor) => *monitor,
            Discovered::Refused { note, counted } => {
                return Ok(Outcome::Untracked { note, counted });
            }
        },
    };
    let open = db::open_run(tx, project_id, run_id).await?;
    // Its monitor's last run, ended: what comes of it now is late, or sent again.
    if open.is_none() && monitor.last_run_id == Some(run_id) {
        return Ok(Outcome::Skipped);
    }
    if open
        .as_ref()
        .is_some_and(|open| open.monitor_id != monitor.id)
    {
        tracing::debug!(
            project_id = %project_id,
            run_id = %run_id,
            "ingest: a run's id is open under another monitor; it is left be"
        );
        return Ok(Outcome::Skipped);
    }
    let recorded = match run.ended {
        None => hear(&monitor, run_id, run, open.is_some(), now, key, tx).await?,
        Some(ended) => end(project_id, run_id, run, ended, open.is_some(), tx).await?,
    };
    let exit_code = None;
    pings::settle(
        (project_id, organization_id),
        &monitor,
        &recorded,
        exit_code,
        now,
        tx,
    )
    .await?;
    Ok(Outcome::Settled)
}

/// A run still going: heard where it is open, opened where it is not, its
/// export's steps and tokens added either way and its bounds checked.
async fn hear(
    monitor: &MonitorRow,
    run_id: Uuid,
    run: &Reported,
    is_open: bool,
    now: DateTime<Utc>,
    key: &ActingKey,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<Recorded, TelmoniError> {
    let project_id = &key.project_id;
    if !is_open {
        let opened = db::insert_open_run(
            tx,
            project_id,
            &key.organization_id,
            run_id,
            OpenRunWrite {
                monitor_id: monitor.id,
                agent: &monitor.agent,
                source: SOURCE,
                ping_key_id: None,
                started_at: run.first_heard.min(now),
                // Heard now, its ceiling from now: a run reported late is not
                // quiet, nor ended, for the time it went unheard.
                heard_at: now,
                ends_by: ends_by(now, monitor.expected_duration_s),
                steps: run.adds.steps,
                input_tokens: run.adds.input_tokens,
                output_tokens: run.adds.output_tokens,
                cost_usd: 0.0,
                stall_after_s: tighter(monitor.stall_after_s, run.adds.stall_after_s),
                max_steps: tighter(monitor.max_steps, run.adds.max_steps),
                max_cost_usd: tighter(monitor.max_cost_usd, run.adds.max_cost_usd),
            },
        )
        .await?;
        if let Some(opened) = opened {
            let event = match hearing(&opened) {
                RunEvent::OverBound => RunEvent::OverBound,
                _ => RunEvent::Opened,
            };
            return Ok(Recorded {
                run_id,
                event,
                revived: false,
                ended: None,
            });
        }
        // Opened beside this export by another of the run's: heard below.
    }
    let heard = db::add_to_open_run(tx, project_id, run_id, monitor.id, run.adds).await?;
    Ok(Recorded {
        run_id,
        event: heard
            .as_ref()
            .map_or(RunEvent::Gone, |heard| hearing(&heard.run)),
        revived: heard.as_ref().is_some_and(|heard| heard.was_quiet),
        ended: None,
    })
}

/// A run its own span ended: its open run closed where it had one, and its
/// totals — what the row held and what this export adds — left on its
/// monitor; one recorded after it ended, or in one export, from that export
/// alone.
async fn end(
    project_id: &ProjectId,
    run_id: Uuid,
    run: &Reported,
    ended: Ended,
    is_open: bool,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<Recorded, TelmoniError> {
    let (mut steps, mut started_at, mut revived) = (run.adds.steps, ended.started_at, false);
    if is_open {
        match db::close_open_run(tx, project_id, run_id).await? {
            Some(closed) => {
                steps = steps.saturating_add(closed.run.steps);
                started_at = started_at.min(closed.run.started_at);
                revived = closed.was_quiet;
            }
            // Ended beside this export, by another of the run's: that one
            // decided the run and the monitor.
            None => {
                return Ok(Recorded {
                    run_id,
                    event: RunEvent::Gone,
                    revived: false,
                    ended: None,
                });
            }
        }
    }
    let last = LastRun {
        run_id,
        started_at,
        ended_at: ended.ended_at.max(started_at),
        status: ended.status,
        steps: (steps > 0).then_some(steps),
        cost_usd: None,
    };
    let event = if ended.status == SpanStatus::Error {
        RunEvent::EndedError
    } else {
        RunEvent::EndedOk
    };
    Ok(Recorded {
        run_id,
        event,
        revived,
        ended: Some(last),
    })
}

/// What an agent no monitor has came to.
enum Discovered {
    Made(Box<MonitorRow>),
    /// Nothing was made, and the export is told why; `counted` is the slug
    /// the plan's monitors refused.
    Refused {
        note: String,
        counted: Option<String>,
    },
}

impl Discovered {
    fn refused(note: String) -> Self {
        Self::Refused {
            note,
            counted: None,
        }
    }
}

/// A monitor made by a run naming an agent the project has none for, as a
/// ping's unknown slug makes one ([`pings`]): `new`, its slug made from the
/// agent, under the organization's chain lock and held to the unset monitors
/// a project may hold and to the plan's ceiling, recorded on the chain as
/// made by the key, and the owner told once in the feed. Past either bound,
/// for an agent the console could not save as one, or a slug another agent's
/// monitor holds, nothing is made and the run's spans are kept, their export
/// told why.
async fn discover(
    state: &AppState,
    key: &ActingKey,
    agent: &str,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<Discovered, TelmoniError> {
    let (project_id, organization_id) = (&key.project_id, &key.organization_id);
    // The console's own rules for a monitor's agent and slug, so a monitor
    // made here is one its owner can save.
    let savable = agent.trim() == agent && !agent.chars().any(char::is_control);
    let Some(slug) = slug::slugify(agent)
        .filter(|slug| savable && !RESERVED.contains(&slug.as_str()) && !slug::offends(slug))
    else {
        return Ok(Discovered::refused(format!(
            "run of agent {agent:?}: no monitor watches it, and none can be made for it"
        )));
    };
    let ceiling = chain::ceiling(state.entitlements.as_ref(), organization_id).await;
    chain::lock(project_id, organization_id, tx).await?;
    // Another export may have made it while this one waited for the lock.
    if let Some(made) = db::hold_monitor_by_agent(tx, project_id, agent).await? {
        return Ok(Discovered::Made(Box::new(made)));
    }
    if let Some(holder) = db::hold_monitor(tx, project_id, &slug).await? {
        return Ok(Discovered::refused(format!(
            "run of agent {agent:?}: no monitor watches it; the monitor {slug} watches agent \
             {:?}, so set its agent to this one to watch these runs",
            holder.agent
        )));
    }
    db::ensure_settings(tx, project_id, organization_id).await?;
    if db::new_monitor_count(tx, project_id).await? >= NEW_MONITORS_MAX {
        return Ok(Discovered::refused(format!(
            "run of agent {agent:?}: no monitor watches it, since this project holds \
             {NEW_MONITORS_MAX} monitors nobody has set up; set them up or delete them"
        )));
    }
    if let Some(ceiling) = ceiling {
        let held = db::monitor_count(tx, organization_id).await?;
        if ceiling
            .admit(u64::try_from(held).unwrap_or(u64::MAX))
            .is_err()
        {
            return Ok(Discovered::Refused {
                note: format!(
                    "run of agent {agent:?}: no monitor watches it. {}",
                    ceiling.detail
                ),
                counted: Some(slug),
            });
        }
    }
    let name: String = agent.chars().take(NAME_MAX).collect();
    let made = db::insert_monitor(
        tx,
        project_id,
        organization_id,
        Uuid::new_v4(),
        MonitorState::New,
        MonitorWrite {
            slug: &slug,
            name: &name,
            agent,
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
    .await;
    let monitor = match made {
        Ok(monitor) => monitor,
        Err(error) => {
            return match refused_as_taken(
                error,
                "a monitor by that slug was made just now, for another agent",
                "a monitor of that agent was made just now",
            ) {
                TelmoniError::Auth(AuthError::Conflict(words)) => Ok(Discovered::refused(format!(
                    "run of agent {agent:?}: no monitor watches it: {words}"
                ))),
                other => Err(other),
            };
        }
    };
    emit_audit(
        tx,
        AuditEvent {
            organization_id,
            in_project: Some(project_id),
            actor: Actor::External("api-key"),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Monitor,
            resource_id: Some(&monitor.id.to_string()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "slug": monitor.slug,
                "agent": monitor.agent,
                "token_id": key.token_id,
            })),
        },
    )
    .await?;
    outbox::enqueue_notice(
        tx,
        project_id,
        organization_id,
        &notify::monitor_discovered_by_run(&monitor),
    )
    .await?;
    tracing::info!(
        project_id = %project_id,
        monitor_id = %monitor.id,
        "ingest: a monitor made by its agent's first run, as new"
    );
    Ok(Discovered::Made(Box::new(monitor)))
}
