//! Another exporter's spans, with no `telmoni.run_id` and `telmoni.agent`:
//! held at the door ([`hold`]), then placed in the run their trace's tree
//! says and landed by a loop on every replica ([`run`], [`place`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::acting::ActingKey;
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::digest::sha256_hex;
use telmoni_shared::envelope::SealedRef;
use telmoni_shared::{
    ContentMode, OrInternal, OrganizationId, ProjectId, Redacted, SpanKind, SpanStatus,
    TelmoniError,
};

use crate::AppState;
use crate::content;
use crate::contract;
use crate::db::TelemetryLane;
use crate::ingest::batch::{Batch, SpanContent};
use crate::ingest::span::{Refusal, Refused, units_of};
use crate::store::SpanRow;

/// The loop's name on the watchdog's heartbeat.
pub const HEARTBEAT: &str = "foreign";

/// How often each replica looks for traces gone quiet.
const EVERY: Duration = Duration::from_secs(15);

/// How long a trace sends nothing before its spans are placed.
const QUIET_SECS: f64 = 60.0;

/// How long a trace still sending waits before what it sent so far lands.
const WAITED_MAX_SECS: f64 = 300.0;

/// How long a span waits for a parent that has not arrived: a span ends
/// before its parent, so an agent's own span comes last, after its run.
pub const ORPHAN_MAX_SECS: i64 = 3600;

/// [`ORPHAN_MAX_SECS`] as an interval's seconds.
const ORPHAN_MAX_INTERVAL: f64 = 3600.0;

/// How long a landing begun is left to its replica before another retries it.
const LANDING_RETRY_SECS: f64 = 300.0;

/// The traces one pass lands, oldest first.
const TRACES_A_PASS: i64 = 200;

/// The spans a project holds waiting at once; past it, an export's are
/// refused, so a flood of a service's other spans fills no table.
pub const HELD_MAX: i64 = 100_000;

/// The name runs derived here are made under, so no other id collides.
const NAMESPACE: Uuid = Uuid::from_u128(0x6a1f_3c52_9e04_4d7b_b8e1_52c0_7f3a_9d16);

/// The resource's name of the process, which names a run of calls alone.
const SERVICE_NAME: &str = "service.name";

/// What a process names itself with no `service.name`, as OpenTelemetry does.
const UNKNOWN_SERVICE: &str = "unknown_service";

/// A run's id read from its own span: the same on every replica.
#[must_use]
pub fn run_of_span(project_id: &ProjectId, trace_id: &str, span_id: &str) -> Uuid {
    Uuid::new_v5(
        &NAMESPACE,
        format!("{project_id}/{trace_id}/{span_id}").as_bytes(),
    )
}

/// The run of a trace's calls with no run above them.
#[must_use]
pub fn run_of_trace(project_id: &ProjectId, trace_id: &str) -> Uuid {
    Uuid::new_v5(&NAMESPACE, format!("{project_id}/{trace_id}").as_bytes())
}

/// The span id of the run [`run_of_trace`] names, which no exporter sent.
fn run_span_of_trace(trace_id: &str) -> String {
    sha256_hex(format!("{trace_id}/run").as_bytes())
        .chars()
        .take(16)
        .collect()
}

/// What a held field is sealed under: never a kept field's associated data,
/// so one cannot be passed off as the other.
fn held_aad(span_id: &str, field: &str) -> String {
    format!("held/{}", content::field_aad(span_id, field))
}

/// An export's spans from another exporter, held for their trace's tree, each
/// field sealed under the project's key; a span held once is not held again.
/// Answers the spans refused past [`HELD_MAX`].
///
/// # Errors
///
/// Postgres or the key service did not answer: the export is retried.
pub async fn hold(
    state: &AppState,
    key: &ActingKey,
    mut held: Vec<(SpanRow, Vec<(String, String)>)>,
) -> Result<Vec<Refused>, TelmoniError> {
    if held.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = project_scope(&state.db, &key.project_id).await?;
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT 1 FROM telemetry.held_spans
                                WHERE project_id = $1 AND landed_at IS NULL LIMIT $2) AS w",
    )
    .bind(&key.project_id)
    .bind(HELD_MAX)
    .fetch_one(tx.conn())
    .await?;
    let room = usize::try_from(HELD_MAX.saturating_sub(waiting)).unwrap_or(0);
    let refused: Vec<Refused> = held
        .split_off(room.min(held.len()))
        .into_iter()
        .map(|(row, _)| Refused {
            span_id: row.span_id,
            reason: Refusal::Waiting,
        })
        .collect();
    let dek = if held.iter().any(|(_, fields)| !fields.is_empty()) {
        Some(content::key_for(state, &key.project_id).await?)
    } else {
        None
    };
    for (row, fields) in held {
        let (mut names, mut ciphertexts, mut nonces) = (Vec::new(), Vec::new(), Vec::new());
        if let Some(dek) = &dek {
            for (name, text) in fields {
                let aad = held_aad(&row.span_id, &name);
                let sealed = content::seal_field(&state.vault, dek, &Redacted::from(text), &aad)?;
                names.push(name);
                ciphertexts.push(sealed.ciphertext);
                nonces.push(sealed.nonce);
            }
        }
        let value =
            serde_json::to_value(&row).or_internal("foreign: a held span would not encode")?;
        sqlx::query(
            "INSERT INTO telemetry.held_spans
                 (project_id, trace_id, span_id, parent_span_id, received_at,
                  row, fields, ciphertexts, nonces)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT DO NOTHING",
        )
        .bind(&key.project_id)
        .bind(&row.trace_id)
        .bind(&row.span_id)
        .bind(&row.parent_span_id)
        .bind(row.received_at)
        .bind(value)
        .bind(&names)
        .bind(&ciphertexts)
        .bind(&nonces)
        .execute(tx.conn())
        .await?;
    }
    tx.commit().await?;
    Ok(refused)
}

/// A span already landed: the run it was placed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    pub span_id: String,
    pub run_id: Uuid,
    pub agent: String,
}

/// What a trace's held spans came to.
#[derive(Debug, Default, PartialEq)]
pub struct Placed {
    /// The spans to write, each in its run, and a run's own span the loop
    /// wrote for calls with no run above them.
    pub rows: Vec<SpanRow>,
    /// Each span placed, and that run's span, as the held rows mark them.
    pub runs: Vec<Landed>,
    /// The spans no run holds.
    pub dropped: Vec<String>,
    /// The spans whose parent has not arrived, left waiting for it.
    pub waiting: Vec<String>,
}

/// Where a held span goes.
enum Place {
    /// The run of a span among the held ones.
    Root(String),
    /// A run a span of it landed in before.
    Landed(Uuid, String),
    /// The run of the trace's calls with no run above them.
    Trace,
    Nowhere,
    /// Its parent has not arrived yet.
    Wait,
}

/// A trace's held spans placed under their topmost agent, else a call in the trace's
/// run of calls, else dropped; one whose parent has not arrived waits until `waited`.
#[must_use]
pub fn place(
    project_id: &ProjectId,
    trace_id: &str,
    waiting: Vec<SpanRow>,
    landed: &[Landed],
    waited: DateTime<Utc>,
) -> Placed {
    let by_id: HashMap<&str, &SpanRow> = waiting
        .iter()
        .map(|row| (row.span_id.as_str(), row))
        .collect();
    let landed_by_id: HashMap<&str, &Landed> = landed
        .iter()
        .map(|one| (one.span_id.as_str(), one))
        .collect();
    let places: Vec<Place> = waiting
        .iter()
        .map(|row| {
            let mut root = None;
            let mut at = row;
            let mut lost = false;
            // A loop in the tree is the sender's; it ends the walk.
            for _ in 0..=waiting.len() {
                if at.kind == SpanKind::Run {
                    root = Some(at.span_id.clone());
                }
                let parent = at.parent_span_id.as_str();
                if parent.is_empty() {
                    break;
                }
                if let Some(next) = by_id.get(parent) {
                    at = next;
                    continue;
                }
                if let Some(one) = landed_by_id.get(parent) {
                    return Place::Landed(one.run_id, one.agent.clone());
                }
                lost = true;
                break;
            }
            if lost && row.received_at >= waited {
                return Place::Wait;
            }
            match root {
                Some(root) => Place::Root(root),
                None if matches!(
                    row.kind,
                    SpanKind::ModelCall | SpanKind::ToolCall | SpanKind::Retrieval
                ) =>
                {
                    Place::Trace
                }
                None => Place::Nowhere,
            }
        })
        .collect();

    let trace_run = run_of_trace(project_id, trace_id);
    let trace_span = run_span_of_trace(trace_id);
    let service = waiting
        .iter()
        .find_map(|row| {
            row.attributes
                .iter()
                .find(|(key, _)| key == SERVICE_NAME)
                .map(|(_, name)| name.clone())
        })
        .unwrap_or_else(|| UNKNOWN_SERVICE.to_owned());
    let trace_agent = cut(&service);
    let names: HashMap<String, String> = waiting
        .iter()
        .filter(|row| row.kind == SpanKind::Run)
        .map(|row| (row.span_id.clone(), cut(&row.name)))
        .collect();

    let mut placed = Placed::default();
    let mut calls: Vec<SpanRow> = Vec::new();
    for (mut row, place) in waiting.into_iter().zip(places) {
        let (run_id, agent, of_trace) = match place {
            Place::Root(root) => {
                let agent = names.get(&root).cloned().unwrap_or_default();
                if row.span_id != root && row.kind == SpanKind::Run {
                    row.kind = SpanKind::Span;
                }
                (run_of_span(project_id, trace_id, &root), agent, false)
            }
            Place::Landed(run_id, agent) => {
                if row.kind == SpanKind::Run {
                    row.kind = SpanKind::Span;
                }
                (run_id, agent, false)
            }
            Place::Trace => (trace_run, trace_agent.clone(), true),
            Place::Nowhere => {
                placed.dropped.push(row.span_id);
                continue;
            }
            Place::Wait => {
                placed.waiting.push(row.span_id);
                continue;
            }
        };
        row.run_id = run_id;
        row.agent.clone_from(&agent);
        row.units = units_of(&row);
        placed.runs.push(Landed {
            span_id: row.span_id.clone(),
            run_id,
            agent,
        });
        if of_trace {
            calls.push(row);
        } else {
            placed.rows.push(row);
        }
    }
    if !calls.is_empty() {
        // A parent outside the run was a span no run keeps: the call hangs
        // from the run's own span instead.
        let mut ours: HashSet<String> = calls.iter().map(|call| call.span_id.clone()).collect();
        ours.extend(
            landed
                .iter()
                .filter(|one| one.run_id == trace_run)
                .map(|one| one.span_id.clone()),
        );
        ours.insert(trace_span.clone());
        for call in &mut calls {
            if !ours.contains(&call.parent_span_id) {
                call.parent_span_id.clone_from(&trace_span);
            }
        }
        // Written once: the store keeps no second copy to move its end.
        if let Some(first) = calls.first()
            && !landed.iter().any(|one| one.span_id == trace_span)
        {
            let run = run_row(trace_run, &trace_agent, &trace_span, first, &calls);
            placed.runs.push(Landed {
                span_id: trace_span.clone(),
                run_id: trace_run,
                agent: trace_agent.clone(),
            });
            placed.rows.push(run);
        }
        placed.rows.extend(calls);
    }
    placed
}

/// A name cut to an agent's cap.
fn cut(name: &str) -> String {
    name.chars().take(contract::AGENT_MAX).collect()
}

/// The span of a run of calls alone: from the first call's start to the last
/// one's end, failed where any failed, for the user and session they name.
fn run_row(
    run_id: Uuid,
    agent: &str,
    span_id: &str,
    first: &SpanRow,
    calls: &[SpanRow],
) -> SpanRow {
    let named = |pick: fn(&SpanRow) -> &String| {
        calls
            .iter()
            .map(pick)
            .find(|name| !name.is_empty())
            .cloned()
            .unwrap_or_default()
    };
    let failed = calls.iter().find(|c| c.status == SpanStatus::Error);
    let mut row = SpanRow {
        run_id,
        agent: agent.to_owned(),
        span_id: span_id.to_owned(),
        parent_span_id: String::new(),
        kind: SpanKind::Run,
        name: agent.to_owned(),
        started_at: calls
            .iter()
            .map(|c| c.started_at)
            .min()
            .unwrap_or(first.started_at),
        ended_at: calls
            .iter()
            .map(|c| c.ended_at)
            .max()
            .unwrap_or(first.ended_at),
        received_at: calls
            .iter()
            .map(|c| c.received_at)
            .min()
            .unwrap_or(first.received_at),
        status: if failed.is_some() {
            SpanStatus::Error
        } else {
            SpanStatus::Ok
        },
        error_type: failed.map(|c| c.error_type.clone()).unwrap_or_default(),
        model: String::new(),
        provider: String::new(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        cost: None,
        time_to_first_token: None,
        user_id: named(|c| &c.user_id),
        session_id: named(|c| &c.session_id),
        tags: Vec::new(),
        sample_rate: None,
        attributes: Vec::new(),
        sent: Vec::new(),
        ..first.clone()
    };
    row.units = units_of(&row);
    row
}

/// [`land_quiet`], every fifteen seconds, forever.
pub async fn run(state: Arc<AppState>) {
    state
        .heartbeats
        .every(&state.db, HEARTBEAT, EVERY, EVERY, || {
            let state = state.clone();
            async move { land_quiet(&state).await }
        })
        .await;
}

/// Each trace quiet a minute or waiting five landed, oldest first, and each landing
/// overdue retried; the day-old marks go. One trace that cannot land holds up no other.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn land_quiet(state: &Arc<AppState>) -> Result<(), TelmoniError> {
    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    sqlx::query("DELETE FROM telemetry.held_spans WHERE landed_at < now() - interval '1 day'")
        .execute(lane.conn())
        .await?;
    let traces: Vec<(ProjectId, String)> = sqlx::query_as(
        "SELECT project_id, trace_id
           FROM telemetry.held_spans
          WHERE landed_at IS NULL
          GROUP BY project_id, trace_id
         HAVING bool_or(landing IS NOT NULL AND tried_at < now() - make_interval(secs => $5))
             OR ((max(received_at) < now() - make_interval(secs => $1)
                  OR min(received_at) < now() - make_interval(secs => $2))
                 AND (max(tried_at) IS NULL OR max(received_at) > max(tried_at)
                      OR min(received_at) < now() - make_interval(secs => $3)))
          ORDER BY min(received_at)
          LIMIT $4",
    )
    .bind(QUIET_SECS)
    .bind(WAITED_MAX_SECS)
    .bind(ORPHAN_MAX_INTERVAL)
    .bind(TRACES_A_PASS)
    .bind(LANDING_RETRY_SECS)
    .fetch_all(lane.conn())
    .await?;
    lane.commit().await?;
    for (project_id, trace_id) in traces {
        if let Err(error) = land_trace(state, &project_id, &trace_id).await {
            tracing::warn!(
                project_id = %project_id,
                error = %error,
                "foreign: a trace's held spans could not land; next pass"
            );
        }
    }
    Ok(())
}

/// A held span's fields: their names, and each one's ciphertext and nonce.
type SealedFields = (Vec<String>, Vec<Vec<u8>>, Vec<Vec<u8>>);

/// A held span as its row is read back.
type HeldRow = (
    String,
    Option<String>,
    Vec<String>,
    Vec<Vec<u8>>,
    Vec<Vec<u8>>,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    bool,
);

/// The trace's held spans under the lock no other replica takes at once.
async fn locked(
    lane: &mut telmoni_shared::db::tenant_session::Scoped<
        '_,
        telmoni_shared::db::tenant_session::Maintenance<TelemetryLane>,
    >,
    project_id: &ProjectId,
    trace_id: &str,
) -> Result<Option<Vec<HeldRow>>, TelmoniError> {
    let ours: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("held/{project_id}/{trace_id}"))
            .fetch_one(lane.conn())
            .await?;
    if !ours {
        return Ok(None);
    }
    let held = sqlx::query_as(
        "SELECT span_id, row::text, fields, ciphertexts, nonces, run_id, agent, landing,
                landing IS NOT NULL AND tried_at > now() - make_interval(secs => $3)
           FROM telemetry.held_spans
          WHERE project_id = $1 AND trace_id = $2
          ORDER BY span_id",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(LANDING_RETRY_SECS)
    .fetch_all(lane.conn())
    .await?;
    Ok(Some(held))
}

/// One trace's held spans placed and landed, marked first with their landing's token
/// so a retry lands the same spans under it and the store keeps one copy.
async fn land_trace(
    state: &Arc<AppState>,
    project_id: &ProjectId,
    trace_id: &str,
) -> Result<(), TelmoniError> {
    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    // Read as the project stands now, so a moved project's spans are its
    // organization's, and content switched off since is not kept.
    let settings: Option<(OrganizationId, ContentMode)> = sqlx::query_as(
        "SELECT organization_id, content_mode FROM telemetry.project_settings
          WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(lane.conn())
    .await?;
    let Some((organization_id, mode)) = settings else {
        // Its project gone: what an export held as the purge ran goes too.
        sqlx::query("DELETE FROM telemetry.held_spans WHERE project_id = $1 AND trace_id = $2")
            .bind(project_id)
            .bind(trace_id)
            .execute(lane.conn())
            .await?;
        lane.commit().await?;
        return Ok(());
    };
    let Some(held) = locked(&mut lane, project_id, trace_id).await? else {
        return Ok(());
    };
    // The lock goes with the token's commit: a landing still writing is its replica's.
    if held.iter().any(|one| one.8) {
        return Ok(());
    }

    let mut waiting = Vec::new();
    let mut landed = Vec::new();
    let mut sealed: HashMap<String, SealedFields> = HashMap::new();
    let mut unreadable = Vec::new();
    let mut token = None;
    for (span_id, row, fields, ciphertexts, nonces, run_id, agent, landing, _) in held {
        match (row, run_id, agent) {
            (_, Some(run_id), Some(agent)) => landed.push(Landed {
                span_id,
                run_id,
                agent,
            }),
            // Read from text: the row's ids decode borrowed, as ClickHouse's are.
            (Some(row), None, _) => match serde_json::from_str::<SpanRow>(&row) {
                Ok(row) => {
                    // A landing begun holds its spans alone, under its token.
                    if landing.is_some() && token.is_none() {
                        waiting.clear();
                        token = landing.clone();
                    }
                    if landing == token || token.is_none() {
                        if !fields.is_empty() {
                            sealed.insert(span_id, (fields, ciphertexts, nonces));
                        }
                        waiting.push(row);
                    }
                }
                Err(_) => unreadable.push(span_id),
            },
            _ => unreadable.push(span_id),
        }
    }
    if !unreadable.is_empty() {
        tracing::warn!(
            project_id = %project_id,
            spans = unreadable.len(),
            "foreign: held spans this build cannot read are dropped"
        );
        sqlx::query(
            "DELETE FROM telemetry.held_spans
              WHERE project_id = $1 AND trace_id = $2 AND span_id = ANY($3)",
        )
        .bind(project_id)
        .bind(trace_id)
        .bind(&unreadable)
        .execute(lane.conn())
        .await?;
    }
    let waited = Utc::now() - chrono::Duration::seconds(ORPHAN_MAX_SECS);
    let mut placed = place(project_id, trace_id, waiting, &landed, waited);
    sqlx::query(
        "UPDATE telemetry.held_spans SET tried_at = now()
          WHERE project_id = $1 AND trace_id = $2 AND span_id = ANY($3)",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(&placed.waiting)
    .execute(lane.conn())
    .await?;
    let mut taken: Vec<String> = placed
        .runs
        .iter()
        .map(|one| one.span_id.clone())
        .chain(placed.dropped.iter().cloned())
        .collect();
    if taken.is_empty() {
        lane.commit().await?;
        return Ok(());
    }
    taken.sort();
    let token = match token {
        Some(token) => {
            sqlx::query(
                "UPDATE telemetry.held_spans SET tried_at = now()
                  WHERE project_id = $1 AND trace_id = $2 AND landing = $3",
            )
            .bind(project_id)
            .bind(trace_id)
            .bind(&token)
            .execute(lane.conn())
            .await?;
            token
        }
        None => {
            let token = format!(
                "held/{}",
                sha256_hex(format!("{project_id}/{trace_id}/{}", taken.join(",")).as_bytes())
            );
            sqlx::query(
                "UPDATE telemetry.held_spans SET landing = $4, tried_at = now()
                  WHERE project_id = $1 AND trace_id = $2 AND span_id = ANY($3)",
            )
            .bind(project_id)
            .bind(trace_id)
            .bind(&taken)
            .bind(&token)
            .execute(lane.conn())
            .await?;
            token
        }
    };
    lane.commit().await?;

    let kept = if mode == ContentMode::On {
        opened(state, project_id, &placed.rows, &sealed).await?
    } else {
        Vec::new()
    };
    let storage = crate::hashing::storage_for_project(state, project_id).await?;
    let mut batch = Batch {
        rows: std::mem::take(&mut placed.rows),
        content: kept,
        ..Batch::default()
    };
    crate::handler::ingest::land(
        state,
        &organization_id,
        project_id,
        &storage,
        Utc::now(),
        &mut batch,
        &token,
    )
    .await?;
    crate::scores::place_waiting(
        &state.store,
        project_id,
        trace_id,
        &batch.rows,
        &placed.dropped,
        &token,
    )
    .await;

    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    let (mut spans, mut runs, mut agents) = (Vec::new(), Vec::new(), Vec::new());
    for one in placed.runs {
        spans.push(one.span_id);
        runs.push(one.run_id);
        agents.push(one.agent);
    }
    // Each placed span, and a run's span the loop wrote, marked with its run.
    sqlx::query(
        "INSERT INTO telemetry.held_spans AS h
             (project_id, trace_id, span_id, parent_span_id, received_at,
              row, run_id, agent, landed_at)
         SELECT $1, $2, p.span_id, '', now(), NULL, p.run_id, p.agent, now()
           FROM unnest($3::text[], $4::uuid[], $5::text[]) AS p(span_id, run_id, agent)
         ON CONFLICT (project_id, trace_id, span_id) DO UPDATE
            SET row = NULL, fields = '{}', ciphertexts = '{}', nonces = '{}',
                landing = NULL, tried_at = NULL,
                run_id = excluded.run_id, agent = excluded.agent, landed_at = now()",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(&spans)
    .bind(&runs)
    .bind(&agents)
    .execute(lane.conn())
    .await?;
    sqlx::query(
        "DELETE FROM telemetry.held_spans
          WHERE project_id = $1 AND trace_id = $2 AND span_id = ANY($3)",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(&placed.dropped)
    .execute(lane.conn())
    .await?;
    lane.commit().await?;
    Ok(())
}

/// The placed spans' held content, opened to be kept again; a span whose
/// field will not open, its key gone with a purge, lands without it.
async fn opened(
    state: &AppState,
    project_id: &ProjectId,
    rows: &[SpanRow],
    sealed: &HashMap<String, SealedFields>,
) -> Result<Vec<SpanContent>, TelmoniError> {
    if !rows.iter().any(|row| sealed.contains_key(&row.span_id)) {
        return Ok(Vec::new());
    }
    let dek = content::key_for(state, project_id).await?;
    let mut kept = Vec::new();
    let mut lost = 0usize;
    for row in rows {
        let Some((names, ciphertexts, nonces)) = sealed.get(&row.span_id) else {
            continue;
        };
        let fields: Result<Vec<(String, String)>, TelmoniError> = names
            .iter()
            .zip(ciphertexts)
            .zip(nonces)
            .map(|((name, ciphertext), nonce)| {
                content::open_field(
                    &state.vault,
                    &dek,
                    SealedRef { ciphertext, nonce },
                    &held_aad(&row.span_id, name),
                )
                .map(|text| (name.clone(), text))
            })
            .collect();
        match fields {
            Ok(fields) => kept.push(SpanContent {
                run_id: row.run_id,
                trace_id: row.trace_id.clone(),
                span_id: row.span_id.clone(),
                fields,
            }),
            Err(_) => lost += 1,
        }
    }
    if lost > 0 {
        tracing::warn!(
            project_id = %project_id,
            spans = lost,
            "foreign: held content that no longer opens lands without it"
        );
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(id: &str, parent: &str, kind: SpanKind, name: &str) -> SpanRow {
        // To the microsecond, as ingest keeps a time.
        let now = chrono::SubsecRound::trunc_subsecs(Utc::now(), 6);
        SpanRow {
            project_id: ProjectId::new(),
            received_at: now,
            units: 1,
            run_id: Uuid::nil(),
            agent: String::new(),
            trace_id: "ab".repeat(16),
            span_id: id.into(),
            parent_span_id: parent.into(),
            kind,
            name: name.into(),
            started_at: now,
            ended_at: now,
            status: SpanStatus::Ok,
            error_type: String::new(),
            model: String::new(),
            provider: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
            cost: None,
            time_to_first_token: None,
            user_id: String::new(),
            session_id: String::new(),
            environment: String::new(),
            release: String::new(),
            tags: Vec::new(),
            sample_rate: None,
            attributes: vec![(SERVICE_NAME.into(), "checkout".into())],
            sent: Vec::new(),
        }
    }

    /// A wait every span has waited out, so none waits on a parent.
    fn waited_out() -> DateTime<Utc> {
        Utc::now() + chrono::Duration::days(1)
    }

    /// A held row reads back from Postgres's text as it was written.
    #[test]
    fn a_held_row_reads_back_as_it_was_written() {
        let row = span("a", "", SpanKind::Run, "planner");
        let value = serde_json::to_value(&row).expect("encoded").to_string();
        match serde_json::from_str::<SpanRow>(&value) {
            Ok(back) => assert_eq!(back, row),
            Err(error) => panic!("{error}: {value}"),
        }
    }

    /// The topmost agent is the run, a nested one a span of it; its calls and
    /// plain spans are its, and a span above it, in no run, is dropped.
    #[test]
    fn an_agent_with_none_above_it_is_the_run() {
        let (project, trace) = (ProjectId::new(), "ab".repeat(16));
        let placed = place(
            &project,
            &trace,
            vec![
                span("http", "", SpanKind::Span, "GET /chat"),
                span("agent", "http", SpanKind::Run, "planner"),
                span("sub", "agent", SpanKind::Run, "researcher"),
                span("call", "sub", SpanKind::ModelCall, "chat gpt"),
                span("db", "agent", SpanKind::Span, "SELECT"),
            ],
            &[],
            waited_out(),
        );
        let run = run_of_span(&project, &trace, "agent");
        assert_eq!(placed.dropped, vec!["http".to_owned()]);
        assert_eq!(placed.rows.len(), 4);
        assert!(
            placed
                .rows
                .iter()
                .all(|row| row.run_id == run && row.agent == "planner")
        );
        let kind = |id: &str| placed.rows.iter().find(|r| r.span_id == id).map(|r| r.kind);
        assert_eq!(kind("agent"), Some(SpanKind::Run));
        assert_eq!(kind("sub"), Some(SpanKind::Span), "one run's span a run");
        assert_ne!(
            run,
            run_of_span(&ProjectId::new(), &trace, "agent"),
            "another project's trace is another run"
        );
    }

    /// Calls and retrievals with no run above them are one run, named by the
    /// service, whose own span is written once, its calls under it; anything
    /// else is dropped, a plan among them.
    #[test]
    fn calls_with_no_run_above_them_are_one_run_of_the_service() {
        let (project, trace) = (ProjectId::new(), "ab".repeat(16));
        let mut failed = span("tool", "", SpanKind::ToolCall, "search");
        failed.status = SpanStatus::Error;
        failed.error_type = "TimeoutError".into();
        failed.user_id = "u1".into();
        let placed = place(
            &project,
            &trace,
            vec![
                span("http", "", SpanKind::Span, "POST /ask"),
                span("plan", "http", SpanKind::Step, "plan"),
                span("found", "http", SpanKind::Retrieval, "docs-index"),
                span("call", "http", SpanKind::ModelCall, "chat gpt"),
                failed,
            ],
            &[],
            waited_out(),
        );
        let run_span = run_span_of_trace(&trace);
        assert_eq!(placed.dropped, vec!["http".to_owned(), "plan".to_owned()]);
        let run = placed
            .rows
            .iter()
            .find(|row| row.kind == SpanKind::Run)
            .expect("the run's own span");
        assert_eq!(
            (run.span_id.as_str(), run.agent.as_str()),
            (run_span.as_str(), "checkout")
        );
        assert_eq!(run.run_id, run_of_trace(&project, &trace));
        assert_eq!(
            (run.status, run.error_type.as_str(), run.user_id.as_str()),
            (SpanStatus::Error, "TimeoutError", "u1"),
            "it fails as its calls did, for whom they name"
        );
        for id in ["found", "call", "tool"] {
            let call = placed.rows.iter().find(|r| r.span_id == id).expect("kept");
            assert_eq!(call.parent_span_id, run_span, "{id} hangs from the run");
            assert_eq!(call.run_id, run.run_id);
        }

        // A late call, its parent waited out, joins the same run; its span is not written again.
        let later = place(
            &project,
            &trace,
            vec![span("late", "http", SpanKind::ModelCall, "chat gpt")],
            &placed.runs,
            waited_out(),
        );
        assert_eq!(later.rows.len(), 1);
        assert_eq!(later.rows[0].run_id, run.run_id);
    }

    /// A child landing after its run did is placed in that run.
    #[test]
    fn a_late_child_joins_the_run_its_parent_landed_in() {
        let (project, trace) = (ProjectId::new(), "ab".repeat(16));
        let run = run_of_span(&project, &trace, "agent");
        let placed = place(
            &project,
            &trace,
            vec![span("tool", "agent", SpanKind::ToolCall, "search")],
            &[Landed {
                span_id: "agent".into(),
                run_id: run,
                agent: "planner".into(),
            }],
            waited_out(),
        );
        assert!(placed.dropped.is_empty());
        assert_eq!(
            (placed.rows[0].run_id, placed.rows[0].agent.as_str()),
            (run, "planner")
        );
    }

    /// An agent's children, sent before it ends, wait for it rather than land
    /// as calls alone; past the wait, a call lands in the trace's run.
    #[test]
    fn children_wait_for_a_parent_not_yet_arrived() {
        let (project, trace) = (ProjectId::new(), "ab".repeat(16));
        let early = || {
            vec![
                span("call", "agent", SpanKind::ModelCall, "chat gpt"),
                span("db", "agent", SpanKind::Span, "SELECT"),
            ]
        };
        let placed = place(
            &project,
            &trace,
            early(),
            &[],
            Utc::now() - chrono::Duration::hours(1),
        );
        assert!(placed.rows.is_empty() && placed.dropped.is_empty());
        assert_eq!(placed.waiting, ["call", "db"]);

        let gave_up = place(&project, &trace, early(), &[], waited_out());
        assert_eq!(gave_up.dropped, ["db"]);
        assert!(
            gave_up
                .rows
                .iter()
                .any(|r| r.span_id == "call" && r.run_id == run_of_trace(&project, &trace))
        );
    }
}
