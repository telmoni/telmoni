//! The client: a tracer provider of its own, and the traces it records.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
use opentelemetry::{Context, InstrumentationScope, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider, SpanLimits, SpanProcessor as _};

use crate::VERSION;
use crate::config::Config;
use crate::contract;
use crate::error::Error;
use crate::exporter::{ContentMode, Exporter};
use crate::guard::{OwnRates, own_rates};
use crate::ids::Ids;
use crate::processor::{Batching, Processor};
use crate::propagation::{self, Carrier};
use crate::records::{self, TraceRecord};
use crate::sampler::RecordingSampler;
use crate::scores::{self, Score, ScoreTarget};
use crate::traces::{self, Trace, TraceOptions};
use crate::transport::{Reqwest, Transport};
use crate::warnings::{Sink, Warner, Warning};

/// How long [`Telmoni::flush`] and [`Telmoni::shutdown`] wait.
const WAIT: Duration = Duration::from_secs(30);

/// What the client is built on, each part the real one unless given: the
/// transport, the sleep between attempts, the warning sink and the batching.
/// Tests inject theirs here.
pub struct Hooks {
    /// Sends each export; reqwest's blocking client unless given.
    pub transport: Box<dyn Transport>,
    /// Waits between attempts at one export; `std::thread::sleep` unless given.
    pub sleep: Box<dyn Fn(Duration) + Send + Sync>,
    /// Takes each warning's line and what it describes; `tracing::warn!` under
    /// the target `telmoni`, with its fields, unless given.
    pub warn: Box<Sink>,
    /// How spans are batched on their way out.
    pub batching: Batching,
}

impl Default for Hooks {
    fn default() -> Self {
        Self {
            transport: Box::new(Reqwest::default()),
            sleep: Box::new(std::thread::sleep),
            warn: Box::new(|text: &str, warning: &Warning| {
                // An `Option` field records nothing when it is `None`.
                tracing::warn!(
                    target: "telmoni",
                    kind = warning.kind.as_str(),
                    status = warning.status,
                    problem_type = warning.problem_type.as_deref(),
                    "{text}"
                );
            }),
            batching: Batching::default(),
        }
    }
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hooks")
            .field("batching", &self.batching)
            .finish_non_exhaustive()
    }
}

/// What the client and its traces share.
pub(crate) struct Inner {
    pub(crate) config: Config,
    pub(crate) tracer: SdkTracer,
    provider: SdkTracerProvider,
    pub(crate) processor: Processor,
    exporter: Arc<Exporter>,
    pub(crate) warner: Arc<Warner>,
    /// The project's own prices, which the guard counts a call by first.
    pub(crate) prices: OwnRates,
}

/// The client: a tracer provider of its own, and the traces it records.
///
/// The provider is built part by part — resource, sampler, limits, id
/// generator, processor — so no `OTEL_*` variable reaches it, and an
/// application's own OpenTelemetry is neither replaced, sent to Telmoni, nor
/// changed. Cloning shares the client.
#[derive(Clone)]
pub struct Telmoni {
    inner: Arc<Inner>,
}

impl Telmoni {
    /// A client on `config`, built on the real transport, sleep and warning
    /// channel. Safe to call inside an async runtime: nothing blocking is
    /// built until the processor's own thread sends the first export.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self::with_hooks(config, Hooks::default())
    }

    /// A client on [`Config::from_env`].
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(Config::from_env())
    }

    /// A client on `config`, built on `hooks`.
    #[must_use]
    pub fn with_hooks(config: Config, hooks: Hooks) -> Self {
        let config = config.normalized();
        let warner = Arc::new(Warner::new(hooks.warn));
        let exporter = Arc::new(Exporter::new(
            &config,
            hooks.transport,
            hooks.sleep,
            Arc::clone(&warner),
        ));
        let processor =
            Processor::start(Arc::clone(&exporter), hooks.batching, Arc::clone(&warner));
        let resource = Resource::builder_empty()
            .with_attributes([
                KeyValue::new(contract::DISTRO_NAME, contract::DISTRO),
                KeyValue::new("telemetry.distro.version", VERSION),
            ])
            .build();
        // Every part given, since the builder's defaults read OTEL_TRACES_SAMPLER,
        // OTEL_SPAN_* and, for the resource, OTEL_RESOURCE_ATTRIBUTES and
        // OTEL_SERVICE_NAME, which are the application's.
        let provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_sampler(RecordingSampler::new(config.sampler.clone()))
            // One link: the application's span a trace is opened beside.
            .with_span_limits(SpanLimits {
                max_attributes_per_span: 128,
                max_events_per_span: 32,
                max_links_per_span: 1,
                max_attributes_per_event: 32,
                max_attributes_per_link: 0,
            })
            .with_id_generator(Ids::default())
            .with_span_processor(processor.clone())
            .build();
        let tracer = provider.tracer_with_scope(
            InstrumentationScope::builder(contract::SCOPE)
                .with_version(VERSION)
                .build(),
        );
        let prices = own_rates(&config.prices, &warner);
        Self {
            inner: Arc::new(Inner {
                config,
                tracer,
                provider,
                processor,
                exporter,
                warner,
                prices,
            }),
        }
    }

    /// The configuration, as the client uses it.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// The project's content mode, as the last answer named it; `None` before
    /// one, or for a word ingest does not send, where the other SDKs hand the
    /// word back.
    #[must_use]
    pub fn content_mode(&self) -> Option<ContentMode> {
        self.inner.exporter.content_mode()
    }

    /// A trace of `agent`, as a guard: the span `invoke_agent <agent>` at the
    /// root of a trace of its own, linked to whatever span the application
    /// had current, ended when the guard drops or on [`Trace::end`]. An agent
    /// past 128 characters is cut to them, and a tag out of range, a tag past
    /// the first 32 by key, or a user or session past its cap is left off,
    /// each with one warning.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyAgent`] for a blank name; [`Error::MaxSteps`] for a
    /// `max_steps` of 0; [`Error::MaxSpend`] for a `max_spend` not a finite
    /// number of US dollars above 0.
    pub fn trace(&self, agent: &str, options: TraceOptions) -> Result<Trace, Error> {
        traces::open(&self.inner, agent, options)
    }

    /// [`Self::trace`] around `f`: the trace ends when `f` returns, and `f`'s
    /// value is returned.
    ///
    /// # Errors
    ///
    /// As [`Self::trace`].
    pub fn trace_with<R>(
        &self,
        agent: &str,
        options: TraceOptions,
        f: impl FnOnce(&Trace) -> R,
    ) -> Result<R, Error> {
        let trace = self.trace(agent, options)?;
        let value = f(&trace);
        trace.end();
        Ok(value)
    }

    /// [`Self::trace_with`] for a fallible `f`: an `Err` marks the trace
    /// failed with `E`'s type name (its last path segment) and its message,
    /// content under the exporter's lock, and is returned unchanged. An `Err`
    /// is the only error a closure can carry out; a panic unwinding through
    /// marks the span `panic`.
    ///
    /// # Errors
    ///
    /// `f`'s, or a trace that did not open ([`Self::trace`]), turned into `E`.
    pub fn try_trace_with<T, E: From<Error> + std::fmt::Display>(
        &self,
        agent: &str,
        options: TraceOptions,
        f: impl FnOnce(&Trace) -> Result<T, E>,
    ) -> Result<T, E> {
        let trace = self.trace(agent, options).map_err(E::from)?;
        let result = trace.on_result(f(&trace));
        trace.end();
        result
    }

    /// Sends a trace that has already ended, from its record, as the live
    /// calls would have sent it: for a job that keeps its own log, or a hook
    /// that runs after the turn it reports. The same exporter and content
    /// rules apply.
    ///
    /// Its trace id is the record's, whatever trace is current, and each
    /// span's id is derived from that id and the span's place in the record,
    /// so the same record sent again is the same bytes, which ingest answers
    /// once: give a trace the same id each time it is sent. Its spans travel
    /// in exports of their own, whatever else is sent meanwhile, in one when
    /// it holds no more than a batch, the agent's own span last.
    ///
    /// ```no_run
    /// use std::time::{Duration, SystemTime};
    ///
    /// use telmoni::{Config, ModelCallRecord, Telmoni, ToolCallRecord, TraceRecord, Usage};
    ///
    /// # fn main() -> Result<(), telmoni::Error> {
    /// let telmoni = Telmoni::new(Config::from_env());
    /// // 2026-10-12T09:00:00Z, and what came after it.
    /// let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_795_600);
    /// let at = |millis| start + Duration::from_millis(millis);
    ///
    /// telmoni.record_trace(TraceRecord {
    ///     session_id: Some("conv_7".into()),
    ///     inside: vec![
    ///         ModelCallRecord {
    ///             provider: Some("openai".into()),
    ///             usage: Some(Usage {
    ///                 input_tokens: Some(1200),
    ///                 output_tokens: Some(85),
    ///                 ..Usage::default()
    ///             }),
    ///             ..ModelCallRecord::new("gpt-4o-mini", at(100), at(2_000))
    ///         }
    ///         .into(),
    ///         ToolCallRecord {
    ///             error_type: Some("TimeoutError".into()),
    ///             ..ToolCallRecord::new("search", at(2_000), at(3_000))
    ///         }
    ///         .into(),
    ///     ],
    ///     // Yours, kept with the log.
    ///     ..TraceRecord::new("3d6f8a1c5b2e4f7a9c0d8e1f2a3b4c5d", "nightly-triage", start, at(9_000))
    /// })?;
    /// telmoni.flush();
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::EmptyAgent`], [`Error::TraceId`], [`Error::EmptyName`],
    /// [`Error::TooLong`], [`Error::RecordedTime`] or [`Error::EndBeforeStart`]
    /// for a record ingest would refuse, as each says; [`Error::TooDeep`] for
    /// steps nested past 16 deep; [`Error::QueueFull`] when the export queue
    /// has no room for the whole record; and [`Error::Shutdown`] once the
    /// client is shut down. A record refused sends nothing, and is taken: send
    /// a clone where it may be recorded again.
    pub fn record_trace(&self, record: TraceRecord) -> Result<(), Error> {
        records::send(&self.inner, record)
    }

    /// Scores what has already ended — a trace, one of its spans or a session
    /// — sent to `/v1/scores` with the next export, as a flush or a shutdown
    /// sends the spans queued. A score of a span still open goes on its
    /// handle's `score` instead. One breaking a score's rules, or a target
    /// that is none, is not sent, with one warning.
    pub fn score(&self, target: &ScoreTarget, score: Score) {
        let Some(sent) = scores::sent(target, &score, &self.inner.warner) else {
            return;
        };
        // A span of its own trace, outside whatever trace is current, so the
        // queue carries it as it carries any span, and the exporter takes it out.
        self.inner
            .tracer
            .span_builder(contract::SCORE_SPAN)
            .with_attributes([KeyValue::new(contract::SCORE_SENT, sent)])
            .start_with_context(&self.inner.tracer, &Context::new())
            .end();
    }

    /// Writes into `headers` what the next service continues the trace from:
    /// the W3C `traceparent` of the span current inside the trace, and the
    /// agent as the W3C `baggage` member `telmoni.agent`, beside whatever
    /// members the headers hold. The span is the current context's — what a
    /// handle's [`attach`](Trace::attach) or `FutureExt::with_context` made
    /// current; [`Self::inject_from`] takes one given. Outside a trace it
    /// writes nothing.
    pub fn inject(&self, headers: &mut dyn Carrier) {
        self.inject_from(&Context::current(), headers);
    }

    /// [`Self::inject`] from `cx` — a handle's [`context`](Trace::context) —
    /// rather than the current context.
    pub fn inject_from(&self, cx: &Context, headers: &mut dyn Carrier) {
        propagation::inject(cx, headers);
    }

    /// The trace `headers` name, continued here: its steps and calls take the
    /// caller's span as their parent and carry the caller's agent, so the
    /// trace shows the work where it was asked for. Nothing ends a continued
    /// trace here; the caller's own span does, so dropping the guard or
    /// calling [`Trace::end`] sends nothing. `None` for headers that name no
    /// trace.
    #[must_use]
    pub fn extract(&self, headers: &dyn Carrier) -> Option<Trace> {
        let read = propagation::read_headers(headers)?;
        Some(traces::continued(&self.inner, read))
    }

    /// Sends everything queued and waits, up to 30 seconds; `true` once
    /// nothing is left to send, kept or not, and at once after a shutdown. A
    /// batch refused or dropped is warned of.
    pub fn flush(&self) -> bool {
        self.inner.processor.flush_within(WAIT).is_ok()
    }

    /// Flushes, then stops the exporter's thread; spans opened afterwards go
    /// nowhere. Past 30 seconds what is left is given up: no batch or attempt
    /// follows. Dropping the last clone of the client and its traces does the
    /// same, with OpenTelemetry's shorter wait.
    pub fn shutdown(&self) {
        // Ours first, with the longer wait: the provider's own call afterwards
        // finds the processor stopped and returns at once.
        let _ = self.inner.processor.shutdown_with_timeout(WAIT);
        let _ = self.inner.provider.shutdown();
    }
}

impl fmt::Debug for Telmoni {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Telmoni")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use opentelemetry::trace::{SpanId, TraceId};

    use super::*;
    use crate::ids;
    use crate::transport::{Answer, Method, TransportError};

    /// Counts the exports it is handed, answering each as kept.
    #[derive(Clone, Default)]
    struct Counting(Arc<AtomicUsize>);

    impl Transport for Counting {
        fn send(
            &self,
            _method: Method,
            _url: &str,
            _headers: &[(String, String)],
            _body: &[u8],
        ) -> Result<Answer, TransportError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Answer {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            })
        }
    }

    /// Ends one span in `trace` under a record's hold, then drops the hold,
    /// told the record was built only when `built`.
    fn hold_one(telmoni: &Telmoni, trace: TraceId, built: bool) {
        let hold = telmoni.inner.processor.hold(trace, 1).unwrap();
        {
            let _given = ids::given(trace, SpanId::from_bytes([1; 8]));
            telmoni.inner.tracer.start("staged").end();
        }
        if built {
            hold.queue();
        }
    }

    /// Nothing a caller passes can fail a build part-way, so the guard is
    /// held here, below `record_trace`, to queue only a record built whole.
    #[test]
    fn a_record_not_built_queues_nothing() {
        let sent = Counting::default();
        let hooks = Hooks {
            transport: Box::new(sent.clone()),
            sleep: Box::new(|_| {}),
            ..Hooks::default()
        };
        let config = Config::new("http://localhost:1", None, Some("k".to_owned()));
        let telmoni = Telmoni::with_hooks(config, hooks);
        let trace = TraceId::from_bytes([1; 16]);
        hold_one(&telmoni, trace, false);
        assert!(telmoni.flush());
        assert_eq!(
            sent.0.load(Ordering::SeqCst),
            0,
            "a record not built was sent"
        );
        hold_one(&telmoni, trace, true);
        assert!(telmoni.flush());
        assert_eq!(
            sent.0.load(Ordering::SeqCst),
            1,
            "a record built was not sent"
        );
        telmoni.shutdown();
    }
}
