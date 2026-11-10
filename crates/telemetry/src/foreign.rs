//! Another exporter's spans, with no `telmoni.agent`: held at the door
//! ([`hold`]), then each named for the agent its trace's tree puts it under
//! and landed by a loop on every replica ([`run`], [`place`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

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
/// before its parent, so an agent's own span comes last, after its calls.
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

/// The resource's name of the process, which names a trace with no agent's
/// span of its own.
const SERVICE_NAME: &str = "service.name";

/// What a process names itself with no `service.name`, as OpenTelemetry does.
const UNKNOWN_SERVICE: &str = "unknown_service";

/// The span id of the agent's span the loop writes for a trace whose
/// exporter sent none, which no exporter sent.
fn agent_span_of_trace(trace_id: &str) -> String {
    sha256_hex(format!("{trace_id}/agent").as_bytes())
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

/// A span already landed: the agent its children are placed under, its own
/// name where it was an agent's span. The trace's own agent is kept under
/// the span id the loop would write one with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    pub span_id: String,
    pub agent: String,
}

/// What a trace's held spans came to.
#[derive(Debug, Default, PartialEq)]
pub struct Placed {
    /// The spans to write, each named for its agent, and an agent's span the
    /// loop wrote for a trace whose exporter sent none.
    pub rows: Vec<SpanRow>,
    /// Each span placed, and that agent's span, as the held rows mark them.
    pub placed: Vec<Landed>,
    /// Another exporter's agents nested under the trace's own, written as
    /// plain spans, so the trace has one agent's span.
    pub nested: Vec<String>,
    /// The spans whose parent has not arrived, left waiting for it.
    pub waiting: Vec<String>,
}

/// A trace's held spans, each named for the nearest agent above it, else the
/// trace's own: the topmost agent's name, or the service's for a trace with
/// no agent's span at all, which then gets one the loop writes. Every other
/// agent's span, nested, beside it or arriving later, becomes a plain span,
/// so the trace keeps one; a span whose parent has not arrived waits until
/// `waited`.
#[must_use]
pub fn place(
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
    // Each span's agents above it, nearest first, as far as the tree is known;
    // `None` where its parent has not arrived and it has not waited out.
    let above: Vec<Option<Vec<String>>> = waiting
        .iter()
        .map(|row| {
            let mut agents = Vec::new();
            let mut at = row;
            // A loop in the tree is the sender's; it ends the walk.
            for _ in 0..=waiting.len() {
                let parent = at.parent_span_id.as_str();
                if parent.is_empty() {
                    return Some(agents);
                }
                if let Some(next) = by_id.get(parent) {
                    if next.kind == SpanKind::Agent {
                        agents.push(cut(&next.name));
                    }
                    at = next;
                    continue;
                }
                if let Some(one) = landed_by_id.get(parent) {
                    agents.push(one.agent.clone());
                    return Some(agents);
                }
                return (row.received_at < waited).then_some(agents);
            }
            Some(agents)
        })
        .collect();
    let mut placed = Placed::default();
    let written_span = agent_span_of_trace(trace_id);
    // A trace placed before has its agent's span already, and its name on
    // the mark that placement left.
    let settled = landed
        .iter()
        .find(|one| one.span_id == written_span)
        .or_else(|| landed.first())
        .map(|one| one.agent.clone());
    // Else its one agent's span: of those with no agent above, the first to
    // start, so siblings and the order they were read in change nothing.
    let chosen = if settled.is_some() {
        None
    } else {
        waiting
            .iter()
            .zip(&above)
            .filter(|(row, agents)| {
                row.kind == SpanKind::Agent && agents.as_ref().is_some_and(Vec::is_empty)
            })
            .map(|(row, _)| row)
            .min_by(|a, b| (a.started_at, &a.span_id).cmp(&(b.started_at, &b.span_id)))
            .map(|row| (row.span_id.clone(), cut(&row.name)))
    };
    // The trace's own agent: the one placed before, the one chosen, else the service's.
    let own = settled
        .clone()
        .or_else(|| chosen.as_ref().map(|(_, name)| name.clone()))
        .unwrap_or_else(|| {
            let service = waiting
                .iter()
                .find_map(|row| {
                    row.attributes
                        .iter()
                        .find(|(key, _)| key == SERVICE_NAME)
                        .map(|(_, name)| name.clone())
                })
                .unwrap_or_else(|| UNKNOWN_SERVICE.to_owned());
            cut(&service)
        });
    let has_agent = settled.is_some() || chosen.is_some();
    let mut rows = Vec::new();
    for (mut row, agents) in waiting.into_iter().zip(above) {
        let Some(agents) = agents else {
            placed.waiting.push(row.span_id);
            continue;
        };
        row.agent = agents.first().cloned().unwrap_or_else(|| own.clone());
        // What sits under an agent's span is that agent's, demoted or not.
        let mut below = row.agent.clone();
        if row.kind == SpanKind::Agent {
            below = cut(&row.name);
            if chosen.as_ref().is_none_or(|(id, _)| *id != row.span_id) {
                row.kind = SpanKind::Span;
                placed.nested.push(row.span_id.clone());
            }
        }
        // A trace with no agent's span hangs its roots from the one the loop writes.
        if !has_agent && row.parent_span_id.is_empty() {
            row.parent_span_id.clone_from(&written_span);
        }
        row.units = units_of(&row);
        placed.placed.push(Landed {
            span_id: row.span_id.clone(),
            agent: below,
        });
        rows.push(row);
    }
    // The first placement marks the trace with its agent's name, and writes
    // the agent's span where the exporter sent none.
    if settled.is_none() && !rows.is_empty() {
        if chosen.is_none()
            && let Some(agent) = agent_row(&own, &written_span, &rows)
        {
            placed.rows.push(agent);
        }
        placed.placed.push(Landed {
            span_id: written_span,
            agent: own,
        });
    }
    placed.rows.extend(rows);
    placed
}

/// A name cut to an agent's cap.
fn cut(name: &str) -> String {
    name.chars().take(contract::AGENT_MAX).collect()
}

/// The agent's span of a trace whose exporter sent none: from the first
/// span's start to the last one's end, failed where any failed, for the user
/// and session they name; none where no span landed.
fn agent_row(agent: &str, span_id: &str, spans: &[SpanRow]) -> Option<SpanRow> {
    let first = spans.first()?;
    let named = |pick: fn(&SpanRow) -> &String| {
        spans
            .iter()
            .map(pick)
            .find(|name| !name.is_empty())
            .cloned()
            .unwrap_or_default()
    };
    let failed = spans.iter().find(|c| c.status == SpanStatus::Error);
    let mut row = SpanRow {
        agent: agent.to_owned(),
        span_id: span_id.to_owned(),
        parent_span_id: String::new(),
        kind: SpanKind::Agent,
        name: agent.to_owned(),
        started_at: spans
            .iter()
            .map(|c| c.started_at)
            .min()
            .unwrap_or(first.started_at),
        ended_at: spans
            .iter()
            .map(|c| c.ended_at)
            .max()
            .unwrap_or(first.ended_at),
        received_at: spans
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
        fault: failed.and_then(|c| c.fault),
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
    Some(row)
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
        "SELECT span_id, row::text, fields, ciphertexts, nonces, agent, landing,
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
    if held.iter().any(|one| one.7) {
        return Ok(());
    }

    let mut waiting = Vec::new();
    let mut landed = Vec::new();
    let mut sealed: HashMap<String, SealedFields> = HashMap::new();
    let mut unreadable = Vec::new();
    let mut token = None;
    for (span_id, row, fields, ciphertexts, nonces, agent, landing, _) in held {
        match (row, agent) {
            (_, Some(agent)) => landed.push(Landed { span_id, agent }),
            // Read from text: the row's ids decode borrowed, as ClickHouse's are.
            (Some(row), None) => match serde_json::from_str::<SpanRow>(&row) {
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
    let mut placed = place(trace_id, waiting, &landed, waited);
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
        .placed
        .iter()
        .map(|one| one.span_id.clone())
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
    crate::scores::demote(&state.store, project_id, trace_id, &placed.nested, &token).await;

    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    let (mut spans, mut agents) = (Vec::new(), Vec::new());
    for one in placed.placed {
        spans.push(one.span_id);
        agents.push(one.agent);
    }
    // Each placed span, and an agent's span the loop wrote, marked with its agent.
    sqlx::query(
        "INSERT INTO telemetry.held_spans AS h
             (project_id, trace_id, span_id, parent_span_id, received_at,
              row, agent, landed_at)
         SELECT $1, $2, p.span_id, '', now(), NULL, p.agent, now()
           FROM unnest($3::text[], $4::text[]) AS p(span_id, agent)
         ON CONFLICT (project_id, trace_id, span_id) DO UPDATE
            SET row = NULL, fields = '{}', ciphertexts = '{}', nonces = '{}',
                landing = NULL, tried_at = NULL,
                agent = excluded.agent, landed_at = now()",
    )
    .bind(project_id)
    .bind(trace_id)
    .bind(&spans)
    .bind(&agents)
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
            fault: None,
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
        let row = span("a", "", SpanKind::Agent, "planner");
        let value = serde_json::to_value(&row).expect("encoded").to_string();
        match serde_json::from_str::<SpanRow>(&value) {
            Ok(back) => assert_eq!(back, row),
            Err(error) => panic!("{error}: {value}"),
        }
    }

    /// The topmost agent is the trace's; a nested one is a plain span, and
    /// what sits under it is named for it; a span above the agent is the
    /// trace's too, named for its agent.
    #[test]
    fn the_topmost_agent_names_the_trace_and_each_span_its_nearest() {
        let trace = "ab".repeat(16);
        let placed = place(
            &trace,
            vec![
                span("http", "", SpanKind::Span, "GET /chat"),
                span("agent", "http", SpanKind::Agent, "planner"),
                span("sub", "agent", SpanKind::Agent, "researcher"),
                span("call", "sub", SpanKind::ModelCall, "chat gpt"),
                span("db", "agent", SpanKind::Span, "SELECT"),
            ],
            &[],
            waited_out(),
        );
        assert!(placed.waiting.is_empty());
        assert_eq!(placed.rows.len(), 5, "every span of the trace lands");
        let of = |id: &str| {
            placed
                .rows
                .iter()
                .find(|r| r.span_id == id)
                .map(|r| (r.kind, r.agent.as_str()))
        };
        assert_eq!(of("http"), Some((SpanKind::Span, "planner")));
        assert_eq!(of("agent"), Some((SpanKind::Agent, "planner")));
        assert_eq!(
            of("sub"),
            Some((SpanKind::Span, "planner")),
            "a nested agent is a span, under the agent above it"
        );
        assert_eq!(of("call"), Some((SpanKind::ModelCall, "researcher")));
        assert_eq!(of("db"), Some((SpanKind::Span, "planner")));
        assert_eq!(placed.nested, ["sub"]);
        assert_eq!(
            placed
                .rows
                .iter()
                .filter(|r| r.kind == SpanKind::Agent)
                .count(),
            1,
            "one agent's span a trace"
        );
    }

    /// A trace with no agent's span is named by the service, which gets an
    /// agent's span written once, its roots hanging from it.
    #[test]
    fn a_trace_with_no_agent_is_the_services_with_a_span_written_once() {
        let trace = "ab".repeat(16);
        let mut failed = span("tool", "", SpanKind::ToolCall, "search");
        failed.status = SpanStatus::Error;
        failed.error_type = "TimeoutError".into();
        failed.user_id = "u1".into();
        let placed = place(
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
        let written = agent_span_of_trace(&trace);
        let agent = placed
            .rows
            .iter()
            .find(|row| row.kind == SpanKind::Agent)
            .expect("the agent's span");
        assert_eq!(
            (agent.span_id.as_str(), agent.agent.as_str()),
            (written.as_str(), "checkout")
        );
        assert_eq!(
            (
                agent.status,
                agent.error_type.as_str(),
                agent.user_id.as_str()
            ),
            (SpanStatus::Error, "TimeoutError", "u1"),
            "it fails as its spans did, for whom they name"
        );
        for id in ["http", "tool"] {
            let root = placed.rows.iter().find(|r| r.span_id == id).expect("kept");
            assert_eq!(
                root.parent_span_id, written,
                "{id} hangs from the agent's span"
            );
        }
        assert!(
            placed.rows.iter().all(|r| r.agent == "checkout"),
            "every span is the service's"
        );
        assert_eq!(placed.rows.len(), 6);

        // A late span, its parent waited out, joins the same trace; the span is not written again.
        let later = place(
            &trace,
            vec![span("late", "http", SpanKind::ModelCall, "chat gpt")],
            &placed.placed,
            waited_out(),
        );
        assert_eq!(later.rows.len(), 1);
        assert_eq!(later.rows[0].agent, "checkout");
    }

    /// A child landing after its parent did is named for the parent's agent.
    #[test]
    fn a_late_child_is_named_for_the_agent_its_parent_landed_under() {
        let trace = "ab".repeat(16);
        let placed = place(
            &trace,
            vec![span("tool", "agent", SpanKind::ToolCall, "search")],
            &[Landed {
                span_id: "agent".into(),
                agent: "planner".into(),
            }],
            waited_out(),
        );
        assert!(placed.waiting.is_empty());
        assert_eq!(placed.rows.len(), 1);
        assert_eq!(placed.rows[0].agent, "planner");
        assert!(placed.nested.is_empty());
    }

    /// Two agents side by side under a request are one trace: the first to
    /// start is its agent's span, whatever order they are read in.
    #[test]
    fn sibling_agents_leave_one_agents_span() {
        let trace = "ab".repeat(16);
        let mut second = span("b", "http", SpanKind::Agent, "writer");
        second.started_at += chrono::Duration::seconds(1);
        let spans = || {
            vec![
                span("http", "", SpanKind::Span, "GET /chat"),
                second.clone(),
                span("a", "http", SpanKind::Agent, "planner"),
                span("call", "b", SpanKind::ModelCall, "chat gpt"),
            ]
        };
        let mut reversed = spans();
        reversed.reverse();
        for waiting in [spans(), reversed] {
            let placed = place(&trace, waiting, &[], waited_out());
            let of = |id: &str| {
                placed
                    .rows
                    .iter()
                    .find(|r| r.span_id == id)
                    .map(|r| (r.kind, r.agent.as_str()))
            };
            assert_eq!(of("a"), Some((SpanKind::Agent, "planner")));
            assert_eq!(of("b"), Some((SpanKind::Span, "planner")));
            assert_eq!(of("call"), Some((SpanKind::ModelCall, "writer")));
            assert_eq!(of("http"), Some((SpanKind::Span, "planner")));
            assert_eq!(placed.nested, ["b"]);
        }
    }

    /// A span landing after its trace was placed is named as it would have
    /// been beside it: a nested agent's child for that agent, a root for the
    /// trace's, and a late agent's span is a plain span.
    #[test]
    fn a_late_span_is_placed_as_it_would_have_been_with_the_rest() {
        let trace = "ab".repeat(16);
        let first = place(
            &trace,
            vec![
                span("agent", "", SpanKind::Agent, "planner"),
                span("sub", "agent", SpanKind::Agent, "researcher"),
            ],
            &[],
            waited_out(),
        );
        let later = place(
            &trace,
            vec![
                span("call", "sub", SpanKind::ModelCall, "chat gpt"),
                span("root", "", SpanKind::Span, "cleanup"),
                span("other", "", SpanKind::Agent, "reviewer"),
            ],
            &first.placed,
            waited_out(),
        );
        let of = |id: &str| {
            later
                .rows
                .iter()
                .find(|r| r.span_id == id)
                .map(|r| (r.kind, r.agent.as_str()))
        };
        assert_eq!(of("call"), Some((SpanKind::ModelCall, "researcher")));
        assert_eq!(of("root"), Some((SpanKind::Span, "planner")));
        assert_eq!(of("other"), Some((SpanKind::Span, "planner")));
        assert_eq!(later.rows.len(), 3, "no agent's span is written again");
    }

    /// An agent's children, sent before it ends, wait for it rather than
    /// land under the service's name; past the wait, they land.
    #[test]
    fn children_wait_for_a_parent_not_yet_arrived() {
        let trace = "ab".repeat(16);
        let early = || {
            vec![
                span("call", "agent", SpanKind::ModelCall, "chat gpt"),
                span("db", "agent", SpanKind::Span, "SELECT"),
            ]
        };
        let placed = place(
            &trace,
            early(),
            &[],
            Utc::now() - chrono::Duration::hours(1),
        );
        assert!(placed.rows.is_empty());
        assert_eq!(placed.waiting, ["call", "db"]);

        let gave_up = place(&trace, early(), &[], waited_out());
        assert!(gave_up.waiting.is_empty());
        assert!(
            gave_up
                .rows
                .iter()
                .any(|r| r.span_id == "call" && r.agent == "checkout")
        );
        assert!(gave_up.rows.iter().any(|r| r.kind == SpanKind::Agent));
    }
}
