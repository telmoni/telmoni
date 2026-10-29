//! The exporter: OTLP's JSON over the transport, content off unless asked and allowed,
//! each attempt stamped with this clock, each answer's content mode and drops read.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::trace::{SpanId, SpanKind, Status};
use opentelemetry::{Array, Key, Value};
use opentelemetry_sdk::resource::{ResourceDetector as _, TelemetryResourceDetector};
use opentelemetry_sdk::trace::SpanData;
use serde::Serialize;
use serde_json::json;

use crate::config::Config;
use crate::contract;
use crate::transport::{Answer, Transport, refusal};
use crate::warnings::Warner;
use crate::{VERSION, lock};

/// How many times one batch is sent before it is dropped.
const ATTEMPTS: u32 = 3;
/// The longest a `Retry-After` is honoured for.
const RETRY_AFTER_MAX_SECS: u64 = 60;
/// Where a dropped content field's warning sends the reader.
const CONTENT_DOCS: &str = "https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door";

pub(crate) struct Exporter {
    url: String,
    /// Why nothing is sent to the endpoint ([`refusal`]), which the first
    /// export says.
    refusal: Option<String>,
    bearer: Option<String>,
    capture_content: bool,
    transport: Box<dyn Transport>,
    sleep: Box<dyn Fn(Duration) + Send + Sync>,
    warner: Arc<Warner>,
    content_mode: Mutex<Option<String>>,
    /// The resource's attributes, encoded once: the exporter's own list,
    /// whatever the provider's resource holds.
    resource: Vec<serde_json::Value>,
    /// Set by a shutdown past its deadline: no attempt follows, nor a wait.
    given_up: AtomicBool,
}

impl Exporter {
    pub(crate) fn new(
        config: &Config,
        transport: Box<dyn Transport>,
        sleep: Box<dyn Fn(Duration) + Send + Sync>,
        warner: Arc<Warner>,
    ) -> Self {
        let resource = resource(config, &warner);
        Self {
            url: format!("{}/otlp/v1/traces", config.endpoint),
            refusal: refusal(&config.endpoint),
            bearer: config.api_key.clone(),
            capture_content: config.capture_content,
            transport,
            sleep,
            warner,
            content_mode: Mutex::new(None),
            resource,
            given_up: AtomicBool::new(false),
        }
    }

    /// The project's mode, as the last answer named it; `None` before one.
    pub(crate) fn content_mode(&self) -> Option<String> {
        lock(&self.content_mode).clone()
    }

    /// Gives up what is left: no attempt follows the one in flight, which a
    /// wait already under way still ends on its own.
    pub(crate) fn give_up(&self) {
        self.given_up.store(true, Ordering::Release);
    }

    fn gave_up(&self) -> bool {
        self.given_up.load(Ordering::Acquire)
    }

    /// Sends `spans`, halved past ingest's caps; false once they failed for a
    /// reason not their own, which the rest of a batch they are part of would meet.
    pub(crate) fn export(&self, spans: &[SpanData]) -> bool {
        if spans.is_empty() {
            return true;
        }
        if let Some(refusal) = &self.refusal {
            self.warner.once(
                "endpoint".to_owned(),
                &format!("telmoni: {refusal}; nothing is sent"),
            );
            return false;
        }
        // Ingest refuses an export past 1,000 spans whole, as one past 1 MiB below.
        if spans.len() > contract::BATCH_SPANS_MAX {
            return self.halve(spans);
        }
        let Ok(mut body) = serde_json::to_vec(&self.encode(spans, false)) else {
            return true;
        };
        if body.len() > contract::BATCH_MAX_BYTES {
            // Ingest refuses it whole, its 413 maybe breaking the upload still
            // going out, which reads as no answer and is sent again: halved.
            if spans.len() > 1 {
                return self.halve(spans);
            }
            // A span alone past it goes without what ingest drops for its length,
            // and then fits, unless its own name does not.
            match serde_json::to_vec(&self.encode(spans, true)) {
                Ok(trimmed) if trimmed.len() <= contract::BATCH_MAX_BYTES => {
                    self.warner.once(
                        "trimmed".to_owned(),
                        "telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length",
                    );
                    body = trimmed;
                }
                _ => {
                    self.warner.once(
                        "too-large".to_owned(),
                        "telmoni: a span past the 1 MiB an export may hold was dropped",
                    );
                    return true;
                }
            }
        }
        let mut headers = vec![
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("User-Agent".to_owned(), format!("telmoni-rust/{VERSION}")),
        ];
        if let Some(bearer) = &self.bearer {
            headers.push(("Authorization".to_owned(), format!("Bearer {bearer}")));
        }
        let mut failure = String::new();
        for attempt in 0..ATTEMPTS {
            if self.gave_up() {
                return false;
            }
            // This clock as the attempt leaves, the spans' own.
            let mut sent = headers.clone();
            sent.push((
                contract::SENT_AT_HEADER.to_owned(),
                nanos(std::time::SystemTime::now()),
            ));
            let answer = match self.transport.send(&self.url, &sent, &body) {
                Ok(answer) => answer,
                Err(error) => {
                    failure = error.name;
                    self.wait(None, attempt);
                    continue;
                }
            };
            if (200..300).contains(&answer.status) {
                self.read(&answer);
                return true;
            }
            if answer.status == 429 || answer.status == 503 {
                failure = format!("{} {}", answer.status, problem_type(&answer.body));
                self.wait(answer.header("Retry-After"), attempt);
                continue;
            }
            self.warner.once(
                format!("status:{}", answer.status),
                &format!(
                    "telmoni: export refused with {} {}; the batch was dropped",
                    answer.status,
                    problem_type(&answer.body)
                ),
            );
            // Refused for what it holds, the rest of a batch may still go.
            return answer.status == 400 || answer.status == 413;
        }
        self.warner.once(
            format!("failed:{failure}"),
            &format!(
                "telmoni: export failed after {ATTEMPTS} attempts ({failure}); the batch was dropped"
            ),
        );
        false
    }

    /// Exports each half of `spans`, the second only where the first went.
    fn halve(&self, spans: &[SpanData]) -> bool {
        let (head, tail) = spans.split_at(spans.len() / 2);
        self.export(head) && self.export(tail)
    }

    /// OTLP's `ExportTraceServiceRequest`, in its JSON encoding; `trimmed`, without
    /// the attributes ingest drops for their length.
    fn encode<'a>(&'a self, spans: &'a [SpanData], trimmed: bool) -> Request<'a> {
        // Only an export's answer moves the mode, on this thread: once a batch.
        let keep_content = self.capture_content && self.content_mode().as_deref() == Some("on");
        Request {
            resource_spans: [ResourceSpans {
                resource: Resource {
                    attributes: &self.resource,
                },
                scope_spans: [ScopeSpans {
                    scope: Scope {
                        name: contract::SCOPE,
                        version: VERSION,
                    },
                    spans: spans
                        .iter()
                        .map(|span| self.span(span, trimmed, keep_content))
                        .collect(),
                }],
            }],
        }
    }

    fn span<'a>(&self, span: &'a SpanData, trimmed: bool, keep_content: bool) -> OtlpSpan<'a> {
        // The last value set under a key is the one that stands, as a map
        // would have it: OpenTelemetry's span keeps every set in a list.
        let mut attributes: Vec<(&str, &Value)> = Vec::new();
        for attribute in &span.attributes {
            let key = attribute.key.as_str();
            if !keep_content && contract::is_content(key) {
                continue;
            }
            match attributes.iter_mut().find(|(seen, _)| *seen == key) {
                Some(slot) => slot.1 = &attribute.value,
                None => attributes.push((key, &attribute.value)),
            }
        }
        let attributes = attributes
            .into_iter()
            .filter(|(key, value)| !(trimmed && past_caps(key, value)))
            .map(|(key, value)| OtlpAttribute {
                key,
                value: any_value(value),
            })
            .collect();
        OtlpSpan {
            trace_id: span.span_context.trace_id().to_string(),
            span_id: span.span_context.span_id().to_string(),
            parent_span_id: (span.parent_span_id != SpanId::INVALID)
                .then(|| span.parent_span_id.to_string()),
            name: &span.name,
            kind: kind(&span.span_kind),
            start_time_unix_nano: nanos(span.start_time),
            end_time_unix_nano: nanos(span.end_time),
            attributes,
            // Events and the status message are where an exception writes its
            // message and stack trace: never sent.
            status: matches!(span.status, Status::Error { .. }).then_some(ErrorStatus { code: 2 }),
        }
    }

    fn read(&self, answer: &Answer) {
        if let Some(mode) = answer
            .header(contract::CONTENT_MODE_HEADER)
            .map(str::trim)
            .filter(|mode| !mode.is_empty())
        {
            *lock(&self.content_mode) = Some(mode.to_owned());
        }
        let body: &[u8] = if answer.body.is_empty() {
            b"{}"
        } else {
            &answer.body
        };
        let Ok(decoded) = serde_json::from_slice::<serde_json::Value>(body) else {
            return;
        };
        let Some(message) = decoded
            .get("partialSuccess")
            .and_then(|partial| partial.get("errorMessage"))
            .and_then(serde_json::Value::as_str)
            .filter(|message| !message.is_empty())
        else {
            return;
        };
        self.note(message);
    }

    fn note(&self, message: &str) {
        for line in message.lines() {
            if let Some((attribute, reason, rest)) = dropped(line) {
                let mut text = format!("telmoni: the server dropped {attribute} ({reason}");
                if let Some(rest) = rest.filter(|rest| !rest.is_empty()) {
                    text.push_str(&format!("; {rest}"));
                }
                text.push_str(&format!("); {}", remedy(attribute, reason)));
                self.warner
                    .once(format!("dropped:{attribute}:{reason}"), &text);
                continue;
            }
            if let Some(why) = refused(line) {
                self.warner.once(
                    format!("refused:{why}"),
                    &format!("telmoni: the server refused a span: {why}"),
                );
                continue;
            }
            if line.starts_with("and ") || line.is_empty() {
                continue;
            }
            self.warner.once(
                format!("line:{line}"),
                &format!("telmoni: the server said: {line}"),
            );
        }
    }

    /// Before the next attempt; nothing after the last, or once given up.
    fn wait(&self, retry_after: Option<&str>, attempt: u32) {
        if attempt + 1 >= ATTEMPTS || self.gave_up() {
            return;
        }
        // The header's whole seconds when it has them, else 1, 2, 4… seconds.
        let seconds = retry_after
            .and_then(retry_seconds)
            .unwrap_or(1u64 << attempt);
        (self.sleep)(Duration::from_secs(seconds.min(RETRY_AFTER_MAX_SECS)));
    }
}

/// `span <id> dropped <attribute>: <reason>; <rest>`, the rest optional:
/// the attribute, the reason and the rest.
fn dropped(line: &str) -> Option<(&str, &str, Option<&str>)> {
    let (span_id, rest) = line.strip_prefix("span ")?.split_once(' ')?;
    if span_id.is_empty() {
        return None;
    }
    let (attribute, rest) = rest.strip_prefix("dropped ")?.split_once(": ")?;
    if attribute.is_empty() || attribute.contains(char::is_whitespace) {
        return None;
    }
    let (reason, rest) = match rest.split_once("; ") {
        Some((reason, rest)) => (reason, Some(rest)),
        None => (rest, None),
    };
    let word = |c: char| c.is_alphanumeric() || c == '_';
    if reason.is_empty() || !reason.chars().all(word) {
        return None;
    }
    Some((attribute, reason, rest))
}

/// `span <id> refused: <why>`: the why.
fn refused(line: &str) -> Option<&str> {
    let (span_id, rest) = line.strip_prefix("span ")?.split_once(' ')?;
    if span_id.is_empty() {
        return None;
    }
    rest.strip_prefix("refused: ")
}

fn remedy(attribute: &str, reason: &str) -> String {
    match reason {
        // The page, not a setting to change: which modes keep content is the
        // platform's to say, and no project can choose one that does yet.
        "content" => {
            format!("the project's content mode decides whether content is kept: {CONTENT_DOCS}")
        }
        "unlisted" => format!(
            "send it as the tag {}{attribute} to keep it",
            contract::TAG_PREFIX
        ),
        "over_cap" => "shorten it to keep it".to_owned(),
        "cardinality" => {
            "keep tags to a few keys of a few values each: an id goes as the user or the session"
                .to_owned()
        }
        _ => "see the server's answer".to_owned(),
    }
}

/// The problem type a refusal's body opens with, else nothing.
fn problem_type(body: &[u8]) -> String {
    let body: &[u8] = if body.is_empty() { b"{}" } else { body };
    let Ok(decoded) = serde_json::from_slice::<serde_json::Value>(body) else {
        return String::new();
    };
    let Some(message) = decoded.get("message").and_then(serde_json::Value::as_str) else {
        return String::new();
    };
    if !message.starts_with("/errors/") {
        return String::new();
    }
    message
        .split_once(':')
        .map_or(message, |(problem, _)| problem)
        .to_owned()
}

/// `Retry-After`'s whole seconds, read as text: a number of any length reaches
/// the cap without converting, and one below zero is none.
fn retry_seconds(header: &str) -> Option<u64> {
    let text = header.trim_matches([' ', '\t']);
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let whole = digits.trim_start_matches('0');
    Some(match (negative, whole.len()) {
        (true, _) => 0,
        (false, 0..=2) => whole.parse().unwrap_or(0),
        (false, _) => RETRY_AFTER_MAX_SECS,
    })
}

/// The exporter's five resource attributes, and the deployment's two where
/// the configuration names one: never from `OTEL_RESOURCE_ATTRIBUTES`, which
/// the application's exporters read.
fn resource(config: &Config, warner: &Warner) -> Vec<serde_json::Value> {
    let mut attributes = vec![
        (contract::DISTRO_NAME, contract::DISTRO.to_owned()),
        ("telemetry.distro.version", VERSION.to_owned()),
        ("telemetry.sdk.language", "rust".to_owned()),
        ("telemetry.sdk.name", "opentelemetry".to_owned()),
        ("telemetry.sdk.version", sdk_version()),
    ];
    for (key, value, most, what) in [
        (
            contract::ENVIRONMENT,
            &config.environment,
            contract::ENVIRONMENT_MAX,
            "environment",
        ),
        (
            contract::RELEASE,
            &config.release,
            contract::RELEASE_MAX,
            "release",
        ),
    ] {
        let Some(value) = value else { continue };
        if value.chars().nth(most).is_some() {
            warner.once(
                format!("long:{key}"),
                &format!("telmoni: the {what} must be up to {most} characters; it is not sent"),
            );
            continue;
        }
        attributes.push((key, value.clone()));
    }
    attributes
        .into_iter()
        .map(|(key, value)| {
            let value = Value::from(value);
            serde_json::to_value(OtlpAttribute {
                key,
                value: any_value(&value),
            })
            .unwrap_or_default()
        })
        .collect()
}

/// OTLP's `ExportTraceServiceRequest`, written straight to bytes: one
/// resource, one scope, the batch's spans.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    resource_spans: [ResourceSpans<'a>; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceSpans<'a> {
    resource: Resource<'a>,
    scope_spans: [ScopeSpans<'a>; 1],
}

#[derive(Serialize)]
struct Resource<'a> {
    attributes: &'a [serde_json::Value],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScopeSpans<'a> {
    scope: Scope,
    spans: Vec<OtlpSpan<'a>>,
}

#[derive(Serialize)]
struct Scope {
    name: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OtlpSpan<'a> {
    trace_id: String,
    span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_span_id: Option<String>,
    name: &'a str,
    kind: u8,
    start_time_unix_nano: String,
    end_time_unix_nano: String,
    attributes: Vec<OtlpAttribute<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<ErrorStatus>,
}

#[derive(Serialize)]
struct ErrorStatus {
    code: u8,
}

/// One OTLP `KeyValue`.
#[derive(Serialize)]
struct OtlpAttribute<'a> {
    key: &'a str,
    value: AnyValue<'a>,
}

/// One OTLP `AnyValue`: 64-bit integers travel as decimal strings, and a
/// float that is not a number as `null`, as `serde_json` writes one.
#[derive(Serialize)]
enum AnyValue<'a> {
    #[serde(rename = "boolValue")]
    Bool(bool),
    #[serde(rename = "intValue")]
    Int(String),
    #[serde(rename = "doubleValue")]
    Double(f64),
    #[serde(rename = "stringValue")]
    String(Cow<'a, str>),
    #[serde(rename = "arrayValue")]
    Array { values: Vec<Self> },
}

/// The OpenTelemetry SDK's version, as its own detector reports it.
fn sdk_version() -> String {
    TelemetryResourceDetector
        .detect()
        .get(&Key::new("telemetry.sdk.version"))
        .map_or_else(|| "unknown".to_owned(), |value| value.as_str().into_owned())
}

/// OTLP numbers the kinds from 1.
fn kind(kind: &SpanKind) -> u8 {
    match kind {
        SpanKind::Internal => 1,
        SpanKind::Server => 2,
        SpanKind::Client => 3,
        SpanKind::Producer => 4,
        SpanKind::Consumer => 5,
    }
}

/// Nanoseconds since the epoch as ingest reads a time: none at or before it,
/// or past 64 bits.
pub(crate) fn unix_nanos(time: SystemTime) -> Option<u64> {
    let nanos = u64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()?;
    (nanos > 0).then_some(nanos)
}

/// Nanoseconds since the epoch, as the decimal string OTLP's JSON wants for a
/// 64-bit integer.
fn nanos(time: SystemTime) -> String {
    // 0 out of range, for ingest to refuse that span rather than the export.
    unix_nanos(time).unwrap_or(0).to_string()
}

fn any_value(value: &Value) -> AnyValue<'_> {
    match value {
        Value::Bool(flag) => AnyValue::Bool(*flag),
        Value::I64(int) => AnyValue::Int(int.to_string()),
        Value::F64(float) => AnyValue::Double(*float),
        Value::String(text) => AnyValue::String(Cow::Borrowed(text.as_str())),
        Value::Array(array) => AnyValue::Array {
            values: array_values(array),
        },
        other => AnyValue::String(Cow::Owned(other.to_string())),
    }
}

fn array_values(array: &Array) -> Vec<AnyValue<'_>> {
    match array {
        Array::Bool(flags) => flags.iter().map(|flag| AnyValue::Bool(*flag)).collect(),
        Array::I64(ints) => ints
            .iter()
            .map(|int| AnyValue::Int(int.to_string()))
            .collect(),
        Array::F64(floats) => floats
            .iter()
            .map(|float| AnyValue::Double(*float))
            .collect(),
        Array::String(texts) => texts
            .iter()
            .map(|text| AnyValue::String(Cow::Borrowed(text.as_str())))
            .collect(),
        other => vec![AnyValue::String(Cow::Owned(format!("{other:?}")))],
    }
}

/// Whether ingest drops an attribute for its length alone: a key past 128
/// characters, or a value past 1,024 as ingest writes it, a list as its JSON.
fn past_caps(key: &str, value: &Value) -> bool {
    if key.chars().count() > contract::ATTRIBUTE_KEY_MAX {
        return true;
    }
    let length = match value {
        Value::String(text) => text.as_str().chars().count(),
        Value::Array(array) => {
            let items: Vec<serde_json::Value> = match array {
                Array::Bool(items) => items.iter().map(|item| json!(item)).collect(),
                Array::I64(items) => items.iter().map(|item| json!(item)).collect(),
                Array::F64(items) => items.iter().map(|item| json!(item)).collect(),
                Array::String(items) => items.iter().map(|item| json!(item.as_str())).collect(),
                _ => Vec::new(),
            };
            serde_json::Value::Array(items).to_string().chars().count()
        }
        other => other.to_string().chars().count(),
    };
    length > contract::ATTRIBUTE_VALUE_MAX
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use opentelemetry::KeyValue;
    use opentelemetry::trace::{SpanContext, TraceFlags, TraceId, TraceState};
    use opentelemetry_sdk::trace::{SpanEvents, SpanLinks};

    use super::*;
    use crate::transport::TransportError;

    /// A transport that keeps each body it is given, and answers 200.
    #[derive(Clone, Default)]
    struct Kept(Arc<Mutex<Vec<Vec<u8>>>>);

    impl Transport for Kept {
        fn send(
            &self,
            _url: &str,
            _headers: &[(String, String)],
            body: &[u8],
        ) -> Result<Answer, TransportError> {
            lock(&self.0).push(body.to_vec());
            Ok(Answer {
                status: 200,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            })
        }
    }

    fn exporter(kept: &Kept, warnings: &Arc<Mutex<Vec<String>>>) -> Exporter {
        let warned = Arc::clone(warnings);
        let config = Config {
            api_key: Some("k".to_owned()),
            ..Config::default()
        };
        Exporter::new(
            &config,
            Box::new(kept.clone()),
            Box::new(|_| {}),
            Arc::new(Warner::new(Box::new(move |text| {
                lock(&warned).push(text.to_owned());
            }))),
        )
    }

    /// A span named `s<n>` for each size, an attribute of that many bytes beside
    /// one of a few.
    fn spans(sizes: &[usize]) -> Vec<SpanData> {
        let context = SpanContext::new(
            TraceId::from_bytes([1; 16]),
            SpanId::from_bytes([1; 8]),
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        );
        sizes
            .iter()
            .enumerate()
            .map(|(n, size)| SpanData {
                span_context: context.clone(),
                parent_span_id: SpanId::INVALID,
                parent_span_is_remote: false,
                span_kind: SpanKind::Internal,
                name: format!("s{n}").into(),
                start_time: UNIX_EPOCH + Duration::from_secs(1),
                end_time: UNIX_EPOCH + Duration::from_secs(2),
                attributes: vec![
                    KeyValue::new("telmoni.tag.big", "x".repeat(*size)),
                    KeyValue::new("app.kept", "yes"),
                ],
                dropped_attributes_count: 0,
                events: SpanEvents::default(),
                links: SpanLinks::default(),
                status: Status::Unset,
                instrumentation_scope: opentelemetry::InstrumentationScope::default(),
            })
            .collect()
    }

    /// Each span the bodies carry, in the order sent, as its name and its
    /// attributes' keys.
    fn sent(kept: &Kept) -> Vec<String> {
        lock(&kept.0)
            .iter()
            .flat_map(|body| {
                let value: serde_json::Value = serde_json::from_slice(body).unwrap();
                value["resourceSpans"][0]["scopeSpans"][0]["spans"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|span| {
                        let mut keys: Vec<&str> = span["attributes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|each| each["key"].as_str().unwrap())
                            .collect();
                        keys.sort_unstable();
                        format!("{}:{}", span["name"].as_str().unwrap(), keys.join(","))
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Ingest refuses an export past its 1 MiB whole, and its 413 may break an
    /// upload still going out: halved until each part fits.
    #[test]
    fn an_export_past_ingests_mebibyte_is_halved_until_each_part_fits() {
        let (kept, warnings) = (Kept::default(), Arc::default());
        exporter(&kept, &warnings).export(&spans(&[300_000; 8]));
        let sizes: Vec<usize> = lock(&kept.0).iter().map(Vec::len).collect();
        assert_eq!(sizes.len(), 4, "{sizes:?}");
        assert!(sizes.iter().all(|size| *size <= 1 << 20), "{sizes:?}");
        let want: Vec<String> = (0..8)
            .map(|n| format!("s{n}:app.kept,telmoni.tag.big"))
            .collect();
        assert_eq!(sent(&kept), want);
        assert!(lock(&warnings).is_empty());
    }

    /// Ingest refuses an export past its 1,000 spans whole; one of 1,000 goes as it is.
    #[test]
    fn an_export_past_ingests_thousand_spans_is_halved_until_each_part_holds_them() {
        let (kept, warnings) = (Kept::default(), Arc::default());
        exporter(&kept, &warnings).export(&spans(&[1; 2001]));
        let counts: Vec<usize> = lock(&kept.0)
            .iter()
            .map(|body| {
                let value: serde_json::Value = serde_json::from_slice(body).unwrap();
                value["resourceSpans"][0]["scopeSpans"][0]["spans"]
                    .as_array()
                    .unwrap()
                    .len()
            })
            .collect();
        assert_eq!(counts, [1000, 500, 501]);
        let want: Vec<String> = (0..2001)
            .map(|n| format!("s{n}:app.kept,telmoni.tag.big"))
            .collect();
        assert_eq!(sent(&kept), want);
    }

    /// Rather than lost, since a run's own span carries the run's end.
    #[test]
    fn a_span_alone_past_it_goes_without_what_ingest_drops_for_its_length() {
        let (kept, warnings) = (Kept::default(), Arc::default());
        let sender = exporter(&kept, &warnings);
        sender.export(&spans(&[100, 1 << 20, 100]));
        sender.export(&spans(&[1 << 20]));
        assert_eq!(
            sent(&kept),
            [
                "s0:app.kept,telmoni.tag.big",
                "s1:app.kept",
                "s2:app.kept,telmoni.tag.big",
                "s0:app.kept"
            ]
        );
        assert_eq!(
            *lock(&warnings),
            [
                "telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length"
            ]
        );
    }

    #[test]
    fn a_span_whose_own_name_passes_it_is_dropped_with_one_warning() {
        let (kept, warnings) = (Kept::default(), Arc::default());
        let mut named = spans(&[1]);
        named[0].name = "x".repeat(1 << 20).into();
        exporter(&kept, &warnings).export(&named);
        assert!(lock(&kept.0).is_empty());
        assert_eq!(
            *lock(&warnings),
            ["telmoni: a span past the 1 MiB an export may hold was dropped"]
        );
    }

    /// A transport that answers each request from `answers` in turn, `None` a
    /// failure to reach it, then `200`, counting them.
    #[derive(Clone, Default)]
    struct Scripted {
        answers: Arc<Mutex<Vec<Option<u16>>>>,
        sent: Arc<Mutex<usize>>,
    }

    impl Transport for Scripted {
        fn send(
            &self,
            _url: &str,
            _headers: &[(String, String)],
            _body: &[u8],
        ) -> Result<Answer, TransportError> {
            *lock(&self.sent) += 1;
            let mut answers = lock(&self.answers);
            let next = if answers.is_empty() {
                Some(200)
            } else {
                answers.remove(0)
            };
            let status = next.ok_or_else(|| TransportError::new("Connect"))?;
            Ok(Answer {
                status,
                headers: Vec::new(),
                body: b"{}".to_vec(),
            })
        }
    }

    /// A batch in parts goes no further once a part fails as the rest would,
    /// the platform out of reach, and on past a part refused for what it holds.
    #[test]
    fn a_batch_in_parts_ends_at_a_part_that_fails_as_the_rest_would() {
        for (answers, sent, warned) in [
            (
                vec![None; 9],
                3,
                "telmoni: export failed after 3 attempts (Connect); the batch was dropped",
            ),
            (
                vec![Some(400)],
                3,
                "telmoni: export refused with 400 ; the batch was dropped",
            ),
        ] {
            let transport = Scripted::default();
            *lock(&transport.answers) = answers;
            let warnings = Arc::new(Mutex::new(Vec::new()));
            let warned_to = Arc::clone(&warnings);
            let config = Config {
                api_key: Some("k".to_owned()),
                ..Config::default()
            };
            let exporter = Exporter::new(
                &config,
                Box::new(transport.clone()),
                Box::new(|_| {}),
                Arc::new(Warner::new(Box::new(move |text| {
                    lock(&warned_to).push(text.to_owned());
                }))),
            );
            exporter.export(&spans(&[1; 2001]));
            assert_eq!(*lock(&transport.sent), sent);
            assert_eq!(*lock(&warnings), [warned]);
        }
    }

    /// A float of NaN or an infinity, which JSON cannot carry, is sent as null,
    /// as in every SDK: that attribute lost at ingest, never the export it rides in.
    #[test]
    fn a_non_finite_float_is_sent_as_null_with_the_rest_of_its_export() {
        let (kept, warnings) = (Kept::default(), Arc::default());
        let mut both = spans(&[1, 1]);
        both[0].attributes.extend([
            KeyValue::new("app.nan", f64::NAN),
            KeyValue::new("app.below", f64::NEG_INFINITY),
            KeyValue::new("app.ratios", Value::Array(vec![1.5, f64::INFINITY].into())),
            KeyValue::new("app.ratio", 0.25),
        ]);
        exporter(&kept, &warnings).export(&both);
        let bodies = lock(&kept.0);
        assert_eq!(bodies.len(), 1, "one export of both spans");
        let body: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
        let sent = &body["resourceSpans"][0]["scopeSpans"][0]["spans"];
        assert_eq!(sent.as_array().map(Vec::len), Some(2));
        let typed: BTreeMap<&str, &serde_json::Value> = sent[0]["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|each| (each["key"].as_str().unwrap(), &each["value"]))
            .collect();
        assert_eq!(typed["app.nan"], &json!({ "doubleValue": null }));
        assert_eq!(typed["app.below"], &json!({ "doubleValue": null }));
        assert_eq!(
            typed["app.ratios"],
            &json!({ "arrayValue": { "values": [{ "doubleValue": 1.5 }, { "doubleValue": null }] } })
        );
        assert_eq!(typed["app.ratio"], &json!({ "doubleValue": 0.25 }));
    }

    /// A list is measured as ingest writes it, its JSON, so a span past 1 MiB for
    /// a list of many short strings goes without that list rather than dropped.
    #[test]
    fn a_span_past_it_for_a_list_of_short_strings_goes_without_the_list() {
        let (kept, warnings) = (Kept::default(), Arc::default());
        let mut one = spans(&[1]);
        one[0].attributes.push(KeyValue::new(
            "app.list",
            Value::Array(vec![opentelemetry::StringValue::from(""); 60_000].into()),
        ));
        exporter(&kept, &warnings).export(&one);
        assert_eq!(sent(&kept), ["s0:app.kept,telmoni.tag.big"]);
    }

    /// 2^64 ns after the epoch is written 0, a nanosecond less as its digits.
    #[test]
    fn a_time_past_64_bits_of_nanoseconds_is_written_as_0() {
        let last = UNIX_EPOCH + Duration::from_nanos(u64::MAX);
        assert_eq!(nanos(last), "18446744073709551615");
        assert_eq!(nanos(last + Duration::from_nanos(1)), "0");
    }
}
