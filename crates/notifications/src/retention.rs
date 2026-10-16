//! The retention sweep: the feed, finished deliveries, and expired handshake
//! states; and, first, each deleted project's purge that its delete could
//! not finish. A loop inside the server, as every sweep is, since the
//! notifications role alone holds a grant on this schema. Detached, unlike
//! delivery: a stalled sweep keeps rows too long — a deleted project's
//! grants among them, until a sweep finishes its purge — where a stalled
//! delivery loop loses customer events; and each sweep is a task of its own,
//! so none ends the loop.
//!
//! Auth calls a project's purge once, after the delete commits and within
//! five seconds; one that fails — a database that does not answer, or a
//! vendor slow to take a grant back, which spends the budget before the
//! rows are reached — leaves the project's rows, its connectors' sealed
//! grants and webhook secrets among them. So each sweep reads the projects
//! the rows name a page at a time, asks auth where they are, and purges
//! again each one auth no longer has ([`purge_gone_projects`]). A deleted
//! project never comes back, so auth's word that one is gone is for good;
//! and the purge is safe to repeat, beside another replica's sweep too.
//!
//! ⚠ An answer auth cannot give ends the pass with every row as it is,
//! never "none of them exist"; so does a database that does not answer,
//! which every purge after would wait on in turn.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::{ProjectId, TelmoniError};

use crate::db::NotificationsLane;
use crate::{AppState, db, handler};

pub use telmoni_shared::db::retention::{DELIVERY_RETENTION_DAYS, FEED_RETENTION_DAYS};

/// Hourly: small against a ninety-day window.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// How long after boot the first sweep runs: a minute rather than the hour,
/// so a server restarted more often than hourly, through a day of deploys,
/// still sweeps, and still finishes a deleted project's purge.
const START_DELAY: Duration = Duration::from_secs(60);

/// Rows deleted per statement per table.
const SWEEP_CHUNK: i64 = 5_000;

/// How many chunks one sweep will take before waiting for the next hour. It
/// only binds after a backlog, where stopping is right: an instance deleting
/// for an hour is an instance not delivering.
const SWEEP_MAX_ROUNDS: usize = 200;

/// Projects read, and asked of auth, at once.
const PROJECTS_PAGE: i64 = 500;

/// What one pass over the projects the rows name did: those it purged, and
/// those whose purge failed again, left for the next sweep.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Purged {
    pub purged: u64,
    pub left: u64,
}

/// The sweep loop: the first after `START_DELAY`, then hourly, each sweep
/// a task of its own, so one that panics ends that sweep alone; one that
/// overruns delays the next rather than stacking. Errors are logged and the
/// loop continues: a failed sweep is rows kept too long, not rows lost.
pub async fn run(state: Arc<AppState>) {
    let mut ticker =
        tokio::time::interval_at(tokio::time::Instant::now() + START_DELAY, SWEEP_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let sweep = tokio::spawn({
            let state = state.clone();
            async move { tick(&state).await }
        });
        if let Err(e) = sweep.await {
            tracing::error!(
                error = %e,
                "notifications: the retention sweep died; the next sweep retries"
            );
        }
    }
}

/// One sweep: the pass over the projects the rows name, then the windows,
/// each logged, and a pass that fails never keeping the windows from
/// running.
pub async fn tick(state: &Arc<AppState>) {
    match purge_gone_projects(state).await {
        Ok(Purged { purged: 0, left: 0 }) => {}
        Ok(Purged { purged, left }) => tracing::info!(
            purged,
            left,
            "notifications: the retention sweep purged deleted projects their delete could not; \
             any left wait for the next sweep"
        ),
        Err(e) => tracing::warn!(
            error = %e,
            "notifications: the retention sweep's pass over deleted projects failed; the next \
             sweep retries"
        ),
    }
    match sweep_once(state).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(removed = n, "notifications retention sweep"),
        Err(e) => tracing::warn!(error = %e, "retention sweep failed; will retry"),
    }
}

/// Every project this module's rows name, a page at a time in id order
/// ([`db::projects_after`]), each page asked of auth, and each project auth
/// no longer has purged again as its delete purges it
/// ([`handler::purge_project`]): its connectors torn down upstream, then its
/// rows. A project auth still has is left as it is, whichever organization
/// its rows name: the pass finishes a deletion, never a hand-over.
///
/// # Errors
///
/// Postgres did not answer a page's read, or a purge
/// ([`TelmoniError::is_database_unanswered`]); or auth could not say where a
/// page's projects are. Each ends the pass, every row not purged yet as it
/// was. A purge that fails otherwise is logged and left for the next sweep,
/// and the pass goes on to the rest.
pub async fn purge_gone_projects(state: &AppState) -> Result<Purged, TelmoniError> {
    let mut swept = Purged::default();
    let mut after = String::new();
    loop {
        let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
        let page = db::projects_after(&mut tx, &after, PROJECTS_PAGE).await?;
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
            match handler::purge_project(state, project_id).await {
                Ok(_) => swept.purged += 1,
                Err(e) if e.is_database_unanswered() => return Err(e),
                Err(e) => {
                    tracing::warn!(
                        project_id = %project_id,
                        error = %e,
                        "notifications: a deleted project's purge failed again; the next sweep \
                         retries"
                    );
                    swept.left += 1;
                }
            }
        }
        if i64::try_from(page.len()).is_ok_and(|n| n < PROJECTS_PAGE) {
            return Ok(swept);
        }
        after = last.as_str().to_owned();
    }
}

/// One retention sweep: bounded chunks, a transaction per chunk, so no one
/// statement timeout covers the sweep and an interrupted one keeps its progress.
/// One replica at a time: a chunk whose lock another instance holds is that
/// instance's sweep, and this one stops.
pub async fn sweep_once(state: &Arc<AppState>) -> Result<u64, TelmoniError> {
    // Its own short transaction, committed before the deletes: its row locks
    // are on live connections the loop stamps after every batch, and held
    // across a round of five-second deletes they would time that stamp out.
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    if !db::try_retention_lock(&mut tx).await? {
        tx.commit().await?;
        return Ok(0);
    }
    let cleared = db::clear_expired_prior_tokens(&mut tx).await?;
    tx.commit().await?;
    if cleared > 0 {
        tracing::info!(
            cleared,
            "rotated webhook secrets past their overlap forgotten"
        );
    }

    let mut removed = 0u64;
    for _ in 0..SWEEP_MAX_ROUNDS {
        let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
        if !db::try_retention_lock(&mut tx).await? {
            tx.commit().await?;
            break;
        }
        let feed = db::sweep_retention(&mut tx, FEED_RETENTION_DAYS, SWEEP_CHUNK).await?;
        let connector =
            db::sweep_connector_rows(&mut tx, DELIVERY_RETENTION_DAYS, SWEEP_CHUNK).await?;
        tx.commit().await?;
        removed += feed + connector;
        let chunk = u64::try_from(SWEEP_CHUNK).unwrap_or(u64::MAX);
        if feed < chunk && connector < chunk {
            break;
        }
    }
    Ok(removed)
}
