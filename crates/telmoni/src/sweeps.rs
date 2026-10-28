//! Auth's sweeps on a timer, inside `telmoni-server serve`.
//!
//! The deletion and audit-verify sweeps take a leader lock for their tick
//! ([`telmoni_auth::sweep`]), so replicas running the same timers cannot both
//! sweep one tick; retention needs none, its DELETEs being idempotent, nor the
//! audit exports' retry, each build claiming its export first. Every
//! step is idempotent, so a tick cut short by a rollout is finished by the
//! next. A failed tick is logged and the next one runs; one that ends beats in
//! `auth.heartbeats`, for the watchdog. `telmoni-server sweep <name>` runs any
//! of them once by hand.

use std::sync::Arc;
use std::time::Duration;

use telmoni_auth::AppState;
use telmoni_auth::db::AuthLane;
use telmoni_shared::TelmoniError;
use telmoni_shared::heartbeat::Heartbeats;

use crate::cli::AuthSweep;

/// How long after boot the first tick of each sweep runs: a minute, past the
/// readiness probe's first passes and the thirty seconds Kubernetes gives the
/// replaced replica to drain its own tick.
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

/// Start the four sweeps as tasks of this process, each beating in
/// `heartbeats`.
pub fn spawn(auth: Arc<AppState>, heartbeats: &Arc<Heartbeats<AuthLane>>) {
    for (sweep, period, start) in [
        (AuthSweep::Deletion, DELETION_EVERY, Start::Logged),
        (AuthSweep::Retention, RETENTION_EVERY, Start::Logged),
        (AuthSweep::AuditVerify, AUDIT_VERIFY_EVERY, Start::Logged),
        (AuthSweep::AuditExports, AUDIT_EXPORTS_EVERY, Start::Quiet),
    ] {
        tokio::spawn(every(
            auth.clone(),
            heartbeats.clone(),
            sweep,
            period,
            start,
        ));
    }
}

/// One run of `sweep`, as its timer and `sweep <name>` both run it.
pub(crate) async fn once(auth: &AppState, sweep: AuthSweep) -> Result<(), TelmoniError> {
    match sweep {
        AuthSweep::Deletion => {
            let swept = telmoni_auth::sweep::deletion(auth).await?;
            tracing::info!(
                organizations = swept.organizations,
                purged = swept.purged,
                people = swept.people,
                "deletion sweep complete"
            );
        }
        AuthSweep::Retention => {
            telmoni_auth::sweep::retention(auth).await?;
        }
        AuthSweep::AuditVerify => telmoni_auth::sweep::audit_verify(auth).await?,
        AuthSweep::AuditExports => {
            let swept = telmoni_auth::audit_export::sweep(auth).await?;
            // Most runs find nothing to do; one that did something is the one
            // worth a line.
            if swept != telmoni_auth::audit_export::ExportsSwept::default() {
                tracing::info!(
                    finished = swept.finished,
                    expired = swept.expired,
                    "audit exports sweep complete"
                );
            }
        }
    }
    Ok(())
}

/// One sweep on its timer, forever, the first tick after [`START_DELAY`].
async fn every(
    auth: Arc<AppState>,
    heartbeats: Arc<Heartbeats<AuthLane>>,
    sweep: AuthSweep,
    period: Duration,
    start: Start,
) {
    heartbeats
        .every(&auth.db, sweep.as_str(), START_DELAY, period, || {
            let auth = auth.clone();
            async move {
                if start == Start::Logged {
                    tracing::info!(sweep = sweep.as_str(), "sweep starting");
                }
                once(&auth, sweep).await
            }
        })
        .await;
}
