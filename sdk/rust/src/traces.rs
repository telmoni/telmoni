//! Traces, steps, model calls, tool calls and retrievals, as guards and
//! closures.
//!
//! A trace is the agent's span at the root of a trace of its own, linked to
//! whatever span the application had current; every span inside it carries
//! the agent, user and session, so ingest knows each span's agent without
//! waiting for the trace's tree. A step, a model call, a tool call or a
//! retrieval is opened from the handle it belongs under, so nothing can be
//! opened outside a trace, and opens unless the trace reached a cap
//! ([`crate::guard`]).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use opentelemetry::trace::{
    Link, Span as _, SpanKind, SpanRef, Status, TraceContextExt as _, Tracer as _,
};
use opentelemetry::{Context, ContextGuard, KeyValue, Value};
use serde::Serialize;

use crate::client::Inner;
use crate::contract;
use crate::error::Error;
use crate::guard::{self, Guard, Opening, Tokens};
use crate::lock;
use crate::propagation::Continued;
use crate::scores::{self, Score};
use crate::warnings::{Warner, WarningKind};

/// The trace a context is inside, carried by every span opened under it:
/// what `inject` reads off the current context.
#[derive(Clone, Debug)]
pub(crate) struct TraceInfo {
    pub(crate) agent: String,
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

impl Usage {
    /// What this reply's usage says, and what `earlier` held where it says
    /// nothing. Each field named, so a new one cannot be skipped.
    pub(crate) fn or(self, earlier: Self) -> Self {
        let Self {
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_write_input_tokens,
            cost,
        } = self;
        Self {
            input_tokens: input_tokens.or(earlier.input_tokens),
            output_tokens: output_tokens.or(earlier.output_tokens),
            cache_read_input_tokens: cache_read_input_tokens.or(earlier.cache_read_input_tokens),
            cache_write_input_tokens: cache_write_input_tokens.or(earlier.cache_write_input_tokens),
            cost: cost.or(earlier.cost),
        }
    }
}

/// What a trace is opened with, each part optional: `TraceOptions::default()`
/// is a trace of the agent alone.
#[derive(Debug, Clone, Default)]
pub struct TraceOptions {
    /// The end user the trace is for, as the application's own id for them;
    /// ingest hashes it. Blank reads as none.
    pub user_id: Option<String>,
    /// The session the trace belongs to. Blank reads as none.
    pub session_id: Option<String>,
    /// Tags, sent as `telmoni.tag.<key>`.
    pub tags: BTreeMap<String, String>,
    /// The guard's cap on the trace's steps: its next step past it is
    /// [`Error::CapExceeded`].
    pub max_steps: Option<u64>,
    /// The guard's cap on the trace's spend, in US dollars: reached, what it
    /// opens next is [`Error::CapExceeded`].
    pub max_spend: Option<f64>,
    /// The messages the agent was asked with: content, which leaves the
    /// process only when [`Config::capture_content`](crate::Config::capture_content)
    /// is on and the project's mode allows.
    pub input_messages: Option<Content>,
}

/// What the agent answered, for [`Trace::answered`].
#[derive(Debug, Clone, Default)]
pub struct TraceReply {
    /// The messages the agent produced: content, which leaves the process
    /// only when [`Config::capture_content`](crate::Config::capture_content)
    /// is on and the project's mode allows.
    pub output_messages: Option<Content>,
}

/// What a model call is opened with; `ModelCallOptions::default()` is a
/// `chat` call with nothing but its model.
#[derive(Debug, Clone, Default)]
pub struct ModelCallOptions {
    /// The provider, as the conventions spell it (`openai`, `anthropic`, …).
    /// Blank reads as none.
    pub provider: Option<String>,
    /// The operation: `chat` unless given (`generate_content`,
    /// `text_completion`, `embeddings`); a blank one is `chat` too, and any
    /// other, with one warning.
    pub operation: Option<String>,
    /// The prompt it used, by name: sent in every content mode, so a prompt,
    /// never a person, who goes in [`TraceOptions::user_id`]. Blank reads as none.
    pub prompt: Option<String>,
    /// The prompt's version, sent only beside its name. Blank reads as none.
    pub prompt_version: Option<String>,
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

/// What a retrieval is opened with; `RetrievalOptions::default()` is a search
/// of a vector store or an index with nothing but its source.
#[derive(Debug, Clone, Default)]
pub struct RetrievalOptions {
    /// What it searched for: content, which leaves the process only when
    /// [`Config::capture_content`](crate::Config::capture_content) is on and
    /// the project's mode allows.
    pub query: Option<Content>,
    /// How many results it asked for.
    pub top_k: Option<u32>,
    /// A search of a memory store, `search_memory`, rather than of a vector
    /// store or an index.
    pub memory: bool,
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
    /// From the call's start to its answer's first token.
    pub time_to_first_token: Option<Duration>,
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
    /// under, and what [`context`](Trace::context) hands out.
    cx: Context,
    /// What every span inside the trace carries, shared by every span's handle.
    inherited: Arc<[KeyValue]>,
    /// The trace's caps, and what it has used of them.
    guard: Arc<Guard>,
    failed: AtomicBool,
    /// How many scores the span holds, held to [`contract::SCORES_PER_SPAN_MAX`].
    scores: AtomicUsize,
    ended: bool,
    /// What a caller may set again, an answer, a result or a mark, by key, set
    /// once as the span ends: OpenTelemetry's Rust span keeps every set, a key
    /// set again included, and drops each past its 128.
    last: Mutex<Vec<KeyValue>>,
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
            scores: AtomicUsize::new(0),
            ended: false,
            last: Mutex::new(Vec::new()),
        }
    }

    /// Holds `attributes` to be set as the span ends, each in place of what
    /// its key held.
    fn set_last(&self, attributes: impl IntoIterator<Item = KeyValue>) {
        let mut last = lock(&self.last);
        for attribute in attributes {
            match last.iter_mut().find(|held| held.key == attribute.key) {
                Some(held) => *held = attribute,
                None => last.push(attribute),
            }
        }
    }

    fn step(&self, name: &str) -> Result<Step, Error> {
        self.guard.opens(Opening::Step)?;
        let warner = &self.client.warner;
        // Ingest refuses a span named nothing.
        let name = if name.is_empty() {
            warner.once(
                "step:name".to_owned(),
                "telmoni: a step needs its name; one without is sent named step",
                WarningKind::Input.into(),
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
        self.guard.opens(Opening::Call)?;
        let warner = &self.client.warner;
        if model.is_empty() {
            warner.once(
                "call:model".to_owned(),
                "telmoni: a model call needs its model; one without is sent named by its operation alone",
                WarningKind::Input.into(),
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
        let operation = model_operation(options.operation.as_deref(), warner);
        let prompt = prompt_of(options.prompt, options.prompt_version, warner);
        let (name, mut attributes) = model_call_opening(&model, operation, provider, prompt);
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
        self.guard.opens(Opening::Call)?;
        let name = cut(name, contract::NAME_MAX, "tool's name", &self.client.warner);
        let (name, mut attributes) = tool_call_opening(&name);
        if let Some(arguments) = options.arguments {
            attributes.push(KeyValue::new(contract::TOOL_CALL_ARGUMENTS, arguments.0));
        }
        Ok(ToolCall {
            handle: self.child(name, SpanKind::Internal, attributes),
        })
    }

    fn retrieval(&self, source: &str, options: RetrievalOptions) -> Result<Retrieval, Error> {
        self.guard.opens(Opening::Call)?;
        let warner = &self.client.warner;
        let RetrievalOptions {
            query,
            top_k,
            memory,
        } = options;
        let (operation, source_key, query_key) = if memory {
            (
                contract::SEARCH_MEMORY,
                contract::MEMORY_STORE_ID,
                contract::MEMORY_QUERY_TEXT,
            )
        } else {
            (
                contract::RETRIEVAL_OPERATION,
                contract::DATA_SOURCE_ID,
                contract::RETRIEVAL_QUERY_TEXT,
            )
        };
        // Ingest names a retrieval by its source, and one with none by its span.
        let source = if source.trim().is_empty() {
            warner.once(
                "retrieval:source".to_owned(),
                "telmoni: a retrieval needs its source; one without is sent named by its operation alone",
                WarningKind::Input.into(),
            );
            None
        } else {
            Some(cut(
                source,
                contract::NAME_MAX,
                "retrieval's source",
                warner,
            ))
        };
        let name = match &source {
            Some(source) if !memory => format!("{operation} {source}"),
            _ => operation.to_owned(),
        };
        let mut attributes = vec![KeyValue::new(contract::OPERATION, operation)];
        if let Some(source) = source {
            attributes.push(KeyValue::new(source_key, source));
        }
        if let Some(top_k) = top_k {
            attributes.push(KeyValue::new(contract::RETRIEVAL_TOP_K, i64::from(top_k)));
        }
        if let Some(query) = query {
            attributes.push(KeyValue::new(query_key, query.0));
        }
        Ok(Retrieval {
            handle: self.child(name, SpanKind::Client, attributes),
            memory,
        })
    }

    fn fail(&self, error_type: &str) {
        self.set_failed(error_type, None);
    }

    /// Marks it failed by `error`, whose message is content: kept on the span
    /// as an exception event and the status's, for the exporter's lock.
    fn fail_by<E: fmt::Display>(&self, error: &E) {
        let mut message = String::new();
        // A `Display` that errs leaves what it wrote, where `to_string` panics.
        let _ = fmt::Write::write_fmt(&mut message, format_args!("{error}"));
        self.set_failed(&error_type::<E>(), Some(message));
    }

    fn set_failed(&self, error_type: &str, message: Option<String>) {
        let warner = &self.client.warner;
        let error_type = cut(
            error_type,
            contract::ERROR_TYPE_MAX,
            "span's error type",
            warner,
        );
        let span = self.cx.span();
        match message {
            Some(message) => {
                let mut attributes =
                    vec![KeyValue::new(contract::EXCEPTION_MESSAGE, message.clone())];
                if !error_type.is_empty() {
                    attributes.push(KeyValue::new(contract::EXCEPTION_TYPE, error_type.clone()));
                }
                span.add_event(contract::EXCEPTION_EVENT, attributes);
                span.set_status(Status::error(message));
            }
            None => span.set_status(Status::error("")),
        }
        self.set_last(error_type_attribute(error_type));
        self.failed.store(true, Ordering::Relaxed);
    }

    fn cancel(&self) {
        self.set_last([cancelled_attribute()]);
    }

    fn score(&self, score: &Score) {
        let span = self.cx.span();
        let warner = &self.client.warner;
        // Another service's trace, continued here, sends nothing from this side.
        if !span.is_recording() {
            return;
        }
        let held = self.scores.fetch_add(1, Ordering::Relaxed);
        if let Some(attributes) = scores::event(score, held, warner) {
            span.add_event(contract::EVALUATION_EVENT, attributes);
        } else {
            self.scores.fetch_sub(1, Ordering::Relaxed);
        }
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
        let span = self.cx.span();
        span.set_attributes(std::mem::take(&mut *lock(&self.last)));
        span.end();
    }
}

/// The last path segment of `E`'s type name, generics aside: `std::io::Error`
/// reads as `Error`, a crate's `PlanningFailed` as `PlanningFailed`.
fn error_type<E>() -> String {
    let name = std::any::type_name::<E>();
    let name = name.split('<').next().unwrap_or(name);
    name.rsplit("::").next().unwrap_or(name).to_owned()
}

/// A trace: the agent's span `invoke_agent <agent>`, ended when the guard
/// drops or on [`Trace::end`]. Opened by [`Telmoni::trace`](crate::Telmoni::trace).
pub struct Trace {
    handle: Handle,
    trace_id: String,
    agent: String,
}

/// A step inside a trace: a span named as the step is.
pub struct Step {
    handle: Handle,
}

/// A model call inside a trace: the span `<operation> <model>`, kind
/// `CLIENT`; [`ModelCall::answered`] records the reply.
pub struct ModelCall {
    handle: Handle,
    priced: Mutex<Priced>,
}

/// What a model call is priced by, as sent, its usage as its span holds it,
/// and what it last counted to its trace's spend.
struct Priced {
    provider: String,
    model: String,
    held: Usage,
    counted: f64,
}

/// A tool call inside a trace: the span `execute_tool <name>`;
/// [`ToolCall::returned`] records its result.
pub struct ToolCall {
    handle: Handle,
}

/// A retrieval inside a trace: the span `retrieval <source>`, or
/// `search_memory` for a memory, kind `CLIENT`; [`Retrieval::returned`]
/// records what it found.
pub struct Retrieval {
    handle: Handle,
    /// Whether it searched a memory, whose names its finds go under.
    memory: bool,
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
            /// `error.type`, none if empty, with no message.
            pub fn fail(&self, error_type: &str) {
                self.handle.fail(error_type);
            }

            /// Marks the span cancelled by the agent, `telmoni.cancelled`,
            /// which is neither ok nor an error.
            pub fn cancel(&self) {
                self.handle.cancel();
            }

            /// Scores the span, as OpenTelemetry's evaluation event on it; a
            /// score breaking a score's rules is not sent, with one warning.
            pub fn score(&self, score: Score) {
                self.handle.score(&score);
            }

            /// The span's id, 16 lowercase hex digits: what the client's
            /// [`score`](crate::Telmoni::score) names it by once it ended.
            #[must_use]
            pub fn span_id(&self) -> String {
                self.handle.cx.span().span_context().span_id().to_string()
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

handle_methods!(Trace, "Trace");
handle_methods!(Step, "Step");
handle_methods!(ModelCall, "ModelCall");
handle_methods!(ToolCall, "ToolCall");
handle_methods!(Retrieval, "Retrieval");

/// What a handle that can hold work has: steps, model calls, tool calls and
/// retrievals opened under it, as guards or around closures.
macro_rules! opens_children {
    ($handle:ident) => {
        impl $handle {
            /// A step under this span: a span named `name`, ended when its
            /// guard drops or on [`Step::end`].
            ///
            /// # Errors
            ///
            /// [`Error::CapExceeded`], opening nothing, once the trace reached
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
            /// [`Error::CapExceeded`], opening nothing, once the trace reached
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
            /// [`Error::CapExceeded`], opening nothing, once the trace reached
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
            /// failed with `E`'s type name (its last path segment) and its
            /// message, content under the exporter's lock, and is returned
            /// unchanged. An `Err` is the only error a closure can
            /// carry out; a panic unwinding through marks the span `panic`.
            ///
            /// # Errors
            ///
            /// `f`'s, or [`Self::step`]'s as an `E`, before `f` runs.
            pub fn try_step_with<T, E: From<Error> + fmt::Display>(
                &self,
                name: &str,
                f: impl FnOnce(&Step) -> Result<T, E>,
            ) -> Result<T, E> {
                let step = self.step(name)?;
                let result = f(&step);
                if let Err(error) = &result {
                    step.handle.fail_by(error);
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
            /// the call failed with `E`'s type name and its message and is
            /// returned unchanged.
            ///
            /// # Errors
            ///
            /// `f`'s, or [`Self::model_call`]'s as an `E`, before `f` runs.
            pub fn try_model_call_with<T, E: From<Error> + fmt::Display>(
                &self,
                model: &str,
                options: ModelCallOptions,
                f: impl FnOnce(&ModelCall) -> Result<T, E>,
            ) -> Result<T, E> {
                let call = self.model_call(model, options)?;
                let result = f(&call);
                if let Err(error) = &result {
                    call.handle.fail_by(error);
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
            /// call failed with `E`'s type name and its message and is returned
            /// unchanged.
            ///
            /// # Errors
            ///
            /// `f`'s, or [`Self::tool_call`]'s as an `E`, before `f` runs.
            pub fn try_tool_call_with<T, E: From<Error> + fmt::Display>(
                &self,
                name: &str,
                options: ToolCallOptions,
                f: impl FnOnce(&ToolCall) -> Result<T, E>,
            ) -> Result<T, E> {
                let call = self.tool_call(name, options)?;
                let result = f(&call);
                if let Err(error) = &result {
                    call.handle.fail_by(error);
                }
                call.end();
                result
            }

            /// A search of `source` under this span, the span `retrieval <source>`
            /// or `search_memory`; ended when its guard drops or on [`Retrieval::end`].
            ///
            /// # Errors
            ///
            /// [`Error::CapExceeded`], opening nothing, once the trace reached
            /// its spend cap.
            pub fn retrieval(
                &self,
                source: &str,
                options: RetrievalOptions,
            ) -> Result<Retrieval, Error> {
                self.handle.retrieval(source, options)
            }

            /// [`Self::retrieval`] around `f`: the retrieval ends when `f`
            /// returns, and `f`'s value is returned.
            ///
            /// # Errors
            ///
            /// As [`Self::retrieval`], before `f` runs.
            pub fn retrieval_with<R>(
                &self,
                source: &str,
                options: RetrievalOptions,
                f: impl FnOnce(&Retrieval) -> R,
            ) -> Result<R, Error> {
                let retrieval = self.retrieval(source, options)?;
                let value = f(&retrieval);
                retrieval.end();
                Ok(value)
            }

            /// [`Self::retrieval_with`] for a fallible `f`: an `Err` marks the
            /// retrieval failed with `E`'s type name and its message and is
            /// returned unchanged.
            ///
            /// # Errors
            ///
            /// `f`'s, or [`Self::retrieval`]'s as an `E`, before `f` runs.
            pub fn try_retrieval_with<T, E: From<Error> + fmt::Display>(
                &self,
                source: &str,
                options: RetrievalOptions,
                f: impl FnOnce(&Retrieval) -> Result<T, E>,
            ) -> Result<T, E> {
                let retrieval = self.retrieval(source, options)?;
                let result = f(&retrieval);
                if let Err(error) = &result {
                    retrieval.handle.fail_by(error);
                }
                retrieval.end();
                result
            }
        }
    };
}

opens_children!(Trace);
opens_children!(Step);
opens_children!(ToolCall);
opens_children!(Retrieval);

impl Trace {
    /// The trace's id, OpenTelemetry's: 32 lowercase hex digits, on every
    /// span inside the trace.
    #[must_use]
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    /// The agent's name as the trace sends it, cut to 128 characters.
    #[must_use]
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// What the trace's model calls have cost so far in US dollars, counted
    /// as its spend cap counts them, in this process.
    #[must_use]
    pub fn spent(&self) -> f64 {
        self.handle.guard.spent()
    }

    /// What the agent answered: its messages. Ending the trace takes the
    /// handle, so no answer can come after.
    pub fn answered(&self, reply: TraceReply) {
        if let Some(messages) = reply.output_messages {
            self.handle
                .set_last([KeyValue::new(contract::OUTPUT_MESSAGES, messages.0)]);
        }
    }

    /// Marks the trace failed with `E`'s type name and its message when
    /// `result` is an `Err`, and gives `result` back unchanged: what
    /// [`Telmoni::try_trace_with`](crate::Telmoni::try_trace_with) does for a
    /// closure, for a trace held as a guard.
    pub fn on_result<T, E: fmt::Display>(&self, result: Result<T, E>) -> Result<T, E> {
        if let Err(error) = &result {
            self.handle.fail_by(error);
        }
        result
    }
}

impl ModelCall {
    /// What the model answered: its name, its tokens, its messages, and when
    /// its first token came.
    pub fn answered(&self, reply: Reply) {
        let warner = &self.handle.client.warner;
        // Ingest refuses the call over an empty one.
        let answered_as = reply
            .response_model
            .filter(|model| !model.is_empty())
            .map(|model| cut(&model, contract::MODEL_MAX, "response model", warner));
        let usage = reply.usage.map(|usage| sendable_usage(usage, warner));
        if let Some(first) = reply.time_to_first_token {
            self.handle.set_last([KeyValue::new(
                contract::TIME_TO_FIRST_CHUNK,
                first.as_secs_f64(),
            )]);
        }
        self.count(answered_as, usage, reply.output_messages);
    }

    /// The answer on its span, and its cost to its trace's spend in place of
    /// what it counted before: reported, else priced by its name as sent,
    /// else nothing.
    fn count(&self, answered_as: Option<String>, usage: Option<Usage>, messages: Option<Content>) {
        // Given once the lock is released: the sink is the application's.
        if let Some((key, text)) = self.hold(answered_as, usage, messages) {
            self.handle
                .client
                .warner
                .once(key, &text, WarningKind::Input.into());
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
        if let Some(model) = answered_as {
            self.handle
                .set_last([KeyValue::new(contract::RESPONSE_MODEL, model.clone())]);
            // The name the call is priced by, as ingest reads it.
            priced.model = model;
        }
        if let Some(messages) = messages {
            self.handle
                .set_last([KeyValue::new(contract::OUTPUT_MESSAGES, messages.0)]);
        }
        if let Some(sent) = usage {
            self.handle.set_last(usage_attributes(sent));
            priced.held = sent.or(priced.held);
        }
        let Usage {
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_write_input_tokens,
            cost,
        } = priced.held;
        let whole = |count: Option<i64>| count.and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
        let tokens = Tokens {
            input: whole(input_tokens),
            output: whole(output_tokens),
            cache_read: whole(cache_read_input_tokens),
            cache_write: whole(cache_write_input_tokens),
        };
        let cost = cost.or_else(|| {
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
                    "telmoni: no price for {model} ({provider}), which reported no cost; it counts nothing toward the trace's spend cap"
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
            .set_last([KeyValue::new(contract::TOOL_CALL_RESULT, result.into().0)]);
    }
}

impl Retrieval {
    /// What the search found: the documents, content sent only when asked and
    /// allowed, and how many, sent in every mode; the last list given stands.
    pub fn returned(&self, documents: &[serde_json::Value]) {
        let (found, count) = if self.memory {
            (contract::MEMORY_RECORDS, contract::MEMORY_RECORD_COUNT)
        } else {
            (contract::RETRIEVAL_DOCUMENTS, contract::RETRIEVAL_RETURNED)
        };
        let sent = Content::from(serde_json::Value::from(documents));
        let held = i64::try_from(documents.len()).unwrap_or(i64::MAX);
        // One hold, under its one lock, so the last list sets both.
        self.handle
            .set_last([KeyValue::new(found, sent.0), KeyValue::new(count, held)]);
    }
}

/// Opens a trace for the client: the agent's span, at the root of a trace of
/// its own, linked to the span the application had current.
pub(crate) fn open(inner: &Arc<Inner>, agent: &str, options: TraceOptions) -> Result<Trace, Error> {
    if agent.trim().is_empty() {
        return Err(Error::EmptyAgent);
    }
    let agent = cut(agent, contract::AGENT_MAX, "trace's agent", &inner.warner);
    if options.max_steps == Some(0) {
        return Err(Error::MaxSteps);
    }
    if let Some(spend) = options.max_spend
        && !(spend.is_finite() && spend > 0.0)
    {
        return Err(Error::MaxSpend(spend));
    }

    let user_id = (options.user_id)
        .filter(|id| fits(id, contract::USER_ID_MAX, "user id", "trace", &inner.warner));
    let session_id = (options.session_id).filter(|id| {
        fits(
            id,
            contract::SESSION_ID_MAX,
            "session id",
            "trace",
            &inner.warner,
        )
    });
    let inherited = inherited(&agent, user_id, session_id);
    let mut attributes = agent_attributes(&agent, &inherited, options.tags, &inner.warner);
    if let Some(messages) = options.input_messages {
        attributes.push(KeyValue::new(contract::INPUT_MESSAGES, messages.0));
    }

    let application = Context::current();
    let tracer = &inner.tracer;
    let opened_at = SystemTime::now();
    let mut builder = tracer
        .span_builder(format!("{} {agent}", contract::AGENT_OPERATION))
        .with_kind(SpanKind::Internal)
        .with_attributes(attributes)
        .with_start_time(opened_at);
    // Linked to the application's span rather than opened under it, so a job
    // looping over executions is a trace each, never one.
    let linked = application.span().span_context().clone();
    if linked.is_valid() {
        builder = builder.with_links(vec![Link::new(linked, Vec::new(), 0)]);
    }
    let span = builder.start_with_context(tracer, &Context::new());
    let trace_id = span.span_context().trace_id().to_string();
    let cx = application.with_span(span).with_value(TraceInfo {
        agent: agent.clone(),
    });
    // The trace's opening, under the agent's span and at its start: the
    // processor sends it at once, so the trace is seen open before it ends.
    let mut start = tracer
        .span_builder(contract::TRACE_START)
        .with_kind(SpanKind::Internal)
        .with_attributes(inherited.clone())
        .with_start_time(opened_at)
        .start_with_context(tracer, &cx);
    start.end_with_timestamp(opened_at);

    Ok(Trace {
        handle: Handle {
            client: Arc::clone(inner),
            cx,
            inherited: Arc::from(inherited),
            guard: Arc::new(Guard::new(options.max_steps, options.max_spend)),
            failed: AtomicBool::new(false),
            scores: AtomicUsize::new(0),
            ended: false,
            last: Mutex::new(Vec::new()),
        },
        trace_id,
        agent,
    })
}

/// A trace another service opened, continued here under the span the
/// headers named: a trace whose agent's span is the caller's, remote and
/// non-recording here, so its steps and calls nest under it and carry the
/// agent, and nothing of it is sent or ended from this side.
pub(crate) fn continued(inner: &Arc<Inner>, read: Continued) -> Trace {
    let agent = cut(
        &read.agent,
        contract::AGENT_MAX,
        "trace's agent",
        &inner.warner,
    );
    let inherited = inherited(&agent, None, None);
    let trace_id = read.span_context.trace_id().to_string();
    let cx = Context::current()
        .with_remote_span_context(read.span_context)
        .with_value(TraceInfo {
            agent: agent.clone(),
        });
    Trace {
        handle: Handle {
            client: Arc::clone(inner),
            cx,
            inherited: Arc::from(inherited),
            guard: Arc::new(Guard::default()),
            failed: AtomicBool::new(false),
            scores: AtomicUsize::new(0),
            ended: false,
            last: Mutex::new(Vec::new()),
        },
        trace_id,
        agent,
    }
}

/// What every span inside a trace carries: its agent, and its user and
/// session when it has them.
pub(crate) fn inherited(
    agent: &str,
    user_id: Option<String>,
    session_id: Option<String>,
) -> Vec<KeyValue> {
    // Shared, so each span's copy of them is a count rather than a string.
    let shared = |text: &str| Value::from(Arc::<str>::from(text));
    let mut inherited = vec![KeyValue::new(contract::AGENT, shared(agent))];
    if let Some(user) = user_id.filter(|id| !id.is_empty()) {
        inherited.push(KeyValue::new(contract::USER_ID, shared(&user)));
    }
    if let Some(session) = session_id.filter(|id| !id.is_empty()) {
        inherited.push(KeyValue::new(contract::SESSION_ID, shared(&session)));
    }
    inherited
}

/// The agent span's own attributes: the operation and the agent, what every
/// span inside carries and the tags.
pub(crate) fn agent_attributes(
    agent: &str,
    inherited: &[KeyValue],
    mut tags: BTreeMap<String, String>,
    warner: &Warner,
) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(contract::OPERATION, contract::AGENT_OPERATION),
        KeyValue::new(contract::AGENT_NAME, agent.to_owned()),
    ];
    attributes.extend_from_slice(inherited);
    // Ingest drops these before it counts its 32, so they take no place there.
    let given = tags.len();
    tags.retain(|key, value| {
        (1..=contract::TAG_KEY_MAX).contains(&key.chars().count())
            && value.chars().count() <= contract::TAG_VALUE_MAX
    });
    if tags.len() < given {
        warner.once(
            "trace:tag".to_owned(),
            &format!(
                "telmoni: a tag's key must be 1 to {} characters and its value up to {}; the tag is not sent",
                contract::TAG_KEY_MAX,
                contract::TAG_VALUE_MAX
            ),
            WarningKind::Input.into(),
        );
    }
    // Ingest keeps no more, and more could crowd out what the trace's end sets.
    if tags.len() > contract::TAGS_MAX {
        warner.once(
            "trace:tags".to_owned(),
            &format!(
                "telmoni: a trace carries up to {} tags, the most ingest keeps; the rest are not sent",
                contract::TAGS_MAX
            ),
            WarningKind::Input.into(),
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

/// A model call's operation as ingest reads one: `chat` for a blank one, and,
/// with one warning, for one ingest would file as a step, an agent or a tool call.
pub(crate) fn model_operation(given: Option<&str>, warner: &Warner) -> String {
    match given.filter(|operation| !operation.trim().is_empty()) {
        Some(operation) if contract::MODEL_OPERATIONS.contains(&operation) => operation.to_owned(),
        Some(_) => {
            warner.once(
                "call:operation".to_owned(),
                "telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat",
                WarningKind::Input.into(),
            );
            contract::DEFAULT_MODEL_OPERATION.to_owned()
        }
        None => contract::DEFAULT_MODEL_OPERATION.to_owned(),
    }
}

/// A model call's span name, `<operation> <model>`, and what it opens with
/// besides what the trace carries and any content.
pub(crate) fn model_call_opening(
    model: &str,
    operation: String,
    provider: Option<String>,
    prompt: Option<(String, Option<String>)>,
) -> (String, Vec<KeyValue>) {
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
    if let Some((prompt, version)) = prompt {
        attributes.push(KeyValue::new(contract::PROMPT_NAME, prompt));
        if let Some(version) = version {
            attributes.push(KeyValue::new(contract::PROMPT_VERSION, version));
        }
    }
    (name, attributes)
}

/// A live call's prompt and its version: a blank one is none, and one past its
/// cap or a version without its name is left off with one warning.
fn prompt_of(
    prompt: Option<String>,
    version: Option<String>,
    warner: &Warner,
) -> Option<(String, Option<String>)> {
    let kept = |text: Option<String>, what: &str| {
        text.filter(|text| {
            !text.trim().is_empty()
                && fits(text, contract::ATTRIBUTE_VALUE_MAX, what, "call", warner)
        })
    };
    let (prompt, version) = (
        kept(prompt, "prompt's name"),
        kept(version, "prompt's version"),
    );
    if prompt.is_none() && version.is_some() {
        warner.once(
            "call:prompt_version".to_owned(),
            "telmoni: a prompt's version needs its prompt's name; the call is sent without it",
            WarningKind::Input.into(),
        );
    }
    prompt.map(|prompt| (prompt, version))
}

/// A tool call's span name, `execute_tool <name>`, and what it opens with
/// besides what the trace carries and any content.
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
    let Usage {
        input_tokens,
        output_tokens,
        cache_read_input_tokens,
        cache_write_input_tokens,
        cost,
    } = usage;
    [
        (contract::INPUT_TOKENS, input_tokens),
        (contract::OUTPUT_TOKENS, output_tokens),
        (contract::CACHE_READ_INPUT_TOKENS, cache_read_input_tokens),
        (contract::CACHE_WRITE_INPUT_TOKENS, cache_write_input_tokens),
    ]
    .into_iter()
    .filter_map(|(key, count)| count.map(|count| KeyValue::new(key, count)))
    .chain(cost.map(|cost| KeyValue::new(contract::COST, cost)))
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
                WarningKind::Input.into(),
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
                    WarningKind::Input.into(),
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
        WarningKind::Input.into(),
    );
    text.chars().take(most).collect()
}

/// Whether an id is within ingest's cap; one past it is left off its trace or
/// call with one warning, since cut it would name another.
fn fits(id: &str, most: usize, what: &str, span: &str, warner: &Warner) -> bool {
    if id.chars().nth(most).is_none() {
        return true;
    }
    warner.once(
        format!("long:{what}"),
        &format!(
            "telmoni: a {what} must be up to {most} characters; the {span} is sent without it"
        ),
        WarningKind::Input.into(),
    );
    false
}

/// Marks `span` failed: `error.type`, unless empty, which ingest would refuse
/// the span for, and status `ERROR` with no description, which is content.
pub(crate) fn mark_failed(span: &SpanRef<'_>, error_type: String) {
    if let Some(attribute) = error_type_attribute(error_type) {
        span.set_attribute(attribute);
    }
    span.set_status(Status::error(""));
}

/// A failure's `error.type`, none if empty.
fn error_type_attribute(error_type: String) -> Option<KeyValue> {
    (!error_type.is_empty()).then(|| KeyValue::new(contract::ERROR_TYPE, error_type))
}

/// Marks `span` cancelled by the agent, which is neither ok nor an error.
pub(crate) fn mark_cancelled(span: &SpanRef<'_>) {
    span.set_attribute(cancelled_attribute());
}

fn cancelled_attribute() -> KeyValue {
    KeyValue::new(contract::CANCELLED, true)
}
