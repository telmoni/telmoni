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
use crate::error::Error;
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
    /// Told to give up by a shutdown past its deadline.
    exporter: Arc<Exporter>,
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
    /// The record being built, with the room its hold keeps for it.
    stage: Option<Stage>,
    dropped: usize,
    flushing: usize,
    exporting: bool,
    life: Life,
}

/// Where the processor is in its life: taking spans, draining them for a
/// shutdown, or with its worker gone, which `shutdown` waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Life {
    #[default]
    Running,
    Stopping,
    Finished,
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
        let exporting = Arc::clone(&exporter);
        let spawned = std::thread::Builder::new()
            .name("telmoni-exporter".to_owned())
            .spawn(move || work(&worker, &exporting, &warner));
        if let Err(error) = spawned {
            // Without the worker nothing leaves, and nothing should wait for it.
            lock(&shared.state).life = Life::Finished;
            exporter_thread_failed(error.kind());
        }
        Self { shared, exporter }
    }

    /// Stages the spans of the record in `trace` as they end, for
    /// [`Hold::queue`] to queue whole, to travel in exports of their own: so
    /// the same record sent again is the same bytes, whatever else ends
    /// meanwhile. One record at a time, its room kept for it; refused, holding
    /// nothing, once shut down or when the queue has none for its `need` spans.
    pub(crate) fn hold(&self, trace: TraceId, need: usize) -> Result<Hold<'_>, Error> {
        let recording = lock(&self.shared.recording);
        let mut state = lock(&self.shared.state);
        if state.life != Life::Running {
            return Err(Error::Shutdown);
        }
        if state.queue.len() + need > self.shared.batching.max_queue_size {
            return Err(Error::QueueFull { spans: need });
        }
        state.stage = Some(Stage {
            trace,
            spans: Vec::new(),
            reserved: need,
        });
        Ok(Hold {
            shared: &self.shared,
            built: false,
            _recording: recording,
        })
    }

    /// Everything queued sent, waiting up to `timeout`.
    pub(crate) fn flush_within(&self, timeout: Duration) -> OTelSdkResult {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared.state);
        if state.life != Life::Running {
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
        if state.life != Life::Running {
            return;
        }
        if let Some(stage) = &mut state.stage
            && span.span_context.trace_id() == stage.trace
        {
            stage.spans.push(span);
            return;
        }
        let reserved = state.stage.as_ref().map_or(0, |stage| stage.reserved);
        if state.queue.len() + reserved >= self.shared.batching.max_queue_size {
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

    /// Drains, then takes nothing more. Past the timeout what is left is given
    /// up: no batch or attempt follows, as in the other SDKs.
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared.state);
        if state.life != Life::Running {
            return Ok(());
        }
        state.life = Life::Stopping;
        self.shared.wake.notify_all();
        while state.life != Life::Finished {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                state.queue.clear();
                state.fences.clear();
                self.exporter.give_up();
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
            if state.life != Life::Running && state.queue.is_empty() {
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
    state.life != Life::Running
        || state.flushing > 0
        || !state.fences.is_empty()
        || state.queue.len() >= batching.max_batch_size
}

/// A record being built: its trace, its spans as they end, and the room in
/// the queue kept for them.
struct Stage {
    trace: TraceId,
    spans: Vec<SpanData>,
    reserved: usize,
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
        if !self.built || state.life != Life::Running || staged.is_empty() {
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
        state.life = Life::Finished;
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::UNIX_EPOCH;

    use opentelemetry::InstrumentationScope;
    use opentelemetry::trace::{SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceState};
    use opentelemetry_sdk::trace::{SpanEvents, SpanLinks};

    use super::*;
    use crate::config::Config;
    use crate::transport::{Answer, Transport, TransportError};

    /// Answers every export 429, the first only once the test opens the gate.
    #[derive(Clone, Default)]
    struct Throttled {
        sent: Arc<AtomicUsize>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Throttled {
        fn open(&self) {
            *lock(&self.gate.0) = true;
            self.gate.1.notify_all();
        }
    }

    impl Transport for Throttled {
        fn send(
            &self,
            _url: &str,
            _headers: &[(String, String)],
            _body: &[u8],
        ) -> Result<Answer, TransportError> {
            if self.sent.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut open = lock(&self.gate.0);
                while !*open {
                    open = self
                        .gate
                        .1
                        .wait(open)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
            Ok(Answer {
                status: 429,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            })
        }
    }

    fn span(n: u8) -> SpanData {
        SpanData {
            span_context: SpanContext::new(
                TraceId::from_bytes([n; 16]),
                SpanId::from_bytes([n; 8]),
                TraceFlags::SAMPLED,
                false,
                TraceState::default(),
            ),
            parent_span_id: SpanId::INVALID,
            parent_span_is_remote: false,
            span_kind: SpanKind::Internal,
            name: "s".into(),
            start_time: UNIX_EPOCH,
            end_time: UNIX_EPOCH,
            attributes: Vec::new(),
            dropped_attributes_count: 0,
            events: SpanEvents::default(),
            links: SpanLinks::default(),
            status: Status::Unset,
            instrumentation_scope: InstrumentationScope::default(),
        }
    }

    /// Past a shutdown's deadline the attempt in flight is the last: no batch or attempt follows.
    #[test]
    fn a_shutdown_past_its_deadline_starts_no_batch_or_attempt() {
        let transport = Throttled::default();
        let warner = Arc::new(Warner::new(Box::new(|_| {})));
        let config = Config {
            api_key: Some("k".to_owned()),
            ..Config::default()
        };
        let exporter = Arc::new(Exporter::new(
            &config,
            Box::new(transport.clone()),
            Box::new(|_| {}),
            Arc::clone(&warner),
        ));
        // Batches of one: the second span waits behind the first, whose attempt hangs.
        let batching = Batching {
            max_queue_size: 2048,
            max_batch_size: 1,
            schedule_delay: Duration::from_secs(60),
        };
        let processor = Processor::start(exporter, batching, warner);
        processor.on_end(span(1));
        processor.on_end(span(2));
        let started = Instant::now();
        while transport.sent.load(Ordering::SeqCst) == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "no attempt left"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            processor
                .shutdown_with_timeout(Duration::from_millis(50))
                .is_err()
        );
        transport.open();
        while lock(&processor.shared.state).life != Life::Finished {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the worker stayed"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(transport.sent.load(Ordering::SeqCst), 1);
    }
}
