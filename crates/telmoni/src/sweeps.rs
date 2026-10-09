//! Auth's sweeps on a timer, inside `telmoni serve`.
//!
//! The deletion and audit-verify sweeps take a leader lock for their tick
//! ([`telmoni_auth::sweep`]), so replicas running the same timers cannot both
//! sweep one tick; retention needs none, its DELETEs being idempotent, nor the
//! audit exports' retry, each build claiming its export first. Every
//! step is idempotent, so a tick cut short by a rollout is finished by the
//! next. A failed tick is logged and the next one runs. `telmoni sweep
//! <name>` runs any of them once by hand.

use std::sync::Arc;
use std::time::Duration;

use telmoni_auth::AppState;

/// How long after boot the first tick of each sweep runs: past the startup
/// probes, and past the replica this one is replacing draining its own.
const START_DELAY: Duration = Duration::from_secs(60);

/// How often the deletion sweep runs: the retry net behind every inline
/// tail, so a failed hook purge or a person's erasure waits at most this.
const DELETION_EVERY: Duration = Duration::from_mins(10);

/// How often the retention sweep prunes what nothing else prunes.
const RETENTION_EVERY: Duration = Duration::from_hours(24);

/// How often every organization's audit chain is walked.
const AUDIT_VERIFY_EVERY: Duration = Duration::from_hours(24);

/// How often the audit exports' sweep runs: the retry net behind the build
/// each request starts, so one a restart dropped waits at most this, and the
/// deletion of every file past its week, so none outlives it by more.
const AUDIT_EXPORTS_EVERY: Duration = Duration::from_mins(5);

/// Whether a tick logs its start: a sweep on a timer of minutes leaves it to
/// the ticks that did something, or a quiet day is three hundred lines a
/// replica of nothing happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Logged,
    Quiet,
}

/// Start the four sweeps as tasks of this process.
pub fn spawn(auth: Arc<AppState>) {
    tokio::spawn(every(
        auth.clone(),
        "deletion",
        DELETION_EVERY,
        Start::Logged,
        |auth| async move {
            let swept = telmoni_auth::sweep::deletion(&auth).await?;
            tracing::info!(
                organizations = swept.organizations,
                purged = swept.purged,
                people = swept.people,
                "deletion sweep complete"
            );
            Ok(())
        },
    ));
    tokio::spawn(every(
        auth.clone(),
        "retention",
        RETENTION_EVERY,
        Start::Logged,
        |auth| async move { telmoni_auth::sweep::retention(&auth).await.map(|_| ()) },
    ));
    tokio::spawn(every(
        auth.clone(),
        "audit-verify",
        AUDIT_VERIFY_EVERY,
        Start::Logged,
        |auth| async move { telmoni_auth::sweep::audit_verify(&auth).await },
    ));
    tokio::spawn(every(
        auth,
        "audit-exports",
        AUDIT_EXPORTS_EVERY,
        Start::Quiet,
        |auth| async move {
            let swept = telmoni_auth::audit_export::sweep(&auth).await?;
            // Most ticks find nothing to do; a tick that did something is the
            // one worth a line.
            if swept != telmoni_auth::audit_export::ExportsSwept::default() {
                tracing::info!(
                    finished = swept.finished,
                    expired = swept.expired,
                    "audit exports sweep complete"
                );
            }
            Ok(())
        },
    ));
}

/// One sweep on its timer, forever. The first tick waits [`START_DELAY`];
/// a tick that overruns delays the next rather than stacking.
async fn every<F, Fut>(
    auth: Arc<AppState>,
    name: &'static str,
    period: Duration,
    start: Start,
    tick: F,
) where
    F: Fn(Arc<AppState>) -> Fut,
    Fut: Future<Output = Result<(), telmoni_shared::TelmoniError>>,
{
    tokio::time::sleep(START_DELAY).await;
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if start == Start::Logged {
            tracing::info!(sweep = name, "sweep starting");
        }
        if let Err(e) = tick(auth.clone()).await {
            tracing::warn!(sweep = name, error = %e, "sweep failed; the next tick retries");
        }
    }
}
