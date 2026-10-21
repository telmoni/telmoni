//! The client: a tracer provider of its own, and the runs it records.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{Context, InstrumentationScope, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider, SpanLimits, SpanProcessor as _};
use uuid::Uuid;

use crate::VERSION;
use crate::config::Config;
use crate::contract;
use crate::error::Error;
use crate::exporter::Exporter;
use crate::ids::Ids;
use crate::processor::{Batching, Processor};
use crate::propagation::{self, Carrier};
use crate::records::{self, RunRecord};
use crate::runs::{self, Run, RunOptions};
use crate::sampler::RecordingSampler;
use crate::transport::{Reqwest, Transport};
use crate::warnings::Warner;

/// How long [`Telmoni::flush`] and [`Telmoni::shutdown`] wait.
const WAIT: Duration = Duration::from_secs(30);

/// What the client is built on, each part the real one unless given: the
/// transport, the sleep between attempts, the warning sink, the random source
/// behind the sample rate, and the batching. Tests inject theirs here.
pub struct Hooks {
    /// Sends each export; reqwest's blocking client unless given.
    pub transport: Box<dyn Transport>,
    /// Waits between attempts at one export; `std::thread::sleep` unless given.
    pub sleep: Box<dyn Fn(Duration) + Send + Sync>,
    /// Takes each warning; `tracing::warn!` under the target `telmoni` unless
    /// given.
    pub warn: Box<dyn Fn(&str) + Send + Sync>,
    /// A number in `[0, 1)` drawn as a run starts or a record is sent: the
    /// run is kept when it falls below the sample rate.
    pub random: Box<dyn Fn() -> f64 + Send + Sync>,
    /// How spans are batched on their way out.
    pub batching: Batching,
}

impl Default for Hooks {
    fn default() -> Self {
        Self {
            transport: Box::new(Reqwest::default()),
            sleep: Box::new(std::thread::sleep),
            warn: Box::new(|text: &str| tracing::warn!(target: "telmoni", "{text}")),
            random: Box::new(random_unit),
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

/// A number in `[0, 1)` from 32 random bits of a fresh v4 UUID: its last four
/// bytes, which carry neither its version nor its variant. That is the
/// randomness the crate already has, and more than a sampling decision needs.
fn random_unit() -> f64 {
    let [.., a, b, c, d] = *Uuid::new_v4().as_bytes();
    f64::from(u32::from_be_bytes([a, b, c, d])) / 4_294_967_296.0
}

/// What the client and its runs share.
pub(crate) struct Inner {
    pub(crate) config: Config,
    pub(crate) tracer: SdkTracer,
    provider: SdkTracerProvider,
    pub(crate) processor: Processor,
    exporter: Arc<Exporter>,
    pub(crate) warner: Arc<Warner>,
    pub(crate) random: Box<dyn Fn() -> f64 + Send + Sync>,
}

/// The client: a tracer provider of its own, and the runs it records.
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
            .with_span_limits(SpanLimits {
                max_attributes_per_span: 128,
                max_events_per_span: 0,
                max_links_per_span: 0,
                max_attributes_per_event: 0,
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
        Self {
            inner: Arc::new(Inner {
                config,
                tracer,
                provider,
                processor,
                exporter,
                warner,
                random: hooks.random,
            }),
        }
    }

    /// The configuration, as the client uses it.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// The project's content mode, learned from the first answer: `off`, `on`
    /// or `sealed`; `None` before one.
    #[must_use]
    pub fn content_mode(&self) -> Option<String> {
        self.inner.exporter.content_mode()
    }

    /// A run of `agent`, as a guard: the span `invoke_agent <agent>` in
    /// whatever trace is current, opened by `telmoni.run.start`, sent at once,
    /// and ended when the guard drops or on [`Run::end`]. An agent past 128
    /// characters is cut to them, and a bound or tag out of range, a tag past
    /// the first 32 by key, or a user or session past its cap is left off,
    /// each with one warning.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyAgent`] for a blank name; [`Error::SampleRate`] for a
    /// rate, given here or configured, not above 0 and up to 1.
    pub fn run(&self, agent: &str, options: RunOptions) -> Result<Run, Error> {
        runs::open(&self.inner, agent, options)
    }

    /// [`Self::run`] around `f`: the run ends when `f` returns, and `f`'s
    /// value is returned.
    ///
    /// # Errors
    ///
    /// As [`Self::run`].
    pub fn run_with<R>(
        &self,
        agent: &str,
        options: RunOptions,
        f: impl FnOnce(&Run) -> R,
    ) -> Result<R, Error> {
        let run = self.run(agent, options)?;
        let value = f(&run);
        run.end();
        Ok(value)
    }

    /// [`Self::run_with`] for a fallible `f`: an `Err` marks the run failed
    /// with `E`'s type name (its last path segment) and is returned
    /// unchanged. An `Err` is the only error a closure can carry out; a panic
    /// unwinding through marks the span `panic`.
    ///
    /// # Errors
    ///
    /// `f`'s, or a run that did not open ([`Self::run`]), turned into `E`.
    pub fn try_run_with<T, E: From<Error>>(
        &self,
        agent: &str,
        options: RunOptions,
        f: impl FnOnce(&Run) -> Result<T, E>,
    ) -> Result<T, E> {
        let run = self.run(agent, options).map_err(E::from)?;
        let result = run.on_result(f(&run));
        run.end();
        result
    }

    /// Sends a run that has already ended, from its record, as the live calls
    /// would have sent it: for a job that keeps its own log, or a hook that
    /// runs after the turn it reports. The same exporter, content rules and
    /// sample rate apply.
    ///
    /// Its trace is its own, named by the run's id, whatever trace is
    /// current, and each span's id is derived from that id and the span's
    /// place in the record, so the same record sent again is the same bytes,
    /// which ingest answers once: give a run the same id each time it is
    /// sent. Its spans travel in exports of their own, whatever else is sent
    /// meanwhile, in one when it holds no more than a batch, its start first
    /// and the run's own span last.
    ///
    /// ```no_run
    /// use std::time::{Duration, SystemTime};
    ///
    /// use telmoni::{Config, ModelCallRecord, RunRecord, Telmoni, ToolCallRecord, Usage};
    ///
    /// # fn main() -> Result<(), telmoni::Error> {
    /// let telmoni = Telmoni::new(Config::from_env());
    /// // 2026-10-12T09:00:00Z, and what came after it.
    /// let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_795_600);
    /// let at = |millis| start + Duration::from_millis(millis);
    ///
    /// telmoni.record_run(RunRecord {
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
    ///     ..RunRecord::new("3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d", "nightly-triage", start, at(9_000))
    /// })?;
    /// telmoni.flush();
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::EmptyAgent`], [`Error::RunId`], [`Error::EmptyName`],
    /// [`Error::TooLong`], [`Error::RecordedTime`] or [`Error::EndBeforeStart`]
    /// for a record ingest would refuse, as each says; [`Error::SampleRate`]
    /// for a configured rate outside (0, 1]. A record refused sends nothing.
    pub fn record_run(&self, record: RunRecord) -> Result<(), Error> {
        records::send(&self.inner, record)
    }

    /// Writes into `headers` what the next service continues the run from:
    /// the W3C `traceparent` of the span current inside the run, and the
    /// run's id and agent as the W3C `baggage` members `telmoni.run_id` and
    /// `telmoni.agent`, beside whatever members the headers hold. The span
    /// is the current context's — what a handle's [`attach`](Run::attach) or
    /// `FutureExt::with_context` made current; [`Self::inject_from`] takes
    /// one given. Outside a run, or for a run sampled out, it writes nothing.
    pub fn inject(&self, headers: &mut dyn Carrier) {
        self.inject_from(&Context::current(), headers);
    }

    /// [`Self::inject`] from `cx` — a handle's [`context`](Run::context) —
    /// rather than the current context.
    pub fn inject_from(&self, cx: &Context, headers: &mut dyn Carrier) {
        propagation::inject(cx, headers);
    }

    /// The run `headers` name, continued here: its steps and calls take the
    /// caller's span as their parent and carry the caller's run, so the trace
    /// shows the work where it was asked for. Nothing ends a continued run
    /// here; the caller's own span does, so dropping the guard or calling
    /// [`Run::end`] sends nothing. `None` for headers that name no run.
    #[must_use]
    pub fn extract(&self, headers: &dyn Carrier) -> Option<Run> {
        let read = propagation::read_headers(headers)?;
        Some(runs::continued(&self.inner, read))
    }

    /// Sends everything queued and waits, up to 30 seconds; `true` once the
    /// queue is empty.
    pub fn flush(&self) -> bool {
        self.inner.processor.flush_within(WAIT).is_ok()
    }

    /// Flushes, then stops the exporter's thread; spans opened afterwards go
    /// nowhere. Dropping the last clone of the client and its runs does the
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

    use opentelemetry::trace::{Span as _, SpanId, TraceId, Tracer as _};

    use super::*;
    use crate::ids;
    use crate::transport::{Answer, TransportError};

    /// Counts the exports it is handed, answering each as kept.
    #[derive(Clone, Default)]
    struct Counting(Arc<AtomicUsize>);

    impl Transport for Counting {
        fn send(
            &self,
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
    /// held here, below `record_run`, to queue only a record built whole.
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
