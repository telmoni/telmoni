//! One OTLP span, read by the contract into a row of `spans`: refused whole
//! where it breaks a field's rule, kept with an attribute dropped where only
//! the attribute does.

use std::cmp::Ordering;
use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::common::v1::{KeyValue, any_value::Value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{Span, status::StatusCode};
use uuid::Uuid;

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
    /// What a `user_id` is stored as: the project's own hash of it, or
    /// itself where the project keeps it plain.
    pub store_user_id: &'a dyn Fn(&str) -> String,
}

/// Why a span was refused whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No `telmoni.run_id` and `telmoni.agent`: a span from an exporter other
    /// than ours, refused until a run can be read out of any trace.
    NotARun,
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
    /// Past a cap: which, in words.
    OverCap(&'static str),
    /// A tag past its project's cap for the day ([`crate::ingest::cardinality`]):
    /// which, in words.
    Cardinality(&'static str),
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
            fields.attributes.push((kv.key.clone(), text));
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
        .unwrap_or_default()
}

/// The span as a row, or the refusal; every attribute dropped on the way is
/// pushed to `dropped`.
///
/// # Errors
///
/// The span broke a field's rule: it is refused whole, and nothing of it is
/// kept.
pub fn map(span: &Span, context: &Context<'_>, dropped: &mut Drops) -> Result<SpanRow, Refused> {
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

    let mut fields = Fields {
        attributes: context.resource.to_vec(),
        ..Fields::default()
    };
    for kv in &span.attributes {
        fields.read(kv, &span_id, dropped).map_err(refuse)?;
    }

    let (Some(run_id), Some(agent)) = (fields.run_id, fields.agent.take()) else {
        return Err(refuse(Refusal::NotARun));
    };
    let kind = match fields
        .operation
        .as_deref()
        .and_then(contract::kind_of_operation)
    {
        Some(kind) => kind,
        None if context.scope == contract::SCOPE => SpanKind::Step,
        None => SpanKind::Span,
    };
    let name = match kind {
        SpanKind::Run => fields
            .agent_name
            .take()
            .or_else(|| fields.workflow_name.take())
            .unwrap_or_else(|| agent.clone()),
        SpanKind::ToolCall => fields.tool_name.take().unwrap_or_else(|| span.name.clone()),
        SpanKind::Step | SpanKind::ModelCall | SpanKind::Span => span.name.clone(),
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
        .map_or_else(String::new, context.store_user_id);

    let mut row = SpanRow {
        project_id: context.project_id.clone(),
        received_at: context.received_at,
        units: 0,
        run_id,
        agent,
        trace_id: hex(&span.trace_id),
        span_id,
        parent_span_id: hex(&span.parent_span_id),
        kind,
        name,
        started_at,
        ended_at,
        status,
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
        cost: fields.cost,
        user_id,
        session_id: fields.session_id.unwrap_or_default(),
        environment: context.environment.to_owned(),
        release: context.release.to_owned(),
        tags: fields.tags,
        sample_rate: fields.sample_rate,
        attributes: fields.attributes,
    };
    row.units = units_of(&row);
    Ok(row)
}

/// What a row counts against its plan: one unit per started KiB of its text
/// as kept, and [`SPAN_FIXED_BYTES`] for its ids, times and counts — the ids
/// counted there alone, never again as text.
#[must_use]
pub fn units_of(row: &SpanRow) -> u32 {
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
    run_id: Option<Uuid>,
    agent: Option<String>,
    operation: Option<String>,
    agent_name: Option<String>,
    workflow_name: Option<String>,
    tool_name: Option<String>,
    error_type: Option<String>,
    response_model: Option<String>,
    request_model: Option<String>,
    provider: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    cost: Option<f64>,
    user_id: Option<String>,
    session_id: Option<String>,
    cancelled: bool,
    sample_rate: Option<f64>,
    tags: Vec<(String, String)>,
    attributes: Vec<(String, String)>,
}

impl Fields {
    /// Read one attribute: into a column, into `tags` or `attributes`, or
    /// onto `dropped`. A field whose value breaks its rule refuses the span.
    fn read(&mut self, kv: &KeyValue, span_id: &str, dropped: &mut Drops) -> Result<(), Refusal> {
        let key = contract::current_name(&kv.key);
        let Some(value) = kv.value.as_ref().and_then(|v| v.value.as_ref()) else {
            return Ok(());
        };
        let mut drop_attribute =
            |attribute: &str, reason: DropReason| dropped.push(span_id, attribute, reason);

        if let Some(tag) = key.strip_prefix(contract::TAG_PREFIX) {
            self.read_tag(tag, key, value, &mut drop_attribute);
            return Ok(());
        }
        match key {
            contract::RUN_ID => {
                self.run_id = Some(
                    text(value)
                        .and_then(|t| Uuid::parse_str(&t).ok())
                        .ok_or(Refusal::Invalid(contract::RUN_ID))?,
                );
            }
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
            contract::SAMPLE_RATE => {
                let rate = number(value).ok_or(Refusal::Invalid(contract::SAMPLE_RATE))?;
                let above_zero = rate.partial_cmp(&0.0) == Some(Ordering::Greater);
                let at_most_one = matches!(
                    rate.partial_cmp(&1.0),
                    Some(Ordering::Less | Ordering::Equal)
                );
                if !(above_zero && at_most_one) {
                    return Err(Refusal::Invalid(contract::SAMPLE_RATE));
                }
                self.sample_rate = Some(rate);
            }
            contract::CANCELLED => {
                self.cancelled = match value {
                    Value::BoolValue(b) => *b,
                    _ => return Err(Refusal::Invalid(contract::CANCELLED)),
                };
            }
            // Ours, for sealed content later: consumed now, read then.
            contract::CONTENT_MARK => {}
            contract::OPERATION => self.operation = text(value),
            contract::AGENT_NAME => self.agent_name = text(value),
            contract::WORKFLOW_NAME => self.workflow_name = text(value),
            contract::TOOL_NAME => self.tool_name = text(value),
            contract::ERROR_TYPE => {
                self.error_type = Some(bounded_text(
                    value,
                    contract::ERROR_TYPE_MAX,
                    contract::ERROR_TYPE,
                )?);
            }
            contract::RESPONSE_MODEL => {
                self.response_model = Some(bounded_text(
                    value,
                    contract::MODEL_MAX,
                    contract::RESPONSE_MODEL,
                )?);
            }
            contract::REQUEST_MODEL => {
                self.request_model = Some(bounded_text(
                    value,
                    contract::MODEL_MAX,
                    contract::REQUEST_MODEL,
                )?);
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
            contract::CACHE_WRITE_INPUT_TOKENS => {
                self.cache_write_input_tokens =
                    Some(count(value, contract::CACHE_WRITE_INPUT_TOKENS)?);
            }
            contract::COST => {
                let cost = number(value)
                    .filter(|cost| cost.is_finite() && *cost >= 0.0)
                    .ok_or(Refusal::Invalid(contract::COST))?;
                // -0 is 0, which a sum would otherwise print signed.
                self.cost = Some(cost.abs());
            }
            contract::SESSION_ID => {
                self.session_id = Some(bounded_text(
                    value,
                    contract::SESSION_ID_MAX,
                    contract::SESSION_ID,
                )?);
            }
            // ⚠ Dropped in every mode: no table keeps content yet, and the
            // switch offers no mode but `off` until one does. The answer
            // names the mode, so a sender reads why.
            _ if contract::is_content(key) => drop_attribute(key, DropReason::Content),
            _ if key.starts_with("telmoni.") => drop_attribute(key, DropReason::Unlisted),
            _ if contract::is_inventory_span(key) => match as_text(value) {
                Some(rendered) => self.keep_attribute(key, rendered, &mut drop_attribute),
                None => drop_attribute(key, DropReason::Unlisted),
            },
            _ => match value {
                Value::IntValue(_) | Value::DoubleValue(_) | Value::BoolValue(_) => {
                    if let Some(rendered) = scalar(value) {
                        self.keep_attribute(key, rendered, &mut drop_attribute);
                    }
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
            return drop(
                key,
                DropReason::OverCap("a tag's key is 1 to 64 characters"),
            );
        }
        let Some(rendered) = scalar(value) else {
            return drop(key, DropReason::Unlisted);
        };
        if rendered.chars().count() > contract::TAG_VALUE_MAX {
            return drop(
                key,
                DropReason::OverCap("a tag's value is up to 256 characters"),
            );
        }
        if self.tags.len() >= contract::TAGS_MAX {
            return drop(key, DropReason::OverCap("a span carries up to 32 tags"));
        }
        self.tags.push((tag.to_owned(), rendered));
    }

    fn keep_attribute(
        &mut self,
        key: &str,
        rendered: String,
        drop: &mut impl FnMut(&str, DropReason),
    ) {
        if key.chars().count() > contract::ATTRIBUTE_KEY_MAX {
            return drop(
                key,
                DropReason::OverCap("an attribute's key is up to 128 characters"),
            );
        }
        if rendered.chars().count() > contract::ATTRIBUTE_VALUE_MAX {
            return drop(
                key,
                DropReason::OverCap("an attribute's value is up to 1,024 characters"),
            );
        }
        if self.attributes.len() >= ATTRIBUTES_MAX {
            return drop(key, DropReason::OverCap("a span keeps up to 64 attributes"));
        }
        self.attributes.push((key.to_owned(), rendered));
    }
}

/// A text field: any scalar, as text; empty reads as unset.
fn text(value: &Value) -> Option<String> {
    scalar(value).filter(|t| !t.is_empty())
}

/// A text field under its cap, in characters.
fn bounded_text(value: &Value, max: usize, field: &'static str) -> Result<String, Refusal> {
    let text = text(value).ok_or(Refusal::Invalid(field))?;
    if text.chars().count() > max {
        return Err(Refusal::OverCap(field));
    }
    Ok(text)
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
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::StringValue(s) => Some(s.clone()),
        Value::BoolValue(b) => Some(b.to_string()),
        Value::IntValue(n) => Some(n.to_string()),
        Value::DoubleValue(d) => Some(d.to_string()),
        Value::StringValueStrindex(_)
        | Value::ArrayValue(_)
        | Value::KvlistValue(_)
        | Value::BytesValue(_) => None,
    }
}

/// An attribute's value as `attributes` keeps it: a scalar as text, an array
/// of scalars as its JSON; nothing nested, and no bytes.
fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::ArrayValue(array) => {
            let items = array
                .values
                .iter()
                .map(|item| match item.value.as_ref()? {
                    Value::StringValue(s) => Some(serde_json::Value::from(s.as_str())),
                    Value::BoolValue(b) => Some(serde_json::Value::from(*b)),
                    Value::IntValue(n) => Some(serde_json::Value::from(*n)),
                    Value::DoubleValue(d) => Some(serde_json::Value::from(*d)),
                    Value::StringValueStrindex(_)
                    | Value::ArrayValue(_)
                    | Value::KvlistValue(_)
                    | Value::BytesValue(_) => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(serde_json::Value::Array(items).to_string())
        }
        scalar_value => scalar(scalar_value),
    }
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

/// Bytes as lowercase hex: how the table spells an id.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::AnyValue;
    use opentelemetry_proto::tonic::trace::v1::Status;

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

    const RUN: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01";

    /// A model call inside a run, as our SDK sends one.
    fn call() -> Span {
        Span {
            trace_id: vec![0x4b; 16],
            span_id: vec![0x0f; 8],
            parent_span_id: vec![0xaa; 8],
            name: "chat claude-sonnet-5-5".into(),
            start_time_unix_nano: 1_760_000_000_000_000_000,
            end_time_unix_nano: 1_760_000_001_500_000_000,
            attributes: vec![
                kv(contract::RUN_ID, s(RUN)),
                kv(contract::AGENT, s("nightly-report")),
                kv(contract::OPERATION, s("chat")),
                kv(contract::RESPONSE_MODEL, s("claude-sonnet-5-5")),
                kv(contract::PROVIDER, s("anthropic")),
                kv(contract::INPUT_TOKENS, Value::IntValue(1_200)),
                kv(contract::OUTPUT_TOKENS, Value::IntValue(300)),
                kv(
                    "gen_ai.usage.cache_creation.input_tokens",
                    Value::IntValue(40),
                ),
                kv(contract::COST, Value::DoubleValue(0.0081)),
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
                kv("gen_ai.request.temperature", Value::DoubleValue(0.5)),
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
            store_user_id: store,
        }
    }

    #[test]
    fn a_model_call_is_read_into_its_columns_and_its_content_dropped() {
        let project = ProjectId::new();
        let resource = vec![("service.name".to_owned(), "reporter".to_owned())];
        let hash = |id: &str| format!("h({id})");
        let mut dropped = Drops::default();
        let row = map(&call(), &context(&project, &resource, &hash), &mut dropped).expect("kept");

        assert_eq!(row.kind, SpanKind::ModelCall);
        assert_eq!(row.name, "chat claude-sonnet-5-5");
        assert_eq!(row.run_id.to_string(), RUN);
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
            "the replaced cache name reads as the current one"
        );
        assert_eq!(row.cost, Some(0.0081));
        assert_eq!(row.status, SpanStatus::Error);
        assert_eq!(row.user_id, "h(acme-42)", "stored as the project hashes it");
        assert_eq!(row.session_id, "7", "a scalar reads as text");
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
                ("gen_ai.request.temperature".to_owned(), "0.5".to_owned()),
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
            ]
        );
        assert!(dropped.named().iter().all(|d| d.span_id == "0f".repeat(8)));
    }

    #[test]
    fn a_run_is_named_by_its_agent_and_carries_its_sample_rate() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let mut dropped = Drops::default();
        let run = Span {
            name: "invoke_agent".into(),
            parent_span_id: Vec::new(),
            attributes: vec![
                kv(contract::RUN_ID, s(RUN)),
                kv(contract::AGENT, s("nightly-report")),
                kv(contract::OPERATION, s("invoke_agent")),
                kv(contract::SAMPLE_RATE, Value::DoubleValue(0.25)),
                kv(contract::CANCELLED, Value::BoolValue(true)),
            ],
            status: None,
            ..call()
        };
        let row = map(&run, &context(&project, &[], &keep), &mut dropped).expect("kept");
        assert_eq!(row.kind, SpanKind::Run);
        assert_eq!(
            row.name, "nightly-report",
            "no gen_ai.agent.name: the agent's"
        );
        assert_eq!(row.parent_span_id, "");
        assert_eq!(row.status, SpanStatus::Cancelled);
        assert_eq!(row.sample_rate, Some(0.25));
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_step_is_ours_under_our_scope_and_a_span_elsewhere() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let plain = Span {
            attributes: vec![
                kv(contract::RUN_ID, s(RUN)),
                kv(contract::AGENT, s("nightly-report")),
            ],
            ..call()
        };
        let ours = context(&project, &[], &keep);
        let row = map(&plain, &ours, &mut Drops::default()).expect("kept");
        assert_eq!(row.kind, SpanKind::Step);
        let theirs = Context {
            scope: "opentelemetry.instrumentation.httpx",
            ..ours
        };
        let row = map(&plain, &theirs, &mut Drops::default()).expect("kept");
        assert_eq!(row.kind, SpanKind::Span);
    }

    /// ⚠ For now a span with no run is another exporter's: refused, the
    /// rest of its batch kept.
    #[test]
    fn a_span_outside_a_run_is_refused() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let foreign = Span {
            attributes: vec![kv(contract::OPERATION, s("chat"))],
            ..call()
        };
        let refused = map(
            &foreign,
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .unwrap_err();
        assert_eq!(refused.reason, Refusal::NotARun);
        assert_eq!(refused.span_id, "0f".repeat(8));
    }

    #[test]
    fn a_field_that_breaks_its_rule_refuses_the_span_whole() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let with = |key: &str, value: Value| Span {
            attributes: vec![
                kv(contract::RUN_ID, s(RUN)),
                kv(contract::AGENT, s("nightly-report")),
                kv(key, value),
            ],
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
                contract::SAMPLE_RATE,
                Value::DoubleValue(1.5),
                Refusal::Invalid(contract::SAMPLE_RATE),
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
        let bad_run = with(contract::RUN_ID, s("not-a-uuid"));
        let refused = map(
            &bad_run,
            &context(&project, &[], &keep),
            &mut Drops::default(),
        )
        .unwrap_err();
        assert_eq!(refused.reason, Refusal::Invalid(contract::RUN_ID));
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
                kv(contract::RUN_ID, s(RUN)),
                kv(contract::AGENT, s("nightly-report")),
                kv(contract::OPERATION, s("generate_content")),
                kv(contract::REQUEST_MODEL, s(&widest_model)),
            ],
            ..call()
        };
        let row = map(
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
                kv(contract::RUN_ID, s(RUN)),
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
            .cost
        };
        assert_eq!(cost(Value::DoubleValue(0.25)), Some(0.25));
        assert_eq!(cost(Value::IntValue(3)), Some(3.0));
        assert_eq!(cost(Value::DoubleValue(0.0)), Some(0.0), "free is a cost");
        assert!(
            cost(Value::DoubleValue(-0.0)).is_some_and(f64::is_sign_positive),
            "-0 is kept as 0"
        );
        let unreported = map(
            &Span {
                attributes: vec![
                    kv(contract::RUN_ID, s(RUN)),
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
            kv(contract::RUN_ID, s(RUN)),
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
        let row = map(&span, &context(&project, &[], &keep), &mut dropped).expect("kept");
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

    #[test]
    fn units_count_each_started_kib_of_text() {
        let project = ProjectId::new();
        let keep = |id: &str| id.to_owned();
        let row = map(
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
        let row = map(
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
        let row = map(&call(), &on_every_span, &mut Drops::default()).expect("kept");
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
