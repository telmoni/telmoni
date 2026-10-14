//! The outbox: what telemetry hands on once a transaction has committed,
//! and must not lose. A ping's finished run becomes a row of ClickHouse's
//! `spans`, and a notice for a project's feed is handed to notifications
//! through the seam; each is written as a row here in the transaction that
//! ends the run or makes the monitor, so a monitor is never late because
//! ClickHouse was down, and nothing is told twice: the seam answers a
//! replayed dedup key with its first row and queues nothing.
//!
//! The loop is one every replica runs, on the delivery loop's pattern: each
//! tick leases rows across every tenant, `FOR UPDATE SKIP LOCKED` under a
//! lease, lands each, deletes what landed and holds the rest for a while
//! before they are tried again — up to a cap, past which a row stays for an
//! operator to see and the depth line counts it.
//!
//! ⚠ The loop's handle is raced against the server in the binary's `serve`,
//! as the delivery loop's is: a dead loop would leave a Ready pod with every
//! ping's row unwritten, so the process exits and the restart is the
//! recovery.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, maintenance_scope};
use telmoni_shared::digest::hex_lower;
use telmoni_shared::seam::Notice;
use telmoni_shared::{
    NotificationKind, OrganizationId, ProjectId, SpanKind, SpanStatus, TelmoniError,
};

use crate::contract::{EXIT_CODE, RUN_COST_USD, RUN_STEPS};
use crate::db::{self, OutboxRow, TelemetryLane};
use crate::ingest::live;
use crate::store::SpanRow;
use crate::{AppState, store};

/// How often the loop looks for rows: the delivery loop's poll, so a ping's
/// row lands within seconds.
const POLL_SECS: u64 = 5;
/// How many rows one lease takes.
const BATCH: i64 = 100;
/// How many batches one tick drains before it waits for the next.
const DRAIN_ROUNDS: usize = 20;
/// How long a leased row is held before another replica may take it:
/// longer than an insert's deadline and a notice's write, so a row is never
/// landed twice by two replicas.
const LEASE_SECS: i64 = 120;
/// How many times a row is tried before it is left for an operator: with
/// the waits below, about an hour of a store that will not take it.
const MAX_ATTEMPTS: i32 = 12;
/// The wait before a row that did not land is tried again: five seconds,
/// doubled each time, up to five minutes.
const RETRY_FLOOR_SECS: i64 = 5;
const RETRY_CEILING_SECS: i64 = 300;
/// How often the depth line is written: the delivery loop's minute.
const DEPTH_EVERY_TICKS: u64 = 60 / POLL_SECS;

/// The `kind` of a row carrying a finished run.
pub const KIND_SPAN: &str = "span";
/// The `kind` of a row carrying a notice.
pub const KIND_NOTICE: &str = "notice";

/// A run that ended, as its row carries it until it is a row of `spans`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishedRun {
    pub run_id: Uuid,
    /// The monitor's agent: the span's name, and what it is counted under.
    pub agent: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    /// The server's clock when the ending ping arrived: the span's partition
    /// day, however long the row waited here.
    pub received_at: DateTime<Utc>,
    pub status: SpanStatus,
    /// What a `/fail` reported.
    pub exit_code: Option<i32>,
    /// What its `/checkpoint`s reported last, where they reported anything.
    pub steps: Option<i32>,
    pub cost_usd: Option<f64>,
}

impl FinishedRun {
    /// The run's row of `spans`: the span of kind `run`, named for its agent,
    /// its ids from its run id — the trace's the id's bytes, the span's the
    /// last eight — since a ping's run is in no trace of a sender's; its
    /// progress and its exit code as attributes, numbers as their text; and
    /// the bounds it declared none of, since a `curl` run is held to its
    /// monitor's alone. Counting no unit: a ping is not a span (`PLANS.md`,
    /// *What a span is*) — the plan counts its monitor, and the ping ceiling
    /// bounds its runs — so the row costs its bytes on disk and nothing
    /// against the grant.
    #[must_use]
    pub fn row(&self, project_id: &ProjectId) -> SpanRow {
        let mut attributes = Vec::with_capacity(3);
        if let Some(steps) = self.steps {
            attributes.push((RUN_STEPS.to_owned(), steps.to_string()));
        }
        if let Some(cost) = self.cost_usd {
            attributes.push((RUN_COST_USD.to_owned(), cost.to_string()));
        }
        if let Some(code) = self.exit_code {
            attributes.push((EXIT_CODE.to_owned(), code.to_string()));
        }
        SpanRow {
            project_id: project_id.clone(),
            received_at: self.received_at,
            units: 0,
            run_id: self.run_id,
            agent: self.agent.clone(),
            trace_id: self.run_id.simple().to_string(),
            span_id: hex_lower(&self.run_id.as_bytes()[8..]),
            parent_span_id: String::new(),
            kind: SpanKind::Run,
            name: self.agent.clone(),
            started_at: self.started_at,
            ended_at: self.ended_at,
            status: self.status,
            error_type: String::new(),
            model: String::new(),
            provider: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
            user_id: String::new(),
            session_id: String::new(),
            tags: Vec::new(),
            bound_stall_after_s: None,
            bound_max_steps: None,
            bound_max_cost_usd: None,
            sample_rate: None,
            attributes,
        }
    }
}

/// A notice for a project's feed, as its row carries it until the seam has
/// taken it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedNotice {
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    pub metadata: serde_json::Value,
    /// The seam's dedup key, `telemetry:<what>:<id>:<state>`, so a row
    /// handed over twice — a loop that died between the seam's answer and
    /// the row's delete — is one notice.
    pub dedup_key: String,
}

/// A finished run put in the outbox, in the transaction that ended it.
pub async fn enqueue_run(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    run: &FinishedRun,
) -> Result<(), TelmoniError> {
    let payload = serde_json::to_value(run)
        .map_err(|e| TelmoniError::internal("a finished run did not encode", e))?;
    db::enqueue_outbox(
        tx,
        project_id,
        organization_id,
        Uuid::now_v7(),
        KIND_SPAN,
        &payload,
    )
    .await?;
    Ok(())
}

/// A notice put in the outbox, in the transaction of what it tells of.
pub async fn enqueue_notice(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    notice: &QueuedNotice,
) -> Result<(), TelmoniError> {
    let payload = serde_json::to_value(notice)
        .map_err(|e| TelmoniError::internal("a notice did not encode", e))?;
    db::enqueue_outbox(
        tx,
        project_id,
        organization_id,
        Uuid::now_v7(),
        KIND_NOTICE,
        &payload,
    )
    .await?;
    Ok(())
}

/// What one tick, or one batch, came to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settled {
    /// Rows that landed and are gone.
    pub landed: u32,
    /// Rows that did not, held for their next try.
    pub held: u32,
}

/// The outbox loop. A failed tick is logged and the next one runs.
pub async fn run(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(POLL_SECS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // skip the immediate first tick
    let mut ticks = 0;
    loop {
        ticker.tick().await;
        match tick_once(&state).await {
            Ok(Settled { landed: 0, held: 0 }) => {}
            Ok(settled) => tracing::debug!(
                landed = settled.landed,
                held = settled.held,
                "telemetry outbox tick"
            ),
            Err(e) => {
                tracing::warn!(error = %e, "telemetry outbox tick failed; the next one retries");
            }
        }

        ticks += 1;
        if ticks >= DEPTH_EVERY_TICKS {
            ticks = 0;
            match depth(&state).await {
                Ok((pending, stuck, oldest_age_secs)) => tracing::info!(
                    pending,
                    stuck,
                    oldest_age_secs = oldest_age_secs.unwrap_or(0),
                    "telemetry outbox depth"
                ),
                Err(e) => tracing::warn!(error = %e, "telemetry outbox depth read failed"),
            }
        }
    }
}

/// What the loop logs once a minute as `telemetry outbox depth`: the rows
/// still to land, those past their tries, and the oldest's age in seconds.
pub async fn depth(state: &AppState) -> Result<(i64, i64, Option<i64>), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let depth = db::outbox_depth(&mut tx, MAX_ATTEMPTS).await?;
    tx.commit().await?;
    Ok(depth)
}

/// One tick: the outbox drained batch after batch, until a batch comes back
/// short or the rounds run out. What landed and what was held, summed.
pub async fn tick_once(state: &Arc<AppState>) -> Result<Settled, TelmoniError> {
    let mut total = Settled::default();
    for _ in 0..DRAIN_ROUNDS {
        let settled = drain_batch(state).await?;
        total.landed = total.landed.saturating_add(settled.landed);
        total.held = total.held.saturating_add(settled.held);
        if i64::from(settled.landed.saturating_add(settled.held)) < BATCH {
            break;
        }
    }
    Ok(total)
}

/// Lease one batch and land it: the runs in one insert, the notices one by
/// one through the seam; then, in one transaction, what landed deleted and
/// the rest held for their next try.
async fn drain_batch(state: &Arc<AppState>) -> Result<Settled, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let leased = db::lease_outbox(&mut tx, BATCH, LEASE_SECS, MAX_ATTEMPTS).await?;
    tx.commit().await?;
    if leased.is_empty() {
        return Ok(Settled::default());
    }

    let mut landed: Vec<Uuid> = Vec::new();
    let mut held: Vec<(Uuid, i32)> = Vec::new();
    let (runs, notices): (Vec<OutboxRow>, Vec<OutboxRow>) =
        leased.into_iter().partition(|row| row.kind == KIND_SPAN);
    land_runs(state, runs, &mut landed, &mut held).await;
    for row in notices {
        land_notice(state, row, &mut landed, &mut held).await;
    }

    let settled = Settled {
        landed: u32::try_from(landed.len()).unwrap_or(u32::MAX),
        held: u32::try_from(held.len()).unwrap_or(u32::MAX),
    };
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    if !landed.is_empty() {
        db::delete_outbox(&mut tx, &landed).await?;
    }
    for (id, attempts) in held {
        db::hold_outbox(&mut tx, id, retry_wait(attempts)).await?;
    }
    tx.commit().await?;
    Ok(settled)
}

/// How long a row waits after its `attempts`th try: doubled each time, from
/// the floor to the ceiling.
fn retry_wait(attempts: i32) -> i64 {
    let doublings = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(16);
    RETRY_FLOOR_SECS
        .saturating_mul(1_i64 << doublings)
        .min(RETRY_CEILING_SECS)
}

/// The batch's finished runs, as rows of `spans` in one insert, and the
/// organization's open consoles told of them once it holds them.
async fn land_runs(
    state: &Arc<AppState>,
    rows: Vec<OutboxRow>,
    landed: &mut Vec<Uuid>,
    held: &mut Vec<(Uuid, i32)>,
) {
    let mut spans = Vec::with_capacity(rows.len());
    let mut ids = Vec::with_capacity(rows.len());
    let mut nudges: BTreeMap<(String, String), Vec<Uuid>> = BTreeMap::new();
    for row in rows {
        match serde_json::from_value::<FinishedRun>(row.payload) {
            Ok(run) => {
                nudges
                    .entry((row.organization_id.to_string(), row.project_id.to_string()))
                    .or_default()
                    .push(run.run_id);
                spans.push(run.row(&row.project_id));
                ids.push((row.id, row.attempts));
            }
            Err(error) => {
                // A row no version of this code wrote: held and said so,
                // since dropping it would lose a run in silence.
                tracing::error!(
                    outbox_id = %row.id,
                    error = %error,
                    "telemetry outbox: a finished run does not parse; the row is held"
                );
                held.push((row.id, row.attempts));
            }
        }
    }
    if spans.is_empty() {
        return;
    }
    match state.store.insert_spans(&spans).await {
        Ok(()) => {
            landed.extend(ids.iter().map(|(id, _)| *id));
            for ((organization, project), runs) in nudges {
                live::tell(Arc::clone(state), &organization, &project, runs);
            }
        }
        Err(error) => {
            // ⚠ The error's text can carry ClickHouse's own message: its
            // kind alone is logged.
            let kind = store::error_kind(&error);
            if kind == "ours" {
                tracing::error!(
                    kind,
                    rows = spans.len(),
                    "telemetry outbox: ClickHouse refused the runs' rows; they are held"
                );
            } else {
                tracing::warn!(
                    kind,
                    rows = spans.len(),
                    "telemetry outbox: ClickHouse did not take the runs' rows; they are held"
                );
            }
            held.extend(ids);
        }
    }
}

/// One notice handed through the seam, under its dedup key.
async fn land_notice(
    state: &AppState,
    row: OutboxRow,
    landed: &mut Vec<Uuid>,
    held: &mut Vec<(Uuid, i32)>,
) {
    let notice = match serde_json::from_value::<QueuedNotice>(row.payload) {
        Ok(notice) => notice,
        Err(error) => {
            tracing::error!(
                outbox_id = %row.id,
                error = %error,
                "telemetry outbox: a notice does not parse; the row is held"
            );
            held.push((row.id, row.attempts));
            return;
        }
    };
    let handed = state
        .notifications
        .emit(
            &row.organization_id,
            Some(&row.project_id),
            Notice {
                kind: notice.kind,
                subject_user_id: None,
                title: &notice.title,
                body: &notice.body,
                metadata: notice.metadata.clone(),
                dedup_key: Some(&notice.dedup_key),
            },
        )
        .await;
    match handed {
        Ok(_) => landed.push(row.id),
        Err(error) => {
            tracing::warn!(
                outbox_id = %row.id,
                project_id = %row.project_id,
                error = %error,
                "telemetry outbox: notifications did not take a notice; the row is held"
            );
            held.push((row.id, row.attempts));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ping's run is a span of kind `run` named for its agent, its ids from
    /// its run id, its progress and exit code as attributes and nothing else
    /// set, counting no unit.
    #[test]
    fn a_finished_run_is_a_run_span_of_its_agent() {
        let run_id = Uuid::parse_str("8f3c6d2a-1b4e-4c5d-9e7f-0a1b2c3d4e5f").expect("a uuid");
        let now = Utc::now();
        let run = FinishedRun {
            run_id,
            agent: "nightly-report".into(),
            started_at: now - chrono::Duration::seconds(90),
            ended_at: now,
            received_at: now,
            status: SpanStatus::Error,
            exit_code: Some(2),
            steps: Some(4),
            cost_usd: Some(0.08),
        };
        let row = run.row(&ProjectId::from_trusted("project_a"));
        assert_eq!(row.kind, SpanKind::Run);
        assert_eq!(row.name, "nightly-report");
        assert_eq!(row.agent, "nightly-report");
        assert_eq!(row.run_id, run_id);
        assert_eq!(row.trace_id, "8f3c6d2a1b4e4c5d9e7f0a1b2c3d4e5f");
        assert_eq!(row.span_id, "9e7f0a1b2c3d4e5f");
        assert_eq!(row.parent_span_id, "");
        assert_eq!(row.status, SpanStatus::Error);
        assert_eq!(
            row.attributes,
            vec![
                ("telmoni.run.steps".to_owned(), "4".to_owned()),
                ("telmoni.run.cost_usd".to_owned(), "0.08".to_owned()),
                ("process.exit.code".to_owned(), "2".to_owned()),
            ]
        );
        assert_eq!(row.units, 0, "a ping is not a span: its monitor is counted");
        assert_eq!(row.bound_max_steps, None, "a curl run declares no bound");

        let bare = FinishedRun {
            status: SpanStatus::Ok,
            exit_code: None,
            steps: None,
            cost_usd: None,
            ..run
        }
        .row(&ProjectId::from_trusted("project_a"));
        assert!(bare.attributes.is_empty());
        assert_eq!(bare.status, SpanStatus::Ok);
    }

    /// A row's payload is its own round trip, so what one version of this
    /// code writes the next reads.
    #[test]
    fn a_payload_round_trips() {
        let run = FinishedRun {
            run_id: Uuid::new_v4(),
            agent: "nightly-report".into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            received_at: Utc::now(),
            status: SpanStatus::Ok,
            exit_code: None,
            steps: Some(3),
            cost_usd: None,
        };
        let back: FinishedRun =
            serde_json::from_value(serde_json::to_value(&run).expect("encodes")).expect("parses");
        assert_eq!(back, run);
        let notice = QueuedNotice {
            kind: NotificationKind::MonitorDiscovered,
            title: "New monitor: nightly-report".into(),
            body: "…".into(),
            metadata: serde_json::json!({ "slug": "nightly-report" }),
            dedup_key: "telemetry:monitor:x:discovered".into(),
        };
        let back: QueuedNotice =
            serde_json::from_value(serde_json::to_value(&notice).expect("encodes"))
                .expect("parses");
        assert_eq!(back, notice);
    }

    /// The wait doubles from the floor and stops at the ceiling.
    #[test]
    fn a_held_row_waits_longer_each_time() {
        assert_eq!(retry_wait(1), 5);
        assert_eq!(retry_wait(2), 10);
        assert_eq!(retry_wait(5), 80);
        assert_eq!(retry_wait(7), 300);
        assert_eq!(retry_wait(100), 300);
        assert_eq!(retry_wait(0), 5);
    }
}
