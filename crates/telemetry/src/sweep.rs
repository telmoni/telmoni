//! The sweep that finishes a project's purge cut short. Auth calls the
//! purge once, after the project's delete commits and within two seconds
//! ([`crate::seam`]); one that fails — a ClickHouse that does not answer, or
//! a ping holding the project past the purge's two seconds — leaves the
//! project's settings row, which stands while its spans do, and its ping
//! lane closed. So a loop of this module's own, a pass every ten minutes on
//! every replica, reads the settings rows a page at a time, asks auth where
//! their projects are ([`telmoni_shared::seam::Auth::project_homes`]), and
//! purges again each one auth no longer has, as auth's own deletion sweep
//! retries an organization's purges. A deleted project never comes back,
//! so auth's word that one is gone is for good; and the purge is safe to
//! repeat, and beside another replica's pass, each holding the project
//! alone in turn. A project auth still has is left as it is, under
//! whichever organization: settling a move is auth's.
//!
//! ⚠ An answer auth cannot give ends the pass with every row as it is,
//! never "none of them exist"; so does a ClickHouse or a Postgres that does
//! not answer, which every purge after would wait on in turn. A purge that
//! fails otherwise is logged, and the pass goes on to the rest; each is
//! tried again next pass. Each pass is a task of its own, so one that panics
//! ends that pass alone.
//!
//! ⚠ The loop's handle is raced against the server in the binary's `serve`,
//! as the outbox's and the rotation's are: a dead loop would finish no purge
//! on its replica for as long as the pod ran, so the process exits and the
//! restart is the recovery.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::{ProjectId, TelemetryError, TelmoniError};

use crate::AppState;
use crate::db::{self, TelemetryLane};
use crate::seam;

/// How often a pass looks: as often as auth's deletion sweep, the retry net
/// behind an organization's purges.
const EVERY: Duration = Duration::from_secs(10 * 60);

/// How long after boot the first pass runs: past the startup probes.
const START_DELAY: Duration = Duration::from_secs(60);

/// Settings rows read, and asked of auth, at once.
const PAGE: i64 = 500;

/// What one pass did: the projects it purged, and those whose purge failed
/// again, left for the next.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    pub purged: u64,
    pub left: u64,
}

/// The sweep, forever: the first pass after `START_DELAY`, then one every
/// `EVERY`; a pass that overruns delays the next rather than stacking.
pub async fn run(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + START_DELAY, EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let pass = tokio::spawn({
            let state = state.clone();
            async move { sweep(&state).await }
        });
        match pass.await {
            Ok(Ok(Swept { purged: 0, left: 0 })) => {}
            Ok(Ok(Swept { purged, left })) => tracing::info!(
                purged,
                left,
                "telemetry: the purge sweep's pass over deleted projects' purges cut short; any \
                 left wait for the next"
            ),
            Ok(Err(e)) => tracing::warn!(
                error = %e,
                "telemetry: the purge sweep failed; the next pass retries"
            ),
            Err(e) => tracing::error!(
                error = %e,
                "telemetry: the purge sweep died; the next pass retries"
            ),
        }
    }
}

/// One pass over every settings row, a page at a time.
///
/// # Errors
///
/// Postgres did not answer a page's read, or a purge (`db::unanswered`);
/// auth could not say where a page's projects are; or ClickHouse did not
/// answer a purge's mark. Each ends the pass, every row not purged yet as
/// it was.
pub async fn sweep(state: &AppState) -> Result<Swept, TelmoniError> {
    let mut swept = Swept::default();
    let mut after = String::new();
    loop {
        let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
        let page = db::settings_after(&mut tx, &after, PAGE).await?;
        tx.commit().await?;
        let Some(last) = page.last() else {
            return Ok(swept);
        };
        let held: HashSet<ProjectId> = state
            .auth
            .project_homes(&page)
            .await?
            .into_iter()
            .map(|home| home.project_id)
            .collect();
        for project_id in page.iter().filter(|project| !held.contains(*project)) {
            match seam::purge_project(state, project_id).await {
                Ok(_) => swept.purged += 1,
                Err(e @ TelmoniError::Telemetry(TelemetryError::StoreUnavailable(_))) => {
                    return Err(e);
                }
                Err(e) if db::unanswered(&e) => return Err(e),
                Err(e) => {
                    tracing::warn!(
                        project_id = %project_id,
                        error = %e,
                        "telemetry: a deleted project's purge failed again; the next pass retries"
                    );
                    swept.left += 1;
                }
            }
        }
        if i64::try_from(page.len()).is_ok_and(|n| n < PAGE) {
            return Ok(swept);
        }
        after = last.as_str().to_owned();
    }
}
