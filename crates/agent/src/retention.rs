//! The retention sweep: each copy in the index goes when its source's own
//! window closes, and a conversation ninety days after it was last used.
//! The windows are the sources' own constants, imported rather than
//! restated, so an index can never keep an audit event, a notice or a
//! delivery longer than the row it was made from.

use std::sync::Arc;
use std::time::Duration;

use telmoni_shared::TelmoniError;
use telmoni_shared::db::retention::RETENTION;
use telmoni_shared::db::tenant_session::maintenance_scope;

use crate::AppState;
use crate::db::{self, AgentLane, Source};

/// How long a conversation is kept after it was last used. ⚠ **Published**
/// in the privacy policy beside the rest of what a person's account holds.
pub const CONVERSATION_RETENTION_DAYS: i32 = 90;

/// Hourly, as notifications' sweep runs.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// Rows deleted per statement.
const SWEEP_CHUNK: i64 = 5_000;

/// Chunks one sweep takes before waiting for the next hour.
const SWEEP_MAX_ROUNDS: usize = 200;

/// The audit chain's window, from the rotation registry that drops its
/// partitions.
fn audit_retention_days() -> i32 {
    RETENTION
        .iter()
        .find(|t| t.schema == "audit" && t.table == "events")
        .and_then(|t| i32::try_from(t.retention_days).ok())
        .unwrap_or(i32::MAX)
}

/// Each indexed source and how long its copies stay.
/// Each indexed source and how long its copies stay. The docs are not
/// here: a page stays while the corpus has it, and leaves when it does not.
fn window(source: Source) -> Option<i32> {
    match source {
        Source::Docs => None,
        Source::Audit => Some(audit_retention_days()),
        Source::Feed => Some(telmoni_shared::db::retention::FEED_RETENTION_DAYS),
        Source::Delivery => Some(telmoni_shared::db::retention::DELIVERY_RETENTION_DAYS),
        Source::Conversation => Some(CONVERSATION_RETENTION_DAYS),
    }
}

/// Every source with a window, and the window.
fn windows() -> impl Iterator<Item = (Source, i32)> {
    Source::ALL
        .into_iter()
        .filter_map(|source| Some((source, window(source)?)))
}

/// The sweep loop. A failed sweep is rows kept too long, and is retried.
pub async fn run(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(SWEEP_EVERY);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        match sweep_once(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, "agent retention sweep"),
            Err(e) => tracing::warn!(error = %e, "agent retention sweep failed; will retry"),
        }
    }
}

/// One sweep, in bounded chunks with a transaction each, by one replica at
/// a time.
pub async fn sweep_once(state: &AppState) -> Result<u64, TelmoniError> {
    let mut removed = 0u64;
    let chunk = u64::try_from(SWEEP_CHUNK).unwrap_or(u64::MAX);
    for _ in 0..SWEEP_MAX_ROUNDS {
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        if !db::try_retention_lock(&mut tx).await? {
            tx.commit().await?;
            break;
        }
        let mut full = false;
        for (source, days) in windows() {
            let n = db::sweep_chunks(&mut tx, source, days, SWEEP_CHUNK).await?;
            full |= n >= chunk;
            removed += n;
        }
        let n = db::sweep_conversations(&mut tx, CONVERSATION_RETENTION_DAYS, SWEEP_CHUNK).await?;
        full |= n >= chunk;
        removed += n;
        tx.commit().await?;
        if !full {
            break;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index keeps nothing longer than the row it copied.
    #[test]
    fn the_windows_are_the_sources_own() {
        assert_eq!(
            windows().collect::<Vec<_>>(),
            vec![
                (Source::Audit, 730),
                (Source::Feed, 90),
                (Source::Delivery, 30),
                (Source::Conversation, 90),
            ]
        );
    }
}
