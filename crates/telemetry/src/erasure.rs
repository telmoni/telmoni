//! A trace's deletion and an end user's erasure, one path for both: the
//! traces they name found in ClickHouse, remembered in Redis for a day, their
//! content deleted from Postgres by trace, its bytes counted off, and the
//! traces marked deleted by a lightweight delete, on a task of their own so a
//! caller that hangs up does not cut them short. An end user's are every
//! trace that carries the keyed hash ingest stored for their id, and the
//! traces of their sessions that name nobody. Ingest drops what a deleted
//! trace still sends ([`drop_deleted`]) and marks again what landed as it
//! went ([`mark_late`]).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use clickhouse::Row;
use serde::Deserialize;

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::seam::Receipt;
use telmoni_shared::{ProjectId, TelmoniError, UserId};

use crate::AppState;
use crate::db::{self, TelemetryLane};
use crate::ingest::batch::Batch;
use crate::store::{self, Projects};

/// How long a deleted trace is remembered: past the end of any trace still
/// sending when it was deleted.
const REMEMBERED: Duration = Duration::from_secs(24 * 60 * 60);

/// The most values one statement or command names: a ClickHouse parameter
/// travels in the URL, which it caps at 1 MiB.
const PER_STATEMENT: usize = 5_000;

/// The Redis set of a project's deleted traces.
fn deleted_traces(project_id: &ProjectId) -> String {
    format!("telemetry:deleted-traces:{project_id}")
}

/// The traces an end user's hash is on, and their sessions: by `user_id_idx`.
const USER_SQL: &str = "SELECT DISTINCT trace_id, session_id FROM telemetry.spans
     WHERE project_id IN {projects:Array(String)} AND user_id = {user:String}";

/// The traces of some sessions: by `session_id_idx`.
const SESSIONS_SQL: &str = "SELECT DISTINCT trace_id FROM telemetry.spans
     WHERE project_id IN {projects:Array(String)} AND has({sessions:Array(String)}, session_id)";

/// Each of some traces whole, by `trace_id_idx`: its spans, those carrying
/// the hash, and those naming anyone.
const TRACES_SQL: &str = "SELECT trace_id, count() AS spans,
            countIf(user_id = {user:String}) AS theirs, countIf(user_id != '') AS named
       FROM telemetry.spans
      WHERE project_id IN {projects:Array(String)} AND has({traces:Array(String)}, trace_id)
      GROUP BY trace_id";

/// What a deletion takes.
#[derive(Debug, Clone, Copy)]
pub enum Erase<'a> {
    /// One trace, by its id.
    Trace(&'a str),
    /// An end user, by the hash their id is stored as.
    EndUser(&'a str),
}

#[derive(Row, Deserialize)]
struct UserTrace {
    trace_id: String,
    session_id: String,
}

#[derive(Row, Deserialize)]
struct SessionTrace {
    trace_id: String,
}

#[derive(Row, Deserialize)]
struct TraceCount {
    trace_id: String,
    spans: u64,
    theirs: u64,
    named: u64,
}

/// The traces a deletion takes, found before anything goes.
#[derive(Debug, Default)]
pub struct Found {
    /// Each trace once.
    pub traces: Vec<String>,
    /// Their spans.
    pub spans: u64,
    /// An end user's sessions, whose own scores go with them.
    pub sessions: Vec<String>,
}

/// The traces `erase` names in the project: a trace whole, an end user's
/// those carrying their hash, and those of their sessions naming nobody,
/// never a trace naming someone else on any of its spans.
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
    let mut found_sessions = Vec::new();
    let (candidates, user) = match erase {
        Erase::Trace(trace) => (vec![trace.to_owned()], ""),
        Erase::EndUser(user) => {
            let theirs: Vec<UserTrace> = state
                .store
                .read(&projects, USER_SQL)
                .param("user", user)
                .fetch_all()
                .await
                .map_err(read_failed)?;
            let mut traces: HashSet<String> =
                theirs.iter().map(|trace| trace.trace_id.clone()).collect();
            let sessions: Vec<String> = theirs
                .into_iter()
                .map(|trace| trace.session_id)
                .filter(|session| !session.is_empty())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            found_sessions = sessions.clone();
            for some in sessions.chunks(PER_STATEMENT) {
                let shared: Vec<SessionTrace> = state
                    .store
                    .read(&projects, SESSIONS_SQL)
                    .param("sessions", some)
                    .fetch_all()
                    .await
                    .map_err(read_failed)?;
                traces.extend(shared.into_iter().map(|trace| trace.trace_id));
            }
            (traces.into_iter().collect(), user)
        }
    };
    let mut found = Found {
        sessions: found_sessions,
        ..Found::default()
    };
    for some in candidates.chunks(PER_STATEMENT) {
        let counted: Vec<TraceCount> = state
            .store
            .read(&projects, TRACES_SQL)
            .param("traces", some)
            .param("user", user)
            .fetch_all()
            .await
            .map_err(read_failed)?;
        for trace in counted {
            let taken = match erase {
                Erase::Trace(_) => true,
                Erase::EndUser(_) => trace.theirs > 0 || trace.named == 0,
            };
            if taken {
                found.traces.push(trace.trace_id);
                found.spans += trace.spans;
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
    let (trace, user) = match erase {
        Erase::Trace(trace) => (Some(trace.to_owned()), None),
        Erase::EndUser(user) => (None, Some(user.to_owned())),
    };
    let (state, project_id, to) = (Arc::clone(state), project_id.clone(), to.clone());
    tokio::spawn(async move {
        // Each held trace they are in goes whole, so its children cannot land later.
        if let Some(user) = &user {
            let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
            sqlx::query(
                "DELETE FROM telemetry.held_spans
                  WHERE project_id = $1 AND landed_at IS NULL
                    AND trace_id IN (SELECT trace_id FROM telemetry.held_spans
                                      WHERE project_id = $1 AND landed_at IS NULL
                                        AND row->>'user_id' = $2)",
            )
            .bind(&project_id)
            .bind(user)
            .execute(tx.conn())
            .await?;
            tx.commit().await?;
        }
        let fields = delete(&state, &project_id, &found.traces).await?;
        state
            .store
            .delete_session_scores(&project_id, &found.sessions)
            .await
            .map_err(|e| {
                store::failed(
                    &e,
                    "a deletion's mark",
                    "the span store did not answer; try again",
                )
            })?;
        let receipt = match &trace {
            Some(trace_id) => Receipt::Trace {
                project_id: &project_id,
                trace_id,
                spans: found.spans,
            },
            None => Receipt::EndUser {
                project_id: &project_id,
                traces: found.traces.len() as u64,
            },
        };
        state.auth.mail_receipt(&to, receipt).await;
        Ok(fields)
    })
    .await
    .map_err(|_| TelmoniError::Internal("telemetry: a deletion's task ended early".into()))?
}

/// `traces` remembered, their content deleted, then marked deleted in
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
    traces: &[String],
) -> Result<u64, TelmoniError> {
    if traces.is_empty() {
        return Ok(0);
    }
    remember(state, project_id, traces).await;
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let fields = db::delete_traces_content(tx.conn(), project_id, traces).await?;
    tx.commit().await?;
    state
        .store
        .delete_traces(project_id, traces)
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

/// `traces` remembered as deleted, before anything goes, so a span landing
/// meanwhile is dropped too. Without Redis they are not, logged.
async fn remember(state: &AppState, project_id: &ProjectId, traces: &[String]) {
    let Some(mut connection) = state.redis.connection().await else {
        tracing::warn!(project_id = %project_id, "telemetry: a deletion's traces could not be remembered; spans they send later are kept");
        return;
    };
    let key = deleted_traces(project_id);
    let mut pipe = redis::pipe();
    for some in traces.chunks(PER_STATEMENT) {
        pipe.sadd(&key, some).ignore();
    }
    pipe.expire(
        &key,
        i64::try_from(REMEMBERED.as_secs()).unwrap_or(i64::MAX),
    )
    .ignore();
    if let Err(error) = pipe.query_async::<()>(&mut connection).await {
        state.redis.stood_in("a deletion's traces", &error);
    }
}

/// Which of the batch's traces were deleted: one ask of Redis, none without it.
async fn deleted_of(state: &AppState, project_id: &ProjectId, batch: &Batch) -> HashSet<String> {
    let traces: Vec<String> = batch
        .rows
        .iter()
        .map(|row| row.trace_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if traces.is_empty() {
        return HashSet::new();
    }
    let Some(mut connection) = state.redis.connection().await else {
        return HashSet::new();
    };
    let answer: Result<Vec<bool>, redis::RedisError> = redis::cmd("SMISMEMBER")
        .arg(deleted_traces(project_id))
        .arg(&traces)
        .query_async(&mut connection)
        .await;
    match answer {
        Ok(answer) => traces
            .into_iter()
            .zip(answer)
            .filter_map(|(trace, deleted)| deleted.then_some(trace))
            .collect(),
        Err(error) => {
            state.redis.stood_in("the deleted traces", &error);
            HashSet::new()
        }
    }
}

/// Whether `trace` was deleted lately, as ingest's late spans are told: one
/// ask of Redis, and not, without it.
pub async fn was_deleted(state: &AppState, project_id: &ProjectId, trace: &str) -> bool {
    let Some(mut connection) = state.redis.connection().await else {
        return false;
    };
    let answer: Result<bool, redis::RedisError> = redis::cmd("SISMEMBER")
        .arg(deleted_traces(project_id))
        .arg(trace)
        .query_async(&mut connection)
        .await;
    answer.unwrap_or_else(|error| {
        state.redis.stood_in("the deleted traces", &error);
        false
    })
}

/// The batch's spans of `gone`, and their content, taken out.
fn take_out(batch: &mut Batch, gone: &HashSet<String>) {
    batch.rows.retain(|row| !gone.contains(&row.trace_id));
    batch.content.retain(|kept| !gone.contains(&kept.trace_id));
}

/// Before the insert: the batch's spans of a deleted trace dropped, with
/// their content; answers how many traces.
pub async fn drop_deleted(state: &AppState, project_id: &ProjectId, batch: &mut Batch) -> usize {
    let gone = deleted_of(state, project_id, batch).await;
    take_out(batch, &gone);
    gone.len()
}

/// After the insert: traces deleted while it was under way marked deleted
/// again, and their content not kept, since a deletion's mark misses rows
/// ClickHouse had not written yet. A mark it does not take is logged.
pub async fn mark_late(state: &AppState, project_id: &ProjectId, batch: &mut Batch) {
    let gone = deleted_of(state, project_id, batch).await;
    if gone.is_empty() {
        return;
    }
    let traces: Vec<String> = gone.iter().cloned().collect();
    if let Err(error) = state.store.delete_traces(project_id, &traces).await {
        tracing::warn!(
            project_id = %project_id,
            kind = store::Fault::of(&error).as_str(),
            "ingest: spans of traces deleted as they landed were not marked deleted again"
        );
    }
    take_out(batch, &gone);
}
