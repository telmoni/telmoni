//! The outbox: what telemetry hands on once a transaction has committed, and
//! must not lose. A ping's finished run becomes a row of ClickHouse's
//! `spans`, and a notice for a project's feed is handed to notifications
//! through the seam; each is written as a row here in the transaction that
//! ends the run, makes the monitor or retires the key, so a monitor is never
//! late because ClickHouse was down. A notice is never told twice: the seam
//! answers a replayed dedup key with its first row and queues nothing. It
//! goes to the organization auth holds its project in when the loop asks, and
//! to none once that organization's deletion is marked or the project is
//! gone, since a notice written after a deletion's purges would outlive them;
//! a project found gone, before the write or after it, is purged from
//! notifications once more. A run is written once but for a sliver: `spans`
//! is a plain `MergeTree`, read without `FINAL`, which keeps every copy it is
//! given, so before each insert the loop reads which of the batch's runs the
//! table holds already and drops those rows as landed — a batch leased again
//! after a loop died between its insert and its delete, a retried end ping
//! that filed a second run under the same `rid`, two end pings for one run —
//! and writes no run twice within one batch. What is left is two replicas
//! landing one run inside one insert's time, which no read can see.
//!
//! The loop is one every replica runs, on the delivery loop's pattern: each
//! tick leases rows across every tenant, `FOR UPDATE SKIP LOCKED` under a
//! lease, lands each, deletes what landed and holds the rest. Whose fault a
//! row's failure is decides what it costs (`Fault`): a store that could
//! not be reached, would not answer, or said it cannot take rows now hands
//! the row back for the next tick with its try returned, so an outage of
//! ours strands nothing however long it lasts; a row the store refused, or
//! whose payload does not parse, waits longer each time, up to a cap, past
//! which it stays for an operator — named in the log as it crosses, counted
//! in the depth line, and tried again by `telmoni release outbox` once what
//! refused it is mended. A batch the store refused is written again one row
//! at a time first, so one row stops one row.
//!
//! ⚠ The loop's handle is raced against the server in the binary's `serve`,
//! as the delivery loop's is: a dead loop would leave a Ready pod with every
//! ping's row unwritten, so the process exits and the restart is the
//! recovery.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, maintenance_scope};
use telmoni_shared::digest::hex_lower;
use telmoni_shared::seam::Notice;
use telmoni_shared::{
    NotificationKind, OrganizationId, OrganizationStatus, ProjectId, SpanKind, SpanStatus,
    TelmoniError,
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
/// How many batches one tick drains before it waits for the next, while
/// every batch lands whole.
const DRAIN_ROUNDS: usize = 20;
/// How long a leased row is held before another replica may take it:
/// longer than an insert's deadline and a notice's write, so a row is never
/// landed twice by two replicas.
const LEASE_SECS: i64 = 120;
/// How many times a row the store refused, or that does not parse, is tried
/// before it is left for an operator: with the waits below, half an hour of
/// refusals. A row the store was at fault for ([`Fault::Store`]) is handed
/// back with its try, and never reaches it.
const MAX_ATTEMPTS: i32 = 12;
/// The wait before a row the store refused is tried again: five seconds,
/// doubled each time, up to five minutes.
const RETRY_FLOOR_SECS: i64 = 5;
const RETRY_CEILING_SECS: i64 = 300;
/// How often the depth line is written: the delivery loop's minute.
const DEPTH_EVERY_TICKS: u64 = 60 / POLL_SECS;
/// How long notifications may take to purge a project a notice was for:
/// auth's own budget for the same call, since a purge tears the project's
/// connections down at their vendors while the loop holds its lease.
const PURGE_BUDGET: Duration = Duration::from_secs(5);

/// The `kind` of a row carrying a finished run.
pub const KIND_SPAN: &str = "span";
/// The `kind` of a row carrying a notice.
pub const KIND_NOTICE: &str = "notice";

/// How far back the loop looks for a run `spans` holds already: a double
/// comes within minutes of its first copy — a retried ping, a batch leased
/// again after two minutes, a row held for an hour at the outside — so a day
/// bounds the read to the partitions it needs, and a crontab line that
/// reuses one `rid` names one run a day.
const HELD_RUNS_WINDOW_SECS: i64 = 24 * 60 * 60;

/// Which of a batch's runs `spans` holds already, by project and run id,
/// among the rows that arrived since `since`.
const HELD_RUNS: &str = "SELECT project_id, run_id FROM telemetry.spans \
     WHERE project_id IN {projects:Array(String)} \
       AND run_id IN {runs:Array(UUID)} \
       AND received_at >= {since:DateTime64(6, 'UTC')}";

/// A run `spans` holds, as the read answers it.
#[derive(Debug, clickhouse::Row, Deserialize)]
struct HeldRun {
    project_id: String,
    #[serde(with = "clickhouse::serde::uuid")]
    run_id: Uuid,
}

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
    /// Rows that landed, carried a run landed already, or carried a notice
    /// with no feed left to land on, and are gone.
    pub landed: u32,
    /// Rows that did not: held for their next try, or handed back for the
    /// next tick with their try returned, where the store was at fault.
    pub held: u32,
}

/// Whose fault it is that a row did not land, which decides what its try
/// costs: the delivery loop's posture, which hands a delivery back with its
/// attempt when the KEK it needs cannot be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// The store's: not reached, not answering, or saying it cannot take
    /// rows now. The row is handed back for the next tick with its try
    /// returned, so an outage of ours never strands it.
    Store,
    /// The row's: refused by the store, or a payload that does not parse.
    /// The row waits longer each time, up to [`MAX_ATTEMPTS`].
    Row,
}

/// ClickHouse's codes for a server that is up and cannot take rows now —
/// `TIMEOUT_EXCEEDED`, `CANNOT_ALLOCATE_MEMORY`,
/// `TOO_MANY_SIMULTANEOUS_QUERIES`, `SOCKET_TIMEOUT`, `NETWORK_ERROR`,
/// `ABORTED` (stopping), `MEMORY_LIMIT_EXCEEDED`, `TABLE_IS_READ_ONLY`,
/// `NOT_ENOUGH_SPACE`, `TOO_MANY_PARTS`, `UNKNOWN_STATUS_OF_INSERT`,
/// `CANNOT_SCHEDULE_TASK`, `SERVER_OVERLOADED` — its own state, not the
/// rows', which its next hour or an operator mends.
const STORE_UNWELL: [u32; 13] = [
    159, 173, 202, 209, 210, 236, 241, 242, 243, 252, 319, 439, 745,
];

/// Whose fault a ClickHouse error is: the store's when it could not be
/// reached or did not answer in time, and when it answered with one of
/// [`STORE_UNWELL`], which it puts at the front of its message; the rows'
/// otherwise — a shape it refused, or a row this side could not encode,
/// which retrying will not mend.
fn fault_of(error: &clickhouse::error::Error) -> Fault {
    use clickhouse::error::Error;
    match error {
        Error::Network(_) | Error::TimedOut => Fault::Store,
        Error::BadResponse(reason)
            if exception_code(reason).is_some_and(|code| STORE_UNWELL.contains(&code)) =>
        {
            Fault::Store
        }
        _ => Fault::Row,
    }
}

/// The code at the front of ClickHouse's message — `Code: 241. DB::Exception:
/// …`, or `Code: 241` alone where the body could not be read and the header
/// named it — or none where something else answered.
fn exception_code(reason: &str) -> Option<u32> {
    reason
        .strip_prefix("Code: ")?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// A row that did not land, and whose fault that was.
struct Held {
    id: Uuid,
    /// How many times it has been leased, this time counted.
    attempts: i32,
    kind: &'static str,
    project_id: ProjectId,
    fault: Fault,
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
                // Empty is the outbox's usual state: a line a minute saying
                // so, on every replica, would drown the ones that matter.
                Ok((0, 0, _)) => {
                    tracing::debug!(pending = 0, stuck = 0, "telemetry outbox depth");
                }
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

/// What the loop logs once a minute as `telemetry outbox depth`, at info
/// while anything waits: the rows still to land, those past their tries,
/// and the oldest's age in seconds.
pub async fn depth(state: &AppState) -> Result<(i64, i64, Option<i64>), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let depth = db::outbox_depth(&mut tx, MAX_ATTEMPTS).await?;
    tx.commit().await?;
    Ok(depth)
}

/// Every row past its tries given them back, for the loop to take again:
/// `telmoni release outbox`, an operator's once what refused the rows is
/// mended — a row refused again waits out its tries again. Answers how many
/// were released.
pub async fn release(state: &AppState) -> Result<u64, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let released = db::release_outbox(&mut tx, MAX_ATTEMPTS).await?;
    tx.commit().await?;
    Ok(released)
}

/// One tick: the outbox drained batch after batch, until a batch comes back
/// short, holds anything, or the rounds run out. A batch with anything held
/// ends the tick, as it ends the delivery loop's: a store that would not
/// answer, or refused, is not helped by more batches of failing inserts,
/// and the next tick is seconds away. What landed and what was held,
/// summed.
pub async fn tick_once(state: &Arc<AppState>) -> Result<Settled, TelmoniError> {
    let mut total = Settled::default();
    for _ in 0..DRAIN_ROUNDS {
        let settled = drain_batch(state).await?;
        total.landed = total.landed.saturating_add(settled.landed);
        total.held = total.held.saturating_add(settled.held);
        if settled.held > 0 || i64::from(settled.landed) < BATCH {
            break;
        }
    }
    Ok(total)
}

/// Lease one batch and land it: the runs in one insert, the notices one by
/// one through the seam; then, in one transaction, what landed deleted,
/// what the store was at fault for handed back with its try, and the rest
/// held for their next try — a row crossing the cap named in the log, since
/// nothing takes it again until an operator does.
async fn drain_batch(state: &Arc<AppState>) -> Result<Settled, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let leased = db::lease_outbox(&mut tx, BATCH, LEASE_SECS, MAX_ATTEMPTS).await?;
    tx.commit().await?;
    if leased.is_empty() {
        return Ok(Settled::default());
    }

    let mut landed: Vec<Uuid> = Vec::new();
    let mut held: Vec<Held> = Vec::new();
    let (runs, notices): (Vec<OutboxRow>, Vec<OutboxRow>) =
        leased.into_iter().partition(|row| row.kind == KIND_SPAN);
    land_runs(state, runs, &mut landed, &mut held).await;
    let mut notices = notices.into_iter();
    while let Some(row) = notices.next() {
        if land_notice(state, row, &mut landed, &mut held)
            .await
            .is_break()
        {
            held.extend(notices.map(|row| NoticeRow::of(row).held(Fault::Store)));
            break;
        }
    }

    let settled = Settled {
        landed: u32::try_from(landed.len()).unwrap_or(u32::MAX),
        held: u32::try_from(held.len()).unwrap_or(u32::MAX),
    };
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    if !landed.is_empty() {
        db::delete_outbox(&mut tx, &landed).await?;
    }
    let mut handed_back: Vec<Uuid> = Vec::new();
    for row in held {
        match row.fault {
            Fault::Store => handed_back.push(row.id),
            Fault::Row => {
                db::hold_outbox(&mut tx, row.id, retry_wait(row.attempts)).await?;
                if row.attempts >= MAX_ATTEMPTS {
                    tracing::error!(
                        outbox_id = %row.id,
                        kind = row.kind,
                        project_id = %row.project_id,
                        attempts = row.attempts,
                        "telemetry outbox: a row is past its tries and stays until `telmoni release outbox`"
                    );
                }
            }
        }
    }
    if !handed_back.is_empty() {
        db::unlease_outbox(&mut tx, &handed_back).await?;
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

/// A finished run as its row carried it, with what the row says of it.
struct QueuedRun {
    id: Uuid,
    attempts: i32,
    project_id: ProjectId,
    organization_id: OrganizationId,
    run: FinishedRun,
}

impl QueuedRun {
    /// The row, not landed, with whose fault that was.
    fn held(self, fault: Fault) -> Held {
        Held {
            id: self.id,
            attempts: self.attempts,
            kind: KIND_SPAN,
            project_id: self.project_id,
            fault,
        }
    }
}

/// The batch's finished runs, read from their rows, oldest row first. A row
/// no version of this code wrote is held and said so, since dropping it
/// would lose a run in silence.
fn parse_runs(rows: Vec<OutboxRow>, held: &mut Vec<Held>) -> Vec<QueuedRun> {
    let mut runs = Vec::with_capacity(rows.len());
    for row in rows {
        match serde_json::from_value::<FinishedRun>(row.payload) {
            Ok(run) => runs.push(QueuedRun {
                id: row.id,
                attempts: row.attempts,
                project_id: row.project_id,
                organization_id: row.organization_id,
                run,
            }),
            Err(error) => {
                tracing::error!(
                    outbox_id = %row.id,
                    project_id = %row.project_id,
                    error = %error,
                    "telemetry outbox: a finished run does not parse; the row is held"
                );
                held.push(Held {
                    id: row.id,
                    attempts: row.attempts,
                    kind: KIND_SPAN,
                    project_id: row.project_id,
                    fault: Fault::Row,
                });
            }
        }
    }
    // A row's id is its clock, so the oldest copy of a run is the one
    // written, whichever order the lease took them in.
    runs.sort_by_key(|queued| queued.id);
    runs
}

/// The batch's finished runs, as rows of `spans` in one insert — but for the
/// runs the table holds already, dropped as landed rather than written twice
/// ([`held_runs`]) — and the organization's open consoles told of what
/// landed. A read that fails hands the whole batch back with its tries,
/// whatever the read's fault: the rows did nothing. An insert the store
/// refuses is tried again one row at a time ([`land_one_by_one`]).
async fn land_runs(
    state: &Arc<AppState>,
    rows: Vec<OutboxRow>,
    landed: &mut Vec<Uuid>,
    held: &mut Vec<Held>,
) {
    let runs = parse_runs(rows, held);
    if runs.is_empty() {
        return;
    }
    let mut written = match held_runs(state, &runs).await {
        Ok(written) => written,
        Err(error) => {
            // ⚠ The error's text can carry ClickHouse's own message: its
            // kind alone is logged.
            let kind = store::error_kind(&error);
            match fault_of(&error) {
                Fault::Store => tracing::warn!(
                    kind,
                    rows = runs.len(),
                    "telemetry outbox: ClickHouse did not say which runs it holds; the rows wait"
                ),
                // A read the server refused is this query's fault, which a
                // deploy mends: said at error, every tick, until one does.
                Fault::Row => tracing::error!(
                    kind,
                    rows = runs.len(),
                    "telemetry outbox: ClickHouse refused the read of which runs it holds; the rows wait"
                ),
            }
            held.extend(runs.into_iter().map(|queued| queued.held(Fault::Store)));
            return;
        }
    };
    let mut queued: Vec<QueuedRun> = Vec::with_capacity(runs.len());
    let mut spans: Vec<SpanRow> = Vec::with_capacity(runs.len());
    for run in runs {
        if !written.insert((run.project_id.to_string(), run.run.run_id)) {
            tracing::info!(
                outbox_id = %run.id,
                project_id = %run.project_id,
                run_id = %run.run.run_id,
                "telemetry outbox: a run spans holds already; its row is dropped, not written twice"
            );
            landed.push(run.id);
            continue;
        }
        spans.push(run.run.row(&run.project_id));
        queued.push(run);
    }
    if spans.is_empty() {
        return;
    }
    let mut nudges: BTreeMap<(String, String), Vec<Uuid>> = BTreeMap::new();
    match state.store.insert_spans(&spans).await {
        Ok(()) => {
            for run in &queued {
                nudge(&mut nudges, run);
                landed.push(run.id);
            }
        }
        Err(error) => {
            // ⚠ The error's text can carry ClickHouse's own message: its
            // kind alone is logged.
            let kind = store::error_kind(&error);
            match fault_of(&error) {
                Fault::Store => {
                    tracing::warn!(
                        kind,
                        rows = spans.len(),
                        "telemetry outbox: ClickHouse did not take the runs' rows; they wait with their tries"
                    );
                    held.extend(queued.into_iter().map(|run| run.held(Fault::Store)));
                }
                Fault::Row => {
                    tracing::error!(
                        kind,
                        rows = spans.len(),
                        "telemetry outbox: ClickHouse refused the runs' rows; each is tried alone"
                    );
                    land_one_by_one(&state.store, queued, &spans, landed, held, &mut nudges).await;
                }
            }
        }
    }
    for ((organization, project), runs) in nudges {
        live::tell(Arc::clone(state), &organization, &project, runs);
    }
}

/// A batch the store refused, written again one row at a time, so one row
/// stops one row: a row refused alone is held and its try counted; a row the
/// store fails on — gone away mid-pass — and every row after it are handed
/// back with their tries, and the pass ends.
async fn land_one_by_one(
    store: &store::Store,
    queued: Vec<QueuedRun>,
    spans: &[SpanRow],
    landed: &mut Vec<Uuid>,
    held: &mut Vec<Held>,
    nudges: &mut BTreeMap<(String, String), Vec<Uuid>>,
) {
    let mut rows = queued.into_iter().zip(spans);
    while let Some((run, span)) = rows.next() {
        let took = store.insert_spans(std::slice::from_ref(span)).await;
        if settle_one(took, run, landed, held, nudges).is_break() {
            held.extend(rows.map(|(run, _)| run.held(Fault::Store)));
            return;
        }
    }
}

/// One row's insert settled: landed and told of, held as the row's fault,
/// or — the store gone away — handed back, which ends the pass
/// ([`ControlFlow::Break`]) for every row after it to be handed back too.
fn settle_one(
    took: clickhouse::error::Result<()>,
    run: QueuedRun,
    landed: &mut Vec<Uuid>,
    held: &mut Vec<Held>,
    nudges: &mut BTreeMap<(String, String), Vec<Uuid>>,
) -> ControlFlow<()> {
    let error = match took {
        Ok(()) => {
            nudge(nudges, &run);
            landed.push(run.id);
            return ControlFlow::Continue(());
        }
        Err(error) => error,
    };
    // ⚠ The error's text can carry ClickHouse's own message: its kind alone
    // is logged.
    let kind = store::error_kind(&error);
    match fault_of(&error) {
        Fault::Row => {
            tracing::warn!(
                outbox_id = %run.id,
                project_id = %run.project_id,
                kind,
                "telemetry outbox: ClickHouse refused a run's row; it is held"
            );
            held.push(run.held(Fault::Row));
            ControlFlow::Continue(())
        }
        Fault::Store => {
            tracing::warn!(
                kind,
                "telemetry outbox: ClickHouse did not take a run's row; it and the rest wait with their tries"
            );
            held.push(run.held(Fault::Store));
            ControlFlow::Break(())
        }
    }
}

/// A landed run gathered for `live::tell`, by its organization and project.
fn nudge(nudges: &mut BTreeMap<(String, String), Vec<Uuid>>, run: &QueuedRun) {
    nudges
        .entry((run.organization_id.to_string(), run.project_id.to_string()))
        .or_default()
        .push(run.run.run_id);
}

/// Which of the batch's runs `spans` holds already, by project and run id,
/// among the rows that arrived within [`HELD_RUNS_WINDOW_SECS`] of the
/// batch's earliest: read before the insert, under the row policy as every
/// read is, so a copy is dropped rather than written. The insert that
/// follows sees what the read saw, since every insert is waited for.
async fn held_runs(
    state: &AppState,
    runs: &[QueuedRun],
) -> clickhouse::error::Result<HashSet<(String, Uuid)>> {
    let mut projects: Vec<ProjectId> = Vec::new();
    for queued in runs {
        if !projects.contains(&queued.project_id) {
            projects.push(queued.project_id.clone());
        }
    }
    let Some(projects) = store::Projects::of(projects) else {
        return Ok(HashSet::new());
    };
    let run_ids: Vec<Uuid> = runs
        .iter()
        .map(|queued| queued.run.run_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let since = runs
        .iter()
        .map(|queued| queued.run.received_at)
        .min()
        .unwrap_or_else(Utc::now)
        - chrono::Duration::seconds(HELD_RUNS_WINDOW_SECS);
    let held: Vec<HeldRun> = state
        .store
        .read(&projects, HELD_RUNS)
        .param("runs", &run_ids)
        .param("since", since.format("%Y-%m-%d %H:%M:%S%.6f").to_string())
        .fetch_all()
        .await?;
    Ok(held
        .into_iter()
        .map(|run| (run.project_id, run.run_id))
        .collect())
}

/// A notice's row as the loop settles it, its payload aside.
struct NoticeRow {
    id: Uuid,
    /// How many times it has been leased, this time counted.
    attempts: i32,
    project_id: ProjectId,
}

impl NoticeRow {
    /// The row unread, as one handed back unasked is.
    fn of(row: OutboxRow) -> Self {
        Self {
            id: row.id,
            attempts: row.attempts,
            project_id: row.project_id,
        }
    }

    /// The row, not landed, with whose fault that was.
    fn held(self, fault: Fault) -> Held {
        Held {
            id: self.id,
            attempts: self.attempts,
            kind: KIND_NOTICE,
            project_id: self.project_id,
            fault,
        }
    }
}

/// Where a notice of a project's lands, as auth answers.
enum Landing {
    /// The feed of the organization holding the project now.
    Feed(OrganizationId),
    /// Nowhere, since the organization's deletion is marked: its purges are
    /// still to come, and take whatever lands before them.
    OrganizationClosing,
    /// Nowhere, since auth has no such project: its deletion's purges have
    /// run, or are running.
    ProjectGone,
}

/// One notice handed through the seam, under its dedup key, to the feed of
/// the organization auth holds its project in when the loop asks: a
/// transfer that committed before then takes the notice with the project.
/// Where that organization's deletion is marked, the row is dropped as
/// landed, since the deletion's purges take its feeds, and a notice written
/// after them would outlive them, slug and key prefix and all. Asking before
/// the write is enough there: the purges wait out the deletion's quarter of
/// an hour, and the write follows its ask within seconds, each call held to
/// its pool's acquire and its role's statement timeout. Not for a project,
/// whose feed is purged the moment it is deleted, so a project auth no
/// longer has — before the write, or asked again after it — is purged from
/// notifications once more ([`settle_gone`]), which takes whatever this try
/// wrote, or an earlier one cut off before it could ask again. An ask auth
/// cannot answer hands the row back and answers [`ControlFlow::Break`], for
/// the batch's other notices to be handed back unasked.
///
/// ⚠ A transfer committing between the ask and the write files the notice
/// under the organization the project left, after the transfer's own purge
/// of the project: it stays there, unread, until that organization's purge
/// or the feed's retention.
async fn land_notice(
    state: &AppState,
    row: OutboxRow,
    landed: &mut Vec<Uuid>,
    held: &mut Vec<Held>,
) -> ControlFlow<()> {
    let OutboxRow {
        id,
        attempts,
        project_id,
        payload,
        ..
    } = row;
    let row = NoticeRow {
        id,
        attempts,
        project_id,
    };
    let notice = match serde_json::from_value::<QueuedNotice>(payload) {
        Ok(notice) => notice,
        Err(error) => {
            tracing::error!(
                outbox_id = %row.id,
                project_id = %row.project_id,
                error = %error,
                "telemetry outbox: a notice does not parse; the row is held"
            );
            held.push(row.held(Fault::Row));
            return ControlFlow::Continue(());
        }
    };
    let organization_id = match landing(state, &row.project_id).await {
        Ok(Landing::Feed(organization_id)) => organization_id,
        Ok(Landing::OrganizationClosing) => {
            tracing::debug!(
                outbox_id = %row.id,
                project_id = %row.project_id,
                "telemetry outbox: a notice's organization is being deleted; the row is dropped"
            );
            landed.push(row.id);
            return ControlFlow::Continue(());
        }
        Ok(Landing::ProjectGone) => {
            settle_gone(state, row, landed, held).await;
            return ControlFlow::Continue(());
        }
        Err(error) => {
            // Auth that cannot answer is an outage of ours, as notifications
            // that cannot write is.
            tracing::warn!(
                outbox_id = %row.id,
                project_id = %row.project_id,
                error = %error,
                "telemetry outbox: auth could not say where a notice's project is; the batch's notices wait with their tries"
            );
            held.push(row.held(Fault::Store));
            return ControlFlow::Break(());
        }
    };
    let handed = state
        .notifications
        .emit(
            &organization_id,
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
        Ok(_) => match still_there(state, &row.project_id).await {
            Ok(true) => {
                landed.push(row.id);
                ControlFlow::Continue(())
            }
            Ok(false) => {
                settle_gone(state, row, landed, held).await;
                ControlFlow::Continue(())
            }
            Err(error) => {
                tracing::warn!(
                    outbox_id = %row.id,
                    project_id = %row.project_id,
                    error = %error,
                    "telemetry outbox: auth could not say whether a notice's project outlived its write; the batch's notices wait with their tries"
                );
                held.push(row.held(Fault::Store));
                ControlFlow::Break(())
            }
        },
        Err(error) => {
            // A notice the seam refuses — a bound, a shape — is this code's
            // to mend, and retrying will not land it; notifications that
            // could not write it is an outage of ours, and will.
            let fault = if error.to_problem_details().status >= 500 {
                Fault::Store
            } else {
                Fault::Row
            };
            match fault {
                Fault::Store => tracing::warn!(
                    outbox_id = %row.id,
                    project_id = %row.project_id,
                    error = %error,
                    "telemetry outbox: notifications did not take a notice; the row waits with its try"
                ),
                Fault::Row => tracing::error!(
                    outbox_id = %row.id,
                    project_id = %row.project_id,
                    error = %error,
                    "telemetry outbox: notifications refused a notice; the row is held"
                ),
            }
            held.push(row.held(fault));
            ControlFlow::Continue(())
        }
    }
}

/// Where a notice of `project_id`'s lands, as auth answers: the organization
/// it holds the project in now, unless that organization's deletion is
/// marked. An error answers nothing, never "nowhere".
async fn landing(state: &AppState, project_id: &ProjectId) -> Result<Landing, TelmoniError> {
    let homes = state
        .auth
        .project_homes(std::slice::from_ref(project_id))
        .await?;
    let Some(home) = homes
        .into_iter()
        .find(|home| home.project_id == *project_id)
    else {
        return Ok(Landing::ProjectGone);
    };
    let standing = state
        .auth
        .organization_standing(&home.organization_id)
        .await?;
    Ok(if standing == Some(OrganizationStatus::Active) {
        Landing::Feed(home.organization_id)
    } else {
        Landing::OrganizationClosing
    })
}

/// Whether auth still has the project, asked once its notice is written: a
/// deletion committed after [`landing`] asked, its purge of the project run
/// before the write, would leave the notice behind both. A project that
/// moved meanwhile is still there; its purge would take what its new
/// organization holds of it.
async fn still_there(state: &AppState, project_id: &ProjectId) -> Result<bool, TelmoniError> {
    Ok(state
        .auth
        .project_homes(std::slice::from_ref(project_id))
        .await?
        .iter()
        .any(|home| home.project_id == *project_id))
}

/// A notice's project auth no longer has: purged from notifications once
/// more, within auth's budget for the same call, and the row dropped as
/// landed. The purge is the deletion's own, so it is idempotent, and it
/// takes whatever a try wrote after the deletion's purge ran; a purge that
/// fails, or does not end in time, hands the row back, for the next tick to
/// find the project gone and purge again.
async fn settle_gone(
    state: &AppState,
    row: NoticeRow,
    landed: &mut Vec<Uuid>,
    held: &mut Vec<Held>,
) {
    match tokio::time::timeout(
        PURGE_BUDGET,
        state.notifications.purge_project(&row.project_id),
    )
    .await
    {
        Ok(Ok(_)) => {
            tracing::debug!(
                outbox_id = %row.id,
                project_id = %row.project_id,
                "telemetry outbox: a notice's project is gone; its feed is purged again and the row dropped"
            );
            landed.push(row.id);
        }
        Ok(Err(error)) => {
            tracing::warn!(
                outbox_id = %row.id,
                project_id = %row.project_id,
                error = %error,
                "telemetry outbox: notifications did not purge a notice's project, which is gone; the row waits with its try"
            );
            held.push(row.held(Fault::Store));
        }
        Err(_) => {
            tracing::warn!(
                outbox_id = %row.id,
                project_id = %row.project_id,
                budget_secs = PURGE_BUDGET.as_secs(),
                "telemetry outbox: notifications did not purge a notice's project, which is gone, in time; the row waits with its try"
            );
            held.push(row.held(Fault::Store));
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

    /// The store's fault — unreachable, silent, or saying it cannot take
    /// rows now — hands a row back; a refusal, or a row this side could not
    /// encode, is the row's.
    #[test]
    fn whose_fault_a_store_error_is() {
        use clickhouse::error::Error;
        assert_eq!(
            fault_of(&Error::Network("connection refused".into())),
            Fault::Store
        );
        assert_eq!(fault_of(&Error::TimedOut), Fault::Store);
        assert_eq!(
            fault_of(&Error::BadResponse(
                "Code: 241. DB::Exception: Memory limit (total) exceeded: would use 3.72 GiB. \
                 (MEMORY_LIMIT_EXCEEDED) (version 26.8.19.9 (official build))"
                    .into()
            )),
            Fault::Store
        );
        assert_eq!(
            fault_of(&Error::BadResponse("Code: 243".into())),
            Fault::Store,
            "a body the client could not read, the header naming the code"
        );
        assert_eq!(
            fault_of(&Error::BadResponse(
                "Code: 60. DB::Exception: Table telemetry.spans does not exist. (UNKNOWN_TABLE)"
                    .into()
            )),
            Fault::Row
        );
        assert_eq!(
            fault_of(&Error::BadResponse("503 Service Unavailable".into())),
            Fault::Row,
            "an answer with no code of ClickHouse's"
        );
        assert_eq!(fault_of(&Error::SequenceMustHaveLength), Fault::Row);
        assert_eq!(
            fault_of(&Error::Other("attempting to write an empty block".into())),
            Fault::Row
        );
    }

    /// The code is read from the front of the message and nowhere else.
    #[test]
    fn the_code_is_read_from_the_front_of_the_message() {
        assert_eq!(
            exception_code("Code: 252. DB::Exception: Too many parts (3001 with average size"),
            Some(252)
        );
        assert_eq!(exception_code("Code: 252"), Some(252));
        assert_eq!(exception_code("503 Service Unavailable"), None);
        assert_eq!(exception_code("DB::Exception: Code: 252."), None);
        assert_eq!(exception_code("Code: "), None);
        assert_eq!(exception_code(""), None);
    }

    /// Three runs of one project, as a batch carries them.
    fn a_batch() -> (Vec<QueuedRun>, Vec<SpanRow>) {
        let now = Utc::now();
        let project = ProjectId::from_trusted("project_a");
        let queued: Vec<QueuedRun> = (0..3)
            .map(|_| QueuedRun {
                id: Uuid::now_v7(),
                attempts: 3,
                project_id: project.clone(),
                organization_id: OrganizationId::from_trusted("org_a"),
                run: FinishedRun {
                    run_id: Uuid::new_v4(),
                    agent: "nightly-report".into(),
                    started_at: now,
                    ended_at: now,
                    received_at: now,
                    status: SpanStatus::Ok,
                    exit_code: None,
                    steps: None,
                    cost_usd: None,
                },
            })
            .collect();
        let spans = queued
            .iter()
            .map(|run| run.run.row(&run.project_id))
            .collect();
        (queued, spans)
    }

    /// A row written alone: landed and told of; refused, held with its try
    /// counted and the pass going on; the store gone away, handed back and
    /// the pass ended.
    #[test]
    fn one_row_settled_alone() {
        use clickhouse::error::Error;

        let (queued, spans) = a_batch();
        let ids: Vec<Uuid> = queued.iter().map(|run| run.id).collect();
        let mut rows = queued.into_iter();
        let mut landed = Vec::new();
        let mut held = Vec::new();
        let mut nudges = BTreeMap::new();

        let first = rows.next().expect("a row");
        assert_eq!(
            settle_one(Ok(()), first, &mut landed, &mut held, &mut nudges),
            ControlFlow::Continue(())
        );
        assert_eq!(landed, [ids[0]]);
        assert!(held.is_empty());
        assert_eq!(
            nudges
                .get(&("org_a".to_owned(), "project_a".to_owned()))
                .map(|runs| runs.as_slice()),
            Some([spans[0].run_id].as_slice()),
            "the consoles are told of what landed"
        );

        let second = rows.next().expect("a row");
        assert_eq!(
            settle_one(
                Err(Error::BadResponse(
                    "Code: 53. DB::Exception: Cannot convert: String to UInt32. (TYPE_MISMATCH)"
                        .into()
                )),
                second,
                &mut landed,
                &mut held,
                &mut nudges
            ),
            ControlFlow::Continue(()),
            "one row stops one row"
        );
        assert_eq!(landed, [ids[0]]);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].id, ids[1]);
        assert_eq!(held[0].fault, Fault::Row);
        assert_eq!(held[0].attempts, 3, "the try counts");
        assert_eq!(held[0].kind, KIND_SPAN);
        assert_eq!(held[0].project_id.as_str(), "project_a");

        let third = rows.next().expect("a row");
        assert_eq!(
            settle_one(
                Err(Error::TimedOut),
                third,
                &mut landed,
                &mut held,
                &mut nudges
            ),
            ControlFlow::Break(()),
            "the store gone away ends the pass"
        );
        assert_eq!(held.len(), 2);
        assert_eq!(held[1].id, ids[2]);
        assert_eq!(held[1].fault, Fault::Store);
        assert_eq!(nudges.values().map(Vec::len).sum::<usize>(), 1);
    }

    /// A store that cannot be reached ends the pass at its first row: every
    /// row is handed back with its try, and none is told of. Nothing listens
    /// on the port the store names.
    #[tokio::test]
    async fn a_store_gone_away_hands_the_whole_pass_back() {
        let store = store::Store::connect("http://127.0.0.1:1").expect("a store nobody runs");
        let (queued, spans) = a_batch();
        let ids: Vec<Uuid> = queued.iter().map(|run| run.id).collect();
        let mut landed = Vec::new();
        let mut held = Vec::new();
        let mut nudges = BTreeMap::new();
        land_one_by_one(&store, queued, &spans, &mut landed, &mut held, &mut nudges).await;
        assert!(landed.is_empty());
        assert_eq!(
            held.iter()
                .map(|row| (row.id, row.fault))
                .collect::<Vec<_>>(),
            [
                (ids[0], Fault::Store),
                (ids[1], Fault::Store),
                (ids[2], Fault::Store)
            ]
        );
        assert!(nudges.is_empty());
    }
}
