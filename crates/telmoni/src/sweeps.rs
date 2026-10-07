//! Auth's sweeps on a timer, inside `telmoni serve`.
//!
//! The deletion and audit-verify sweeps take a leader lock for their tick
//! ([`telmoni_auth::sweep`]), so replicas running the same timers cannot both
//! sweep one tick; retention needs none, its DELETEs being idempotent. Every
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

/// Start the three sweeps as tasks of this process.
pub fn spawn(auth: Arc<AppState>) {
    tokio::spawn(every(
        auth.clone(),
        "deletion",
        DELETION_EVERY,
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
        |auth| async move { telmoni_auth::sweep::retention(&auth).await.map(|_| ()) },
    ));
    tokio::spawn(every(
        auth,
        "audit-verify",
        AUDIT_VERIFY_EVERY,
        |auth| async move { telmoni_auth::sweep::audit_verify(&auth).await },
    ));
}

/// One sweep on its timer, forever. The first tick waits [`START_DELAY`];
/// a tick that overruns delays the next rather than stacking.
async fn every<F, Fut>(auth: Arc<AppState>, name: &'static str, period: Duration, tick: F)
where
    F: Fn(Arc<AppState>) -> Fut,
    Fut: Future<Output = Result<(), telmoni_shared::TelmoniError>>,
{
    tokio::time::sleep(START_DELAY).await;
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        tracing::info!(sweep = name, "sweep starting");
        if let Err(e) = tick(auth.clone()).await {
            tracing::warn!(sweep = name, error = %e, "sweep failed; the next tick retries");
        }
    }
}
