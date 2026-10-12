//! Trace and span ids: random, as OpenTelemetry's own generator makes them,
//! except for the spans of a recorded run, whose ids are derived from the
//! run's id, so that a record sent twice is the same bytes and ingest's claim
//! on the body answers it once.

use std::cell::Cell;
use std::marker::PhantomData;

use opentelemetry::trace::{SpanId, TraceId};
use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator};
use uuid::Uuid;

thread_local! {
    /// The ids a recorded span takes as it opens. On the thread rather than
    /// on the generator: the tracer asks for ids on the thread opening the
    /// span, so another thread opening a span of its own meanwhile never takes
    /// them.
    static GIVEN: Cell<Option<(TraceId, SpanId)>> = const { Cell::new(None) };
}

/// The client's id generator: what [`given`] holds on this thread, else
/// OpenTelemetry's random ids.
#[derive(Debug, Default)]
pub(crate) struct Ids {
    random: RandomIdGenerator,
}

impl IdGenerator for Ids {
    fn new_trace_id(&self) -> TraceId {
        GIVEN
            .get()
            .map_or_else(|| self.random.new_trace_id(), |(trace_id, _)| trace_id)
    }

    fn new_span_id(&self) -> SpanId {
        GIVEN
            .get()
            .map_or_else(|| self.random.new_span_id(), |(_, span_id)| span_id)
    }
}

/// Gives these ids to the span opened on this thread until the guard drops.
pub(crate) fn given(trace_id: TraceId, span_id: SpanId) -> Given {
    Given {
        previous: GIVEN.replace(Some((trace_id, span_id))),
        _thread: PhantomData,
    }
}

/// Puts back what the thread held before [`given`] when dropped, a panic
/// unwinding through included: a sampler of the application's runs while the
/// span opens, and the ids must not outlive it.
#[must_use = "the ids are given only while the guard lives"]
pub(crate) struct Given {
    previous: Option<(TraceId, SpanId)>,
    /// Not `Send`: what it puts back is its own thread's.
    _thread: PhantomData<*const ()>,
}

impl Drop for Given {
    fn drop(&mut self) {
        GIVEN.set(self.previous);
    }
}

/// A recorded run's trace id: the run's id, its sixteen bytes.
pub(crate) fn trace_id(run_id: &Uuid) -> TraceId {
    TraceId::from_bytes(*run_id.as_bytes())
}

/// The id of a recorded run's span at `place`: the first eight bytes of the
/// version 5 UUID (RFC 9562) named by the place, in decimal, under the run's
/// id. Its version's bits are among those eight, so it is never zero.
pub(crate) fn span_id(run_id: &Uuid, place: u64) -> SpanId {
    let [a, b, c, d, e, f, g, h, ..] =
        *Uuid::new_v5(run_id, place.to_string().as_bytes()).as_bytes();
    SpanId::from_bytes([a, b, c, d, e, f, g, h])
}
