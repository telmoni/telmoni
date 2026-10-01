//! The retention sweep: the feed, finished deliveries, and expired handshake
//! states. A loop inside the server, as every sweep is, since the
//! notifications role alone holds a grant on this schema. Detached, unlike
//! delivery: a stalled sweep keeps rows too long, a stalled delivery loop
//! loses customer events.

use std::sync::Arc;
use std::time::Duration;

use telmoni_shared::TelmoniError;
use telmoni_shared::db::tenant_session::maintenance_scope;

use crate::db::NotificationsLane;
use crate::{AppState, db};

pub use telmoni_shared::db::retention::{DELIVERY_RETENTION_DAYS, FEED_RETENTION_DAYS};

/// Hourly: small against a ninety-day window.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// Rows deleted per statement per table.
const SWEEP_CHUNK: i64 = 5_000;

/// How many chunks one sweep will take before waiting for the next hour. It
/// only binds after a backlog, where stopping is right: an instance deleting
/// for an hour is an instance not delivering.
const SWEEP_MAX_ROUNDS: usize = 200;

/// The sweep loop. Errors are logged and the loop continues: a failed sweep
/// is rows kept too long, not rows lost.
pub async fn run(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(SWEEP_EVERY);
    ticker.tick().await; // skip the immediate first tick
    loop {
        ticker.tick().await;
        match sweep_once(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "notifications retention sweep"),
            Err(e) => tracing::warn!(error = %e, "retention sweep failed; will retry"),
        }
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
