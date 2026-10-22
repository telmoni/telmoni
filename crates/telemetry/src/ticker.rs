//! The ticker: what no ping can say on its own — a run expected that never
//! came, one gone quiet, one never ended — found by a loop every replica
//! runs every few seconds (`TELEMETRY_TICK_SECS`), no leader. Each tick reads
//! across every tenant the monitors with something to do
//! ([`db::monitors_with_work`]) and visits each in a transaction of its own,
//! deciding every rule again under the locks its write takes: a monitor due
//! is `late` while its grace runs and `failed` once it has run out, its next
//! due time found again; a run quiet past its stall timeout makes its
//! monitor `stalled`; a run past its ceiling (`ends_by`) is ended as `error`
//! and its monitor `failed`. A `new` or paused monitor keeps its state, its
//! runs still ended at their ceiling and the last of them kept on it; one
//! with a run going is never late. The ping lane recovers what the ticker
//! marks.
//!
//! The ticker waits on nobody: a project, monitor or run another writer
//! holds is left to the next tick (`SKIP LOCKED`), so it stands in no circle
//! and holds nothing a ping waits on longer than one monitor's visit.
//!
//! ⚠ The loop's handle is raced against the server in the binary's `serve`,
//! as the outbox's is: a dead ticker would leave a replica on which nothing
//! is ever late. Its heartbeat to the watchdog comes with 2.10.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{
    ProjectAndOrganization, Scoped, maintenance_scope, project_scope,
};
use telmoni_shared::{MonitorState, OrganizationId, ProjectId, SpanStatus, TelmoniError};

use crate::AppState;
use crate::db::{self, LastRun, MonitorRow, TelemetryLane};
use crate::handler::chain;
use crate::handler::monitors::due_after;
use crate::incidents::{self, Trigger};
use crate::live;
use crate::outbox::{self, FinishedRun};

/// How many monitors one tick visits at most; the rest wait for the next,
/// seconds on, and a monitor visited leaves the read.
const PAGE: i64 = 500;
/// How many runs past their ceiling one visit ends at most, so a visit, and
/// a ping or a delete waiting on it, is bounded; the read finds the monitor
/// again at the next tick for the rest.
const RUNS_PER_VISIT: i64 = 100;

/// What one tick did: the monitors it moved, the runs it ended, and the
/// monitors it left for a later tick.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Ticked {
    pub moved: u32,
    pub ended: u32,
    pub left: u32,
}

/// The ticker, forever: the first tick a poll after boot, then one every
/// poll; a tick that overruns delays the next rather than stacking.
pub async fn run(state: Arc<AppState>) {
    let poll = Duration::from_secs(state.config.tick_secs.max(1));
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + poll, poll);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let tick = tokio::spawn({
            let state = state.clone();
            async move { tick_once(&state).await }
        });
        match tick.await {
            Ok(Ok(Ticked {
                moved: 0,
                ended: 0,
                left: 0,
            })) => {}
            Ok(Ok(Ticked { moved, ended, left })) => {
                tracing::info!(moved, ended, left, "telemetry: the ticker's tick");
            }
            Ok(Err(e)) => tracing::warn!(
                error = %e,
                "telemetry: the ticker's tick failed; the next one retries"
            ),
            Err(e) => tracing::error!(
                error = %e,
                "telemetry: the ticker's tick died; the next one retries"
            ),
        }
    }
}

/// One tick: the monitors with something to do read ([`found`]), then each
/// visited ([`visit_each`]).
pub async fn tick_once(state: &Arc<AppState>) -> Result<Ticked, TelmoniError> {
    let found = found(state).await?;
    visit_each(state, &found).await
}

/// The monitors a tick has something to do for, as the rows stand now,
/// across every tenant: a page, by project and id.
pub async fn found(state: &AppState) -> Result<Vec<(ProjectId, Uuid)>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let found = db::monitors_with_work(&mut tx, PAGE).await?;
    tx.commit().await?;
    Ok(found)
}

/// The tick's work on the monitors it read, each visited on its own
/// (`visit`), then the organization's open consoles told of each project
/// whose monitors or runs it changed, once. A monitor that cannot be
/// visited now is left for a later tick; a database that stops answering
/// ends the tick.
pub async fn visit_each(
    state: &Arc<AppState>,
    found: &[(ProjectId, Uuid)],
) -> Result<Ticked, TelmoniError> {
    let mut ticked = Ticked::default();
    let mut changed: HashMap<ProjectId, OrganizationId> = HashMap::new();
    for (project_id, id) in found {
        match visit(state, project_id, *id).await {
            Ok(Visited::Changed {
                organization_id,
                moved,
                ended,
            }) => {
                ticked.moved += u32::from(moved);
                ticked.ended += ended;
                changed.insert(project_id.clone(), organization_id);
            }
            Ok(Visited::Nothing) => {}
            Ok(Visited::Left) => ticked.left += 1,
            Err(e) if e.is_database_unanswered() => return Err(e),
            Err(e) => {
                tracing::warn!(
                    project_id = %project_id,
                    monitor_id = %id,
                    error = %e,
                    "telemetry: the ticker could not visit a monitor; the next tick tries again"
                );
                ticked.left += 1;
            }
        }
    }
    for (project_id, organization_id) in &changed {
        live::monitors_changed(
            Arc::clone(state),
            organization_id.as_str(),
            project_id.as_str(),
        );
    }
    Ok(ticked)
}

/// What became of one monitor the tick read.
#[derive(Debug, PartialEq, Eq)]
enum Visited {
    /// Its state or its due time moved, a run of its was ended, or a
    /// flapping incident of its was let go of: the project's open consoles
    /// are told.
    Changed {
        organization_id: OrganizationId,
        moved: bool,
        ended: u32,
    },
    /// Nothing to do, as the rows stand under the lock: within its grace, or
    /// its runs heard or ended beside the tick, or its owner's to move, or
    /// gone with its delete or its purge.
    Nothing,
    /// Left for a later tick: its project held by a move or a purge, or the
    /// monitor held by another writer.
    Left,
}

/// One monitor visited in a transaction of its own, its project held in
/// place and the transaction bound to the organization its rows stand under
/// now, read there, never the one the tick's read named.
async fn visit(
    state: &AppState,
    project_id: &ProjectId,
    id: Uuid,
) -> Result<Visited, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    if !project_move::try_hold_in_place(tx.conn(), project_id).await? {
        chain::rolled_back(tx.rollback().await, project_id, "ticker");
        tracing::debug!(
            project_id = %project_id,
            monitor_id = %id,
            "telemetry: the ticker left a monitor whose project is moving to another \
             organization or being purged; a later tick finds it where it went, or gone"
        );
        return Ok(Visited::Left);
    }
    let Some(organization_id) = db::settings_organization(&mut tx, project_id).await? else {
        // Its purge took the settings row last, so the monitor went before
        // it: nothing to retry.
        chain::rolled_back(tx.rollback().await, project_id, "ticker");
        return Ok(Visited::Nothing);
    };
    let mut tx = tx.bind_organization(&organization_id).await?;
    // The database's clock, which read the monitor as due: on the app's, a
    // clock behind it would read the same row as not yet due, tick after
    // tick, for as long as the two differ.
    let now = db::clock(&mut tx).await?;
    match visit_held(project_id, &organization_id, id, now, &mut tx).await {
        Ok(changed @ Visited::Changed { .. }) => {
            tx.commit().await?;
            Ok(changed)
        }
        Ok(left) => {
            chain::rolled_back(tx.rollback().await, project_id, "ticker");
            Ok(left)
        }
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, project_id, "ticker");
            Err(refused)
        }
    }
}

/// The visit itself, inside the transaction [`visit`] opened: the monitor
/// held before any of its runs ([`db::hold_monitor_by_id`]), its runs past
/// their ceiling ended, then the monitor locked as the write holds it
/// ([`db::monitor_to_tick`]), the last of the runs ended noted on it, and
/// what the tick finds decided from that row — a monitor another writer
/// holds now left, the runs' end rolling back with it, for the next tick.
async fn visit_held(
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    id: Uuid,
    now: DateTime<Utc>,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<Visited, TelmoniError> {
    let Some(read) = db::hold_monitor_by_id(tx, project_id, id).await? else {
        // Gone with its delete or its purge, which is nothing to retry; or
        // held by one still under way, left to the next tick.
        return Ok(if db::monitor_by_id(tx, project_id, id).await?.is_some() {
            tracing::debug!(
                project_id = %project_id,
                monitor_id = %id,
                "telemetry: the ticker left a monitor its delete or a purge holds"
            );
            Visited::Left
        } else {
            Visited::Nothing
        });
    };
    let (ended, last) = end_runs_past_ceiling(project_id, organization_id, &read, now, tx).await?;
    let Some(held) = db::monitor_to_tick(tx, project_id, id).await? else {
        tracing::debug!(
            project_id = %project_id,
            monitor_id = %id,
            "telemetry: the ticker left a monitor a ping is settling or the console is changing"
        );
        return Ok(Visited::Left);
    };
    if let Some(last) = &last {
        db::note_last_run(tx, project_id, id, last).await?;
    }
    let stalled = db::has_stalled_run(tx, project_id, id).await?;
    let due = Due::of(&held, now);
    let mut row = held;
    let mut moved = false;
    // Nothing found, or its owner's to move: left as it is, its runs ended
    // all the same. Found again where a run ended or the grace ran out,
    // whatever else was found, so a monitor past its grace leaves the read
    // however it stands.
    if let Some(finding) = Finding::of(ended > 0, stalled, due)
        && let Some(state) = state_after_tick(row.state, finding)
    {
        let next_due_at = if ended > 0 || due == Due::Past {
            due_after(project_id, &row, now)
        } else {
            row.next_due_at
        };
        if state != row.state || next_due_at != row.next_due_at {
            let Some(settled) =
                db::settle_monitor_run(tx, project_id, id, state, next_due_at).await?
            else {
                return Ok(Visited::Nothing);
            };
            if settled.state == row.state {
                tracing::debug!(
                    project_id = %project_id,
                    monitor_id = %id,
                    "telemetry: the ticker found a monitor's next due time again"
                );
            } else {
                tracing::info!(
                    project_id = %project_id,
                    monitor_id = %id,
                    from = %row.state,
                    to = %settled.state,
                    finding = ?finding,
                    "telemetry: the ticker moved a monitor"
                );
                incidents::on_change(
                    tx,
                    project_id,
                    organization_id,
                    &settled,
                    row.state,
                    Some(finding.trigger()),
                    now,
                )
                .await?;
            }
            row = settled;
            moved = true;
        }
    }
    // A flapping incident let go of once its monitor has held one state for
    // the window, whatever else the visit did.
    let steadied = incidents::steady(tx, project_id, organization_id, &row, now).await?
        != incidents::Change::Nothing;
    if moved || ended > 0 || steadied {
        Ok(Visited::Changed {
            organization_id: organization_id.clone(),
            moved,
            ended,
        })
    } else {
        Ok(Visited::Nothing)
    }
}

/// The monitor's runs past their ceiling, each ended as `error` with no exit
/// code and handed to the outbox as a finished span that ends when the run
/// was last heard, as a ping ends a run that never said it ended; one a
/// ping holds now is left to it. Answers how many were ended, and the one
/// that ended last, for the monitor to keep.
async fn end_runs_past_ceiling(
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    monitor: &MonitorRow,
    now: DateTime<Utc>,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<(u32, Option<LastRun>), TelmoniError> {
    let mut ended = 0;
    let mut last: Option<LastRun> = None;
    for past in db::runs_past_ceiling(tx, project_id, monitor.id, RUNS_PER_VISIT).await? {
        let Some(closed) = db::close_open_run(tx, project_id, past.id).await? else {
            continue;
        };
        let run = closed.run;
        let finished = FinishedRun {
            run_id: run.id,
            agent: monitor.agent.clone(),
            started_at: run.started_at,
            // Heard by the database's clock, started by the server's: never
            // ended before it started.
            ended_at: run.last_heard_at.max(run.started_at),
            received_at: now,
            status: SpanStatus::Error,
            exit_code: None,
            steps: (run.steps > 0).then_some(run.steps),
            cost_usd: (run.cost_usd > 0.0).then_some(run.cost_usd),
        };
        outbox::enqueue_run(tx, project_id, organization_id, &finished).await?;
        tracing::info!(
            project_id = %project_id,
            monitor_id = %monitor.id,
            run_id = %run.id,
            "telemetry: a run past its ceiling is ended as error by the ticker, since nothing \
             else would end it"
        );
        ended += 1;
        let this = finished.last_run();
        last = Some(last.map_or(this, |last| last.later(this)));
    }
    Ok((ended, last))
}

/// Whether a monitor is due by a tick's clock, and how far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    /// Not yet, or never: it expects no run, or is due at no time.
    Not,
    /// Due, its grace still running.
    Within,
    /// Due, and its grace has run out.
    Past,
}

impl Due {
    /// Where `monitor` stands against its due time at `now`, as its row
    /// stands; no grace reads as none.
    fn of(monitor: &MonitorRow, now: DateTime<Utc>) -> Self {
        let Some(due) = monitor.next_due_at else {
            return Self::Not;
        };
        if due >= now {
            return Self::Not;
        }
        let grace = chrono::Duration::seconds(i64::from(monitor.grace_s.unwrap_or(0)));
        if due + grace < now {
            Self::Past
        } else {
            Self::Within
        }
    }
}

/// What a tick finds of a monitor, the gravest first: one finding decides
/// the state, while the due time is found again on its own, where a run
/// ended or the grace ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finding {
    /// A run of its past its ceiling was ended as `error`.
    Ended,
    /// A run of its is quiet past its stall timeout.
    Stalled,
    /// Due, and its grace has run out.
    Overdue,
    /// Due, its grace still running.
    Due,
}

impl Finding {
    /// What it says moved the monitor, for the incident its state opens.
    const fn trigger(self) -> Trigger {
        match self {
            Self::Ended => Trigger::RunNeverEnded,
            Self::Stalled => Trigger::RunStalled,
            Self::Overdue => Trigger::RunNeverCame,
            Self::Due => Trigger::RunLate,
        }
    }

    /// The gravest of what the tick found: whether a run was ended, whether
    /// one is quiet, and whether the monitor is due. `None` where nothing was.
    fn of(ended: bool, stalled: bool, due: Due) -> Option<Self> {
        if ended {
            return Some(Self::Ended);
        }
        if stalled {
            return Some(Self::Stalled);
        }
        match due {
            Due::Not => None,
            Due::Within => Some(Self::Due),
            Due::Past => Some(Self::Overdue),
        }
    }
}

/// The state a tick's finding puts a monitor in — the one it is in where
/// the finding moves it nowhere — or `None` where its state is its owner's
/// to set, `new` or paused, and nothing of the row is written. A run ended
/// at its ceiling fails its monitor, as a `/fail` does; a quiet run stalls
/// it, but for one failed already, the graver in the states' order, which a
/// `/fail` beside the quiet run would otherwise leave to be stalled a tick
/// later; a monitor with a run going is never late or failed for a due
/// time, since the run's own bounds hold it; past its grace a monitor
/// expecting a run has failed, and within it a healthy one is late, while
/// one late or failed already stays so.
fn state_after_tick(current: MonitorState, finding: Finding) -> Option<MonitorState> {
    let state = match (current, finding) {
        (MonitorState::New | MonitorState::Paused, _) => return None,
        (_, Finding::Ended) => MonitorState::Failed,
        (MonitorState::Failed, Finding::Stalled) => current,
        (_, Finding::Stalled) => MonitorState::Stalled,
        (
            MonitorState::Running | MonitorState::OverBound | MonitorState::Stalled,
            Finding::Overdue | Finding::Due,
        ) => current,
        (_, Finding::Overdue) => MonitorState::Failed,
        (MonitorState::Healthy, Finding::Due) => MonitorState::Late,
        (MonitorState::Late | MonitorState::Failed, Finding::Due) => current,
    };
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(
        state: MonitorState,
        due: Option<DateTime<Utc>>,
        grace_s: Option<i32>,
    ) -> MonitorRow {
        let now = Utc::now();
        MonitorRow {
            id: Uuid::new_v4(),
            slug: "nightly-report".into(),
            name: "Nightly report".into(),
            agent: "nightly-report".into(),
            schedule: None,
            interval_s: Some(3600),
            timezone: "UTC".into(),
            grace_s,
            expected_duration_s: None,
            stall_after_s: None,
            max_steps: None,
            max_cost_usd: None,
            state,
            state_changed_at: now,
            next_due_at: due,
            last_run_id: None,
            last_run_started_at: None,
            last_run_ended_at: None,
            last_run_status: None,
            last_run_steps: None,
            last_run_cost_usd: None,
            last_success_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// A monitor is due only once its time has passed, within its grace or
    /// past it, and the gravest finding decides the state.
    #[test]
    fn the_gravest_finding_decides() {
        use MonitorState::Healthy;
        let now = Utc::now();
        let minute = chrono::Duration::minutes(1);
        assert_eq!(
            Due::of(&monitor(Healthy, Some(now - minute * 2), Some(60)), now),
            Due::Past
        );
        assert_eq!(
            Due::of(&monitor(Healthy, Some(now - minute), Some(120)), now),
            Due::Within
        );
        assert_eq!(
            Due::of(&monitor(Healthy, Some(now + minute), Some(60)), now),
            Due::Not
        );
        assert_eq!(Due::of(&monitor(Healthy, None, None), now), Due::Not);
        // No grace reads as none: due past its time is past its grace.
        assert_eq!(
            Due::of(&monitor(Healthy, Some(now - minute), None), now),
            Due::Past
        );
        assert_eq!(Finding::of(true, true, Due::Past), Some(Finding::Ended));
        assert_eq!(Finding::of(false, true, Due::Past), Some(Finding::Stalled));
        assert_eq!(Finding::of(false, false, Due::Past), Some(Finding::Overdue));
        assert_eq!(Finding::of(false, false, Due::Within), Some(Finding::Due));
        assert_eq!(Finding::of(false, false, Due::Not), None);
        assert_eq!(
            Finding::of(false, true, Due::Not),
            Some(Finding::Stalled),
            "a quiet run stalls a monitor expecting no run too"
        );
    }

    /// What each finding does to each state: the owner's states stand, a run
    /// going keeps a monitor from being late, and the rest follow the finding.
    #[test]
    fn a_monitors_state_follows_what_the_tick_found() {
        use Finding::{Due, Ended, Overdue, Stalled};
        use MonitorState::{Failed, Healthy, Late, New, OverBound, Paused, Running};
        for owners in [New, Paused] {
            for finding in [Ended, Stalled, Overdue, Due] {
                assert_eq!(state_after_tick(owners, finding), None, "{finding:?}");
            }
        }
        for current in [
            Healthy,
            Late,
            Failed,
            Running,
            OverBound,
            MonitorState::Stalled,
        ] {
            assert_eq!(
                state_after_tick(current, Ended),
                Some(Failed),
                "{current:?}"
            );
        }
        for current in [Healthy, Late, Running, OverBound, MonitorState::Stalled] {
            assert_eq!(
                state_after_tick(current, Stalled),
                Some(MonitorState::Stalled),
                "{current:?}"
            );
        }
        assert_eq!(
            state_after_tick(Failed, Stalled),
            Some(Failed),
            "failed is the graver, and stays"
        );
        for going in [Running, OverBound, MonitorState::Stalled] {
            assert_eq!(state_after_tick(going, Overdue), Some(going), "{going:?}");
            assert_eq!(state_after_tick(going, Due), Some(going), "{going:?}");
        }
        for expecting in [Healthy, Late, Failed] {
            assert_eq!(
                state_after_tick(expecting, Overdue),
                Some(Failed),
                "{expecting:?}"
            );
        }
        assert_eq!(state_after_tick(Healthy, Due), Some(Late));
        assert_eq!(state_after_tick(Late, Due), Some(Late));
        assert_eq!(state_after_tick(Failed, Due), Some(Failed));
    }
}
