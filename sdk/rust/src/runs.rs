//! Runs, steps, model calls and tool calls, as guards and closures.
//!
//! A run is a span of its own in whatever trace is current; every span inside
//! it carries the run's id, agent, user and session, so ingest knows each
//! span's run without waiting for the trace's tree. A step, a model call or a
//! tool call is opened from the handle it belongs under, so nothing can be
//! opened outside a run, and opens unless the run reached a cap
//! ([`crate::guard`]).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use opentelemetry::trace::{SpanKind, SpanRef, Status, TraceContextExt as _, Tracer as _};
use opentelemetry::{Context, ContextGuard, KeyValue, Value};
use serde::Serialize;
use uuid::Uuid;

use crate::client::Inner;
use crate::contract;
use crate::error::Error;
use crate::guard::{self, Guard, Tokens};
use crate::lock;
use crate::propagation::Continued;
use crate::warnings::Warner;

/// The run a context is inside, carried by every span opened under it: what
/// `inject` reads off the current context.
#[derive(Clone, Debug)]
pub(crate) struct RunInfo {
    pub(crate) run_id: String,
    pub(crate) agent: String,
    /// Whether the sample rate kept the run out: nothing of it is sent, nor
    /// handed to another service.
    pub(crate) sampled_out: bool,
}

/// A model call's tokens as the conventions count them: input with the cache's
/// reads and writes, output with reasoning; sum a provider's parts first. A
/// negative count is left off the call with one warning.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    /// Every input token.
    pub input_tokens: Option<i64>,
    /// Every output token.
    pub output_tokens: Option<i64>,
    /// Input tokens read from the provider's cache.
    pub cache_read_input_tokens: Option<i64>,
    /// Input tokens written to the provider's cache.
    pub cache_write_input_tokens: Option<i64>,
    /// What the call cost in US dollars, where the provider reports it; one
    /// below 0 or not finite is left off with one warning, and a call without
    /// one is priced by its tokens.
    pub cost: Option<f64>,
}

/// What a run is opened with, each part optional: `RunOptions::default()` is
/// a run of the agent alone.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// The end user the run is for, as the application's own id for them;
    /// ingest hashes it. Blank reads as none.
    pub user_id: Option<String>,
    /// The session the run belongs to. Blank reads as none.
    pub session_id: Option<String>,
    /// Tags, sent as `telmoni.tag.<key>`.
    pub tags: BTreeMap<String, String>,
    /// This run's share of being sent, above 0 and up to 1, in place of the
    /// configuration's.
    pub sample_rate: Option<f64>,
    /// The guard's cap on the run's steps: its next step past it is
    /// [`Error::CapExceeded`].
    pub max_steps: Option<u64>,
    /// The guard's cap on the run's spend, in US dollars: reached, what it
    /// opens next is [`Error::CapExceeded`].
    pub max_spend: Option<f64>,
}

/// What a model call is opened with; `ModelCallOptions::default()` is a
/// `chat` call with nothing but its model.
#[derive(Debug, Clone, Default)]
pub struct ModelCallOptions {
    /// The provider, as the conventions spell it (`openai`, `anthropic`, …).
    /// Blank reads as none.
    pub provider: Option<String>,
    /// The operation: `chat` unless given (`generate_content`,
    /// `text_completion`, `embeddings`); a blank one is `chat` too.
    pub operation: Option<String>,
    /// The messages sent to the model: content, which leaves the process only
    /// when [`Config::capture_content`](crate::Config::capture_content) is on
    /// and the project's mode allows.
    pub input_messages: Option<Content>,
    /// The system instructions: content, as above.
    pub system_instructions: Option<Content>,
}

/// What a tool call is opened with.
#[derive(Debug, Clone, Default)]
pub struct ToolCallOptions {
    /// The tool's arguments: content, which leaves the process only when
    /// [`Config::capture_content`](crate::Config::capture_content) is on and
    /// the project's mode allows.
    pub arguments: Option<Content>,
}

/// What the model answered, for [`ModelCall::answered`]; each part optional.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    /// The model that answered, as the provider names it.
    pub response_model: Option<String>,
    /// The tokens used.
    pub usage: Option<Usage>,
    /// The messages the model produced: content, which leaves the process only
    /// when [`Config::capture_content`](crate::Config::capture_content) is on
    /// and the project's mode allows.
    pub output_messages: Option<Content>,
}

/// Content as it goes on a span: a string, kept as given, or anything else
/// encoded as JSON. `From<&str>`, `From<String>` and
/// `From<serde_json::Value>` keep a string as it is; [`Content::new`] encodes
/// any `Serialize`, a string among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content(String);

impl Content {
    /// `value` encoded as JSON, as `serde_json::to_string` encodes it.
    ///
    /// # Errors
    ///
    /// Whatever `serde_json::to_string` returns for `value`.
    pub fn new<T: Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        serde_json::to_string(value).map(Self)
    }

    /// The content as it goes on the span.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for Content {
    fn from(text: String) -> Self {
        Self(text)
    }
}

impl From<&str> for Content {
    fn from(text: &str) -> Self {
        Self(text.to_owned())
    }
}

impl From<serde_json::Value> for Content {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::String(text) => Self(text),
            other => Self(other.to_string()),
        }
    }
}

/// A span the SDK opened, until it ends.
struct Handle {
    client: Arc<Inner>,
    /// The parent's context with this span set: what children are opened
    /// under, and what [`context`](Run::context) hands out.
    cx: Context,
    /// What every span inside the run carries, shared by every span's handle.
    inherited: Arc<[KeyValue]>,
    /// The run's caps, and what it has used of them.
    guard: Arc<Guard>,
    failed: AtomicBool,
    ended: bool,
}

impl Handle {
    fn child(&self, name: String, kind: SpanKind, own: Vec<KeyValue>) -> Self {
        let mut attributes = Vec::with_capacity(self.inherited.len() + own.len());
        attributes.extend_from_slice(&self.inherited);
        attributes.extend(own);
        let span = self
            .client
            .tracer
            .span_builder(name)
            .with_kind(kind)
            .with_attributes(attributes)
            .start_with_context(&self.client.tracer, &self.cx);
        Self {
            client: Arc::clone(&self.client),
            cx: self.cx.with_span(span),
            inherited: Arc::clone(&self.inherited),
            guard: Arc::clone(&self.guard),
            failed: AtomicBool::new(false),
            ended: false,
        }
    }

    fn step(&self, name: &str) -> Result<Step, Error> {
        self.guard.opens(true)?;
        let warner = &self.client.warner;
        // Ingest refuses a span named nothing.
        let name = if name.is_empty() {
            warner.once(
                "step:name".to_owned(),
                "telmoni: a step needs its name; one without is sent named step",
            );
            "step"
        } else {
            name
        };
        let name = cut(name, contract::NAME_MAX, "step's name", warner);
        Ok(Step {
            handle: self.child(name, SpanKind::Internal, Vec::new()),
        })
    }

    fn model_call(&self, model: &str, options: ModelCallOptions) -> Result<ModelCall, Error> {
        self.guard.opens(false)?;
        let warner = &self.client.warner;
        if model.is_empty() {
            warner.once(
                "call:model".to_owned(),
                "telmoni: a model call needs its model; one without is sent named by its operation alone",
            );
        }
        let model = cut(model, contract::MODEL_MAX, "model", warner);
        let provider = options
            .provider
            .map(|provider| cut(&provider, contract::PROVIDER_MAX, "provider", warner));
        let priced = Priced {
            provider: provider.clone().unwrap_or_default(),
            model: model.clone(),
            held: Usage::default(),
            counted: 0.0,
        };
        let (name, mut attributes) = model_call_opening(&model, options.operation, provider);
        let name = cut(&name, contract::NAME_MAX, "model call's name", warner);
        if let Some(messages) = options.input_messages {
            attributes.push(KeyValue::new(contract::INPUT_MESSAGES, messages.0));
        }
        if let Some(instructions) = options.system_instructions {
            attributes.push(KeyValue::new(contract::SYSTEM_INSTRUCTIONS, instructions.0));
        }
        Ok(ModelCall {
            handle: self.child(name, SpanKind::Client, attributes),
            priced: Mutex::new(priced),
        })
    }

    fn tool_call(&self, name: &str, options: ToolCallOptions) -> Result<ToolCall, Error> {
        self.guard.opens(false)?;
        let name = cut(name, contract::NAME_MAX, "tool's name", &self.client.warner);
        let (name, mut attributes) = tool_call_opening(&name);
        if let Some(arguments) = options.arguments {
            attributes.push(KeyValue::new(contract::TOOL_CALL_ARGUMENTS, arguments.0));
        }
        Ok(ToolCall {
            handle: self.child(name, SpanKind::Internal, attributes),
        })
    }

    fn fail(&self, error_type: &str) {
        let warner = &self.client.warner;
        let error_type = cut(
            error_type,
            contract::ERROR_TYPE_MAX,
            "span's error type",
            warner,
        );
        mark_failed(&self.cx.span(), error_type);
        self.failed.store(true, Ordering::Relaxed);
    }

    fn cancel(&self) {
        mark_cancelled(&self.cx.span());
    }

    fn finish(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        // A panic unwinding through the guard is the error that escaped it.
        // Its payload is not a type, so the span is marked with what it is.
        if std::thread::panicking() && !self.failed.load(Ordering::Relaxed) {
            self.fail("panic");
        }
        self.cx.span().end();
    }
}

/// The last path segment of `E`'s type name, generics aside: `std::io::Error`
/// reads as `Error`, a crate's `PlanningFailed` as `PlanningFailed`.
fn error_type<E>() -> String {
    let name = std::any::type_name::<E>();
    let name = name.split('<').next().unwrap_or(name);
    name.rsplit("::").next().unwrap_or(name).to_owned()
}

/// A run: the span `invoke_agent <agent>`, ended when the guard drops or on
/// [`Run::end`]. Opened by [`Telmoni::run`](crate::Telmoni::run).
pub struct Run {
    handle: Handle,
    run_id: String,
    agent: String,
}

/// A step inside a run: a span named as the step is.
pub struct Step {
    handle: Handle,
}

/// A model call inside a run: the span `<operation> <model>`, kind `CLIENT`;
/// [`ModelCall::answered`] records the reply.
pub struct ModelCall {
    handle: Handle,
    priced: Mutex<Priced>,
}

/// What a model call is priced by, as sent, its usage as its span holds it,
/// and what it last counted to its run's spend.
struct Priced {
    provider: String,
    model: String,
    held: Usage,
    counted: f64,
}

/// A tool call inside a run: the span `execute_tool <name>`;
/// [`ToolCall::returned`] records its result.
pub struct ToolCall {
    handle: Handle,
}

/// What every handle has: the span's end, its marks, and its context.
macro_rules! handle_methods {
    ($handle:ident, $what:literal) => {
        impl $handle {
            /// Ends the span now; dropping the guard does the same.
            pub fn end(mut self) {
                self.handle.finish();
            }

            /// Marks the span failed: status `ERROR`, and `error_type` as its
            /// `error.type`, none if empty. No message: a message is content.
            pub fn fail(&self, error_type: &str) {
                self.handle.fail(error_type);
            }

            /// Marks the span cancelled by the agent, `telmoni.cancelled`,
            /// which is neither ok nor an error.
            pub fn cancel(&self) {
                self.handle.cancel();
            }

            /// The context with this span set: for
            /// [`FutureExt::with_context`](opentelemetry::trace::FutureExt::with_context)
            /// or [`Context::attach`](opentelemetry::Context::attach), so the
            /// application's own spans opened under it nest here.
            #[must_use]
            pub fn context(&self) -> Context {
                self.handle.cx.clone()
            }

            /// Makes this span the current one until the guard drops, for
            /// synchronous code: the guard is `!Send`, so across an `await`
            /// use [`Self::context`] with `FutureExt::with_context` instead.
            #[must_use = "the previous context comes back when the guard drops"]
            pub fn attach(&self) -> ContextGuard {
                self.handle.cx.clone().attach()
            }
        }

        impl Drop for $handle {
            fn drop(&mut self) {
                self.handle.finish();
            }
        }

        impl fmt::Debug for $handle {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct($what)
                    .field("span", &self.handle.cx.span().span_context())
                    .finish_non_exhaustive()
            }
        }
    };
}

handle_methods!(Run, "Run");
handle_methods!(Step, "Step");
handle_methods!(ModelCall, "ModelCall");
handle_methods!(ToolCall, "ToolCall");

/// What a handle that can hold work has: steps, model calls and tool calls
/// opened under it, as guards or around closures.
macro_rules! opens_children {
    ($handle:ident) => {
        impl $handle {
            /// A step under this span: a span named `name`, ended when its
            /// guard drops or on [`Step::end`].
            ///
            /// # Errors
            ///
            /// [`Error::CapExceeded`], opening nothing, once the run reached
            /// its step cap or its spend cap.
            pub fn step(&self, name: &str) -> Result<Step, Error> {
                self.handle.step(name)
            }

            /// A model call under this span, the span `<operation> <model>`,
            /// `chat` unless `options.operation` says otherwise; ended when
            /// its guard drops or on [`ModelCall::end`].
            ///
            /// # Errors
            ///
            /// [`Error::CapExceeded`], opening nothing, once the run reached
            /// its spend cap.
            pub fn model_call(
                &self,
                model: &str,
                options: ModelCallOptions,
            ) -> Result<ModelCall, Error> {
                self.handle.model_call(model, options)
            }

            /// A tool call under this span, the span `execute_tool <name>`;
            /// ended when its guard drops or on [`ToolCall::end`].
            ///
            /// # Errors
            ///
            /// [`Error::CapExceeded`], opening nothing, once the run reached
            /// its spend cap.
            pub fn tool_call(
                &self,
                name: &str,
                options: ToolCallOptions,
            ) -> Result<ToolCall, Error> {
                self.handle.tool_call(name, options)
            }

            /// [`Self::step`] around `f`: the step ends when `f` returns, and
            /// `f`'s value is returned.
            ///
            /// # Errors
            ///
            /// As [`Self::step`], before `f` runs.
            pub fn step_with<R>(&self, name: &str, f: impl FnOnce(&Step) -> R) -> Result<R, Error> {
                let step = self.step(name)?;
                let value = f(&step);
                step.end();
                Ok(value)
            }

            /// [`Self::step_with`] for a fallible `f`: an `Err` marks the step
            /// failed with `E`'s type name (its last path segment) and is
            /// returned unchanged. An `Err` is the only error a closure can
            /// carry out; a panic unwinding through marks the span `panic`.
            ///
            /// # Errors
            ///
            /// `f`'s, or [`Self::step`]'s as an `E`, before `f` runs.
            pub fn try_step_with<T, E: From<Error>>(
                &self,
                name: &str,
                f: impl FnOnce(&Step) -> Result<T, E>,
            ) -> Result<T, E> {
                let step = self.step(name)?;
                let result = f(&step);
                if result.is_err() {
                    step.fail(&error_type::<E>());
                }
                step.end();
                result
            }

            /// [`Self::model_call`] around `f`: the call ends when `f`
            /// returns, and `f`'s value is returned.
            ///
            /// # Errors
            ///
            /// As [`Self::model_call`], before `f` runs.
            pub fn model_call_with<R>(
                &self,
                model: &str,
                options: ModelCallOptions,
                f: impl FnOnce(&ModelCall) -> R,
            ) -> Result<R, Error> {
                let call = self.model_call(model, options)?;
                let value = f(&call);
                call.end();
                Ok(value)
            }

            /// [`Self::model_call_with`] for a fallible `f`: an `Err` marks
            /// the call failed with `E`'s type name and is returned unchanged.
            ///
            /// # Errors
            ///
            /// `f`'s, or [`Self::model_call`]'s as an `E`, before `f` runs.
            pub fn try_model_call_with<T, E: From<Error>>(
                &self,
                model: &str,
                options: ModelCallOptions,
                f: impl FnOnce(&ModelCall) -> Result<T, E>,
            ) -> Result<T, E> {
                let call = self.model_call(model, options)?;
                let result = f(&call);
                if result.is_err() {
                    call.fail(&error_type::<E>());
                }
                call.end();
                result
            }

            /// [`Self::tool_call`] around `f`: the call ends when `f` returns,
            /// and `f`'s value is returned.
            ///
            /// # Errors
            ///
            /// As [`Self::tool_call`], before `f` runs.
            pub fn tool_call_with<R>(
                &self,
                name: &str,
                options: ToolCallOptions,
                f: impl FnOnce(&ToolCall) -> R,
            ) -> Result<R, Error> {
                let call = self.tool_call(name, options)?;
                let value = f(&call);
                call.end();
                Ok(value)
            }

            /// [`Self::tool_call_with`] for a fallible `f`: an `Err` marks the
            /// call failed with `E`'s type name and is returned unchanged.
            ///
            /// # Errors
            ///
            /// `f`'s, or [`Self::tool_call`]'s as an `E`, before `f` runs.
            pub fn try_tool_call_with<T, E: From<Error>>(
                &self,
                name: &str,
                options: ToolCallOptions,
                f: impl FnOnce(&ToolCall) -> Result<T, E>,
            ) -> Result<T, E> {
                let call = self.tool_call(name, options)?;
                let result = f(&call);
                if result.is_err() {
                    call.fail(&error_type::<E>());
                }
                call.end();
                result
            }
        }
    };
}

opens_children!(Run);
opens_children!(Step);
opens_children!(ToolCall);

impl Run {
    /// The run's id: a fresh UUID v4, on every span inside the run.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The agent's name as the run sends it, cut to 128 characters.
    #[must_use]
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// What the run's model calls have cost so far in US dollars, counted as
    /// its spend cap counts them, in this process.
    #[must_use]
    pub fn spent(&self) -> f64 {
        self.handle.guard.spent()
    }

    /// Marks the run failed with `E`'s type name when `result` is an `Err`,
    /// and gives `result` back unchanged: what
    /// [`Telmoni::try_run_with`](crate::Telmoni::try_run_with) does for a
    /// closure, for a run held as a guard.
    pub fn on_result<T, E>(&self, result: Result<T, E>) -> Result<T, E> {
        if result.is_err() {
            self.fail(&error_type::<E>());
        }
        result
    }
}

impl ModelCall {
    /// What the model answered: its name, its tokens, and its messages.
    pub fn answered(&self, reply: Reply) {
        let warner = &self.handle.client.warner;
        // Ingest refuses the call over an empty one.
        let answered_as = reply
            .response_model
            .filter(|model| !model.is_empty())
            .map(|model| cut(&model, contract::MODEL_MAX, "response model", warner));
        let usage = reply.usage.map(|usage| sendable_usage(usage, warner));
        self.count(answered_as, usage, reply.output_messages);
    }

    /// The answer on its span, and its cost to its run's spend in place of what
    /// it counted before: reported, else priced by its name as sent, else nothing.
    fn count(&self, answered_as: Option<String>, usage: Option<Usage>, messages: Option<Content>) {
        // Given once the lock is released: the sink is the application's.
        if let Some((key, text)) = self.hold(answered_as, usage, messages) {
            self.handle.client.warner.once(key, &text);
        }
    }

    /// [`Self::count`] under the call's lock, so answers given at once count
    /// as the span is left holding them; and the warning it calls for.
    fn hold(
        &self,
        answered_as: Option<String>,
        usage: Option<Usage>,
        messages: Option<Content>,
    ) -> Option<(String, String)> {
        let mut priced = lock(&self.priced);
        let span = self.handle.cx.span();
        if let Some(model) = answered_as {
            span.set_attribute(KeyValue::new(contract::RESPONSE_MODEL, model.clone()));
            // The name the call is priced by, as ingest reads it.
            priced.model = model;
        }
        if let Some(messages) = messages {
            span.set_attribute(KeyValue::new(contract::OUTPUT_MESSAGES, messages.0));
        }
        if let Some(sent) = usage {
            span.set_attributes(usage_attributes(sent));
            let held = &mut priced.held;
            held.input_tokens = sent.input_tokens.or(held.input_tokens);
            held.output_tokens = sent.output_tokens.or(held.output_tokens);
            held.cache_read_input_tokens = sent
                .cache_read_input_tokens
                .or(held.cache_read_input_tokens);
            held.cache_write_input_tokens = sent
                .cache_write_input_tokens
                .or(held.cache_write_input_tokens);
            held.cost = sent.cost.or(held.cost);
        }
        let held = priced.held;
        let whole = |count: Option<i64>| count.and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
        let tokens = Tokens {
            input: whole(held.input_tokens),
            output: whole(held.output_tokens),
            cache_read: whole(held.cache_read_input_tokens),
            cache_write: whole(held.cache_write_input_tokens),
        };
        let cost = held.cost.or_else(|| {
            guard::priced(
                &self.handle.client.prices,
                &priced.provider,
                &priced.model,
                tokens,
            )
        });
        let guard = &self.handle.guard;
        let warning = (cost.is_none() && guard.capped() && tokens != Tokens::default()).then(|| {
            let model = if priced.model.is_empty() {
                "a call with no model"
            } else {
                &priced.model
            };
            let provider = if priced.provider.is_empty() {
                "no provider"
            } else {
                &priced.provider
            };
            (
                format!("unpriced:{}:{}", priced.provider, priced.model),
                format!(
                    "telmoni: no price for {model} ({provider}), which reported no cost; it counts nothing toward the run's spend cap"
                ),
            )
        });
        guard.count(&mut priced.counted, cost.unwrap_or(0.0));
        warning
    }
}

impl ToolCall {
    /// What the tool returned: content, sent only when asked and allowed.
    pub fn returned(&self, result: impl Into<Content>) {
        self.handle
            .cx
            .span()
            .set_attribute(KeyValue::new(contract::TOOL_CALL_RESULT, result.into().0));
    }
}

/// Opens a run for the client: its span and its sampling.
pub(crate) fn open(inner: &Arc<Inner>, agent: &str, options: RunOptions) -> Result<Run, Error> {
    if agent.trim().is_empty() {
        return Err(Error::EmptyAgent);
    }
    let agent = cut(agent, contract::AGENT_MAX, "run's agent", &inner.warner);
    let rate = options.sample_rate.unwrap_or(inner.config.sample_rate);
    if !(rate > 0.0 && rate <= 1.0) {
        return Err(Error::SampleRate(rate));
    }
    if options.max_steps == Some(0) {
        return Err(Error::MaxSteps);
    }
    if let Some(spend) = options.max_spend
        && !(spend.is_finite() && spend > 0.0)
    {
        return Err(Error::MaxSpend(spend));
    }

    let sampled_out = rate < 1.0 && (inner.random)() >= rate;
    let run_id = Uuid::new_v4().to_string();
    let user_id =
        (options.user_id).filter(|id| fits(id, contract::USER_ID_MAX, "user id", &inner.warner));
    let session_id = (options.session_id)
        .filter(|id| fits(id, contract::SESSION_ID_MAX, "session id", &inner.warner));
    let mut inherited = inherited(&run_id, &agent, user_id, session_id);
    // On every span of the run from its start, for the processor to drop: a
    // sampler's drop would clear the flag the application's spans inherit.
    if sampled_out {
        inherited.push(KeyValue::new(contract::SAMPLED_OUT, true));
    }
    let attributes = run_attributes(&agent, &inherited, rate, options.tags, &inner.warner);

    let parent = Context::current();
    let tracer = &inner.tracer;
    let span = tracer
        .span_builder(format!("{} {agent}", contract::RUN_OPERATION))
        .with_kind(SpanKind::Internal)
        .with_attributes(attributes)
        .start_with_context(tracer, &parent);
    let cx = parent.with_span(span).with_value(RunInfo {
        run_id: run_id.clone(),
        agent: agent.clone(),
        sampled_out,
    });

    Ok(Run {
        handle: Handle {
            client: Arc::clone(inner),
            cx,
            inherited: Arc::from(inherited),
            guard: Arc::new(Guard::new(options.max_steps, options.max_spend)),
            failed: AtomicBool::new(false),
            ended: false,
        },
        run_id,
        agent,
    })
}

/// A run another service opened, continued here under the span the headers
/// named: a run whose own span is the caller's, remote and non-recording
/// here, so its steps and calls nest under it and carry the run, and nothing
/// of it is sent or ended from this side.
pub(crate) fn continued(inner: &Arc<Inner>, read: Continued) -> Run {
    let agent = cut(
        &read.agent,
        contract::AGENT_MAX,
        "run's agent",
        &inner.warner,
    );
    let inherited = inherited(&read.run_id, &agent, None, None);
    let cx = Context::current()
        .with_remote_span_context(read.span_context)
        .with_value(RunInfo {
            run_id: read.run_id.clone(),
            agent: agent.clone(),
            sampled_out: false,
        });
    Run {
        handle: Handle {
            client: Arc::clone(inner),
            cx,
            inherited: Arc::from(inherited),
            guard: Arc::new(Guard::default()),
            failed: AtomicBool::new(false),
            ended: false,
        },
        run_id: read.run_id,
        agent,
    }
}

/// What every span inside a run carries: its id and agent, and its user and
/// session when it has them.
pub(crate) fn inherited(
    run_id: &str,
    agent: &str,
    user_id: Option<String>,
    session_id: Option<String>,
) -> Vec<KeyValue> {
    // Shared, so each span's copy of them is a count rather than a string.
    let shared = |text: &str| Value::from(Arc::<str>::from(text));
    let mut inherited = vec![
        KeyValue::new(contract::RUN_ID, shared(run_id)),
        KeyValue::new(contract::AGENT, shared(agent)),
    ];
    if let Some(user) = user_id.filter(|id| !id.is_empty()) {
        inherited.push(KeyValue::new(contract::USER_ID, shared(&user)));
    }
    if let Some(session) = session_id.filter(|id| !id.is_empty()) {
        inherited.push(KeyValue::new(contract::SESSION_ID, shared(&session)));
    }
    inherited
}

/// The run span's own attributes: the operation and the agent, what every
/// span inside carries, the sample rate and the tags.
pub(crate) fn run_attributes(
    agent: &str,
    inherited: &[KeyValue],
    rate: f64,
    mut tags: BTreeMap<String, String>,
    warner: &Warner,
) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(contract::OPERATION, contract::RUN_OPERATION),
        KeyValue::new(contract::AGENT_NAME, agent.to_owned()),
    ];
    attributes.extend_from_slice(inherited);
    attributes.push(KeyValue::new(contract::SAMPLE_RATE, rate));
    // Ingest drops these before it counts its 32, so they take no place there.
    let given = tags.len();
    tags.retain(|key, value| {
        (1..=contract::TAG_KEY_MAX).contains(&key.chars().count())
            && value.chars().count() <= contract::TAG_VALUE_MAX
    });
    if tags.len() < given {
        warner.once(
            "run:tag".to_owned(),
            &format!(
                "telmoni: a tag's key must be 1 to {} characters and its value up to {}; the tag is not sent",
                contract::TAG_KEY_MAX,
                contract::TAG_VALUE_MAX
            ),
        );
    }
    // Ingest keeps no more, and more could crowd out what the run's end sets.
    if tags.len() > contract::TAGS_MAX {
        warner.once(
            "run:tags".to_owned(),
            &format!(
                "telmoni: a run carries up to {} tags, the most ingest keeps; the rest are not sent",
                contract::TAGS_MAX
            ),
        );
    }
    for (key, value) in tags.into_iter().take(contract::TAGS_MAX) {
        attributes.push(KeyValue::new(
            format!("{}{key}", contract::TAG_PREFIX),
            value,
        ));
    }
    attributes
}

/// The operation a model call's span is named for: `chat` unless one not
/// blank is given.
pub(crate) fn model_operation(given: Option<&str>) -> &str {
    // Sent blank, the span would be named ` <model>` and ingest would read it
    // as a step.
    given
        .filter(|operation| !operation.trim().is_empty())
        .unwrap_or(contract::MODEL_OPERATION_DEFAULT)
}

/// A model call's span name, `<operation> <model>`, and what it opens with
/// besides what the run carries and any content.
pub(crate) fn model_call_opening(
    model: &str,
    operation: Option<String>,
    provider: Option<String>,
) -> (String, Vec<KeyValue>) {
    let operation = model_operation(operation.as_deref()).to_owned();
    // Ingest refuses a call over an empty model, but keeps one with none.
    let name = if model.is_empty() {
        operation.clone()
    } else {
        format!("{operation} {model}")
    };
    let mut attributes = vec![KeyValue::new(contract::OPERATION, operation)];
    if !model.is_empty() {
        attributes.push(KeyValue::new(contract::REQUEST_MODEL, model.to_owned()));
    }
    if let Some(provider) = provider.filter(|provider| !provider.is_empty()) {
        attributes.push(KeyValue::new(contract::PROVIDER, provider));
    }
    (name, attributes)
}

/// A tool call's span name, `execute_tool <name>`, and what it opens with
/// besides what the run carries and any content.
pub(crate) fn tool_call_opening(name: &str) -> (String, Vec<KeyValue>) {
    (
        format!("{} {name}", contract::TOOL_OPERATION),
        vec![
            KeyValue::new(contract::OPERATION, contract::TOOL_OPERATION),
            KeyValue::new(contract::TOOL_NAME, name.to_owned()),
        ],
    )
}

/// The counts and the cost `usage` holds, as the conventions and Langfuse
/// name them.
pub(crate) fn usage_attributes(usage: Usage) -> impl Iterator<Item = KeyValue> {
    [
        (contract::INPUT_TOKENS, usage.input_tokens),
        (contract::OUTPUT_TOKENS, usage.output_tokens),
        (
            contract::CACHE_READ_INPUT_TOKENS,
            usage.cache_read_input_tokens,
        ),
        (
            contract::CACHE_WRITE_INPUT_TOKENS,
            usage.cache_write_input_tokens,
        ),
    ]
    .into_iter()
    .filter_map(|(key, count)| count.map(|count| KeyValue::new(key, count)))
    .chain(usage.cost.map(|cost| KeyValue::new(contract::COST, cost)))
}

/// `usage` without what ingest refuses the call for, a negative count and a
/// cost below 0 or not finite, each warned of once.
pub(crate) fn sendable_usage(usage: Usage, warner: &Warner) -> Usage {
    let kept = |name: &str, count: Option<i64>| {
        if count.is_some_and(|count| count < 0) {
            warner.once(
                format!("usage:{name}"),
                &format!(
                    "telmoni: {name} must be a whole number from 0 within 64 bits; the call is sent without it"
                ),
            );
            return None;
        }
        count
    };
    Usage {
        input_tokens: kept(contract::INPUT_TOKENS, usage.input_tokens),
        output_tokens: kept(contract::OUTPUT_TOKENS, usage.output_tokens),
        cache_read_input_tokens: kept(
            contract::CACHE_READ_INPUT_TOKENS,
            usage.cache_read_input_tokens,
        ),
        cache_write_input_tokens: kept(
            contract::CACHE_WRITE_INPUT_TOKENS,
            usage.cache_write_input_tokens,
        ),
        cost: usage.cost.filter(|cost| {
            let dollars = cost.is_finite() && *cost >= 0.0;
            if !dollars {
                warner.once(
                    format!("usage:{}", contract::COST),
                    &format!(
                        "telmoni: {} must be a finite number of US dollars from 0; the call is sent without it",
                        contract::COST
                    ),
                );
            }
            dollars
        }),
    }
}

/// `text` up to ingest's cap, in characters, cut there with one warning,
/// since ingest refuses a span past it.
fn cut(text: &str, most: usize, what: &str, warner: &Warner) -> String {
    if text.chars().nth(most).is_none() {
        return text.to_owned();
    }
    warner.once(
        format!("cut:{what}"),
        &format!(
            "telmoni: a {what} must be up to {most} characters; it is sent cut to its first {most}"
        ),
    );
    text.chars().take(most).collect()
}

/// Whether an id is within ingest's cap; one past it is left off with one
/// warning, since cut it would name another.
fn fits(id: &str, most: usize, what: &str, warner: &Warner) -> bool {
    if id.chars().nth(most).is_none() {
        return true;
    }
    warner.once(
        format!("long:{what}"),
        &format!("telmoni: a {what} must be up to {most} characters; the run is sent without it"),
    );
    false
}

/// Marks `span` failed: `error.type`, unless empty, which ingest would refuse
/// the span for, and status `ERROR` with no description, which is content.
pub(crate) fn mark_failed(span: &SpanRef<'_>, error_type: String) {
    if !error_type.is_empty() {
        span.set_attribute(KeyValue::new(contract::ERROR_TYPE, error_type));
    }
    span.set_status(Status::error(""));
}

/// Marks `span` cancelled by the agent, which is neither ok nor an error.
pub(crate) fn mark_cancelled(span: &SpanRef<'_>) {
    span.set_attribute(KeyValue::new(contract::CANCELLED, true));
}
