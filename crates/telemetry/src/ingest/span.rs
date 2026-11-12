//! One OTLP span, read by the contract into a row of `spans`: refused whole
//! where it breaks a field's rule, kept with an attribute dropped where only
//! the attribute does.

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::common::v1::{KeyValue, any_value::Value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{Span, status::StatusCode};

use telmoni_shared::digest::hex_lower as hex;
use telmoni_shared::{ProjectId, SpanKind, SpanStatus};

use crate::contract::{self, ATTRIBUTES_MAX, SPAN_FIXED_BYTES, SPAN_UNIT_BYTES};
use crate::ingest::report::Drops;
use crate::store::SpanRow;

/// What every span of one export is read under.
pub struct Context<'a> {
    /// The key's project: the row's, and nothing above it.
    pub project_id: &'a ProjectId,
    /// The server's clock at receipt, to the microsecond the table keeps.
    pub received_at: DateTime<Utc>,
    /// The instrumentation scope the span was opened under.
    pub scope: &'a str,
    /// The deployment the resource names, and its release; each empty where
    /// it names none ([`read_resource`]).
    pub environment: &'a str,
    pub release: &'a str,
    /// The resource's attributes the inventory names.
    pub resource: &'a [(String, String)],
    /// The inventory rows the resource's attributes fall under.
    pub resource_sent: &'a [&'static str],
    /// What a `user_id` or a `session_id` is stored as: the project's own
    /// hash of it, so an id built from an email is never kept as sent.
    pub store_id: &'a dyn Fn(&str) -> String,
    /// Whether the project keeps content: its mode is `on`.
    pub keep_content: bool,
}

/// Why a span was refused whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Another exporter's span past the spans its project holds waiting for
    /// their trace ([`crate::foreign::HELD_MAX`]).
    Waiting,
    /// A field whose value is not what its rule asks: the field's name.
    Invalid(&'static str),
    /// A field past its cap: the field's name.
    OverCap(&'static str),
}

/// A span refused whole, named by its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The span's id, in hex, as it was sent.
    pub span_id: String,
    pub reason: Refusal,
}

/// Why an attribute was dropped, the span kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason {
    /// Content, on a project that keeps none.
    Content,
    /// Text, or an array, the field inventory does not name.
    Unlisted,
    /// Past a cap: which.
    OverCap(Cap),
    /// Content past its organization's ceiling ([`crate::capacity`]): the
    /// span keeps its metadata alone.
    ContentFull,
    /// A tag past its project's cap for the day ([`crate::ingest::cardinality`]):
    /// which.
    Cardinality(Cap),
}

/// A cap an attribute or a tag was dropped past, worded from its constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    TagKey,
    TagValue,
    Tags,
    AttributeKey,
    AttributeValue,
    Attributes,
    TagValuesPerDay,
    TagKeysPerDay,
}

impl std::fmt::Display for Cap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use contract::{
            ATTRIBUTE_KEY_MAX, ATTRIBUTE_VALUE_MAX, TAG_KEY_MAX, TAG_KEYS_PER_DAY, TAG_VALUE_MAX,
            TAG_VALUES_PER_DAY, TAGS_MAX,
        };
        match self {
            Self::TagKey => write!(f, "a tag's key is 1 to {TAG_KEY_MAX} characters"),
            Self::TagValue => write!(f, "a tag's value is up to {TAG_VALUE_MAX} characters"),
            Self::Tags => write!(f, "a span carries up to {TAGS_MAX} tags"),
            Self::AttributeKey => {
                write!(
                    f,
                    "an attribute's key is up to {ATTRIBUTE_KEY_MAX} characters"
                )
            }
            Self::AttributeValue => write!(
                f,
                "an attribute's value is up to {} characters",
                contract::grouped(ATTRIBUTE_VALUE_MAX)
            ),
            Self::Attributes => write!(f, "a span keeps up to {ATTRIBUTES_MAX} attributes"),
            Self::TagValuesPerDay => {
                write!(
                    f,
                    "a project's tag takes up to {TAG_VALUES_PER_DAY} values a UTC day"
                )
            }
            Self::TagKeysPerDay => {
                write!(
                    f,
                    "a project takes up to {TAG_KEYS_PER_DAY} tag keys a UTC day"
                )
            }
        }
    }
}

/// An attribute dropped from a span that was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// The span's id, in hex.
    pub span_id: String,
    pub attribute: String,
    pub reason: DropReason,
}

/// What is read off a resource: the deployment's two columns, and the
/// attributes the inventory keeps.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResourceFields {
    /// `deployment.environment.name`; empty where the resource names none.
    pub environment: String,
    /// `service.version`; empty where the resource names none.
    pub release: String,
    /// The attributes the inventory names, as `attributes` keeps them.
    pub attributes: Vec<(String, String)>,
    /// The inventory rows every attribute it sent falls under, each once.
    pub sent: Vec<&'static str>,
}

/// The deployment a resource names, and the attributes of it the inventory
/// names, as `attributes` keeps them, under the caps a span's own are held
/// to and a budget of their own together
/// ([`contract::RESOURCE_ATTRIBUTES_BYTES_MAX`]). Any other is dropped
/// without a word — an SDK's resource detectors add them, and nothing the
/// sender does to a span lifts that — and so is one past a cap or the budget:
/// the resource's list is copied onto every span of the export, so what it
/// holds is paid for a thousand times over.
#[must_use]
pub fn read_resource(resource: Option<&Resource>) -> ResourceFields {
    let mut fields = ResourceFields::default();
    let mut bytes = 0usize;
    for kv in resource
        .map(|r| r.attributes.as_slice())
        .unwrap_or_default()
    {
        let value = kv.value.as_ref().and_then(|v| v.value.as_ref());
        if let Some(value) = value {
            note(
                &mut fields.sent,
                contract::sent_as(&kv.key, is_scalar(value), true),
            );
        }
        match kv.key.as_str() {
            contract::ENVIRONMENT => {
                fields.environment = resource_text(value, contract::ENVIRONMENT_MAX);
                continue;
            }
            contract::RELEASE => {
                fields.release = resource_text(value, contract::RELEASE_MAX);
                continue;
            }
            _ => {}
        }
        if fields.attributes.len() >= ATTRIBUTES_MAX {
            continue;
        }
        if !contract::is_inventory_resource(&kv.key)
            || kv.key.chars().count() > contract::ATTRIBUTE_KEY_MAX
        {
            continue;
        }
        if let Some(text) = value
            .and_then(as_text)
            .filter(|text| text.chars().count() <= contract::ATTRIBUTE_VALUE_MAX)
        {
            let taken = bytes + kv.key.len() + text.len();
            if taken > contract::RESOURCE_ATTRIBUTES_BYTES_MAX {
                continue;
            }
            bytes = taken;
            fields.attributes.push((kv.key.clone(), text.into_owned()));
        }
    }
    fields
}

/// A deployment field as the resource names it: text under its cap, and
/// empty for none, a value that is no text, or one past the cap.
fn resource_text(value: Option<&Value>, max: usize) -> String {
    value
        .and_then(text)
        .filter(|text| text.chars().count() <= max)
        .map_or_else(String::new, Cow::into_owned)
}

/// The span as a row and the content it keeps, each field by its name, or the
/// refusal; every attribute dropped on the way is pushed to `dropped`.
///
/// # Errors
///
/// The span broke a field's rule: it is refused whole, and nothing of it is
/// kept.
pub fn map(
    span: &Span,
    context: &Context<'_>,
    dropped: &mut Drops,
) -> Result<(SpanRow, Vec<(String, String)>), Refused> {
    let span_id = hex(&span.span_id);
    let refuse = |reason: Refusal| Refused {
        span_id: span_id.clone(),
        reason,
    };
    if span.trace_id.len() != 16 || span.trace_id.iter().all(|b| *b == 0) {
        return Err(refuse(Refusal::Invalid("trace_id")));
    }
    if span.span_id.len() != 8 || span.span_id.iter().all(|b| *b == 0) {
        return Err(refuse(Refusal::Invalid("span_id")));
    }
    if !(span.parent_span_id.is_empty() || span.parent_span_id.len() == 8) {
        return Err(refuse(Refusal::Invalid("parent_span_id")));
    }

    // Room for the resource's attributes and the span's own, grown once.
    let mut attributes =
        Vec::with_capacity(context.resource.len() + span.attributes.len().min(ATTRIBUTES_MAX));
    attributes.extend_from_slice(context.resource);
    let mut fields = Fields {
        attributes,
        keeps_content: context.keep_content,
        ..Fields::default()
    };
    for kv in &span.attributes {
        fields.read(kv, &span_id, dropped).map_err(refuse)?;
    }
    for &row in context.resource_sent {
        note(&mut fields.sent, row);
    }
    fields.read_failure(span, &span_id, dropped);

    // Without it, a span from another exporter: held, its agent empty, until
    // its trace's tree names the agent it sits under ([`crate::foreign`]).
    let agent = fields.agent.take().unwrap_or_default();
    let kind = match fields
        .operation
        .as_deref()
        .and_then(contract::kind_of_operation)
    {
        Some(kind) => kind,
        None if context.scope == contract::SCOPE && span.name == contract::TRACE_START => {
            SpanKind::TraceStart
        }
        None if context.scope == contract::SCOPE => SpanKind::Step,
        None => SpanKind::Span,
    };
    let span_name = || without_nul(&span.name).into_owned();
    let name = match kind {
        SpanKind::Agent => fields
            .agent_name
            .take()
            .or_else(|| fields.workflow_name.take())
            .unwrap_or_else(|| {
                if agent.is_empty() {
                    span_name()
                } else {
                    agent.clone()
                }
            }),
        SpanKind::ToolCall => fields.tool_name.take().unwrap_or_else(span_name),
        SpanKind::Retrieval => fields.source().unwrap_or_else(span_name),
        SpanKind::TraceStart | SpanKind::Step | SpanKind::ModelCall | SpanKind::Span => span_name(),
    };
    if name.is_empty() {
        return Err(refuse(Refusal::Invalid("name")));
    }
    if name.chars().count() > contract::NAME_MAX {
        return Err(refuse(Refusal::OverCap("name")));
    }
    let started_at =
        instant(span.start_time_unix_nano).ok_or_else(|| refuse(Refusal::Invalid("start_time")))?;
    let ended_at =
        instant(span.end_time_unix_nano).ok_or_else(|| refuse(Refusal::Invalid("end_time")))?;
    if ended_at < started_at {
        return Err(refuse(Refusal::Invalid("end_time")));
    }
    let status = if fields.cancelled {
        SpanStatus::Cancelled
    } else if span
        .status
        .as_ref()
        .is_some_and(|s| s.code == i32::from(StatusCode::Error))
    {
        SpanStatus::Error
    } else {
        SpanStatus::Ok
    };
    let user_id = fields
        .user_id
        .as_deref()
        .map_or_else(String::new, context.store_id);
    let session_id = fields
        .session_id
        .as_deref()
        .map_or_else(String::new, context.store_id);

    let mut row = SpanRow {
        project_id: context.project_id.clone(),
        received_at: context.received_at,
        units: 0,
        agent,
        trace_id: hex(&span.trace_id),
        span_id,
        parent_span_id: hex(&span.parent_span_id),
        kind,
        name,
        started_at,
        ended_at,
        status,
        fault: crate::faults::of(
            status,
            kind,
            fields.error_type.as_deref().unwrap_or_default(),
            fields.finish_fault,
        ),
        error_type: fields.error_type.unwrap_or_default(),
        model: fields
            .response_model
            .or(fields.request_model)
            .unwrap_or_default(),
        provider: fields.provider.unwrap_or_default(),
        input_tokens: fields.input_tokens.unwrap_or_default(),
        output_tokens: fields.output_tokens.unwrap_or_default(),
        cache_read_input_tokens: fields.cache_read_input_tokens.unwrap_or_default(),
        cache_write_input_tokens: fields.cache_write_input_tokens.unwrap_or_default(),
        reasoning_output_tokens: fields.reasoning_output_tokens.unwrap_or_default(),
        cost: fields.cost,
        time_to_first_token: fields.time_to_first_token,
        user_id,
        session_id,
        environment: context.environment.to_owned(),
        release: context.release.to_owned(),
        tags: fields.tags,
        sample_rate: None,
        attributes: fields.attributes,
        sent: fields.sent.into_iter().map(str::to_owned).collect(),
    };
    row.units = units_of(&row);
    Ok((row, fields.content))
}

/// What a row counts against its plan: one unit per started KiB of its text
/// as kept, and [`SPAN_FIXED_BYTES`] for its ids, times and counts — the ids
/// counted there alone, never again as text. A trace's opening costs none:
/// the agent's own span is what the trace costs.
#[must_use]
pub fn units_of(row: &SpanRow) -> u32 {
    if row.kind == SpanKind::TraceStart {
        return 0;
    }
    let text = [
        &row.agent,
        &row.name,
        &row.error_type,
        &row.model,
        &row.provider,
        &row.user_id,
        &row.session_id,
        &row.environment,
        &row.release,
    ]
    .into_iter()
    .map(String::len)
    .chain(row.fault.map(|fault| fault.to_string().len()))
    .chain(row.tags.iter().map(|(k, v)| k.len() + v.len()))
    .chain(row.attributes.iter().map(|(k, v)| k.len() + v.len()))
    .fold(0usize, usize::saturating_add);
    let bytes = u32::try_from(text)
        .unwrap_or(u32::MAX)
        .saturating_add(SPAN_FIXED_BYTES);
    bytes.div_ceil(SPAN_UNIT_BYTES).max(1)
}

/// The fields read off a span's attributes, before they are a row.
#[derive(Default)]
struct Fields {
    agent: Option<String>,
    operation: Option<String>,
    agent_name: Option<String>,
    workflow_name: Option<String>,
    tool_name: Option<String>,
    data_source_id: Option<String>,
    memory_store_id: Option<String>,
    error_type: Option<String>,
    finish_fault: Option<telmoni_shared::SpanFault>,
    response_model: Option<String>,
    request_model: Option<String>,
    provider: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    cost: Option<f64>,
    time_to_first_token: Option<f64>,
    user_id: Option<String>,
    session_id: Option<String>,
    cancelled: bool,
    tags: Vec<(String, String)>,
    attributes: Vec<(String, String)>,
    keeps_content: bool,
    content: Vec<(String, String)>,
    sent: Vec<&'static str>,
}

impl Fields {
    /// Read one attribute: into a column, into `tags` or `attributes`, or
    /// onto `dropped`. A field whose value breaks its rule refuses the span.
    fn read(&mut self, kv: &KeyValue, span_id: &str, dropped: &mut Drops) -> Result<(), Refusal> {
        let sent_key = without_nul(&kv.key);
        let key = contract::current_name(&sent_key);
        let Some(value) = kv.value.as_ref().and_then(|v| v.value.as_ref()) else {
            return Ok(());
        };
        note(
            &mut self.sent,
            contract::sent_as(&sent_key, is_scalar(value), false),
        );
        let mut drop_attribute =
            |attribute: &str, reason: DropReason| dropped.push(span_id, attribute, reason);

        if let Some(tag) = key.strip_prefix(contract::TAG_PREFIX) {
            self.read_tag(tag, key, value, &mut drop_attribute);
            return Ok(());
        }
        match key {
            contract::AGENT => {
                self.agent = Some(bounded_text(value, contract::AGENT_MAX, contract::AGENT)?);
            }
            contract::USER_ID => {
                self.user_id = Some(bounded_text(
                    value,
                    contract::USER_ID_MAX,
                    contract::USER_ID,
                )?);
            }
            contract::CANCELLED => {
                self.cancelled = match value {
                    Value::BoolValue(b) => *b,
                    _ => return Err(Refusal::Invalid(contract::CANCELLED)),
                };
            }
            contract::OPERATION => self.operation = text(value).map(Cow::into_owned),
            contract::AGENT_NAME => self.agent_name = text(value).map(Cow::into_owned),
            contract::WORKFLOW_NAME => self.workflow_name = text(value).map(Cow::into_owned),
            contract::TOOL_NAME => self.tool_name = text(value).map(Cow::into_owned),
            contract::ERROR_TYPE => {
                self.error_type = Some(bounded_text(
                    value,
                    contract::ERROR_TYPE_MAX,
                    contract::ERROR_TYPE,
                )?);
            }
            "gen_ai.response.finish_reasons" => {
                self.finish_fault = crate::faults::finish(value);
                self.keep_listed(key, value, &mut drop_attribute);
            }
            contract::RESPONSE_MODEL => {
                self.response_model = Some(bounded_text(
                    value,
                    contract::MODEL_MAX,
                    contract::RESPONSE_MODEL,
                )?);
            }
            contract::REQUEST_MODEL => {
                // Checked all the same; kept only until the response names its model.
                let model = checked_text(value, contract::MODEL_MAX, contract::REQUEST_MODEL)?;
                if self.response_model.is_none() {
                    self.request_model = Some(model.into_owned());
                }
            }
            contract::PROVIDER => {
                self.provider = Some(bounded_text(
                    value,
                    contract::PROVIDER_MAX,
                    contract::PROVIDER,
                )?);
            }
            contract::INPUT_TOKENS => {
                self.input_tokens = Some(count(value, contract::INPUT_TOKENS)?);
            }
            contract::OUTPUT_TOKENS => {
                self.output_tokens = Some(count(value, contract::OUTPUT_TOKENS)?);
            }
            contract::CACHE_READ_INPUT_TOKENS => {
                self.cache_read_input_tokens =
                    Some(count(value, contract::CACHE_READ_INPUT_TOKENS)?);
            }
            contract::CACHE_CREATION_INPUT_TOKENS => {
                self.cache_write_input_tokens =
                    Some(count(value, contract::CACHE_CREATION_INPUT_TOKENS)?);
            }
            contract::REASONING_OUTPUT_TOKENS => {
                self.reasoning_output_tokens =
                    Some(count(value, contract::REASONING_OUTPUT_TOKENS)?);
            }
            contract::COST => {
                let cost = number(value)
                    .filter(|cost| cost.is_finite() && *cost >= 0.0)
                    .ok_or(Refusal::Invalid(contract::COST))?;
                // -0 is 0, which a sum would otherwise print signed.
                self.cost = Some(cost.abs());
            }
            contract::TIME_TO_FIRST_CHUNK => {
                let seconds = number(value)
                    .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                    .ok_or(Refusal::Invalid(contract::TIME_TO_FIRST_CHUNK))?;
                // -0 is 0, as for a cost.
                self.time_to_first_token = Some(seconds.abs());
            }
            contract::SESSION_ID => {
                self.session_id = Some(bounded_text(
                    value,
                    contract::SESSION_ID_MAX,
                    contract::SESSION_ID,
                )?);
            }
            // ⚠ Dropped unless the project keeps content; the answer names
            // the mode, so a sender reads why.
            _ if contract::is_content(key) => self.content_or_drop(key, value, &mut drop_attribute),
            // Kept as the inventory keeps them, and a retrieval's name besides.
            contract::DATA_SOURCE_ID => {
                self.data_source_id = text(value).map(Cow::into_owned);
                self.keep_listed(key, value, &mut drop_attribute);
            }
            contract::MEMORY_STORE_ID => {
                self.memory_store_id = text(value).map(Cow::into_owned);
                self.keep_listed(key, value, &mut drop_attribute);
            }
            // Before the namespace's refusal, which would drop the two of ours it names.
            _ if contract::is_inventory_span(key) => {
                self.keep_listed(key, value, &mut drop_attribute);
            }
            _ if key.starts_with("telmoni.") => drop_attribute(key, DropReason::Unlisted),
            _ => match value {
                Value::IntValue(_) | Value::DoubleValue(_) | Value::BoolValue(_) => {
                    if let Some(rendered) = scalar(value) {
                        self.keep_attribute(key, rendered, &mut drop_attribute);
                    }
                }
                Value::StringValue(_) | Value::ArrayValue(_) | Value::KvlistValue(_)
                    if self.keeps_content =>
                {
                    self.keep_content(key, value, &mut drop_attribute);
                }
                Value::StringValue(_)
                | Value::StringValueStrindex(_)
                | Value::ArrayValue(_)
                | Value::KvlistValue(_)
                | Value::BytesValue(_) => drop_attribute(key, DropReason::Unlisted),
            },
        }
        Ok(())
    }

    fn read_tag(
        &mut self,
        tag: &str,
        key: &str,
        value: &Value,
        drop: &mut impl FnMut(&str, DropReason),
    ) {
        if tag.is_empty() || tag.chars().count() > contract::TAG_KEY_MAX {
            return drop(key, DropReason::OverCap(Cap::TagKey));
        }
        let Some(rendered) = scalar(value) else {
            return drop(key, DropReason::Unlisted);
        };
        if rendered.chars().count() > contract::TAG_VALUE_MAX {
            return drop(key, DropReason::OverCap(Cap::TagValue));
        }
        if self.tags.len() >= contract::TAGS_MAX {
            return drop(key, DropReason::OverCap(Cap::Tags));
        }
        self.tags.push((tag.to_owned(), rendered.into_owned()));
    }

    fn keep_attribute(
        &mut self,
        key: &str,
        rendered: Cow<'_, str>,
        drop: &mut impl FnMut(&str, DropReason),
    ) {
        if key.chars().count() > contract::ATTRIBUTE_KEY_MAX {
            return drop(key, DropReason::OverCap(Cap::AttributeKey));
        }
        if rendered.chars().count() > contract::ATTRIBUTE_VALUE_MAX {
            return drop(key, DropReason::OverCap(Cap::AttributeValue));
        }
        if self.attributes.len() >= ATTRIBUTES_MAX {
            return drop(key, DropReason::OverCap(Cap::Attributes));
        }
        self.attributes
            .push((key.to_owned(), rendered.into_owned()));
    }

    /// An attribute the inventory names: kept under its name in every mode.
    fn keep_listed(&mut self, key: &str, value: &Value, drop: &mut impl FnMut(&str, DropReason)) {
        match as_text(value) {
            Some(rendered) => self.keep_attribute(key, rendered, drop),
            None => drop(key, DropReason::Unlisted),
        }
    }

    /// What a retrieval searched, by the id its operation names first, while
    /// it fits a name: past that, the span's own name names it.
    fn source(&mut self) -> Option<String> {
        let (first, second) = if self.operation.as_deref() == Some(contract::SEARCH_MEMORY) {
            (self.memory_store_id.take(), self.data_source_id.take())
        } else {
            (self.data_source_id.take(), self.memory_store_id.take())
        };
        first
            .or(second)
            .filter(|source| source.chars().count() <= contract::NAME_MAX)
    }

    /// Content, kept whole under its name: free of an attribute's value cap,
    /// an export's size the one that holds it. Bytes are no text it keeps.
    /// A content field kept where the project keeps content, else named as
    /// dropped: the one rule an attribute and an event both meet.
    fn content_or_drop(
        &mut self,
        key: &str,
        value: &Value,
        drop: &mut impl FnMut(&str, DropReason),
    ) {
        if self.keeps_content {
            self.keep_content(key, value, drop);
        } else {
            drop(key, DropReason::Content);
        }
    }

    /// A failure in full, all content: the status message, the first
    /// `exception` event's class, message and stack trace under their own
    /// names, and every event, that one among them, as one JSON array.
    fn read_failure(&mut self, span: &Span, span_id: &str, dropped: &mut Drops) {
        let mut drop =
            |attribute: &str, reason: DropReason| dropped.push(span_id, attribute, reason);
        let message = span
            .status
            .as_ref()
            .map(|s| s.message.as_str())
            .filter(|m| !m.is_empty());
        if let Some(message) = message {
            note(&mut self.sent, contract::STATUS_MESSAGE);
            self.content_or_drop(
                contract::STATUS_MESSAGE,
                &Value::StringValue(message.to_owned()),
                &mut drop,
            );
        }
        if let Some(exception) = span
            .events
            .iter()
            .find(|e| e.name == contract::EXCEPTION_EVENT)
        {
            for key in [
                contract::EXCEPTION_TYPE,
                contract::EXCEPTION_MESSAGE,
                contract::EXCEPTION_STACKTRACE,
            ] {
                let value = exception
                    .attributes
                    .iter()
                    .find(|kv| kv.key == key)
                    .and_then(|kv| kv.value.as_ref()?.value.as_ref());
                if let Some(value) = value {
                    note(&mut self.sent, key);
                    self.content_or_drop(key, value, &mut drop);
                }
            }
        }
        if !span.events.is_empty() {
            note(&mut self.sent, contract::EVENTS);
            let events: serde_json::Value = span
                .events
                .iter()
                .map(|event| {
                    let attributes: serde_json::Map<_, _> = event
                        .attributes
                        .iter()
                        .map(|kv| {
                            let value = kv.value.as_ref().and_then(|v| v.value.as_ref());
                            (
                                without_nul(&kv.key).into_owned(),
                                value.map_or(serde_json::Value::Null, json_of),
                            )
                        })
                        .collect();
                    serde_json::json!({
                        "name": without_nul(&event.name),
                        "time_unix_nano": event.time_unix_nano,
                        "attributes": attributes,
                    })
                })
                .collect();
            self.content_or_drop(
                contract::EVENTS,
                &Value::StringValue(events.to_string()),
                &mut drop,
            );
        }
    }

    fn keep_content(&mut self, key: &str, value: &Value, drop: &mut impl FnMut(&str, DropReason)) {
        if key.chars().count() > contract::ATTRIBUTE_KEY_MAX {
            return drop(key, DropReason::OverCap(Cap::AttributeKey));
        }
        let text = match value {
            Value::ArrayValue(_) | Value::KvlistValue(_) => json_of(value).to_string(),
            _ => match scalar(value) {
                Some(text) => text.into_owned(),
                None => return drop(key, DropReason::Unlisted),
            },
        };
        self.content.push((key.to_owned(), text));
    }
}

/// Any value as JSON, nested arrays and maps among it; what is no JSON value,
/// bytes or a number JSON cannot spell, as null.
fn json_of(value: &Value) -> serde_json::Value {
    let of = |any: &opentelemetry_proto::tonic::common::v1::AnyValue| {
        any.value.as_ref().map_or(serde_json::Value::Null, json_of)
    };
    match value {
        Value::StringValue(s) => serde_json::Value::from(without_nul(s)),
        Value::BoolValue(b) => serde_json::Value::from(*b),
        Value::IntValue(n) => serde_json::Value::from(*n),
        Value::DoubleValue(d) => {
            serde_json::Number::from_f64(*d).map_or(serde_json::Value::Null, Into::into)
        }
        Value::ArrayValue(array) => array.values.iter().map(of).collect(),
        Value::KvlistValue(map) => map
            .values
            .iter()
            .map(|kv| {
                (
                    without_nul(&kv.key).into_owned(),
                    kv.value.as_ref().map_or(serde_json::Value::Null, of),
                )
            })
            .collect::<serde_json::Map<_, _>>()
            .into(),
        Value::StringValueStrindex(_) | Value::BytesValue(_) => serde_json::Value::Null,
    }
}

/// `text` without a NUL: Postgres's `text` and `jsonb` refuse one, and a
/// held span's row is `jsonb`.
pub(crate) fn without_nul(text: &str) -> Cow<'_, str> {
    if text.contains('\0') {
        Cow::Owned(text.replace('\0', ""))
    } else {
        Cow::Borrowed(text)
    }
}

/// A text field: any scalar, as text; empty reads as unset.
fn text(value: &Value) -> Option<Cow<'_, str>> {
    scalar(value).filter(|t| !t.is_empty())
}

/// A text field under its cap, in characters, still borrowed where it can be.
fn checked_text<'v>(
    value: &'v Value,
    max: usize,
    field: &'static str,
) -> Result<Cow<'v, str>, Refusal> {
    let text = text(value).ok_or(Refusal::Invalid(field))?;
    if text.chars().count() > max {
        return Err(Refusal::OverCap(field));
    }
    Ok(text)
}

/// A text field under its cap, in characters, as the row keeps it.
fn bounded_text(value: &Value, max: usize, field: &'static str) -> Result<String, Refusal> {
    checked_text(value, max, field).map(Cow::into_owned)
}

/// A count: an integer, 0 or more.
fn count(value: &Value, field: &'static str) -> Result<u64, Refusal> {
    match value {
        Value::IntValue(n) => u64::try_from(*n).map_err(|_| Refusal::Invalid(field)),
        _ => Err(Refusal::Invalid(field)),
    }
}

/// A number: a double, or an integer small enough to be one exactly.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::DoubleValue(d) => Some(*d),
        Value::IntValue(n) => i32::try_from(*n).ok().map(f64::from),
        _ => None,
    }
}

/// A scalar as text, the shape every column and map value takes. A string
/// given as an index into the profiles signal's string table is read as
/// absent, as OTLP asks of every other signal's receiver.
fn scalar(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::StringValue(s) => Some(without_nul(s)),
        Value::BoolValue(b) => Some(Cow::Owned(b.to_string())),
        Value::IntValue(n) => Some(Cow::Owned(n.to_string())),
        Value::DoubleValue(d) => Some(Cow::Owned(d.to_string())),
        Value::StringValueStrindex(_)
        | Value::ArrayValue(_)
        | Value::KvlistValue(_)
        | Value::BytesValue(_) => None,
    }
}

/// An attribute's value as `attributes` keeps it: a scalar as text, an array
/// of scalars as its JSON; nothing nested, and no bytes.
fn as_text(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::ArrayValue(array) => {
            let items = array
                .values
                .iter()
                .map(|item| match item.value.as_ref()? {
                    Value::StringValue(s) => Some(serde_json::Value::from(without_nul(s))),
                    Value::BoolValue(b) => Some(serde_json::Value::from(*b)),
                    Value::IntValue(n) => Some(serde_json::Value::from(*n)),
                    Value::DoubleValue(d) => Some(serde_json::Value::from(*d)),
                    Value::StringValueStrindex(_)
                    | Value::ArrayValue(_)
                    | Value::KvlistValue(_)
                    | Value::BytesValue(_) => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Cow::Owned(serde_json::Value::Array(items).to_string()))
        }
        scalar_value => scalar(scalar_value),
    }
}

/// `row` added to what a span sent, once.
fn note(sent: &mut Vec<&'static str>, row: &'static str) {
    if !sent.contains(&row) {
        sent.push(row);
    }
}

/// Whether a value is a number or a boolean, which no person writes.
const fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::IntValue(_) | Value::DoubleValue(_) | Value::BoolValue(_)
    )
}

/// Nanoseconds since the epoch as an instant, to the microsecond the table
/// keeps; `None` for zero, which OpenTelemetry never sends for a span that
/// ended.
fn instant(unix_nanos: u64) -> Option<DateTime<Utc>> {
    if unix_nanos == 0 {
        return None;
    }
    let secs = i64::try_from(unix_nanos / 1_000_000_000).ok()?;
    let micros = u32::try_from(unix_nanos % 1_000_000_000 / 1_000).ok()?;
    DateTime::from_timestamp(secs, micros * 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::AnyValue;
    use opentelemetry_proto::tonic::trace::v1::Status;

    /// An exporter reads these in the answer: worded from the caps, as they were.
    #[test]
    fn each_cap_reads_as_the_answer_words_it() {
        assert_eq!(Cap::TagKey.to_string(), "a tag's key is 1 to 64 characters");
        assert_eq!(
            Cap::AttributeValue.to_string(),
            "an attribute's value is up to 1,024 characters"
        );
        assert_eq!(
            Cap::TagValuesPerDay.to_string(),
            "a project's tag takes up to 500 values a UTC day"
        );
    }

    fn kv(key: &str, value: Value) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue { value: Some(value) }),
            ..KeyValue::default()
        }
    }

    fn s(text: &str) -> Value {
        Value::StringValue(text.into())
    }

    /// A model call inside a trace, as our SDK sends one.
    fn call() -> Span {
        Span {
            trace_id: vec![0x4b; 16],
            span_id: vec![0x0f; 8],
            parent_span_id: vec![0xaa; 8],
            name: "chat claude-sonnet-5-5".into(),
            start_time_unix_nano: 1_760_000_000_000_000_000,
            end_time_unix_nano: 1_760_000_001_500_000_000,
            attributes: vec![
                kv(contract::AGENT, s("nightly-report")),
                kv(contract::OPERATION, s("chat")),
                kv(contract::RESPONSE_MODEL, s("claude-sonnet-5-5")),
                kv(contract::PROVIDER, s("anthropic")),
                kv(contract::INPUT_TOKENS, Value::IntValue(1_200)),
                kv(contract::OUTPUT_TOKENS, Value::IntValue(300)),
                kv(contract::CACHE_CREATION_INPUT_TOKENS, Value::IntValue(40)),
                kv(contract::COST, Value::DoubleValue(0.0081)),
                kv(
                    "gen_ai.response.time_to_first_chunk",
                    Value::DoubleValue(0.35),
                ),
                kv(contract::USER_ID, s("acme-42")),
                kv(contract::SESSION_ID, Value::IntValue(7)),
                kv("telmoni.tag.team", s("search")),
                kv("gen_ai.response.id", s("resp_01")),
                kv(
                    "gen_ai.response.finish_reasons",
                    Value::ArrayValue(opentelemetry_proto::tonic::common::v1::ArrayValue {
                        values: vec![AnyValue {
                            value: Some(s("stop")),
                        }],
                    }),
                ),
                kv("gen_ai.request.choice.count", Value::IntValue(2)),
                kv(
                    "gen_ai.input.messages",
                    s("[{\"role\":\"user\",\"content\":\"hi\"}]"),
                ),
                kv("user.email", s("someone@example.com")),
                kv("vendor.prompt", s("the whole prompt")),
            ],
            status: Some(Status {
                message: "boom".into(),
                code: StatusCode::Error as i32,
            }),
            events: vec![opentelemetry_proto::tonic::trace::v1::span::Event {
                name: contract::EXCEPTION_EVENT.into(),
                time_unix_nano: 1_760_000_001_000_000_000,
                attributes: vec![
                    kv(contract::EXCEPTION_TYPE, s("ValueError")),
                    kv(contract::EXCEPTION_MESSAGE, s("boom")),
                    kv(contract::EXCEPTION_STACKTRACE, s("Traceback: line 1")),
                ],
                dropped_attributes_count: 0,
            }],
            ..Span::default()
        }
    }

    fn context<'a>(
        project: &'a ProjectId,
        resource: &'a [(String, String)],
        store: &'a dyn Fn(&str) -> String,
    ) -> Context<'a> {
        Context {
            project_id: project,
            received_at: Utc::now(),
            scope: contract::SCOPE,
            environment: "",
            release: "",
            resource,
            resource_sent: &[],
            store_id: store,
            keep_content: false,
        }
    }

    #[test]
    fn faults_survive_content_modes_and_attribute_caps_without_changing_status() {
        let project = ProjectId::new();
        let hash = |id: &str| id.to_owned();
        for keep_content in [false, true] {
            let context = Context {
                keep_content,
                ..context(&project, &[], &hash)
            };
            let mut span = call();
            span.status = Some(Status::default());
            span.attributes
                .retain(|kv| kv.key != "gen_ai.response.finish_reasons");
            span.attributes.extend(
                (0..ATTRIBUTES_MAX).map(|n| kv(&format!("metric.{n}"), Value::IntValue(1))),
            );
            span.attributes
                .push(kv("gen_ai.response.finish_reasons", s("max_tokens")));
            let (row, _) = map(&span, &context, &mut Drops::default()).expect("kept");
            assert_eq!(row.status, SpanStatus::Ok);
            assert_eq!(row.fault, Some(telmoni_shared::SpanFault::Truncated));
            assert!(
                !row.attributes
                    .iter()
                    .any(|(key, _)| key == "gen_ai.response.finish_reasons")
            );
            span.status = Some(Status {
                code: StatusCode::Error as i32,
                ..Status::default()
            });
            span.attributes
                .push(kv(contract::ERROR_TYPE, s("openai.RateLimitError")));
            let (row, _) = map(&span, &context, &mut Drops::default()).expect("kept");
            assert_eq!(row.status, SpanStatus::Error);
            assert_eq!(row.fault, Some(telmoni_shared::SpanFault::RateLimited));
        }
    }

    #[test]
    fn reasoning_is_numeric_metadata_in_every_content_mode() {
        let project = ProjectId::new();
        let hash = |id: &str| id.to_owned();
        for keep_content in [false, true] {
            let context = Context {
                keep_content,
                ..context(&project, &[], &hash)
            };
            let mut span = call();
            span.attributes
                .push(kv(contract::REASONING_OUTPUT_TOKENS, Value::IntValue(50)));
            let (row, _) = map(&span, &context, &mut Drops::default()).expect("kept");
            assert_eq!(row.reasoning_output_tokens, 50);
            assert_eq!(row.output_tokens, 300);
            let mut held = serde_json::to_value(&row).expect("held JSON");
            held.as_object_mut()
                .expect("row")
                .remove("reasoning_output_tokens");
            assert_eq!(
                serde_json::from_value::<SpanRow>(held)
                    .expect("an older held row")
                    .reasoning_output_tokens,
                0
            );
            for value in [Value::IntValue(-1), Value::DoubleValue(1.5), s("50")] {
                span.attributes.last_mut().expect("reasoning").value =
                    Some(AnyValue { value: Some(value) });
                assert!(map(&span, &context, &mut Drops::default()).is_err());
            }
        }
    }

    /// A span records each inventory row it and its resource arrived with,
    /// once, what it dropped among them.
    #[test]
    fn a_span_records_the_inventory_rows_it_arrived_with() {
        let project = ProjectId::new();
        let resource = read_resource(Some(&Resource {
            attributes: vec![
                kv("service.name", s("reporter")),
                kv("host.name", s("laptop")),
            ],
            ..Resource::default()
        }));
        let hash = |id: &str| format!("h({id})");
        let on = Context {
            resource_sent: &resource.sent,
            ..context(&project, &resource.attributes, &hash)
        };
        let (row, _) = map(&call(), &on, &mut Drops::default()).expect("kept");
        for sent in [
            contract::AGENT,
            contract::CACHE_CREATION_INPUT_TOKENS,
            contract::TAG_FAMILY,
            "gen_ai.input.messages",
            "user.email",
            contract::OTHER_SCALAR,
            contract::OTHER_TEXT,
            contract::STATUS_MESSAGE,
            contract::EXCEPTION_TYPE,
            contract::EXCEPTION_MESSAGE,
            contract::EXCEPTION_STACKTRACE,
            contract::EVENTS,
            "service.name",
        ] {
            assert_eq!(
                row.sent.iter().filter(|row| *row == sent).count(),
                1,
                "{sent}: {:?}",
                row.sent
            );
        }
    }

    #[test]
    fn a_model_call_is_read_into_its_columns_and_its_content_dropped() {
        let project = ProjectId::new();
        let resource = vec![("service.name".to_owned(), "reporter".to_owned())];
        let hash = |id: &str| format!("h({id})");
        let mut dropped = Drops::default();
        let (row, _) =
            map(&call(), &context(&project, &resource, &hash), &mut dropped).expect("kept");

        assert_eq!(row.kind, SpanKind::ModelCall);
        assert_eq!(row.name, "chat claude-sonnet-5-5");
        assert_eq!(row.agent, "nightly-report");
        assert_eq!(row.trace_id, "4b".repeat(16));
        assert_eq!(row.span_id, "0f".repeat(8));
        assert_eq!(row.parent_span_id, "aa".repeat(8));
        assert_eq!(row.model, "claude-sonnet-5-5");
        assert_eq!(row.provider, "anthropic");
        assert_eq!(
            (
                row.input_tokens,
                row.output_tokens,
                row.cache_write_input_tokens
            ),
            (1_200, 300, 40),
            "the conventions' cache creation count lands directly"
        );
        assert_eq!(row.cost, Some(0.0081));
        assert_eq!(row.time_to_first_token, Some(0.35));
        assert_eq!(row.status, SpanStatus::Error);
        assert_eq!(row.user_id, "h(acme-42)", "stored as the project hashes it");
        assert_eq!(
            row.session_id, "h(7)",
            "a scalar reads as text, stored as the project hashes it"
        );
        assert_eq!(row.tags, [("team".to_owned(), "search".to_owned())]);
        assert_eq!(
            row.attributes,
            [
                ("service.name".to_owned(), "reporter".to_owned()),
                ("gen_ai.response.id".to_owned(), "resp_01".to_owned()),
                (
                    "gen_ai.response.finish_reasons".to_owned(),
                    r#"["stop"]"#.to_owned()
                ),
                ("gen_ai.request.choice.count".to_owned(), "2".to_owned()),
            ],
            "the resource's inventory, the span's, and an unlisted number"
        );
        assert_eq!(
            (row.ended_at - row.started_at).num_milliseconds(),
            1_500,
            "times to the microsecond"
        );
        assert_eq!(row.units, 1);

        let reasons: Vec<(&str, &DropReason)> = dropped
            .named()
            .iter()
            .map(|d| (d.attribute.as_str(), &d.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                ("gen_ai.input.messages", &DropReason::Content),
                ("user.email", &DropReason::Content),
                ("vendor.prompt", &DropReason::Unlisted),
                (contract::STATUS_MESSAGE, &DropReason::Content),
                (contract::EXCEPTION_TYPE, &DropReason::Content),
                (contract::EXCEPTION_MESSAGE, &DropReason::Content),
                (contract::EXCEPTION_STACKTRACE, &DropReason::Content),
                (contract::EVENTS, &DropReason::Content),
            ]
        );
        assert!(dropped.named().iter().all(|d| d.span_id == "0f".repeat(8)));
    }

    /// On a project that keeps content, the content fields and the text no
    /// inventory names are kept whole under their names; the columns, the
    /// kept attributes and what is ours are read as in every mode.
    #[test]
    fn a_project_that_keeps_content_keeps_it_whole_and_its_columns_alike() {
        let project = ProjectId::new();
        let hash = |id: &str| format!("h({id})");
        let long = "x".repeat(contract::ATTRIBUTE_VALUE_MAX * 4);
        let nested = Value::KvlistValue(opentelemetry_proto::tonic::common::v1::KeyValueList {
            values: vec![kv(
                "a",
                Value::ArrayValue(opentelemetry_proto::tonic::common::v1::ArrayValue {
                    values: vec![AnyValue {
                        value: Some(Value::IntValue(1)),
                    }],
                }),
            )],
        });
        let mut span = call();
        span.attributes.extend([
            kv("gen_ai.tool.call.result", s(&long)),
            kv("vendor.metadata", nested),
            kv("vendor.blob", Value::BytesValue(vec![1, 2])),
            kv("telmoni.unknown", s("ours, unread")),
        ]);
        let mut off = Drops::default();
        let (_, kept_off) = map(&span, &context(&project, &[], &hash), &mut off).expect("kept");
        assert!(kept_off.is_empty(), "off keeps none: {kept_off:?}");

        let on = Context {
            keep_content: true,
            ..context(&project, &[], &hash)
        };
        let mut dropped = Drops::default();
        let (row, content) = map(&span, &on, &mut dropped).expect("kept");
        let (row_off, _) =
            map(&span, &context(&project, &[], &hash), &mut Drops::default()).expect("kept");
        assert_eq!(
            (row.attributes, row.units),
            (row_off.attributes, row_off.units),
            "content is no attribute, nor counted"
        );
        assert_eq!(
            content,
            [
                (
                    "gen_ai.input.messages".to_owned(),
                    "[{\"role\":\"user\",\"content\":\"hi\"}]".to_owned()
                ),
                ("user.email".to_owned(), "someone@example.com".to_owned()),
                ("vendor.prompt".to_owned(), "the whole prompt".to_owned()),
                ("gen_ai.tool.call.result".to_owned(), long),
                ("vendor.metadata".to_owned(), r#"{"a":[1]}"#.to_owned()),
                (contract::STATUS_MESSAGE.to_owned(), "boom".to_owned()),
                (contract::EXCEPTION_TYPE.to_owned(), "ValueError".to_owned()),
                (contract::EXCEPTION_MESSAGE.to_owned(), "boom".to_owned()),
                (contract::EXCEPTION_STACKTRACE.to_owned(), "Traceback: line 1".to_owned()),
                (
                    contract::EVENTS.to_owned(),
                    r#"[{"attributes":{"exception.message":"boom","exception.stacktrace":"Traceback: line 1","exception.type":"ValueError"},"name":"exception","time_unix_nano":1760000001000000000}]"#
                        .to_owned()
                ),
            ]
        );
        let reasons: Vec<(&str, &DropReason)> = dropped
            .named()
            .iter()
            .map(|d| (d.attribute.as_str(), &d.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                ("vendor.blob", &DropReason::Unlisted),
                ("telmoni.unknown", &DropReason::Unlisted),
            ]
        );
    }

    #[test]
    fn an_agents_span_is_named_by_its_agent() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let mut dropped = Drops::default();
        let agent = Span {
            name: "invoke_agent".into(),
            parent_span_id: Vec::new(),
            attributes: vec![
                kv(contract::AGENT, s("nightly-report")),
                kv(contract::OPERATION, s("invoke_agent")),
                kv(contract::CANCELLED, Value::BoolValue(true)),
            ],
            status: None,
            events: Vec::new(),
            ..call()
        };
        let (row, _) = map(&agent, &context(&project, &[], &keep), &mut dropped).expect("kept");
        assert_eq!(row.kind, SpanKind::Agent);
        assert_eq!(
            row.name, "nightly-report",
            "no gen_ai.agent.name: the agent's"
        );
        assert_eq!(row.parent_span_id, "");
        assert_eq!(row.status, SpanStatus::Cancelled);
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_step_is_ours_under_our_scope_and_a_span_elsewhere() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let plain = Span {
            attributes: vec![kv(contract::AGENT, s("nightly-report"))],
            ..call()
        };
        let ours = context(&project, &[], &keep);
        let (row, _) = map(&plain, &ours, &mut Drops::default()).expect("kept");
        assert_eq!(row.kind, SpanKind::Step);
        let theirs = Context {
            scope: "opentelemetry.instrumentation.httpx",
            ..ours
        };
        let (row, _) = map(&plain, &theirs, &mut Drops::default()).expect("kept");
        assert_eq!(row.kind, SpanKind::Span);
    }

    /// A retrieval is named by what it searched, its operation's own id
    /// first, and keeps both ids; past a name's cap, by its span's name.
    #[test]
    fn a_retrieval_is_named_by_its_source() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let on = context(&project, &[], &keep);
        let span = |operation: &str, name: &str, ids: &[(&str, &str)]| {
            let mut attributes = vec![
                kv(contract::AGENT, s("nightly-report")),
                kv(contract::OPERATION, s(operation)),
            ];
            attributes.extend(ids.iter().map(|(key, id)| kv(key, s(id))));
            Span {
                name: name.into(),
                attributes,
                status: None,
                events: Vec::new(),
                ..call()
            }
        };
        let row = |span: &Span| map(span, &on, &mut Drops::default()).expect("kept").0;
        let both = [
            (contract::DATA_SOURCE_ID, "docs-index"),
            (contract::MEMORY_STORE_ID, "preferences"),
        ];
        for (operation, named) in [
            ("retrieval", "docs-index"),
            (contract::SEARCH_MEMORY, "preferences"),
        ] {
            let kept = row(&span(operation, operation, &both));
            assert_eq!(
                (kept.kind, kept.name.as_str()),
                (SpanKind::Retrieval, named)
            );
            for (key, id) in both {
                let attribute = (key.to_owned(), id.to_owned());
                assert!(kept.attributes.contains(&attribute), "{operation}: {key}");
            }
        }
        let other = row(&span(contract::SEARCH_MEMORY, "search_memory", &both[..1]));
        assert_eq!(
            other.name, "docs-index",
            "the other id, where its own is unset"
        );
        let empty = row(&span(
            "retrieval",
            "retrieval",
            &[(contract::DATA_SOURCE_ID, "")],
        ));
        assert_eq!(empty.name, "retrieval", "an empty id is none");
        // The cap counts characters, never bytes.
        let widest = "é".repeat(contract::NAME_MAX);
        let past = "é".repeat(contract::NAME_MAX + 1);
        let at_cap = row(&span(
            "retrieval",
            "retrieval",
            &[(contract::DATA_SOURCE_ID, &widest)],
        ));
        assert_eq!(at_cap.name, widest);
        let short = row(&span(
            "retrieval",
            "docs",
            &[(contract::DATA_SOURCE_ID, &past)],
        ));
        assert_eq!(short.name, "docs", "past the cap, the span's own name");
        let conventional = span(
            "retrieval",
            &format!("retrieval {past}"),
            &[(contract::DATA_SOURCE_ID, &past)],
        );
        let refused = map(&conventional, &on, &mut Drops::default()).unwrap_err();
        assert_eq!(refused.reason, Refusal::OverCap("name"));
        // On an agent's span, a data source is an attribute alone.
        let agent = row(&span(
            "invoke_agent",
            "invoke_agent",
            &[(contract::DATA_SOURCE_ID, "docs-index")],
        ));
        assert_eq!(
            (agent.kind, agent.name.as_str()),
            (SpanKind::Agent, "nightly-report")
        );
        let attribute = (contract::DATA_SOURCE_ID.to_owned(), "docs-index".to_owned());
        assert!(agent.attributes.contains(&attribute));
        let theirs = Context {
            scope: "opentelemetry.instrumentation.langchain",
            ..context(&project, &[], &keep)
        };
        let plan = span("plan", "plan nightly-report", &[]);
        let (planned, _) = map(&plan, &theirs, &mut Drops::default()).expect("kept");
        assert_eq!(planned.kind, SpanKind::Step, "from any exporter");
    }

    /// A span naming no agent is another exporter's: kept with its agent
    /// empty, its operation naming its kind, for its trace's tree to place.
    #[test]
    fn a_span_naming_no_agent_is_held_with_it_empty() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let foreign = Span {
            attributes: vec![kv(contract::OPERATION, s("chat"))],
            ..call()
        };
        let (row, _) = map(
            &foreign,
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .expect("held");
        assert_eq!(row.agent, "");
        assert_eq!(row.kind, SpanKind::ModelCall);
        assert_eq!(row.span_id, "0f".repeat(8));
    }

    #[test]
    fn a_field_that_breaks_its_rule_refuses_the_span_whole() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let with = |key: &str, value: Value| Span {
            attributes: vec![kv(contract::AGENT, s("nightly-report")), kv(key, value)],
            ..call()
        };
        for (key, value, expected) in [
            (
                contract::INPUT_TOKENS,
                Value::IntValue(-1),
                Refusal::Invalid(contract::INPUT_TOKENS),
            ),
            (
                contract::INPUT_TOKENS,
                s("12"),
                Refusal::Invalid(contract::INPUT_TOKENS),
            ),
            (
                contract::COST,
                Value::DoubleValue(-0.01),
                Refusal::Invalid(contract::COST),
            ),
            (
                contract::COST,
                Value::DoubleValue(f64::NAN),
                Refusal::Invalid(contract::COST),
            ),
            (
                contract::COST,
                Value::DoubleValue(f64::INFINITY),
                Refusal::Invalid(contract::COST),
            ),
            (contract::COST, s("0.01"), Refusal::Invalid(contract::COST)),
            (
                contract::TIME_TO_FIRST_CHUNK,
                Value::DoubleValue(-0.1),
                Refusal::Invalid(contract::TIME_TO_FIRST_CHUNK),
            ),
            (
                contract::TIME_TO_FIRST_CHUNK,
                s("0.3"),
                Refusal::Invalid(contract::TIME_TO_FIRST_CHUNK),
            ),
            (
                contract::CANCELLED,
                s("yes"),
                Refusal::Invalid(contract::CANCELLED),
            ),
            (
                contract::ERROR_TYPE,
                s(&"E".repeat(129)),
                Refusal::OverCap(contract::ERROR_TYPE),
            ),
            (
                contract::AGENT,
                s(&"a".repeat(contract::AGENT_MAX + 1)),
                Refusal::OverCap(contract::AGENT),
            ),
            (
                contract::SESSION_ID,
                s(&"c".repeat(201)),
                Refusal::OverCap(contract::SESSION_ID),
            ),
        ] {
            let refused = map(
                &with(key, value),
                &context(&project, &[], &keep),
                &mut Drops::default(),
            )
            .unwrap_err();
            assert_eq!(refused.reason, expected, "{key}");
        }
        let long_name = Span {
            name: "n".repeat(contract::NAME_MAX + 1),
            ..with(contract::OPERATION, s("chat"))
        };
        let refused = map(
            &long_name,
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .unwrap_err();
        assert_eq!(refused.reason, Refusal::OverCap("name"));
        let widest_model = "m".repeat(contract::MODEL_MAX);
        let named_by_both = Span {
            name: format!("generate_content {widest_model}"),
            attributes: vec![
                kv(contract::AGENT, s("nightly-report")),
                kv(contract::OPERATION, s("generate_content")),
                kv(contract::REQUEST_MODEL, s(&widest_model)),
            ],
            ..call()
        };
        let (row, _) = map(
            &named_by_both,
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .expect("a model at its cap keeps its call");
        assert_eq!((row.kind, row.model), (SpanKind::ModelCall, widest_model));
        let backwards = Span {
            end_time_unix_nano: 1,
            ..with(contract::OPERATION, s("chat"))
        };
        let refused = map(
            &backwards,
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .unwrap_err();
        assert_eq!(refused.reason, Refusal::Invalid("end_time"));
        let no_trace = Span {
            trace_id: vec![0; 16],
            ..with(contract::OPERATION, s("chat"))
        };
        let refused = map(
            &no_trace,
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .unwrap_err();
        assert_eq!(refused.reason, Refusal::Invalid("trace_id"));
    }

    /// A cost is a number of dollars from 0, a whole one among them, kept as reported.
    #[test]
    fn a_cost_is_kept_as_reported() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let costing = |value: Value| Span {
            attributes: vec![
                kv(contract::AGENT, s("nightly-report")),
                kv(contract::OPERATION, s("chat")),
                kv(contract::COST, value),
            ],
            ..call()
        };
        let cost = |value: Value| {
            map(
                &costing(value),
                &context(&project, &[], &keep),
                &mut Drops::default(),
            )
            .expect("kept")
            .0
            .cost
        };
        assert_eq!(cost(Value::DoubleValue(0.25)), Some(0.25));
        assert_eq!(cost(Value::IntValue(3)), Some(3.0));
        assert_eq!(cost(Value::DoubleValue(0.0)), Some(0.0), "free is a cost");
        assert!(
            cost(Value::DoubleValue(-0.0)).is_some_and(f64::is_sign_positive),
            "-0 is kept as 0"
        );
        let (unreported, _) = map(
            &Span {
                attributes: vec![
                    kv(contract::AGENT, s("nightly-report")),
                    kv(contract::OPERATION, s("chat")),
                ],
                ..call()
            },
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .expect("kept");
        assert_eq!(unreported.cost, None, "none reported is none, not 0");
    }

    #[test]
    fn an_attribute_past_its_cap_is_dropped_and_the_span_kept() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let mut attributes = vec![
            kv(contract::AGENT, s("nightly-report")),
            kv("telmoni.tag.long", s(&"v".repeat(257))),
            kv("telmoni.tag.", s("nameless")),
            kv("telmoni.unknown", s("ours?")),
        ];
        for n in 0..(contract::TAGS_MAX + 1) {
            attributes.push(kv(&format!("telmoni.tag.t{n}"), Value::IntValue(1)));
        }
        for n in 0..(ATTRIBUTES_MAX + 1) {
            attributes.push(kv(&format!("x.n{n}"), Value::IntValue(1)));
        }
        let span = Span {
            attributes,
            ..call()
        };
        let mut dropped = Drops::default();
        let (row, _) = map(&span, &context(&project, &[], &keep), &mut dropped).expect("kept");
        assert_eq!(row.tags.len(), contract::TAGS_MAX);
        assert_eq!(row.attributes.len(), ATTRIBUTES_MAX);
        let over_cap = dropped
            .named()
            .iter()
            .filter(|d| matches!(d.reason, DropReason::OverCap(_)))
            .count();
        assert_eq!(over_cap, 4, "{dropped:?}");
        assert!(
            dropped
                .named()
                .iter()
                .any(|d| d.attribute == "telmoni.unknown" && d.reason == DropReason::Unlisted)
        );
    }

    /// A NUL is removed from every name and value a span and its resource carry.
    #[test]
    fn a_nul_is_removed_from_every_text_a_span_carries() {
        let resource = Resource {
            attributes: vec![
                kv(contract::RELEASE, s("v\u{0}2")),
                kv("service.name", s("rep\0orter")),
            ],
            ..Resource::default()
        };
        let read = read_resource(Some(&resource));
        assert_eq!(read.release, "v2");
        assert_eq!(
            read.attributes,
            [("service.name".to_owned(), "reporter".to_owned())]
        );

        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let span = Span {
            name: "chat\0 claude".into(),
            attributes: vec![
                kv(contract::AGENT, s("nightly\0-report")),
                kv(contract::OPERATION, s("chat")),
                kv(contract::RESPONSE_MODEL, s("claude\0-sonnet")),
                kv("telmoni.tag.te\0am", s("sea\0rch")),
                kv("gen_ai.prompt.name", s("tri\0age")),
                kv("x.cou\0nt", Value::IntValue(1)),
            ],
            ..call()
        };
        let (row, _) =
            map(&span, &context(&project, &[], &keep), &mut Drops::default()).expect("kept");
        assert_eq!(row.name, "chat claude");
        assert_eq!(row.agent, "nightly-report");
        assert_eq!(row.model, "claude-sonnet");
        assert_eq!(row.tags, [("team".to_owned(), "search".to_owned())]);
        assert_eq!(
            row.attributes,
            [
                ("gen_ai.prompt.name".to_owned(), "triage".to_owned()),
                ("x.count".to_owned(), "1".to_owned()),
            ]
        );
    }

    #[test]
    fn units_count_each_started_kib_of_text() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let (row, _) = map(
            &call(),
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .expect("kept");
        assert_eq!(units_of(&row), 1);
        let heavy = SpanRow {
            attributes: (0..60)
                .map(|n| (format!("k{n}"), "v".repeat(100)))
                .collect(),
            ..row
        };
        assert_eq!(units_of(&heavy), 7, "6,000-odd bytes of attributes");
        let long_ids = SpanRow {
            trace_id: "f".repeat(2_048),
            span_id: "f".repeat(2_048),
            parent_span_id: "f".repeat(2_048),
            ..heavy.clone()
        };
        assert_eq!(
            units_of(&long_ids),
            units_of(&heavy),
            "the ids are in the fixed bytes, not the text"
        );
    }

    /// The widest span the contract keeps, four bytes a character, fits a trace's page whole.
    #[test]
    fn the_widest_span_fits_a_traces_page() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let (row, _) = map(
            &call(),
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .expect("kept");
        let wide = |chars: usize| "\u{1F600}".repeat(chars);
        let widest = SpanRow {
            agent: wide(contract::AGENT_MAX),
            name: wide(contract::NAME_MAX),
            error_type: wide(contract::ERROR_TYPE_MAX),
            model: wide(contract::MODEL_MAX),
            provider: wide(contract::PROVIDER_MAX),
            user_id: wide(contract::USER_ID_MAX),
            session_id: wide(contract::SESSION_ID_MAX),
            environment: wide(contract::ENVIRONMENT_MAX),
            release: wide(contract::RELEASE_MAX),
            tags: (0..contract::TAGS_MAX)
                .map(|_| (wide(contract::TAG_KEY_MAX), wide(contract::TAG_VALUE_MAX)))
                .collect(),
            attributes: (0..ATTRIBUTES_MAX)
                .map(|_| {
                    (
                        wide(contract::ATTRIBUTE_KEY_MAX),
                        wide(contract::ATTRIBUTE_VALUE_MAX),
                    )
                })
                .collect(),
            ..row
        };
        let units = units_of(&widest);
        assert!(
            units <= crate::traces::TRACE_UNITS_MAX,
            "{units} units, past the page's"
        );
    }

    #[test]
    fn the_resource_keeps_the_inventory_and_nothing_else() {
        let resource = Resource {
            attributes: vec![
                kv("service.name", s("reporter")),
                kv("telemetry.sdk.language", s("python")),
                kv("process.owner", s("someone")),
                kv("host.name", s("laptop")),
            ],
            ..Resource::default()
        };
        let read = read_resource(Some(&resource));
        assert_eq!(
            read.attributes,
            [
                ("service.name".to_owned(), "reporter".to_owned()),
                ("telemetry.sdk.language".to_owned(), "python".to_owned()),
            ]
        );
        assert_eq!((read.environment.as_str(), read.release.as_str()), ("", ""));
        assert_eq!(read_resource(None), ResourceFields::default());
    }

    /// The deployment is two columns on every span of the export, never an
    /// attribute; past its cap, or as no text, it is read as none.
    #[test]
    fn the_resource_names_the_deployment_as_columns() {
        let resource = Resource {
            attributes: vec![
                kv(contract::ENVIRONMENT, s("production")),
                kv(contract::RELEASE, s("2026.10.18")),
                kv("service.name", s("reporter")),
            ],
            ..Resource::default()
        };
        let read = read_resource(Some(&resource));
        assert_eq!(read.environment, "production");
        assert_eq!(read.release, "2026.10.18");
        assert_eq!(
            read.attributes,
            [("service.name".to_owned(), "reporter".to_owned())]
        );

        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let on_every_span = Context {
            environment: &read.environment,
            release: &read.release,
            ..context(&project, &read.attributes, &keep)
        };
        let (row, _) = map(&call(), &on_every_span, &mut Drops::default()).expect("kept");
        assert_eq!(row.environment, "production");
        assert_eq!(row.release, "2026.10.18");
        assert!(
            row.attributes
                .iter()
                .all(|(k, _)| k == "service.name" || k.starts_with("gen_ai."))
        );

        let past = Resource {
            attributes: vec![
                kv(contract::ENVIRONMENT, s(&"e".repeat(129))),
                kv(contract::RELEASE, Value::IntValue(3)),
            ],
            ..Resource::default()
        };
        let read = read_resource(Some(&past));
        assert_eq!(read.environment, "", "past its cap, none");
        assert_eq!(read.release, "3", "a scalar reads as text");
        assert!(read.attributes.is_empty());
    }

    /// Copied onto every span of the export, so held to a span's own caps.
    #[test]
    fn the_resource_is_held_to_a_spans_caps() {
        let mut attributes = vec![
            kv("service.name", s(&"x".repeat(1_025))),
            kv(&format!("telemetry.sdk.{}", "k".repeat(128)), s("long key")),
        ];
        attributes.extend((0..100).map(|n| kv(&format!("telemetry.sdk.extra{n}"), s("v"))));
        let resource = Resource {
            attributes,
            ..Resource::default()
        };
        let kept = read_resource(Some(&resource)).attributes;
        assert_eq!(kept.len(), ATTRIBUTES_MAX);
        assert!(
            kept.iter()
                .all(|(k, v)| k.starts_with("telemetry.sdk.extra") && v == "v"),
            "an over-long value or key is dropped: {:?}",
            kept.first()
        );

        let heavy = Resource {
            attributes: ["a", "b", "c"]
                .iter()
                .map(|n| kv(&format!("telemetry.sdk.{n}"), s(&"x".repeat(990))))
                .chain([kv("service.name", s("reporter"))])
                .collect(),
            ..Resource::default()
        };
        let kept: Vec<String> = read_resource(Some(&heavy))
            .attributes
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            kept,
            ["telemetry.sdk.a", "telemetry.sdk.b", "service.name"],
            "past the resource's budget an attribute is dropped, and a smaller one after it kept"
        );
    }
}
