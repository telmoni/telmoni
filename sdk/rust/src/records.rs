//! Runs recorded after they ended: a run handed whole, with its times, to
//! [`Telmoni::record_run`](crate::Telmoni::record_run), and sent as the live
//! calls would have sent it.
//!
//! Its trace is its own, named by the run's id, whatever trace is current;
//! each span's id is derived from that id and the span's place in the record.
//! Its spans are queued together under the processor's hold, the run's start
//! first and the run's own last, so the same record sent again is the same
//! bytes.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::time::SystemTime;

use opentelemetry::trace::{SpanKind, TraceContextExt as _, TraceId, Tracer as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::SdkTracer;
use uuid::Uuid;

use crate::client::Inner;
use crate::contract;
use crate::error::Error;
use crate::ids;
use crate::runs::{self, Usage};

/// A run that has ended, as [`Telmoni::record_run`](crate::Telmoni::record_run)
/// sends it: its id, a UUID the caller gives, and everything in it, each with
/// its own times. [`RunRecord::new`] takes what every record has, and the
/// rest is filled in around it:
/// `RunRecord { session_id: Some(..), ..RunRecord::new(..) }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    /// The run's id, a UUID: kept with the log the record comes from, and the
    /// same each time the run is sent. Sent hyphenated and in lowercase,
    /// however it is written here.
    pub run_id: String,
    /// The agent's name; a run is counted under its agent.
    pub agent: String,
    /// The end user the run was for, as the application's own id for them;
    /// ingest hashes it. Empty reads as none.
    pub user_id: Option<String>,
    /// The session the run belongs to. Empty reads as none.
    pub session_id: Option<String>,
    /// Tags, sent as `telmoni.tag.<key>`.
    pub tags: BTreeMap<String, String>,
    /// When the run started.
    pub start: SystemTime,
    /// When the run ended.
    pub end: SystemTime,
    /// What ran inside the run, in the order it ran: steps, each holding its
    /// own calls, and the calls made in the run itself.
    pub inside: Vec<RecordItem>,
    /// The type of the error the run failed with — the exception's class or
    /// the error's name, never its message; `None` when it did not fail.
    /// Empty reads as none.
    pub error_type: Option<String>,
    /// Whether the agent cancelled the run, which is neither ok nor an error.
    pub cancelled: bool,
}

/// What a run record holds: a step, with its own calls, or a call made in
/// the run itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordItem {
    /// A step, with the calls made inside it.
    Step(StepRecord),
    /// A model call made in the run itself.
    ModelCall(ModelCallRecord),
    /// A tool call made in the run itself.
    ToolCall(ToolCallRecord),
}

/// What a step record holds: a model call or a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallRecord {
    /// A model call.
    Model(ModelCallRecord),
    /// A tool call.
    Tool(ToolCallRecord),
}

/// A step that has ended, with the calls made inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    /// The step's name, which its span takes.
    pub name: String,
    /// When the step started.
    pub start: SystemTime,
    /// When the step ended.
    pub end: SystemTime,
    /// The calls made inside the step, in the order they ran.
    pub calls: Vec<CallRecord>,
    /// The type of the error the step failed with, never its message; `None`
    /// when it did not fail. Empty reads as none.
    pub error_type: Option<String>,
    /// Whether the agent cancelled the step.
    pub cancelled: bool,
}

/// A model call that has ended: its model, its times and its tokens. A record
/// holds no content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCallRecord {
    /// The model asked; the span is named `<operation> <model>`.
    pub model: String,
    /// The provider, as the conventions spell it (`openai`, `anthropic`, …).
    /// Empty reads as none.
    pub provider: Option<String>,
    /// The operation: `chat` unless given (`generate_content`,
    /// `text_completion`, `embeddings`); a blank one is `chat` too.
    pub operation: Option<String>,
    /// When the call started.
    pub start: SystemTime,
    /// When the call ended.
    pub end: SystemTime,
    /// The tokens used.
    pub usage: Option<Usage>,
    /// The type of the error the call failed with, never its message; `None`
    /// when it did not fail. Empty reads as none.
    pub error_type: Option<String>,
    /// Whether the agent cancelled the call.
    pub cancelled: bool,
}

/// A tool call that has ended: its name and its times. A record holds no
/// content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRecord {
    /// The tool's name; the span is named `execute_tool <name>`.
    pub name: String,
    /// When the call started.
    pub start: SystemTime,
    /// When the call ended.
    pub end: SystemTime,
    /// The type of the error the call failed with, never its message; `None`
    /// when it did not fail. Empty reads as none.
    pub error_type: Option<String>,
    /// Whether the agent cancelled the call.
    pub cancelled: bool,
}

impl RunRecord {
    /// A record of `agent`'s run `run_id` from `start` to `end`, and nothing
    /// more: no user, session or tags, nothing inside, neither failed nor
    /// cancelled.
    #[must_use]
    pub fn new(
        run_id: impl Into<String>,
        agent: impl Into<String>,
        start: SystemTime,
        end: SystemTime,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            agent: agent.into(),
            user_id: None,
            session_id: None,
            tags: BTreeMap::new(),
            start,
            end,
            inside: Vec::new(),
            error_type: None,
            cancelled: false,
        }
    }
}

impl StepRecord {
    /// A record of the step `name` from `start` to `end`, with no calls,
    /// neither failed nor cancelled.
    #[must_use]
    pub fn new(name: impl Into<String>, start: SystemTime, end: SystemTime) -> Self {
        Self {
            name: name.into(),
            start,
            end,
            calls: Vec::new(),
            error_type: None,
            cancelled: false,
        }
    }
}

impl ModelCallRecord {
    /// A record of a `chat` call to `model` from `start` to `end`, with no
    /// provider or tokens, neither failed nor cancelled.
    #[must_use]
    pub fn new(model: impl Into<String>, start: SystemTime, end: SystemTime) -> Self {
        Self {
            model: model.into(),
            provider: None,
            operation: None,
            start,
            end,
            usage: None,
            error_type: None,
            cancelled: false,
        }
    }
}

impl ToolCallRecord {
    /// A record of a call to the tool `name` from `start` to `end`, neither
    /// failed nor cancelled.
    #[must_use]
    pub fn new(name: impl Into<String>, start: SystemTime, end: SystemTime) -> Self {
        Self {
            name: name.into(),
            start,
            end,
            error_type: None,
            cancelled: false,
        }
    }
}

impl From<StepRecord> for RecordItem {
    fn from(step: StepRecord) -> Self {
        Self::Step(step)
    }
}

impl From<ModelCallRecord> for RecordItem {
    fn from(call: ModelCallRecord) -> Self {
        Self::ModelCall(call)
    }
}

impl From<ToolCallRecord> for RecordItem {
    fn from(call: ToolCallRecord) -> Self {
        Self::ToolCall(call)
    }
}

impl From<CallRecord> for RecordItem {
    fn from(call: CallRecord) -> Self {
        match call {
            CallRecord::Model(call) => Self::ModelCall(call),
            CallRecord::Tool(call) => Self::ToolCall(call),
        }
    }
}

impl From<ModelCallRecord> for CallRecord {
    fn from(call: ModelCallRecord) -> Self {
        Self::Model(call)
    }
}

impl From<ToolCallRecord> for CallRecord {
    fn from(call: ToolCallRecord) -> Self {
        Self::Tool(call)
    }
}

/// Sends `record` for the client, as
/// [`Telmoni::record_run`](crate::Telmoni::record_run) says.
pub(crate) fn send(inner: &Inner, record: RunRecord) -> Result<(), Error> {
    if record.agent.trim().is_empty() {
        return Err(Error::EmptyAgent);
    }
    let run_uuid = Uuid::parse_str(&record.run_id).map_err(|_| Error::RunId)?;
    // The run's id is its trace's, and a trace id of zeros is no trace at all.
    if run_uuid.is_nil() {
        return Err(Error::RunId);
    }
    // Checked whole before anything is queued, so a record refused sends
    // nothing rather than half a run.
    if ends_early(&record) {
        return Err(Error::EndBeforeStart);
    }
    let rate = inner.config.sample_rate;
    if !(rate > 0.0 && rate <= 1.0) {
        return Err(Error::SampleRate(rate));
    }

    let RunRecord {
        agent,
        user_id,
        session_id,
        tags,
        start,
        end,
        inside,
        error_type,
        cancelled,
        ..
    } = record;
    let run_id = run_uuid.to_string();
    let inherited = runs::inherited(&run_id, &agent, user_id, session_id);
    let attributes = runs::run_attributes(&agent, &inherited, &[], rate, tags);
    if rate < 1.0 && (inner.random)() >= rate {
        inner.processor.sample_out(&run_id);
    }

    let _hold = inner.processor.hold();
    let mut spans = Spans {
        tracer: &inner.tracer,
        run_id: run_uuid,
        trace_id: ids::trace_id(&run_uuid),
        place: 0,
    };
    // Under an empty context rather than the current one: the record's trace
    // is its own, whatever the application has open.
    let run = spans.open(
        format!("{} {agent}", contract::RUN_OPERATION),
        &Context::new(),
        SpanKind::Internal,
        attributes,
        start,
    );
    spans
        .open(
            contract::RUN_START,
            &run,
            SpanKind::Internal,
            runs::start_attributes(&inherited, &[], rate),
            start,
        )
        .span()
        .end_with_timestamp(start);
    for item in inside {
        match item {
            RecordItem::Step(step) => {
                let opened = spans.open(
                    step.name,
                    &run,
                    SpanKind::Internal,
                    inherited.clone(),
                    step.start,
                );
                for call in step.calls {
                    spans.call(call, &opened, &inherited);
                }
                close(&opened, step.error_type, step.cancelled, step.end);
            }
            RecordItem::ModelCall(call) => spans.call(CallRecord::Model(call), &run, &inherited),
            RecordItem::ToolCall(call) => spans.call(CallRecord::Tool(call), &run, &inherited),
        }
    }
    close(&run, error_type, cancelled, end);
    Ok(())
}

/// Whether any span of `record` — the run's own, a step's or a call's — ends
/// before it starts.
fn ends_early(record: &RunRecord) -> bool {
    record.end < record.start
        || record.inside.iter().any(|item| match item {
            RecordItem::Step(step) => {
                step.end < step.start || step.calls.iter().any(call_ends_early)
            }
            RecordItem::ModelCall(call) => call.end < call.start,
            RecordItem::ToolCall(call) => call.end < call.start,
        })
}

fn call_ends_early(call: &CallRecord) -> bool {
    match call {
        CallRecord::Model(call) => call.end < call.start,
        CallRecord::Tool(call) => call.end < call.start,
    }
}

/// Opens a record's spans in the record's order, each with the id its place
/// derives, in the trace the run's id names.
struct Spans<'a> {
    tracer: &'a SdkTracer,
    run_id: Uuid,
    trace_id: TraceId,
    /// The next span's place: `0` the run, `1` its start, then each step and
    /// call as the record holds them, a step before its calls.
    place: u64,
}

impl Spans<'_> {
    /// The record's next span, under `parent`, as a context holding it: what
    /// the spans inside it are opened under.
    fn open(
        &mut self,
        name: impl Into<Cow<'static, str>>,
        parent: &Context,
        kind: SpanKind,
        attributes: Vec<KeyValue>,
        start: SystemTime,
    ) -> Context {
        let span_id = ids::span_id(&self.run_id, self.place);
        self.place += 1;
        let span = {
            let _given = ids::given(self.trace_id, span_id);
            self.tracer
                .span_builder(name)
                .with_kind(kind)
                .with_attributes(attributes)
                .with_start_time(start)
                .start_with_context(self.tracer, parent)
        };
        parent.with_span(span)
    }

    /// A call under `parent`, closed as soon as it opens, as a call holds
    /// nothing.
    fn call(&mut self, call: CallRecord, parent: &Context, inherited: &[KeyValue]) {
        let mut attributes = inherited.to_vec();
        match call {
            CallRecord::Model(call) => {
                let (name, own) =
                    runs::model_call_opening(&call.model, call.operation, call.provider);
                attributes.extend(own);
                attributes.extend(call.usage.into_iter().flat_map(runs::usage_attributes));
                let opened = self.open(name, parent, SpanKind::Client, attributes, call.start);
                close(&opened, call.error_type, call.cancelled, call.end);
            }
            CallRecord::Tool(call) => {
                let (name, own) = runs::tool_call_opening(&call.name);
                attributes.extend(own);
                let opened = self.open(name, parent, SpanKind::Internal, attributes, call.start);
                close(&opened, call.error_type, call.cancelled, call.end);
            }
        }
    }
}

/// Marks the span `cx` holds failed or cancelled, as its record says, and
/// ends it when its record says it ended.
fn close(cx: &Context, error_type: Option<String>, cancelled: bool, end: SystemTime) {
    let span = cx.span();
    if let Some(error_type) = error_type.filter(|error_type| !error_type.is_empty()) {
        runs::mark_failed(&span, error_type);
    }
    if cancelled {
        runs::mark_cancelled(&span);
    }
    span.end_with_timestamp(end);
}
