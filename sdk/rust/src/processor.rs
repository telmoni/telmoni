//! The SDK's own batch processor: OpenTelemetry's batching and bounded queue,
//! without the check that skips a span whose trace is not sampled.
//!
//! A run inside a trace the application does not sample is still sent; the
//! run's own sample rate is applied here, by run, never through the trace's
//! flag. The queue is bounded, so a slow Telmoni never slows an agent: past
//! its size, spans are dropped and counted. A recorded run goes at once, in
//! exports of its own.

use std::collections::VecDeque;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use opentelemetry::Context;
use opentelemetry::trace::TraceId;
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
    /// Lets one record at a time be built.
    recording: Mutex<()>,
}

#[derive(Default)]
struct State {
    queue: VecDeque<SpanData>,
    /// The queue's first span is the `taken`th ever queued; a batch stops at
    /// each fence, a record's first span and one past its last.
    taken: usize,
    fences: VecDeque<usize>,
    /// The record being built, and the room its hold keeps for it.
    stage: Option<Stage>,
    reserved: usize,
    dropped: usize,
    flushing: usize,
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
            recording: Mutex::new(()),
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

    /// Stages the spans of the record in `trace` as they end, for
    /// [`Hold::queue`] to queue whole, to travel in exports of their own: so
    /// the same record sent again is the same bytes, whatever else ends
    /// meanwhile. One record at a time, its room kept for it; `None`, holding
    /// nothing, when the queue has none for its `need` spans.
    pub(crate) fn hold(&self, trace: TraceId, need: usize) -> Option<Hold<'_>> {
        let recording = lock(&self.shared.recording);
        let mut state = lock(&self.shared.state);
        if state.queue.len() + need > self.shared.batching.max_queue_size {
            return None;
        }
        state.stage = Some(Stage {
            trace,
            spans: Vec::new(),
        });
        state.reserved = need;
        Some(Hold {
            shared: &self.shared,
            built: false,
            _recording: recording,
        })
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
        if span
            .attributes
            .iter()
            .any(|attribute| attribute.key.as_str() == contract::SAMPLED_OUT)
        {
            return;
        }
        let mut state = lock(&self.shared.state);
        if state.shutdown {
            return;
        }
        if let Some(stage) = &mut state.stage
            && span.span_context.trace_id() == stage.trace
        {
            stage.spans.push(span);
            return;
        }
        if state.queue.len() + state.reserved >= self.shared.batching.max_queue_size {
            state.dropped += 1;
            return;
        }
        state.queue.push_back(span);
        if state.queue.len() >= self.shared.batching.max_batch_size {
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
            let mut size = state.queue.len().min(shared.batching.max_batch_size);
            if let Some(&fence) = state.fences.front() {
                size = size.min(fence.saturating_sub(state.taken));
            }
            let batch: Vec<SpanData> = state.queue.drain(..size).collect();
            state.taken += size;
            while state
                .fences
                .front()
                .is_some_and(|&fence| fence <= state.taken)
            {
                state.fences.pop_front();
            }
            let dropped = std::mem::take(&mut state.dropped);
            state.exporting = !batch.is_empty();
            (batch, dropped)
        };
        // A hook of the application's may panic: the warner catches its sink's,
        // and an export's is caught here, losing that batch alone, said where
        // the hook cannot reach, so the worker outlives either.
        if dropped > 0 {
            warner.warn(&format!(
                "telmoni: the export queue was full; {dropped} spans were dropped"
            ));
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

/// Under the lock: whether the worker should export without waiting; a
/// record queued goes at once.
fn due(state: &State, batching: &Batching) -> bool {
    state.shutdown
        || state.flushing > 0
        || !state.fences.is_empty()
        || state.queue.len() >= batching.max_batch_size
}

/// A record being built: its trace, and its spans as they end.
struct Stage {
    trace: TraceId,
    spans: Vec<SpanData>,
}

/// A record being built, its spans staged until the guard drops: queued
/// whole if [`Hold::queue`] said it was built, dropped otherwise, a panic
/// unwinding through included.
pub(crate) struct Hold<'a> {
    shared: &'a Shared,
    built: bool,
    _recording: MutexGuard<'a, ()>,
}

impl Hold<'_> {
    /// Queues the record whole: every span of it has ended.
    pub(crate) fn queue(mut self) {
        self.built = true;
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        let staged = state
            .stage
            .take()
            .map(|stage| stage.spans)
            .unwrap_or_default();
        state.reserved = 0;
        if !self.built || state.shutdown || staged.is_empty() {
            return;
        }
        let first = state.taken + state.queue.len();
        if first > state.taken {
            state.fences.push_back(first);
        }
        state.fences.push_back(first + staged.len());
        state.queue.extend(staged);
        drop(state);
        self.shared.wake.notify_all();
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

fn exporter_thread_failed(kind: std::io::ErrorKind) {
    tracing::warn!(
        target: "telmoni",
        "telmoni: the exporter's thread could not start ({kind}); nothing will be sent"
    );
}
