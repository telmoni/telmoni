//! Finishes project purges cut short. Auth purges a deleted project once;
//! when that fails (ClickHouse down, or an export holding the project too long)
//! the settings row stays, so every ten minutes this asks auth which of the
//! projects with a settings row are gone and purges them again. A deleted
//! project never comes back, and the purge is safe to repeat.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::{ProjectId, TelemetryError, TelmoniError};

use crate::AppState;
use crate::db::{self, TelemetryLane};
use crate::seam;

/// As often as auth's deletion sweep, which retries organizations' purges.
const EVERY: Duration = Duration::from_secs(10 * 60);

/// How long after boot the first pass runs: a minute, past the readiness probe's
/// first passes and the thirty seconds Kubernetes gives the replaced replica to drain.
const START_DELAY: Duration = Duration::from_secs(60);

/// Settings rows read, and asked of auth, at once.
const PAGE: i64 = 500;

/// Projects one pass purged, and those whose purge failed again.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    pub purged: u64,
    pub left: u64,
}

/// A pass a minute after boot, then every ten minutes, each its own task so
/// that a panic ends that pass alone.
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

/// One pass over every settings row, a page at a time. Ends early when auth
/// cannot answer, or ClickHouse or Postgres does not; any other failed purge
/// is logged and left for the next pass.
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
                Err(e) if e.is_database_unanswered() => return Err(e),
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
