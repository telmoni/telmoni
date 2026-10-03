//! The retention sweep: each copy in the index goes when its source's own
//! window closes, and a conversation ninety days after it was last used.
//! The windows are the sources' own constants, imported rather than
//! restated, so an index can never keep an audit event, a notice or a
//! delivery longer than the row it was made from.
//!
//! It also finds what the agent holds of a project under an organization
//! that no longer has it — the project moved to another, or was deleted —
//! and removes it. Nothing calls the agent when a project leaves: auth asks
//! nothing of it in a transfer's request, so the agent can never refuse or
//! slow one, and what a turn or an index write still landed there afterwards
//! goes the same way.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use telmoni_shared::db::retention::RETENTION;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::{ProjectId, TelmoniError};

use crate::AppState;
use crate::db::{self, AgentLane, Source};

/// How long a conversation is kept after it was last used. ⚠ **Published**
/// in the privacy policy beside the rest of what a person's account holds.
pub const CONVERSATION_RETENTION_DAYS: i32 = 90;

/// How long an erasure's fence is kept: past any retry of the erasure that
/// still needs to know where it reached, and long past any turn it fences.
const ERASURE_FENCE_DAYS: i32 = 30;

/// Hourly, as notifications' sweep runs.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// Rows deleted per statement.
const SWEEP_CHUNK: i64 = 5_000;

/// Rows a project that left loses per statement: fewer, as a conversation
/// takes its messages with it.
const LEFT_CHUNK: i64 = 500;

/// Organization-and-project pairs read, and asked of auth, at once.
const PAIRS_PAGE: i64 = 500;

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

/// One sweep, in bounded chunks with a transaction each: what projects left
/// behind, then each source's window, then the fences nothing needs any
/// more. Each lock is held for its own transaction only, so a replica that
/// finds another mid-step leaves the rest to it, and replicas whose hours
/// fall apart each run a pass: every step is safe to repeat.
pub async fn sweep_once(state: &AppState) -> Result<u64, TelmoniError> {
    let mut removed = drop_projects_that_left(state).await;
    let chunk = u64::try_from(SWEEP_CHUNK).unwrap_or(u64::MAX);
    for _ in 0..SWEEP_MAX_ROUNDS {
        let mut tx = maintenance_scope(&state.db, AgentLane).await?;
        if !db::try_retention_lock(&mut tx).await? {
            tx.commit().await?;
            return Ok(removed);
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
    let mut tx = maintenance_scope(&state.db, AgentLane).await?;
    if db::try_retention_lock(&mut tx).await? {
        db::forget_fences(&mut tx, ERASURE_FENCE_DAYS).await?;
    }
    tx.commit().await?;
    Ok(removed)
}

/// What the agent holds of a project under an organization that no longer
/// has it, read a page of pairs at a time, every pair of them each hour,
/// and each page asked of auth. An answer auth cannot give leaves every row
/// as it is, and a failure here never holds back the windows. A replica
/// that finds another listing a page leaves the pass to it.
async fn drop_projects_that_left(state: &AppState) -> u64 {
    let mut removed = 0u64;
    let mut after = (String::new(), String::new());
    loop {
        let page = async {
            let mut tx = maintenance_scope(&state.db, AgentLane).await?;
            if !db::try_projects_lock(&mut tx).await? {
                tx.commit().await?;
                return Ok::<_, sqlx::Error>(None);
            }
            let pairs = db::held_pairs(&mut tx, (&after.0, &after.1), PAIRS_PAGE).await?;
            tx.commit().await?;
            Ok(Some(pairs))
        }
        .await;
        let pairs = match page {
            Ok(Some(pairs)) if !pairs.is_empty() => pairs,
            Ok(_) => break,
            Err(e) => {
                tracing::warn!(error = %e, "listing the agent's projects failed; the next sweep carries on");
                break;
            }
        };
        let mut projects: Vec<ProjectId> = pairs
            .iter()
            .filter_map(|(_, project)| ProjectId::try_new(project.as_str()).ok())
            .collect();
        projects.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        projects.dedup_by(|a, b| a.as_str() == b.as_str());
        let homes = match state.auth.project_homes(&projects).await {
            Ok(homes) => homes,
            Err(e) => {
                tracing::warn!(error = %e, "auth could not say where projects are held; their rows stay this hour");
                break;
            }
        };
        let home: HashMap<&str, &str> = homes
            .iter()
            .map(|h| (h.project_id.as_str(), h.organization_id.as_str()))
            .collect();
        for (organization, project) in &pairs {
            let asked = projects.iter().any(|p| p.as_str() == project.as_str());
            if !asked || home.get(project.as_str()) == Some(&organization.as_str()) {
                continue;
            }
            removed += drop_left(state, organization, project).await;
        }
        let full = i64::try_from(pairs.len()).is_ok_and(|n| n >= PAIRS_PAGE);
        if let Some(last) = pairs.last() {
            after = last.clone();
        }
        if !full {
            break;
        }
    }
    removed
}

/// Whether `project` is, by auth's answer now, held by an organization other
/// than `organization`, or by none. No answer is no.
async fn still_left(state: &AppState, organization: &str, project: &str) -> bool {
    let Ok(id) = ProjectId::try_new(project) else {
        return false;
    };
    match state.auth.project_homes(std::slice::from_ref(&id)).await {
        Ok(homes) => !homes
            .iter()
            .any(|h| h.organization_id.as_str() == organization),
        Err(_) => false,
    }
}

/// Everything held of `project` under `organization`, a chunk a
/// transaction, passing over rows another transaction holds: the next hour
/// takes those.
///
/// ⚠ **Auth is asked again before every chunk.** The page's answer can be
/// minutes old, and a large project's rows take many chunks: a project that
/// came back meanwhile keeps what it has, and what it wrote since.
async fn drop_left(state: &AppState, organization: &str, project: &str) -> u64 {
    let chunk = u64::try_from(LEFT_CHUNK).unwrap_or(u64::MAX);
    let mut removed = 0u64;
    for _ in 0..SWEEP_MAX_ROUNDS {
        if !still_left(state, organization, project).await {
            break;
        }
        let round = async {
            let mut tx = maintenance_scope(&state.db, AgentLane).await?;
            let n = db::drop_held(&mut tx, organization, project, LEFT_CHUNK).await?;
            tx.commit().await?;
            Ok::<_, sqlx::Error>(n)
        }
        .await;
        match round {
            Ok(n) => {
                removed += n;
                if n < chunk {
                    break;
                }
            }
            Err(e) => {
                tracing::warn!(project_id = project, error = %e,
                    "removing what a project left behind failed; the next sweep carries on");
                break;
            }
        }
    }
    if removed > 0 {
        tracing::info!(
            project_id = project,
            removed,
            "agent rows of a project that left removed"
        );
    }
    removed
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
