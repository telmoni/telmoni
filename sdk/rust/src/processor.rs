//! The SDK's own batch processor: OpenTelemetry's batching and bounded queue,
//! without the check that skips a span whose trace is not sampled.
//!
//! A run inside a trace the application does not sample is still sent; the
//! run's own sample rate is applied here, by run, never through the trace's
//! flag. The queue is bounded, so a slow Telmoni never slows an agent: past
//! its size, spans are dropped and counted. `telmoni.run.start` wakes the
//! worker at once, unless a hold keeps every batch back until what is queued
//! under it is whole.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use opentelemetry::{Context, Value};
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::trace::{Span, SpanData, SpanProcessor};

use crate::contract;
use crate::exporter::Exporter;
use crate::lock;
use crate::warnings::Warner;

/// How spans are batched on their way out: OpenTelemetry's batch processor's
/// numbers, which are its defaults here too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batching {
    /// How many ended spans wait to be sent before further ones are dropped
    /// and counted; a slow platform never holds the agent up.
    pub max_queue_size: usize,
    /// How many spans one export carries at most; a full batch is sent
    /// without waiting for the delay.
    pub max_batch_size: usize,
    /// How long spans wait for company before they are sent anyway.
    pub schedule_delay: Duration,
}

impl Default for Batching {
    fn default() -> Self {
        Self {
            max_queue_size: 2048,
            max_batch_size: 512,
            schedule_delay: Duration::from_secs(5),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Processor {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    batching: Batching,
}

#[derive(Default)]
struct State {
    queue: VecDeque<SpanData>,
    /// The runs the sample rate kept out, by id, until each run's own span ends.
    sampled_out: HashSet<String>,
    dropped: usize,
    urgent: bool,
    flushing: usize,
    /// How many holds are on: while any is, the worker takes no batch unless
    /// shutting down.
    held: usize,
    shutdown: bool,
    exporting: bool,
    /// Whether the worker has returned, which `shutdown` waits for.
    finished: bool,
}

impl Processor {
    pub(crate) fn start(exporter: Arc<Exporter>, batching: Batching, warner: Arc<Warner>) -> Self {
        // A queue of nothing would drop every span, and a batch of nothing
        // would wake the worker forever: each is at least one.
        let batching = Batching {
            max_queue_size: batching.max_queue_size.max(1),
            max_batch_size: batching.max_batch_size.max(1),
            schedule_delay: batching.schedule_delay,
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            batching,
        });
        let worker = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("telmoni-exporter".to_owned())
            .spawn(move || work(&worker, &exporter, &warner));
        if let Err(error) = spawned {
            // Without the worker nothing leaves, and nothing should wait for it.
            let mut state = lock(&shared.state);
            state.shutdown = true;
            state.finished = true;
            drop(state);
            exporter_thread_failed(error.kind());
        }
        Self { shared }
    }

    /// Drop every span of this run, the run's own among them.
    pub(crate) fn sample_out(&self, run_id: &str) {
        lock(&self.shared.state)
            .sampled_out
            .insert(run_id.to_owned());
    }

    /// Takes no batch until the guard drops, so the spans a record queues
    /// meanwhile travel together, in their order, and the record sent again
    /// is the same bytes. `None`, holding nothing, when the queue has no room
    /// for the record's `need` spans, so a record is queued whole or not at
    /// all; a live span ending meanwhile still takes a slot.
    pub(crate) fn hold(&self, need: usize) -> Option<Hold<'_>> {
        let mut state = lock(&self.shared.state);
        if state.queue.len() + need > self.shared.batching.max_queue_size {
            return None;
        }
        state.held += 1;
        Some(Hold(&self.shared))
    }

    /// Everything queued sent, waiting up to `timeout`.
    pub(crate) fn flush_within(&self, timeout: Duration) -> OTelSdkResult {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared.state);
        if state.shutdown {
            return Ok(());
        }
        state.flushing += 1;
        self.shared.wake.notify_all();
        let result = loop {
            if state.queue.is_empty() && !state.exporting {
                break Ok(());
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break Err(OTelSdkError::Timeout(timeout));
            };
            state = self
                .shared
                .wake
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        state.flushing -= 1;
        result
    }
}

impl fmt::Debug for Processor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Processor")
            .field("batching", &self.shared.batching)
            .finish_non_exhaustive()
    }
}

impl SpanProcessor for Processor {
    fn on_start(&self, _span: &mut Span, _cx: &Context) {}

    fn on_end(&self, span: SpanData) {
        let mut state = lock(&self.shared.state);
        if state.shutdown {
            return;
        }
        if let Some(run_id) = attribute(&span, contract::RUN_ID)
            && state.sampled_out.contains(run_id)
        {
            if attribute(&span, contract::OPERATION)
                .is_some_and(|operation| contract::RUN_OPERATIONS.contains(&operation))
            {
                state.sampled_out.remove(run_id);
            }
            return;
        }
        if state.queue.len() >= self.shared.batching.max_queue_size {
            state.dropped += 1;
            return;
        }
        if span.name == contract::RUN_START {
            state.urgent = true;
        }
        state.queue.push_back(span);
        if state.urgent || state.queue.len() >= self.shared.batching.max_batch_size {
            self.shared.wake.notify_all();
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.flush_within(Duration::from_secs(30))
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared.state);
        if state.shutdown {
            return Ok(());
        }
        state.shutdown = true;
        self.shared.wake.notify_all();
        while !state.finished {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(OTelSdkError::Timeout(timeout));
            };
            state = self
                .shared
                .wake
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        Ok(())
    }
}

/// The worker: waits until a batch is due, takes it, sends it.
fn work(shared: &Shared, exporter: &Exporter, warner: &Warner) {
    let _finished = Finished(shared);
    loop {
        let (batch, dropped) = {
            let mut state = lock(&shared.state);
            if !due(&state, &shared.batching) {
                state = shared
                    .wake
                    .wait_timeout(state, shared.batching.schedule_delay)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            if state.shutdown && state.queue.is_empty() {
                return;
            }
            if state.held > 0 && !state.shutdown {
                continue;
            }
            let size = state.queue.len().min(shared.batching.max_batch_size);
            let batch: Vec<SpanData> = state.queue.drain(..size).collect();
            let dropped = std::mem::take(&mut state.dropped);
            state.urgent = false;
            state.exporting = !batch.is_empty();
            (batch, dropped)
        };
        // A hook of the application's — its `warn`, `sleep` or `transport` —
        // may panic; each call is caught on its own, so the worker outlives
        // it, a warning that panics costs no batch, and the next batch still
        // goes. A panic during an export loses that batch, said where the hook
        // cannot reach.
        if dropped > 0 {
            let warned = panic::catch_unwind(AssertUnwindSafe(|| {
                warner.warn(&format!(
                    "telmoni: the export queue was full; {dropped} spans were dropped"
                ));
            }));
            if warned.is_err() {
                tracing::warn!(target: "telmoni", "telmoni: the warn hook panicked");
            }
        }
        if !batch.is_empty()
            && panic::catch_unwind(AssertUnwindSafe(|| exporter.export(&batch))).is_err()
        {
            tracing::warn!(
                target: "telmoni",
                "telmoni: a hook panicked during an export; the batch was dropped"
            );
        }
        let mut state = lock(&shared.state);
        state.exporting = false;
        shared.wake.notify_all();
    }
}

/// Under the lock: whether the worker should export without waiting.
fn due(state: &State, batching: &Batching) -> bool {
    if state.held > 0 && !state.shutdown {
        return false;
    }
    state.shutdown
        || state.flushing > 0
        || state.urgent
        || state.queue.len() >= batching.max_batch_size
}

/// A hold on the worker's batches, lifted when dropped, a panic unwinding
/// through included.
pub(crate) struct Hold<'a>(&'a Shared);

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        lock(&self.0.state).held -= 1;
        // The worker may have slept through what was queued under the hold.
        self.0.wake.notify_all();
    }
}

/// Marks the worker finished however it returns, so a shutdown never waits
/// out its timeout for a worker that is gone, nor a flush for an export the
/// worker left in flight.
struct Finished<'a>(&'a Shared);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.finished = true;
        state.exporting = false;
        drop(state);
        self.0.wake.notify_all();
    }
}

fn attribute<'a>(span: &'a SpanData, key: &str) -> Option<&'a str> {
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)
        .and_then(|attribute| match &attribute.value {
            Value::String(text) => Some(text.as_str()),
            _ => None,
        })
}

fn exporter_thread_failed(kind: std::io::ErrorKind) {
    tracing::warn!(
        target: "telmoni",
        "telmoni: the exporter's thread could not start ({kind}); nothing will be sent"
    );
}
