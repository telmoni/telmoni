//! Runs, steps, model calls and tool calls, as guards and closures.
//!
//! A run is a span of its own in whatever trace is current, opened by
//! `telmoni.run.start`; every span inside it carries the run's id, agent,
//! user and conversation, so ingest knows each span's run without waiting
//! for the trace's tree. A step, a model call or a tool call is opened from
//! the handle it belongs under, so nothing can be opened outside a run.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use opentelemetry::trace::{Span as _, SpanKind, Status, TraceContextExt as _, Tracer as _};
use opentelemetry::{Context, ContextGuard, KeyValue};
use serde::Serialize;
use uuid::Uuid;

use crate::client::Inner;
use crate::contract;
use crate::error::Error;

/// What a monitor holds a run to; each optional. Integers are 64-bit, as the
/// wire carries them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bounds {
    /// How long the run may go without a span before it counts as stalled.
    pub stall_after_s: Option<i64>,
    /// How many steps the run may take.
    pub max_steps: Option<i64>,
    /// How much the run may cost, in US dollars.
    pub max_cost_usd: Option<f64>,
}

/// A model call's tokens, counted as the conventions count them. Input tokens
/// include the cache's reads and writes; output tokens include reasoning.
/// Where a provider reports parts apart, sum them first. Integers are 64-bit,
/// as the wire carries them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Every input token.
    pub input_tokens: Option<i64>,
    /// Every output token.
    pub output_tokens: Option<i64>,
    /// Input tokens read from the provider's cache.
    pub cache_read_input_tokens: Option<i64>,
    /// Input tokens written to the provider's cache.
    pub cache_write_input_tokens: Option<i64>,
}

/// What a run is opened with, each part optional: `RunOptions::default()` is
/// a run of the agent alone.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// The end user the run is for, as the application's own id for them;
    /// ingest hashes it. Blank reads as none.
    pub user_id: Option<String>,
    /// The conversation the run belongs to. Blank reads as none.
    pub conversation_id: Option<String>,
    /// Tags, sent as `telmoni.tag.<key>`.
    pub tags: BTreeMap<String, String>,
    /// What a monitor holds the run to.
    pub bounds: Bounds,
    /// This run's share of being sent, above 0 and up to 1, in place of the
    /// configuration's.
    pub sample_rate: Option<f64>,
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
    /// What every span inside the run carries.
    inherited: Vec<KeyValue>,
    failed: AtomicBool,
    ended: bool,
}

impl Handle {
    fn child(&self, name: String, kind: SpanKind, own: Vec<KeyValue>) -> Self {
        let mut attributes = self.inherited.clone();
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
            inherited: self.inherited.clone(),
            failed: AtomicBool::new(false),
            ended: false,
        }
    }

    fn step(&self, name: &str) -> Step {
        Step {
            handle: self.child(name.to_owned(), SpanKind::Internal, Vec::new()),
        }
    }

    fn model_call(&self, model: &str, options: ModelCallOptions) -> ModelCall {
        // Sent blank, the span would be named ` <model>` and ingest would
        // read it as a step.
        let operation = options
            .operation
            .filter(|operation| !operation.trim().is_empty())
            .unwrap_or_else(|| contract::MODEL_OPERATION_DEFAULT.to_owned());
        let mut attributes = vec![
            KeyValue::new(contract::OPERATION, operation.clone()),
            KeyValue::new(contract::REQUEST_MODEL, model.to_owned()),
        ];
        if let Some(provider) = options.provider.filter(|provider| !provider.is_empty()) {
            attributes.push(KeyValue::new(contract::PROVIDER, provider));
        }
        if let Some(messages) = options.input_messages {
            attributes.push(KeyValue::new(contract::INPUT_MESSAGES, messages.0));
        }
        if let Some(instructions) = options.system_instructions {
            attributes.push(KeyValue::new(contract::SYSTEM_INSTRUCTIONS, instructions.0));
        }
        ModelCall {
            handle: self.child(format!("{operation} {model}"), SpanKind::Client, attributes),
        }
    }

    fn tool_call(&self, name: &str, options: ToolCallOptions) -> ToolCall {
        let mut attributes = vec![
            KeyValue::new(contract::OPERATION, contract::TOOL_OPERATION),
            KeyValue::new(contract::TOOL_NAME, name.to_owned()),
        ];
        if let Some(arguments) = options.arguments {
            attributes.push(KeyValue::new(contract::TOOL_CALL_ARGUMENTS, arguments.0));
        }
        ToolCall {
            handle: self.child(
                format!("{} {name}", contract::TOOL_OPERATION),
                SpanKind::Internal,
                attributes,
            ),
        }
    }

    fn fail(&self, error_type: &str) {
        let span = self.cx.span();
        span.set_attribute(KeyValue::new(contract::ERROR_TYPE, error_type.to_owned()));
        span.set_status(Status::error(""));
        self.failed.store(true, Ordering::Relaxed);
    }

    fn cancel(&self) {
        self.cx
            .span()
            .set_attribute(KeyValue::new(contract::CANCELLED, true));
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

            /// Marks the span failed: `error.type` set to `error_type`, the
            /// exception's class or the error's name, and status `ERROR`. No
            /// message: a message is content.
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
            #[must_use = "a step ends when its guard drops"]
            pub fn step(&self, name: &str) -> Step {
                self.handle.step(name)
            }

            /// A model call under this span, the span `<operation> <model>`,
            /// `chat` unless `options.operation` says otherwise; ended when
            /// its guard drops or on [`ModelCall::end`].
            #[must_use = "a model call ends when its guard drops"]
            pub fn model_call(&self, model: &str, options: ModelCallOptions) -> ModelCall {
                self.handle.model_call(model, options)
            }

            /// A tool call under this span, the span `execute_tool <name>`;
            /// ended when its guard drops or on [`ToolCall::end`].
            #[must_use = "a tool call ends when its guard drops"]
            pub fn tool_call(&self, name: &str, options: ToolCallOptions) -> ToolCall {
                self.handle.tool_call(name, options)
            }

            /// [`Self::step`] around `f`: the step ends when `f` returns, and
            /// `f`'s value is returned.
            pub fn step_with<R>(&self, name: &str, f: impl FnOnce(&Step) -> R) -> R {
                let step = self.step(name);
                let value = f(&step);
                step.end();
                value
            }

            /// [`Self::step_with`] for a fallible `f`: an `Err` marks the step
            /// failed with `E`'s type name (its last path segment) and is
            /// returned unchanged. An `Err` is the only error a closure can
            /// carry out; a panic unwinding through marks the span `panic`.
            pub fn try_step_with<T, E>(
                &self,
                name: &str,
                f: impl FnOnce(&Step) -> Result<T, E>,
            ) -> Result<T, E> {
                let step = self.step(name);
                let result = f(&step);
                if result.is_err() {
                    step.fail(&error_type::<E>());
                }
                step.end();
                result
            }

            /// [`Self::model_call`] around `f`: the call ends when `f`
            /// returns, and `f`'s value is returned.
            pub fn model_call_with<R>(
                &self,
                model: &str,
                options: ModelCallOptions,
                f: impl FnOnce(&ModelCall) -> R,
            ) -> R {
                let call = self.model_call(model, options);
                let value = f(&call);
                call.end();
                value
            }

            /// [`Self::model_call_with`] for a fallible `f`: an `Err` marks
            /// the call failed with `E`'s type name and is returned unchanged.
            pub fn try_model_call_with<T, E>(
                &self,
                model: &str,
                options: ModelCallOptions,
                f: impl FnOnce(&ModelCall) -> Result<T, E>,
            ) -> Result<T, E> {
                let call = self.model_call(model, options);
                let result = f(&call);
                if result.is_err() {
                    call.fail(&error_type::<E>());
                }
                call.end();
                result
            }

            /// [`Self::tool_call`] around `f`: the call ends when `f` returns,
            /// and `f`'s value is returned.
            pub fn tool_call_with<R>(
                &self,
                name: &str,
                options: ToolCallOptions,
                f: impl FnOnce(&ToolCall) -> R,
            ) -> R {
                let call = self.tool_call(name, options);
                let value = f(&call);
                call.end();
                value
            }

            /// [`Self::tool_call_with`] for a fallible `f`: an `Err` marks the
            /// call failed with `E`'s type name and is returned unchanged.
            pub fn try_tool_call_with<T, E>(
                &self,
                name: &str,
                options: ToolCallOptions,
                f: impl FnOnce(&ToolCall) -> Result<T, E>,
            ) -> Result<T, E> {
                let call = self.tool_call(name, options);
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

    /// The agent's name, as given.
    #[must_use]
    pub fn agent(&self) -> &str {
        &self.agent
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
        let span = self.handle.cx.span();
        if let Some(model) = reply.response_model {
            span.set_attribute(KeyValue::new(contract::RESPONSE_MODEL, model));
        }
        if let Some(usage) = reply.usage {
            for (key, count) in [
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
            ] {
                if let Some(count) = count {
                    span.set_attribute(KeyValue::new(key, count));
                }
            }
        }
        if let Some(messages) = reply.output_messages {
            span.set_attribute(KeyValue::new(contract::OUTPUT_MESSAGES, messages.0));
        }
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

/// Opens a run for the client: its span, its sampling, and its start span.
pub(crate) fn open(inner: &Arc<Inner>, agent: &str, options: RunOptions) -> Result<Run, Error> {
    if agent.trim().is_empty() {
        return Err(Error::EmptyAgent);
    }
    let rate = options.sample_rate.unwrap_or(inner.config.sample_rate);
    if !(rate > 0.0 && rate <= 1.0) {
        return Err(Error::SampleRate(rate));
    }

    let run_id = Uuid::new_v4().to_string();
    let mut inherited = vec![
        KeyValue::new(contract::RUN_ID, run_id.clone()),
        KeyValue::new(contract::AGENT, agent.to_owned()),
    ];
    if let Some(user) = options.user_id.filter(|id| !id.is_empty()) {
        inherited.push(KeyValue::new(contract::USER_ID, user));
    }
    if let Some(conversation) = options.conversation_id.filter(|id| !id.is_empty()) {
        inherited.push(KeyValue::new(contract::CONVERSATION_ID, conversation));
    }
    let bounds = bound_attributes(&options.bounds);

    let mut attributes = vec![
        KeyValue::new(contract::OPERATION, contract::RUN_OPERATION),
        KeyValue::new(contract::AGENT_NAME, agent.to_owned()),
    ];
    attributes.extend(inherited.iter().cloned());
    attributes.extend(bounds.iter().cloned());
    attributes.push(KeyValue::new(contract::SAMPLE_RATE, rate));
    for (key, value) in options.tags {
        attributes.push(KeyValue::new(
            format!("{}{key}", contract::TAG_PREFIX),
            value,
        ));
    }

    let parent = Context::current();
    let start = SystemTime::now();
    let tracer = &inner.tracer;
    let span = tracer
        .span_builder(format!("{} {agent}", contract::RUN_OPERATION))
        .with_kind(SpanKind::Internal)
        .with_attributes(attributes)
        .with_start_time(start)
        .start_with_context(tracer, &parent);
    if rate < 1.0 && (inner.random)() >= rate {
        inner.processor.sample_out(&run_id);
    }
    let cx = parent.with_span(span);

    // OpenTelemetry sends a span only once it ends; without this one a run
    // stalled in its first call would never be heard.
    let mut start_attributes = inherited.clone();
    start_attributes.extend(bounds);
    start_attributes.push(KeyValue::new(contract::SAMPLE_RATE, rate));
    let mut opener = tracer
        .span_builder(contract::RUN_START)
        .with_kind(SpanKind::Internal)
        .with_attributes(start_attributes)
        .with_start_time(start)
        .start_with_context(tracer, &cx);
    opener.end_with_timestamp(start);

    Ok(Run {
        handle: Handle {
            client: Arc::clone(inner),
            cx,
            inherited,
            failed: AtomicBool::new(false),
            ended: false,
        },
        run_id,
        agent: agent.to_owned(),
    })
}

fn bound_attributes(bounds: &Bounds) -> Vec<KeyValue> {
    let mut out = Vec::new();
    if let Some(seconds) = bounds.stall_after_s {
        out.push(KeyValue::new(contract::BOUND_STALL_AFTER_S, seconds));
    }
    if let Some(steps) = bounds.max_steps {
        out.push(KeyValue::new(contract::BOUND_MAX_STEPS, steps));
    }
    if let Some(usd) = bounds.max_cost_usd {
        out.push(KeyValue::new(contract::BOUND_MAX_COST_USD, usd));
    }
    out
}
