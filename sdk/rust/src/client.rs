//! The client: a tracer provider of its own, and the runs it records.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{
    RandomIdGenerator, SdkTracer, SdkTracerProvider, SpanLimits, SpanProcessor as _,
};
use uuid::Uuid;

use crate::VERSION;
use crate::config::Config;
use crate::contract;
use crate::error::Error;
use crate::exporter::Exporter;
use crate::processor::{Batching, Processor};
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
    /// A number in `[0, 1)` drawn as a run starts: the run is kept when it
    /// falls below the sample rate.
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
        let processor = Processor::start(Arc::clone(&exporter), hooks.batching, warner);
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
            .with_id_generator(RandomIdGenerator::default())
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
    /// whatever trace is current, opened by `telmoni.run.start`, which is sent
    /// at once. The span ends when the guard drops or on [`Run::end`].
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
