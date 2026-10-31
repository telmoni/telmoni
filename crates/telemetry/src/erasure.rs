//! A trace's deletion and an end user's erasure, one path for both: the runs
//! they name found in ClickHouse, remembered in Redis for a day, their
//! content deleted from Postgres by run, its bytes counted off, and the runs
//! marked deleted by a lightweight delete, on a task of their own so a caller
//! that hangs up does not cut them short. A run is a trace as the console
//! shows it; an end user's are every run that carries the keyed hash ingest
//! stored for their id, and the runs of their sessions that name nobody.
//! Ingest drops what a deleted run still sends ([`drop_deleted`]) and marks
//! again what landed as it went ([`mark_late`]).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use clickhouse::Row;
use serde::Deserialize;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::seam::Receipt;
use telmoni_shared::{ProjectId, TelmoniError, UserId};

use crate::AppState;
use crate::db::{self, TelemetryLane};
use crate::ingest::batch::Batch;
use crate::store::{self, Projects};

/// How long a deleted run is remembered: past the end of any run still
/// sending when it was deleted.
const REMEMBERED: Duration = Duration::from_secs(24 * 60 * 60);

/// The most values one statement or command names: a ClickHouse parameter
/// travels in the URL, which it caps at 1 MiB.
const PER_STATEMENT: usize = 5_000;

/// The Redis set of a project's deleted runs.
fn deleted_runs(project_id: &ProjectId) -> String {
    format!("telemetry:deleted-runs:{project_id}")
}

/// The runs an end user's hash is on, and their sessions: by `user_id_idx`.
const USER_SQL: &str = "SELECT DISTINCT run_id, session_id FROM telemetry.spans
     WHERE project_id IN {projects:Array(String)} AND user_id = {user:String}";

/// The runs of some sessions: by `session_id_idx`.
const SESSIONS_SQL: &str = "SELECT DISTINCT run_id FROM telemetry.spans
     WHERE project_id IN {projects:Array(String)} AND has({sessions:Array(String)}, session_id)";

/// Each of some runs whole, by `run_id_idx`: its spans, those carrying the
/// hash, and those naming anyone.
const RUNS_SQL: &str = "SELECT run_id, count() AS spans,
            countIf(user_id = {user:String}) AS theirs, countIf(user_id != '') AS named
       FROM telemetry.spans
      WHERE project_id IN {projects:Array(String)} AND has({runs:Array(UUID)}, run_id)
      GROUP BY run_id";

/// What a deletion takes.
#[derive(Debug, Clone, Copy)]
pub enum Erase<'a> {
    /// One run.
    Trace(Uuid),
    /// An end user, by the hash their id is stored as.
    EndUser(&'a str),
}

#[derive(Row, Deserialize)]
struct UserRun {
    #[serde(with = "clickhouse::serde::uuid")]
    run_id: Uuid,
    session_id: String,
}

#[derive(Row, Deserialize)]
struct SessionRun {
    #[serde(with = "clickhouse::serde::uuid")]
    run_id: Uuid,
}

#[derive(Row, Deserialize)]
struct RunCount {
    #[serde(with = "clickhouse::serde::uuid")]
    run_id: Uuid,
    spans: u64,
    theirs: u64,
    named: u64,
}

/// The runs a deletion takes, found before anything goes.
#[derive(Debug, Default)]
pub struct Found {
    /// Each run once.
    pub runs: Vec<Uuid>,
    /// Their spans.
    pub spans: u64,
}

/// The runs `erase` names in the project: a run whole, an end user's those
/// carrying their hash, and those of their sessions naming nobody, never a
/// run naming someone else on any of its spans.
///
/// # Errors
///
/// ClickHouse did not answer, a 503 to retry, or refused the read.
pub async fn find(
    state: &AppState,
    project_id: &ProjectId,
    erase: Erase<'_>,
) -> Result<Found, TelmoniError> {
    let projects = Projects::one(project_id.clone());
    let read_failed = |e: clickhouse::error::Error| {
        store::failed(
            &e,
            "a deletion's read",
            "the span store did not answer; try again",
        )
    };
    let (candidates, user) = match erase {
        Erase::Trace(run) => (vec![run], ""),
        Erase::EndUser(user) => {
            let theirs: Vec<UserRun> = state
                .store
                .read(&projects, USER_SQL)
                .param("user", user)
                .fetch_all()
                .await
                .map_err(read_failed)?;
            let mut runs: HashSet<Uuid> = theirs.iter().map(|run| run.run_id).collect();
            let sessions: Vec<String> = theirs
                .into_iter()
                .map(|run| run.session_id)
                .filter(|session| !session.is_empty())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            for some in sessions.chunks(PER_STATEMENT) {
                let shared: Vec<SessionRun> = state
                    .store
                    .read(&projects, SESSIONS_SQL)
                    .param("sessions", some)
                    .fetch_all()
                    .await
                    .map_err(read_failed)?;
                runs.extend(shared.into_iter().map(|run| run.run_id));
            }
            (runs.into_iter().collect(), user)
        }
    };
    let mut found = Found::default();
    for some in candidates.chunks(PER_STATEMENT) {
        let counted: Vec<RunCount> = state
            .store
            .read(&projects, RUNS_SQL)
            .param("runs", some)
            .param("user", user)
            .fetch_all()
            .await
            .map_err(read_failed)?;
        for run in counted {
            let taken = match erase {
                Erase::Trace(_) => true,
                Erase::EndUser(_) => run.theirs > 0 || run.named == 0,
            };
            if taken {
                found.runs.push(run.run_id);
                found.spans += run.spans;
            }
        }
    }
    Ok(found)
}

/// `found` deleted and its receipt mailed to `to`, on a task
/// of its own, so a caller that hangs up neither cuts the deletion short nor
/// loses the receipt; answers how many content fields went.
///
/// # Errors
///
/// ClickHouse or Postgres did not answer, each a 503 to retry.
pub async fn carry_out(
    state: &Arc<AppState>,
    project_id: &ProjectId,
    erase: Erase<'_>,
    found: Found,
    to: &UserId,
) -> Result<u64, TelmoniError> {
    let run = match erase {
        Erase::Trace(run) => Some(run),
        Erase::EndUser(_) => None,
    };
    let (state, project_id, to) = (Arc::clone(state), project_id.clone(), to.clone());
    tokio::spawn(async move {
        let fields = delete(&state, &project_id, &found.runs).await?;
        let receipt = match run {
            Some(run_id) => Receipt::Trace {
                project_id: &project_id,
                run_id,
                spans: found.spans,
            },
            None => Receipt::EndUser {
                project_id: &project_id,
                traces: found.runs.len() as u64,
            },
        };
        state.auth.mail_receipt(&to, receipt).await;
        Ok(fields)
    })
    .await
    .map_err(|_| TelmoniError::Internal("telemetry: a deletion's task ended early".into()))?
}

/// `runs` remembered, their content deleted, then marked deleted in
/// ClickHouse; answers how many content fields went. Safe to repeat: the
/// content goes first, so a mark ClickHouse applies after its answer timed
/// out leaves none behind.
///
/// # Errors
///
/// ClickHouse or Postgres did not answer, each a 503 to retry.
async fn delete(
    state: &AppState,
    project_id: &ProjectId,
    runs: &[Uuid],
) -> Result<u64, TelmoniError> {
    if runs.is_empty() {
        return Ok(0);
    }
    remember(state, project_id, runs).await;
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let fields = db::delete_runs_content(tx.conn(), project_id, runs).await?;
    tx.commit().await?;
    state
        .store
        .delete_runs(project_id, runs)
        .await
        .map_err(|e| {
            store::failed(
                &e,
                "a deletion's mark",
                "the span store did not answer; try again",
            )
        })?;
    Ok(fields)
}

/// `runs` remembered as deleted, before anything goes, so a span landing
/// meanwhile is dropped too. Without Redis they are not, logged.
async fn remember(state: &AppState, project_id: &ProjectId, runs: &[Uuid]) {
    let Some(mut connection) = state.redis.connection().await else {
        tracing::warn!(project_id = %project_id, "telemetry: a deletion's runs could not be remembered; spans they send later are kept");
        return;
    };
    let key = deleted_runs(project_id);
    let mut pipe = redis::pipe();
    for some in runs.chunks(PER_STATEMENT) {
        let ids: Vec<String> = some.iter().map(Uuid::to_string).collect();
        pipe.sadd(&key, ids).ignore();
    }
    pipe.expire(
        &key,
        i64::try_from(REMEMBERED.as_secs()).unwrap_or(i64::MAX),
    )
    .ignore();
    if let Err(error) = pipe.query_async::<()>(&mut connection).await {
        state.redis.stood_in("a deletion's runs", &error);
    }
}

/// Which of the batch's runs were deleted: one ask of Redis, none without it.
async fn deleted_of(state: &AppState, project_id: &ProjectId, batch: &Batch) -> HashSet<Uuid> {
    let runs: Vec<Uuid> = batch
        .rows
        .iter()
        .map(|row| row.run_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if runs.is_empty() {
        return HashSet::new();
    }
    let Some(mut connection) = state.redis.connection().await else {
        return HashSet::new();
    };
    let ids: Vec<String> = runs.iter().map(Uuid::to_string).collect();
    let answer: Result<Vec<bool>, redis::RedisError> = redis::cmd("SMISMEMBER")
        .arg(deleted_runs(project_id))
        .arg(&ids)
        .query_async(&mut connection)
        .await;
    match answer {
        Ok(answer) => runs
            .into_iter()
            .zip(answer)
            .filter_map(|(run, deleted)| deleted.then_some(run))
            .collect(),
        Err(error) => {
            state.redis.stood_in("the deleted runs", &error);
            HashSet::new()
        }
    }
}

/// The batch's spans of `gone`, and their content, taken out.
fn take_out(batch: &mut Batch, gone: &HashSet<Uuid>) {
    batch.rows.retain(|row| !gone.contains(&row.run_id));
    batch.content.retain(|kept| !gone.contains(&kept.run_id));
}

/// Before the insert: the batch's spans of a deleted run dropped, with their
/// content; answers how many runs.
pub async fn drop_deleted(state: &AppState, project_id: &ProjectId, batch: &mut Batch) -> usize {
    let gone = deleted_of(state, project_id, batch).await;
    take_out(batch, &gone);
    gone.len()
}

/// After the insert: runs deleted while it was under way marked deleted
/// again, and their content not kept, since a deletion's mark misses rows
/// ClickHouse had not written yet. A mark it does not take is logged.
pub async fn mark_late(state: &AppState, project_id: &ProjectId, batch: &mut Batch) {
    let gone = deleted_of(state, project_id, batch).await;
    if gone.is_empty() {
        return;
    }
    let runs: Vec<Uuid> = gone.iter().copied().collect();
    if let Err(error) = state.store.delete_runs(project_id, &runs).await {
        tracing::warn!(
            project_id = %project_id,
            kind = store::Fault::of(&error).as_str(),
            "ingest: spans of runs deleted as they landed were not marked deleted again"
        );
    }
    take_out(batch, &gone);
}
