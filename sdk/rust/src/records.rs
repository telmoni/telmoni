//! Runs recorded after they ended: a run handed whole, with its times, to
//! [`Telmoni::record_run`](crate::Telmoni::record_run), and sent as the live
//! calls would have sent it.
//!
//! Its trace is its own, named by the run's id, whatever trace is current;
//! each span's id is derived from that id and the span's place in the record.
//! Its spans are queued whole once built, the run's own last, and travel in
//! exports of their own, so the same record sent again is the same bytes,
//! whatever else is sent meanwhile.

use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};
use std::time::SystemTime;

use opentelemetry::trace::{SpanKind, TraceContextExt as _, TraceId, Tracer as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::SdkTracer;
use uuid::Uuid;

use crate::client::Inner;
use crate::contract;
use crate::error::{Error, Text};
use crate::ids;
use crate::runs::{self, Content, Usage};
use crate::scores::{self, Score};
use crate::warnings::Warner;

/// A run that has ended, as [`Telmoni::record_run`](crate::Telmoni::record_run)
/// sends it: its id, a UUID the caller gives, and everything in it, each with
/// its own times. [`RunRecord::new`] takes what every record has, and the
/// rest is filled in around it:
/// `RunRecord { session_id: Some(..), ..RunRecord::new(..) }`.
#[derive(Debug, Clone, PartialEq)]
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
    /// The messages the run was asked with: content, which leaves the process
    /// only when [`Config::capture_content`](crate::Config::capture_content)
    /// is on and the project's mode allows.
    pub input_messages: Option<Content>,
    /// The messages the run produced: content, as above.
    pub output_messages: Option<Content>,
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
    /// Scores of the span, each sent as its live handle's `score` sends one,
    /// at the span's end.
    pub scores: Vec<Score>,
}

/// What a run record or a step record holds: a step, with what it holds, or a
/// call.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RecordItem {
    /// A step, with what it holds.
    Step(StepRecord),
    /// A model call.
    ModelCall(ModelCallRecord),
    /// A tool call.
    ToolCall(ToolCallRecord),
}

/// A recorded call: a model call or a tool call.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CallRecord {
    /// A model call.
    Model(ModelCallRecord),
    /// A tool call.
    Tool(ToolCallRecord),
}

/// A step that has ended, with the calls made inside it and its own steps, a
/// subagent's say, up to 16 deep.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRecord {
    /// The step's name, which its span takes.
    pub name: String,
    /// The messages the step was asked with, a subagent's task say: content,
    /// as a run's are.
    pub input_messages: Option<Content>,
    /// The messages the step produced, the subagent's answer: content, as
    /// above.
    pub output_messages: Option<Content>,
    /// When the step started.
    pub start: SystemTime,
    /// When the step ended.
    pub end: SystemTime,
    /// What ran inside the step, in the order it ran: its calls, and steps.
    pub calls: Vec<RecordItem>,
    /// The type of the error the step failed with, never its message; `None`
    /// when it did not fail. Empty reads as none.
    pub error_type: Option<String>,
    /// Whether the agent cancelled the step.
    pub cancelled: bool,
    /// Scores of the span, each sent as its live handle's `score` sends one,
    /// at the span's end.
    pub scores: Vec<Score>,
}

/// A model call that has ended: its model, its times and its tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCallRecord {
    /// The model asked; the span is named `<operation> <model>`.
    pub model: String,
    /// The provider, as the conventions spell it (`openai`, `anthropic`, …).
    /// Empty reads as none.
    pub provider: Option<String>,
    /// The operation: `chat` unless given (`generate_content`,
    /// `text_completion`, `embeddings`); a blank one is `chat` too, and any
    /// other, with one warning.
    pub operation: Option<String>,
    /// How hard the model was asked to reason, as the provider names it:
    /// metadata, sent in every content mode. Empty reads as none.
    pub reasoning_effort: Option<String>,
    /// When the call started.
    pub start: SystemTime,
    /// When the call ended.
    pub end: SystemTime,
    /// The tokens used.
    pub usage: Option<Usage>,
    /// The messages sent to the model: content, as a run's are.
    pub input_messages: Option<Content>,
    /// The messages the call produced: content, as above.
    pub output_messages: Option<Content>,
    /// The type of the error the call failed with, never its message; `None`
    /// when it did not fail. Empty reads as none.
    pub error_type: Option<String>,
    /// Whether the agent cancelled the call.
    pub cancelled: bool,
    /// Scores of the span, each sent as its live handle's `score` sends one,
    /// at the span's end.
    pub scores: Vec<Score>,
}

/// A tool call that has ended: its name and its times.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRecord {
    /// The tool's name; the span is named `execute_tool <name>`.
    pub name: String,
    /// The arguments the tool was called with: content, as a run's messages
    /// are.
    pub arguments: Option<Content>,
    /// What the tool returned: content, as above.
    pub result: Option<Content>,
    /// When the call started.
    pub start: SystemTime,
    /// When the call ended.
    pub end: SystemTime,
    /// The type of the error the call failed with, never its message; `None`
    /// when it did not fail. Empty reads as none.
    pub error_type: Option<String>,
    /// Whether the agent cancelled the call.
    pub cancelled: bool,
    /// Scores of the span, each sent as its live handle's `score` sends one,
    /// at the span's end.
    pub scores: Vec<Score>,
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
            input_messages: None,
            output_messages: None,
            start,
            end,
            inside: Vec::new(),
            error_type: None,
            cancelled: false,
            scores: Vec::new(),
        }
    }
}

impl StepRecord {
    /// A record of the step `name` from `start` to `end`, with no content or
    /// calls, neither failed nor cancelled.
    #[must_use]
    pub fn new(name: impl Into<String>, start: SystemTime, end: SystemTime) -> Self {
        Self {
            name: name.into(),
            input_messages: None,
            output_messages: None,
            start,
            end,
            calls: Vec::new(),
            error_type: None,
            cancelled: false,
            scores: Vec::new(),
        }
    }
}

impl ModelCallRecord {
    /// A record of a `chat` call to `model` from `start` to `end`, with no
    /// provider, reasoning effort, tokens or content, neither failed nor
    /// cancelled.
    #[must_use]
    pub fn new(model: impl Into<String>, start: SystemTime, end: SystemTime) -> Self {
        Self {
            model: model.into(),
            provider: None,
            operation: None,
            reasoning_effort: None,
            start,
            end,
            usage: None,
            input_messages: None,
            output_messages: None,
            error_type: None,
            cancelled: false,
            scores: Vec::new(),
        }
    }
}

impl ToolCallRecord {
    /// A record of a call to the tool `name` from `start` to `end`, with no
    /// content, neither failed nor cancelled.
    #[must_use]
    pub fn new(name: impl Into<String>, start: SystemTime, end: SystemTime) -> Self {
        Self {
            name: name.into(),
            arguments: None,
            result: None,
            start,
            end,
            error_type: None,
            cancelled: false,
            scores: Vec::new(),
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
    check(&record)?;
    let rate = inner.config.sample_rate;
    if !(rate > 0.0 && rate <= 1.0) {
        return Err(Error::SampleRate(rate));
    }
    let need = 1 + spans_in(&record.inside);

    let RunRecord {
        agent,
        user_id,
        session_id,
        tags,
        input_messages,
        output_messages,
        start,
        end,
        mut inside,
        error_type,
        cancelled,
        scores,
        ..
    } = record;
    let run_id = run_uuid.to_string();
    let inherited = runs::inherited(&run_id, &agent, Some(rate), user_id, session_id);
    let mut attributes = runs::run_attributes(&agent, &inherited, tags, &inner.warner);
    attributes.extend(content([
        (contract::INPUT_MESSAGES, input_messages),
        (contract::OUTPUT_MESSAGES, output_messages),
    ]));
    // Sampled out, no span is built: none of a record's ends after it, and
    // the same record sent again draws again.
    if rate < 1.0 && (inner.random)() >= rate {
        return Ok(());
    }
    // After the draw, as Python and TypeScript warn, and before the hold: a
    // sink is the application's, and may record a run of its own.
    for call in model_calls(&mut inside) {
        call.usage = call
            .usage
            .map(|usage| runs::sendable_usage(usage, &inner.warner));
        call.operation = Some(runs::model_operation(
            call.operation.as_deref(),
            &inner.warner,
        ));
    }

    // Queued whole or not at all: a record that outgrows the queue's room
    // would lose its tail, the run's own span first.
    let trace_id = ids::trace_id(&run_uuid);
    let judged = judged(&scores, &inside, &inner.warner);
    let hold = inner.processor.hold(trace_id, need)?;
    let mut spans = Spans {
        tracer: &inner.tracer,
        scores: judged,
        run_id: run_uuid,
        trace_id,
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
    spans.items(inside, &run, &inherited);
    spans.close(&run, error_type, cancelled, end);
    hold.queue();
    Ok(())
}

/// Each span's scores as its events, in the order [`Spans::close`] closes the
/// spans — a step after what it holds, the run last — judged before the hold,
/// since a refusal's warning goes to a sink of the application's, which may
/// record a run of its own.
fn judged(run: &[Score], inside: &[RecordItem], warner: &Warner) -> VecDeque<Vec<Vec<KeyValue>>> {
    fn closing(items: &[RecordItem], warner: &Warner, judged: &mut VecDeque<Vec<Vec<KeyValue>>>) {
        for item in items {
            let scores = match item {
                RecordItem::Step(step) => {
                    closing(&step.calls, warner, judged);
                    &step.scores
                }
                RecordItem::ModelCall(call) => &call.scores,
                RecordItem::ToolCall(call) => &call.scores,
            };
            judged.push_back(events(scores, warner));
        }
    }
    let mut judged = VecDeque::new();
    closing(inside, warner, &mut judged);
    judged.push_back(events(run, warner));
    judged
}

/// A span's scores as its events, each one refused warned of.
fn events(scores: &[Score], warner: &Warner) -> Vec<Vec<KeyValue>> {
    let mut held = 0;
    scores
        .iter()
        .filter_map(|score| {
            let attributes = scores::event(score, held, warner)?;
            held += 1;
            Some(attributes)
        })
        .collect()
}

/// How many spans `items` queue: each step, with what it holds, and each call.
fn spans_in(items: &[RecordItem]) -> usize {
    items
        .iter()
        .map(|item| match item {
            RecordItem::Step(step) => 1 + spans_in(&step.calls),
            RecordItem::ModelCall(_) | RecordItem::ToolCall(_) => 1,
        })
        .sum()
}

/// How deep steps nest, a run's own at 1: past any agent's subagents.
const STEP_DEPTH_MAX: usize = 16;

/// Refuses `record` for a step nested too deep, a span named nothing or
/// holding text past ingest's cap, at a time ingest cannot read, or ending
/// before it starts.
fn check(record: &RunRecord) -> Result<(), Error> {
    let mut spans = vec![Checked::run(record)];
    spans_of(&record.inside, 1, &mut spans)?;
    for span in &spans {
        if span.name == Some("") {
            return Err(Error::EmptyName);
        }
        if let Some(&(what, _, max)) = span.texts.iter().find(|&&(_, chars, max)| chars > max) {
            return Err(Error::TooLong { what, max });
        }
    }
    if !spans
        .iter()
        .all(|span| readable(span.start) && readable(span.end))
    {
        return Err(Error::RecordedTime);
    }
    if spans.iter().any(|span| span.end < span.start) {
        return Err(Error::EndBeforeStart);
    }
    Ok(())
}

/// Each span `items` hold, as [`check`] reads it, a step among them `depth`
/// deep.
fn spans_of<'a>(
    items: &'a [RecordItem],
    depth: usize,
    spans: &mut Vec<Checked<'a>>,
) -> Result<(), Error> {
    for item in items {
        match item {
            RecordItem::Step(step) => {
                if depth > STEP_DEPTH_MAX {
                    return Err(Error::TooDeep {
                        max: STEP_DEPTH_MAX,
                    });
                }
                spans.push(Checked::step(step));
                spans_of(&step.calls, depth + 1, spans)?;
            }
            RecordItem::ModelCall(call) => spans.push(Checked::model_call(call)),
            RecordItem::ToolCall(call) => spans.push(Checked::tool_call(call)),
        }
    }
    Ok(())
}

/// A span of a record, as [`check`] reads it.
struct Checked<'a> {
    /// The name ingest refuses empty; none for a tool's, since ingest reads
    /// an empty one as unset.
    name: Option<&'a str>,
    /// Each text ingest caps: what it is, its characters and its cap.
    texts: Vec<(Text, usize, usize)>,
    start: SystemTime,
    end: SystemTime,
}

impl<'a> Checked<'a> {
    fn run(record: &'a RunRecord) -> Self {
        Self {
            name: Some(&record.agent),
            texts: vec![
                (
                    Text::RunAgent,
                    chars(Some(&record.agent)),
                    contract::AGENT_MAX,
                ),
                (
                    Text::RunUserId,
                    chars(record.user_id.as_deref()),
                    contract::USER_ID_MAX,
                ),
                (
                    Text::RunSessionId,
                    chars(record.session_id.as_deref()),
                    contract::SESSION_ID_MAX,
                ),
                (
                    Text::RunErrorType,
                    chars(record.error_type.as_deref()),
                    contract::ERROR_TYPE_MAX,
                ),
            ],
            start: record.start,
            end: record.end,
        }
    }

    fn step(step: &'a StepRecord) -> Self {
        Self {
            name: Some(&step.name),
            texts: vec![
                (Text::StepName, chars(Some(&step.name)), contract::NAME_MAX),
                (
                    Text::StepErrorType,
                    chars(step.error_type.as_deref()),
                    contract::ERROR_TYPE_MAX,
                ),
            ],
            start: step.start,
            end: step.end,
        }
    }

    fn model_call(call: &'a ModelCallRecord) -> Self {
        Self {
            name: Some(&call.model),
            texts: vec![
                (
                    Text::ModelCallModel,
                    chars(Some(&call.model)),
                    contract::MODEL_MAX,
                ),
                (
                    Text::ModelCallProvider,
                    chars(call.provider.as_deref()),
                    contract::PROVIDER_MAX,
                ),
                (
                    Text::ModelCallReasoningEffort,
                    chars(call.reasoning_effort.as_deref()),
                    contract::ATTRIBUTE_VALUE_MAX,
                ),
                (
                    Text::ModelCallErrorType,
                    chars(call.error_type.as_deref()),
                    contract::ERROR_TYPE_MAX,
                ),
            ],
            start: call.start,
            end: call.end,
        }
    }

    fn tool_call(call: &'a ToolCallRecord) -> Self {
        Self {
            name: None,
            texts: vec![
                (
                    Text::ToolCallName,
                    chars(Some(&call.name)),
                    contract::NAME_MAX,
                ),
                (
                    Text::ToolCallErrorType,
                    chars(call.error_type.as_deref()),
                    contract::ERROR_TYPE_MAX,
                ),
            ],
            start: call.start,
            end: call.end,
        }
    }
}

/// The characters in `text`, as ingest counts them; none when unset.
fn chars(text: Option<&str>) -> usize {
    text.map_or(0, |text| text.chars().count())
}

/// Whether ingest reads `time`: nanoseconds since the epoch, above 0 and
/// within 64 bits.
fn readable(time: SystemTime) -> bool {
    crate::exporter::unix_nanos(time).is_some()
}

/// Every model call `inside` holds, each step's among them.
fn model_calls(inside: &mut [RecordItem]) -> Vec<&mut ModelCallRecord> {
    let mut calls = Vec::new();
    for item in inside {
        match item {
            RecordItem::Step(step) => calls.extend(model_calls(&mut step.calls)),
            RecordItem::ModelCall(call) => calls.push(call),
            RecordItem::ToolCall(_) => {}
        }
    }
    calls
}

/// Opens a record's spans in the record's order, each with the id its place
/// derives, in the trace the run's id names.
struct Spans<'a> {
    tracer: &'a SdkTracer,
    /// Each span's scores as events, in the order the spans close.
    scores: VecDeque<Vec<Vec<KeyValue>>>,
    run_id: Uuid,
    trace_id: TraceId,
    /// The next span's place: `0` the run, then each step and call from `1`
    /// as the record holds them, depth first, a step before what it holds.
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

    /// Scores the span `cx` holds with its record's next judged scores and
    /// marks it failed or cancelled, as its record says, then ends it when its
    /// record says it ended.
    fn close(
        &mut self,
        cx: &Context,
        error_type: Option<String>,
        cancelled: bool,
        end: SystemTime,
    ) {
        let span = cx.span();
        for attributes in self.scores.pop_front().unwrap_or_default() {
            span.add_event_with_timestamp(contract::EVALUATION_EVENT, end, attributes);
        }
        if let Some(error_type) = error_type.filter(|error_type| !error_type.is_empty()) {
            runs::mark_failed(&span, error_type);
        }
        if cancelled {
            runs::mark_cancelled(&span);
        }
        span.end_with_timestamp(end);
    }

    /// What a run or a step holds, under `parent`, each step's own under it,
    /// so each closes before what holds it.
    fn items(&mut self, items: Vec<RecordItem>, parent: &Context, inherited: &[KeyValue]) {
        for item in items {
            match item {
                RecordItem::Step(step) => {
                    let mut attributes = inherited.to_vec();
                    attributes.extend(content([
                        (contract::INPUT_MESSAGES, step.input_messages),
                        (contract::OUTPUT_MESSAGES, step.output_messages),
                    ]));
                    let opened = self.open(
                        step.name,
                        parent,
                        SpanKind::Internal,
                        attributes,
                        step.start,
                    );
                    self.items(step.calls, &opened, inherited);
                    self.close(&opened, step.error_type, step.cancelled, step.end);
                }
                RecordItem::ModelCall(call) => {
                    self.call(CallRecord::Model(call), parent, inherited);
                }
                RecordItem::ToolCall(call) => self.call(CallRecord::Tool(call), parent, inherited),
            }
        }
    }

    /// A call under `parent`, closed as soon as it opens, as a call holds
    /// nothing.
    fn call(&mut self, call: CallRecord, parent: &Context, inherited: &[KeyValue]) {
        let mut attributes = inherited.to_vec();
        match call {
            CallRecord::Model(call) => {
                let operation = call.operation.unwrap_or_default();
                let (name, own) = runs::model_call_opening(&call.model, operation, call.provider);
                attributes.extend(own);
                if let Some(effort) = call.reasoning_effort.filter(|effort| !effort.is_empty()) {
                    attributes.push(KeyValue::new(contract::REQUEST_REASONING_LEVEL, effort));
                }
                attributes.extend(call.usage.into_iter().flat_map(runs::usage_attributes));
                attributes.extend(content([
                    (contract::INPUT_MESSAGES, call.input_messages),
                    (contract::OUTPUT_MESSAGES, call.output_messages),
                ]));
                let opened = self.open(name, parent, SpanKind::Client, attributes, call.start);
                self.close(&opened, call.error_type, call.cancelled, call.end);
            }
            CallRecord::Tool(call) => {
                let (name, own) = runs::tool_call_opening(&call.name);
                attributes.extend(own);
                attributes.extend(content([
                    (contract::TOOL_CALL_ARGUMENTS, call.arguments),
                    (contract::TOOL_CALL_RESULT, call.result),
                ]));
                let opened = self.open(name, parent, SpanKind::Internal, attributes, call.start);
                self.close(&opened, call.error_type, call.cancelled, call.end);
            }
        }
    }
}

/// A record's content as attributes, each one given, which the exporter's
/// lock sends or keeps back.
fn content<const N: usize>(
    fields: [(&'static str, Option<Content>); N],
) -> impl Iterator<Item = KeyValue> {
    fields.into_iter().filter_map(|(key, value)| {
        value.map(|value| KeyValue::new(key, String::from(value.as_str())))
    })
}
