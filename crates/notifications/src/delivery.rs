//! The delivery loop: send queued connector deliveries, at-least-once.
//!
//! Each tick leases eligible rows (`FOR UPDATE SKIP LOCKED`, no leader),
//! attempts them, and records outcomes per BATCH, never per row. A failure that
//! names the CONNECTION — uninstalled, archived — retires it rather than
//! spending the budget on rows that cannot land.
//!
//! ⚠ The loop's handle is raced against the server in the binary's `serve`:
//! a dead loop would leave a Ready pod with every delivery `pending`, so the
//! process exits and the restart is the recovery.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::envelope::KekError;
use telmoni_shared::{Redacted, TelmoniError};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::AppState;
use crate::connector::{Connector, DeliveryError, Event, Provider, Terminal};
use crate::db::{
    self, AttemptTrigger, FailedAttempt, LeasedDelivery, NewAttempt, NotificationsLane,
    SealedConnection,
};
use crate::notify;

/// How long a leased delivery is held before another instance may reclaim it.
const LEASE_SECS: i64 = 120;

/// How many inline first attempts may be in flight across this process.
const FIRST_ATTEMPT_INFLIGHT: usize = 32;

/// The slots [`FIRST_ATTEMPT_INFLIGHT`] hands out.
static FIRST_ATTEMPT_SLOTS: Semaphore = Semaphore::const_new(FIRST_ATTEMPT_INFLIGHT);

/// How often the loop says the flag is off. Off is a normal state on a fresh
/// tier, and a line every poll drowns the lines that explain something.
const HELD_WARNING_EVERY_SECS: i64 = 600;

/// When the flag-off line was last written, as unix seconds.
static LAST_HELD_WARNING: AtomicI64 = AtomicI64::new(0);

/// When the KEK-unavailable line was last written, as unix seconds.
static LAST_KEK_WARNING: AtomicI64 = AtomicI64::new(0);

/// Whether `last` is due another line, claiming the slot if so.
fn throttle_due(last: &AtomicI64) -> bool {
    let now = chrono::Utc::now().timestamp();
    let seen = last.load(Ordering::Relaxed);
    now.saturating_sub(seen) >= HELD_WARNING_EVERY_SECS
        && last
            .compare_exchange(seen, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

/// The flag-off line, at most once per [`HELD_WARNING_EVERY_SECS`].
fn note_held() {
    if throttle_due(&LAST_HELD_WARNING) {
        tracing::warn!("connectors flag is off — deliveries are held in place until it is on");
    }
}

/// The rows-held line, on the same throttle, so a KMS outage is not the
/// loudest thing in the log. The cause is a field, because a row can be held
/// for more than one reason.
fn note_rows_held(message: &str) {
    if throttle_due(&LAST_KEK_WARNING) {
        tracing::warn!(
            error = message,
            "deliveries are held in place with their attempts handed back"
        );
    }
}

/// The delivery loop. A failed tick is logged and the next one runs.
pub async fn run(state: Arc<AppState>) {
    let poll = state.config.delivery_poll_secs.max(1);
    let mut ticker = tokio::time::interval(Duration::from_secs(poll));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // skip the immediate first tick
    let mut ticks = 0;
    loop {
        ticker.tick().await;
        match tick_once(&state).await {
            Ok(0) => {}
            Ok(n) => tracing::debug!(attempted = n, "connector delivery tick"),
            Err(e) => tracing::warn!(error = %e, "connector delivery tick failed; will retry"),
        }

        ticks += 1;
        if ticks >= (60 / poll).max(1) {
            ticks = 0;
            match queue_depth(&state).await {
                Ok((pending, oldest_age_secs)) => tracing::info!(
                    pending,
                    oldest_age_secs = oldest_age_secs.unwrap_or(0),
                    "connector queue depth"
                ),
                Err(e) => tracing::warn!(error = %e, "connector queue depth read failed"),
            }
        }
    }
}

/// What the loop logs once a minute as `connector queue depth`. A
/// deployment's stalled-queue alert matches that line by its exact message,
/// so rewording it disarms the alert.
pub async fn queue_depth(state: &AppState) -> sqlx::Result<(i64, Option<i64>)> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let stats = db::queue_stats(&mut tx).await?;
    tx.commit().await?;
    Ok(stats)
}

/// One tick: drain the queue in batches while there is more to take. The
/// `connectors` flag is read first, from auth in process: off, the queue
/// PAUSES in place and drains when it is on again; unreadable, the tick
/// fails rather than send.
pub async fn tick_once(state: &Arc<AppState>) -> Result<u32, TelmoniError> {
    let flags = state.auth.global_flags().await?;
    if !flags.is_on(telmoni_shared::Flag::Connectors) {
        note_held();
        return Ok(0);
    }

    let batch = state.config.delivery_batch;
    let mut attempted: u32 = 0;
    for _ in 0..state.config.delivery_drain_rounds {
        let settled = drain_batch(state, batch).await?;
        attempted = attempted.saturating_add(settled.attempted);
        if settled.held > 0 {
            break;
        }
        if i64::from(settled.attempted) < batch {
            break;
        }
    }
    Ok(attempted)
}

/// What one batch came to, for the drain loop's decision to go round again.
#[derive(Debug, Clone, Copy, Default)]
struct Settled {
    /// Rows leased and attempted.
    attempted: u32,
    /// Rows handed back untouched because nothing could be attempted.
    held: u32,
}

/// Lease one batch and settle it.
async fn drain_batch(state: &Arc<AppState>, batch: i64) -> Result<Settled, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let leased = db::lease_deliveries(&mut tx, batch, LEASE_SECS).await?;
    tx.commit().await?;
    settle(state, leased).await
}

/// Read the batch's connections, open each once, attempt every row, and
/// record every outcome. Shared by the loop and the inline first attempt, so
/// there is no second delivery path to drift.
async fn settle(
    state: &Arc<AppState>,
    leased: Vec<LeasedDelivery>,
) -> Result<Settled, TelmoniError> {
    if leased.is_empty() {
        return Ok(Settled::default());
    }
    let attempted = u32::try_from(leased.len()).unwrap_or(u32::MAX);

    let mut wanted: Vec<Uuid> = leased.iter().map(|d| d.connection_id).collect();
    wanted.sort_unstable();
    wanted.dedup();
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let found = db::sealed_connections(&mut tx, &wanted).await?;
    tx.commit().await?;
    let connections: HashMap<Uuid, Arc<SealedConnection>> =
        found.into_iter().map(|c| (c.id, Arc::new(c))).collect();

    let slots = Arc::new(Semaphore::new(state.config.delivery_concurrency));
    let opened = open_batch(state, &connections, &slots).await;
    let outcomes = attempt_batch(state, leased, &connections, &opened, &slots).await;
    let held = outcomes
        .iter()
        .filter(|o| matches!(o, Outcome::Held { .. }))
        .count();
    record(state, outcomes).await?;
    Ok(Settled {
        attempted,
        held: u32::try_from(held).unwrap_or(u32::MAX),
    })
}

/// One attempt each for freshly enqueued rows, off the request. Past
/// `FIRST_ATTEMPT_INFLIGHT` an id is dropped, because the loop will take it.
pub fn spawn_first_attempts(state: &Arc<AppState>, ids: Vec<Uuid>) {
    for id in ids {
        let Ok(permit) = FIRST_ATTEMPT_SLOTS.try_acquire() else {
            tracing::debug!(
                delivery_id = %id,
                "inline first attempts are at their limit; the loop takes this one"
            );
            continue;
        };
        let state = Arc::clone(state);
        tokio::spawn(async move {
            let _permit = permit;
            // Never cancelled mid-send. A budget here dropped the future after
            // the far end had taken the message and before the outcome was
            // recorded, so the loop sent it again when the lease lapsed. The
            // vendor client's own timeouts bound it.
            if let Err(e) = first_attempt(&state, id).await {
                tracing::warn!(delivery_id = %id, error = %e, "inline first attempt failed; the loop retries");
            }
        });
    }
}

/// Lease one named row and attempt it, on the loop's own terms.
pub async fn first_attempt(state: &Arc<AppState>, id: Uuid) -> Result<(), TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let leased = db::lease_delivery(&mut tx, id, LEASE_SECS).await?;
    tx.commit().await?;
    settle(state, leased.into_iter().collect()).await?;
    Ok(())
}

/// One connection's target and, for a connector that signs, its token, opened
/// once for every send the batch holds for it.
pub struct Opened {
    /// The vendor's incoming-webhook URL, or the customer's endpoint.
    pub url: Redacted,
    /// Slack's bot token or the webhook's signing secret.
    pub token: Option<Redacted>,
    /// A webhook's rotated secret while its overlap lasts.
    pub prior: Option<Redacted>,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("token", &self.token.is_some())
            .field("prior", &self.prior.is_some())
            .finish()
    }
}

/// Open every ACTIVE connection in the batch, once each, concurrently.
async fn open_batch(
    state: &Arc<AppState>,
    connections: &HashMap<Uuid, Arc<SealedConnection>>,
    slots: &Arc<Semaphore>,
) -> HashMap<Uuid, Arc<Result<Opened, DeliveryError>>> {
    let mut joins = tokio::task::JoinSet::new();
    for connection in connections.values() {
        if connection.status != "active" {
            continue;
        }
        let state = Arc::clone(state);
        let connection = Arc::clone(connection);
        let slots = Arc::clone(slots);
        joins.spawn(async move {
            let _permit = slots.acquire_owned().await.ok();
            let id = connection.id;
            (id, open(&state, &connection).await)
        });
    }
    let mut opened = HashMap::with_capacity(joins.len());
    while let Some(joined) = joins.join_next().await {
        match joined {
            Ok((id, result)) => {
                opened.insert(id, Arc::new(result));
            }
            Err(e) => {
                tracing::error!(error = %e, "opening a connection's target did not finish");
            }
        }
    }
    opened
}

/// The connector a connection's `provider` column names.
fn connector_for(
    state: &AppState,
    connection: &SealedConnection,
) -> Result<Arc<dyn Connector>, DeliveryError> {
    let provider: Provider = connection
        .provider
        .parse()
        .map_err(|e: String| DeliveryError::terminal(Terminal::OurBug, e))?;
    state.connectors.get(provider).ok_or_else(|| {
        DeliveryError::terminal(
            Terminal::OurBug,
            format!("{provider} is not configured on this deployment"),
        )
    })
}

/// Unwrap a connection's data key and open its sealed fields. A connection
/// this build cannot open is [`Terminal::OurBug`]; a KEK that cannot be
/// REACHED is [`DeliveryError::Held`], because it may open next tick.
pub async fn open(
    state: &AppState,
    connection: &SealedConnection,
) -> Result<Opened, DeliveryError> {
    let connector = connector_for(state, connection)?;
    let Some(kek) = state.kek.as_ref() else {
        return Err(DeliveryError::terminal(
            Terminal::OurBug,
            "CONNECTOR_KEK is not configured; the stored URL cannot be opened",
        ));
    };
    let dek = kek
        .unwrap_dek(
            &state.http,
            &connection.wrapped_dek,
            connection.id.as_bytes(),
            connection.key_version,
        )
        .await
        .map_err(|e| match e {
            KekError::Unavailable(why) => DeliveryError::Held {
                message: format!("kek unavailable: {why}"),
            },
            KekError::Invalid(why) => DeliveryError::terminal(
                Terminal::OurBug,
                format!("the row's wrapped key would not open: {why}"),
            ),
        })?;
    let url = state
        .vault
        .open(&dek, &connection.target(), connection.id.as_bytes())
        .map_err(|e| DeliveryError::terminal(Terminal::OurBug, e.to_string()))?;
    let open_sealed = |sealed: Option<telmoni_shared::envelope::Sealed>| {
        sealed
            .map(|sealed| {
                state
                    .vault
                    .open(&dek, &sealed, connection.id.as_bytes())
                    .map_err(|e| DeliveryError::terminal(Terminal::OurBug, e.to_string()))
            })
            .transpose()
    };
    let (token, prior) = if connector.delivers_with_token() {
        (
            open_sealed(connection.token())?,
            open_sealed(connection.prior_token(chrono::Utc::now()))?,
        )
    } else {
        (None, None)
    };
    Ok(Opened { url, token, prior })
}

/// POST one notice through an already-opened connection, over the guarded
/// client, answering the success status.
pub async fn dispatch(
    state: &AppState,
    connection: &SealedConnection,
    opened: &Opened,
    event: &Event<'_>,
) -> Result<u16, DeliveryError> {
    connector_for(state, connection)?
        .deliver(
            &state.egress,
            &opened.url,
            opened.token.as_ref(),
            opened.prior.as_ref(),
            event,
        )
        .await
}

/// One send and how long it took, from the moment it left.
pub struct Sent {
    pub result: Result<u16, DeliveryError>,
    pub duration_ms: i32,
}

/// [`dispatch`], timed, for the delivery log.
pub async fn dispatch_timed(
    state: &AppState,
    connection: &SealedConnection,
    opened: &Opened,
    event: &Event<'_>,
) -> Sent {
    let started = std::time::Instant::now();
    let result = dispatch(state, connection, opened, event).await;
    Sent {
        result,
        duration_ms: i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX),
    }
}

impl Sent {
    /// The log's record of this send.
    #[must_use]
    pub fn attempt(&self, delivery_id: Uuid, trigger: AttemptTrigger) -> NewAttempt {
        NewAttempt {
            delivery_id,
            trigger,
            status_code: crate::connector::loggable_status(match &self.result {
                Ok(status) => Some(*status),
                Err(e) => e.status(),
            }),
            duration_ms: self.duration_ms,
            error: self.result.as_ref().err().map(|e| e.message().to_owned()),
        }
    }
}

/// Open a connection and POST one notice through it.
pub async fn send(
    state: &AppState,
    connection: &SealedConnection,
    event: &Event<'_>,
) -> Result<u16, DeliveryError> {
    let opened = open(state, connection).await?;
    dispatch(state, connection, &opened, event).await
}

/// What one attempt did, carried out of the concurrent phase so the batch's
/// bookkeeping is one transaction.
/// `attempt` is the delivery log's record of the send, present exactly when
/// something was sent.
enum Outcome {
    /// The far end took it. Terminal, and the connection's breaker is cleared.
    Delivered {
        id: Uuid,
        connection_id: Uuid,
        attempt: Option<NewAttempt>,
    },
    /// Retry with a backoff, and count toward the connection's breaker.
    Transient {
        id: Uuid,
        connection_id: Uuid,
        message: String,
        wait: i64,
        breaker_limit: Option<i32>,
        attempt: Option<NewAttempt>,
    },
    /// Nothing was attempted, so the row goes back with its attempt returned.
    Held { id: Uuid, message: String },
    /// This row is terminal now; the connection stands.
    Spent {
        id: Uuid,
        reason: String,
        attempt: Option<NewAttempt>,
    },
    /// The connection is terminal; it and its whole queue go.
    Retire {
        connection_id: Uuid,
        class: Terminal,
        reason: String,
        attempt: Option<NewAttempt>,
    },
}

/// Attempt every leased row, `delivery_concurrency` at a time.
async fn attempt_batch(
    state: &Arc<AppState>,
    leased: Vec<LeasedDelivery>,
    connections: &HashMap<Uuid, Arc<SealedConnection>>,
    opened: &HashMap<Uuid, Arc<Result<Opened, DeliveryError>>>,
    slots: &Arc<Semaphore>,
) -> Vec<Outcome> {
    let mut outcomes = Vec::with_capacity(leased.len());
    let mut joins = tokio::task::JoinSet::new();

    for delivery in leased {
        let connection = match connections.get(&delivery.connection_id) {
            Some(c) if c.status == "active" => Ok(Arc::clone(c)),
            Some(c) => Err(format!("the connection is {}", c.status)),
            None => Err("the connection no longer exists".to_owned()),
        };
        let connection = match connection {
            Ok(connection) => connection,
            Err(why) => {
                tracing::info!(
                    delivery_id = %delivery.id,
                    connection_id = %delivery.connection_id,
                    reason = %why,
                    "delivery dropped: its connection was retired after it was leased"
                );
                outcomes.push(Outcome::Spent {
                    id: delivery.id,
                    reason: why,
                    attempt: None,
                });
                continue;
            }
        };
        let Some(opened) = opened.get(&delivery.connection_id).map(Arc::clone) else {
            outcomes.push(Outcome::Held {
                id: delivery.id,
                message: "the connection's target was not opened on this tick".to_owned(),
            });
            continue;
        };
        let state = Arc::clone(state);
        let slots = Arc::clone(slots);
        joins.spawn(async move {
            let _permit = slots.acquire_owned().await.ok();
            attempt_one(&state, &connection, &opened, &delivery).await
        });
    }

    while let Some(joined) = joins.join_next().await {
        match joined {
            Ok(outcome) => outcomes.push(outcome),
            Err(e) => {
                tracing::error!(error = %e, "a delivery task did not finish");
            }
        }
    }
    outcomes
}

/// Render one leased row, send it, and say what that means for it.
async fn attempt_one(
    state: &AppState,
    connection: &SealedConnection,
    opened: &Result<Opened, DeliveryError>,
    delivery: &LeasedDelivery,
) -> Outcome {
    // Every call that reached the connector gets a log entry, a send the guard
    // refused before a socket opened included, since the log is where an
    // owner learns why nothing arrived. A target that would not open or a kind
    // with no renderer never reached it, and the row's `last_error` says so.
    let (result, attempt) = match opened {
        Err(e) => (Err(e.clone()), None),
        Ok(opened) => match delivery.kind.parse() {
            Err(e) => (
                Err(DeliveryError::terminal(
                    Terminal::OurBug,
                    format!("no renderer for notification kind: {e}"),
                )),
                None,
            ),
            Ok(kind) => {
                let event = Event {
                    id: delivery.id,
                    kind,
                    title: &delivery.subject,
                    body: &delivery.body,
                };
                let sent = dispatch_timed(state, connection, opened, &event).await;
                let attempt = sent.attempt(delivery.id, AttemptTrigger::Scheduled);
                (sent.result, Some(attempt))
            }
        },
    };

    match result {
        Ok(_) => Outcome::Delivered {
            id: delivery.id,
            connection_id: delivery.connection_id,
            attempt,
        },
        Err(DeliveryError::Transient {
            message,
            retry_after,
            ..
        }) => Outcome::Transient {
            id: delivery.id,
            connection_id: delivery.connection_id,
            wait: next_wait(
                state.config.delivery_backoff_secs,
                delivery.attempts,
                retry_after,
            ),
            message,
            breaker_limit: breaker_limit(state, connection),
            attempt,
        },
        Err(DeliveryError::Held { message }) => Outcome::Held {
            id: delivery.id,
            message,
        },
        Err(DeliveryError::Terminal {
            class,
            reason,
            status,
        }) => match class {
            Terminal::Retire | Terminal::BadTarget => Outcome::Retire {
                connection_id: delivery.connection_id,
                class,
                reason,
                attempt,
            },
            Terminal::OurBug | Terminal::Unknown => {
                // The vendor's sentence goes to the row, never the log: a far
                // end echoes what it was sent, which names people.
                tracing::error!(
                    delivery_id = %delivery.id,
                    connection_id = %delivery.connection_id,
                    kind = %delivery.kind,
                    class = ?class,
                    status = ?status,
                    "delivery refused by the vendor for a reason retrying cannot fix"
                );
                Outcome::Spent {
                    id: delivery.id,
                    reason,
                    attempt,
                }
            }
        },
    }
}

/// How many consecutive failed sends retire this connection, if its
/// connector keeps a breaker.
fn breaker_limit(state: &AppState, connection: &SealedConnection) -> Option<i32> {
    let provider: Provider = connection.provider.parse().ok()?;
    state.connectors.get(provider)?.consecutive_failure_limit()
}

/// Write the whole batch's bookkeeping, then retire whatever it condemned.
async fn record(state: &Arc<AppState>, outcomes: Vec<Outcome>) -> Result<(), TelmoniError> {
    let mut delivered: Vec<Uuid> = Vec::new();
    let mut touched: Vec<Uuid> = Vec::new();
    let mut failures: Vec<FailedAttempt> = Vec::new();
    let mut held: HashMap<String, Vec<Uuid>> = HashMap::new();
    let mut breaker: HashMap<Uuid, (i32, i32, String)> = HashMap::new();
    let mut retires: Vec<(Uuid, Terminal, String)> = Vec::new();
    let mut attempts: Vec<NewAttempt> = Vec::new();

    for outcome in outcomes {
        match outcome {
            Outcome::Delivered {
                id,
                connection_id,
                attempt,
            } => {
                delivered.push(id);
                touched.push(connection_id);
                attempts.extend(attempt);
            }
            Outcome::Transient {
                id,
                connection_id,
                message,
                wait,
                breaker_limit,
                attempt,
            } => {
                attempts.extend(attempt);
                failures.push(FailedAttempt {
                    id,
                    error: message.clone(),
                    max_attempts: state.config.delivery_max_attempts,
                    backoff_secs: wait,
                });
                if let Some(limit) = breaker_limit {
                    let entry = breaker
                        .entry(connection_id)
                        .or_insert_with(|| (0, limit, String::new()));
                    entry.0 += 1;
                    entry.2 = message;
                }
            }
            Outcome::Held { id, message } => held.entry(message).or_default().push(id),
            Outcome::Spent {
                id,
                reason,
                attempt,
            } => {
                attempts.extend(attempt);
                failures.push(FailedAttempt {
                    id,
                    error: reason,
                    max_attempts: 0,
                    backoff_secs: 0,
                });
            }
            Outcome::Retire {
                connection_id,
                class,
                reason,
                attempt,
            } => {
                attempts.extend(attempt);
                retires.push((connection_id, class, reason));
            }
        }
    }

    touched.sort_unstable();
    touched.dedup();
    for why in held.keys() {
        note_rows_held(why);
    }

    let tallies: Vec<(Uuid, i32)> = breaker.iter().map(|(&id, &(n, ..))| (id, n)).collect();

    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    db::mark_delivered(&mut tx, &delivered).await?;
    db::touch_connections(&mut tx, &touched).await?;
    db::fail_deliveries(&mut tx, &failures).await?;
    for (why, ids) in &held {
        db::unlease_deliveries(&mut tx, ids, why).await?;
    }
    let counts = db::record_consecutive_failures(&mut tx, &tallies).await?;
    tx.commit().await?;

    // The log in its own transaction, after the queue's: it is a record of
    // what happened, and nothing in it may undo what happened. A failed write
    // here loses these log rows, never the batch's outcomes.
    if let Err(e) = log_attempts(state, &attempts).await {
        tracing::warn!(error = %e, sends = attempts.len(), "the delivery log missed a batch of sends");
    }

    for (connection_id, count) in counts {
        if let Some((_, limit, last)) = breaker.get(&connection_id)
            && count >= *limit
        {
            retires.push((
                connection_id,
                Terminal::BadTarget,
                format!("{count} deliveries in a row failed; the last answer was: {last}"),
            ));
        }
    }

    retires.sort_by_key(|&(connection_id, ..)| connection_id);
    retires.dedup_by_key(|&mut (connection_id, ..)| connection_id);
    for (connection_id, class, reason) in retires {
        retire(state, connection_id, class, &reason).await?;
    }
    Ok(())
}

async fn log_attempts(state: &AppState, attempts: &[NewAttempt]) -> Result<(), TelmoniError> {
    if attempts.is_empty() {
        return Ok(());
    }
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    db::record_attempts(&mut tx, attempts).await?;
    tx.commit().await?;
    Ok(())
}

/// Retire a connection on the far end's word: `revoked` for a dead install,
/// `errored` for a bad target or a tripped breaker. A connection no longer
/// `active` — a concurrent attempt got there first — changes nothing.
pub async fn retire(
    state: &Arc<AppState>,
    connection_id: Uuid,
    class: Terminal,
    reason: &str,
) -> Result<(), TelmoniError> {
    let status = match class {
        Terminal::Retire => "revoked",
        Terminal::BadTarget => "errored",
        Terminal::OurBug | Terminal::Unknown => return Ok(()),
    };
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let Some(retired) = db::retire_connection(&mut tx, connection_id, status, reason).await? else {
        tx.commit().await?;
        return Ok(());
    };
    let failed = db::fail_pending_for_connection(&mut tx, connection_id, reason).await?;
    let notices = notify::connector_disconnected(&mut tx, &retired, reason).await?;
    tx.commit().await?;
    // The far end's answer is kept on the row and told to the project; the
    // log gets the class, because that answer is the receiver's own text.
    tracing::error!(
        connection_id = %connection_id,
        project_id = %retired.project_id,
        provider = %retired.provider,
        status,
        class = ?class,
        failed,
        "connector retired on the far end's answer"
    );
    spawn_first_attempts(state, notices);
    Ok(())
}

/// The wait before the next attempt: `base × 4^(attempts−1)`, capped at a
/// day, so five attempts outlast a fifteen-minute deploy.
fn backoff_secs(base: i64, attempts: i32) -> i64 {
    const DAY: i64 = 86_400;
    let exponent = u32::try_from(attempts.saturating_sub(1)).unwrap_or(0);
    base.max(0)
        .saturating_mul(4_i64.saturating_pow(exponent))
        .min(DAY)
}

/// The wait before the next attempt, given what the far end asked for.
fn next_wait(base: i64, attempts: i32, retry_after: Option<i64>) -> i64 {
    const DAY: i64 = 86_400;
    let scheduled = backoff_secs(base, attempts);
    retry_after.map_or(scheduled, |asked| asked.max(scheduled).min(DAY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_grows_past_a_deploy_and_stops_at_a_day() {
        assert_eq!(backoff_secs(30, 1), 30);
        assert_eq!(backoff_secs(30, 2), 120);
        assert_eq!(backoff_secs(30, 3), 480);
        assert_eq!(backoff_secs(30, 5), 7_680);
        assert_eq!(backoff_secs(30, 20), 86_400, "capped at a day");
        assert_eq!(
            backoff_secs(-5, 3),
            0,
            "a negative base is no wait, never a past gate"
        );
        assert_eq!(backoff_secs(30, 0), 30);
    }

    /// ⚠ The schedule is the FLOOR. A far end may ask us to wait longer, never
    /// shorter.
    #[test]
    fn a_far_end_can_ask_for_longer_but_never_for_sooner() {
        assert_eq!(next_wait(30, 2, None), backoff_secs(30, 2));
        assert_eq!(next_wait(30, 3, Some(1)), backoff_secs(30, 3));
        assert_eq!(next_wait(30, 3, Some(0)), backoff_secs(30, 3));
        assert_eq!(next_wait(30, 1, Some(900)), 900);
        assert_eq!(next_wait(30, 1, Some(31_536_000)), 86_400);
    }

    /// The inline attempt is a latency trick, never the load.
    #[test]
    fn inline_first_attempts_are_bounded_and_the_overflow_is_dropped() {
        let held: Vec<_> = (0..FIRST_ATTEMPT_INFLIGHT)
            .map(|_| {
                FIRST_ATTEMPT_SLOTS
                    .try_acquire()
                    .expect("a free slot under the limit")
            })
            .collect();
        assert!(
            FIRST_ATTEMPT_SLOTS.try_acquire().is_err(),
            "the {FIRST_ATTEMPT_INFLIGHT}th slot must be the last"
        );
        drop(held);
        assert!(FIRST_ATTEMPT_SLOTS.try_acquire().is_ok());
    }
}
