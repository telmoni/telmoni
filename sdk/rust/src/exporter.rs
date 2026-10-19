//! The exporter: OTLP's JSON encoding over the transport.
//!
//! It takes content off every span unless the code asked and the project's
//! mode allows, sends to `POST {endpoint}/otlp/v1/traces`, waits out a
//! `Retry-After`, and reads `Telmoni-Content-Mode` and the partial success
//! from each answer, warning once for each attribute an answer names dropped.

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use opentelemetry::trace::{SpanId, SpanKind, Status};
use opentelemetry::{Array, Key, Value};
use opentelemetry_sdk::resource::{ResourceDetector as _, TelemetryResourceDetector};
use opentelemetry_sdk::trace::SpanData;
use serde_json::{Map, json};

use crate::config::Config;
use crate::contract;
use crate::transport::{Answer, Transport, cleartext_host};
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
    /// The host a plain-HTTP endpoint would carry the key to in the clear,
    /// when it is not this machine: nothing is sent, and the first export
    /// says why. The CLI refuses the same.
    cleartext: Option<String>,
    bearer: Option<String>,
    capture_content: bool,
    transport: Box<dyn Transport>,
    sleep: Box<dyn Fn(Duration) + Send + Sync>,
    warner: Arc<Warner>,
    content_mode: Mutex<Option<String>>,
    /// The resource's attributes, encoded once: the exporter's own list,
    /// whatever the provider's resource holds.
    resource: Vec<serde_json::Value>,
}

impl Exporter {
    pub(crate) fn new(
        config: &Config,
        transport: Box<dyn Transport>,
        sleep: Box<dyn Fn(Duration) + Send + Sync>,
        warner: Arc<Warner>,
    ) -> Self {
        Self {
            url: format!("{}/otlp/v1/traces", config.endpoint),
            cleartext: cleartext_host(&config.endpoint),
            bearer: config.api_key.clone(),
            capture_content: config.capture_content,
            transport,
            sleep,
            warner,
            content_mode: Mutex::new(None),
            resource: resource(config),
        }
    }

    /// The project's mode, as the last answer named it; `None` before one.
    pub(crate) fn content_mode(&self) -> Option<String> {
        lock(&self.content_mode).clone()
    }

    pub(crate) fn export(&self, spans: &[SpanData]) {
        if spans.is_empty() {
            return;
        }
        if let Some(host) = &self.cleartext {
            self.warner.once(
                "cleartext".to_owned(),
                &format!(
                    "telmoni: refusing to send the API key over unencrypted HTTP to '{host}'; use HTTPS; nothing is sent"
                ),
            );
            return;
        }
        let Ok(body) = serde_json::to_vec(&self.encode(spans)) else {
            return;
        };
        let mut headers = vec![
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("User-Agent".to_owned(), format!("telmoni-rust/{VERSION}")),
        ];
        if let Some(bearer) = &self.bearer {
            headers.push(("Authorization".to_owned(), format!("Bearer {bearer}")));
        }
        let mut failure = String::new();
        for attempt in 0..ATTEMPTS {
            let answer = match self.transport.send(&self.url, &headers, &body) {
                Ok(answer) => answer,
                Err(error) => {
                    failure = error.name;
                    self.wait(None, attempt);
                    continue;
                }
            };
            if (200..300).contains(&answer.status) {
                self.read(&answer);
                return;
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
            return;
        }
        self.warner.once(
            format!("failed:{failure}"),
            &format!(
                "telmoni: export failed after {ATTEMPTS} attempts ({failure}); the batch was dropped"
            ),
        );
    }

    /// OTLP's `ExportTraceServiceRequest`, in its JSON encoding.
    fn encode(&self, spans: &[SpanData]) -> serde_json::Value {
        let spans: Vec<serde_json::Value> = spans.iter().map(|span| self.span(span)).collect();
        json!({
            "resourceSpans": [{
                "resource": { "attributes": self.resource },
                "scopeSpans": [{
                    "scope": { "name": contract::SCOPE, "version": VERSION },
                    "spans": spans,
                }],
            }],
        })
    }

    fn span(&self, span: &SpanData) -> serde_json::Value {
        let keep_content = self.capture_content && self.content_mode().as_deref() == Some("on");
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
        let attributes: Vec<serde_json::Value> = attributes
            .into_iter()
            .map(|(key, value)| attribute(key, value))
            .collect();

        let mut encoded = Map::new();
        encoded.insert(
            "traceId".to_owned(),
            json!(span.span_context.trace_id().to_string()),
        );
        encoded.insert(
            "spanId".to_owned(),
            json!(span.span_context.span_id().to_string()),
        );
        if span.parent_span_id != SpanId::INVALID {
            encoded.insert(
                "parentSpanId".to_owned(),
                json!(span.parent_span_id.to_string()),
            );
        }
        encoded.insert("name".to_owned(), json!(span.name));
        encoded.insert("kind".to_owned(), json!(kind(&span.span_kind)));
        encoded.insert(
            "startTimeUnixNano".to_owned(),
            json!(nanos(span.start_time)),
        );
        encoded.insert("endTimeUnixNano".to_owned(), json!(nanos(span.end_time)));
        encoded.insert("attributes".to_owned(), json!(attributes));
        // Events and the status message are where an exception writes its
        // message and stack trace: never sent.
        if matches!(span.status, Status::Error { .. }) {
            encoded.insert("status".to_owned(), json!({ "code": 2 }));
        }
        serde_json::Value::Object(encoded)
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

    /// Before the next attempt; nothing after the last.
    fn wait(&self, retry_after: Option<&str>, attempt: u32) {
        if attempt + 1 >= ATTEMPTS {
            return;
        }
        // The header's whole seconds when it has them, else 1, 2, 4… seconds.
        let seconds = retry_after
            .and_then(|header| header.trim().parse::<i64>().ok())
            .map_or(1u64 << attempt, |parsed| u64::try_from(parsed).unwrap_or(0));
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

/// The exporter's five resource attributes, and the deployment's two where
/// the configuration names one: never from `OTEL_RESOURCE_ATTRIBUTES`, which
/// the application's exporters read.
fn resource(config: &Config) -> Vec<serde_json::Value> {
    let mut attributes = vec![
        (contract::DISTRO_NAME, contract::DISTRO.to_owned()),
        ("telemetry.distro.version", VERSION.to_owned()),
        ("telemetry.sdk.language", "rust".to_owned()),
        ("telemetry.sdk.name", "opentelemetry".to_owned()),
        ("telemetry.sdk.version", sdk_version()),
    ];
    if let Some(environment) = &config.environment {
        attributes.push((contract::ENVIRONMENT, environment.clone()));
    }
    if let Some(release) = &config.release {
        attributes.push((contract::RELEASE, release.clone()));
    }
    attributes
        .into_iter()
        .map(|(key, value)| attribute(key, &Value::from(value)))
        .collect()
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

/// Nanoseconds since the epoch, as the decimal string OTLP's JSON wants for a
/// 64-bit integer.
fn nanos(time: std::time::SystemTime) -> String {
    // 0 out of range, for ingest to refuse that span rather than the export.
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_nanos()).unwrap_or(0))
        .to_string()
}

/// One OTLP `KeyValue`: 64-bit integers travel as decimal strings.
fn attribute(key: &str, value: &Value) -> serde_json::Value {
    json!({ "key": key, "value": any_value(value) })
}

fn any_value(value: &Value) -> serde_json::Value {
    match value {
        Value::Bool(flag) => json!({ "boolValue": flag }),
        Value::I64(int) => json!({ "intValue": int.to_string() }),
        Value::F64(float) => json!({ "doubleValue": float }),
        Value::String(text) => json!({ "stringValue": text.as_str() }),
        Value::Array(array) => json!({ "arrayValue": { "values": array_values(array) } }),
        other => json!({ "stringValue": other.to_string() }),
    }
}

fn array_values(array: &Array) -> Vec<serde_json::Value> {
    match array {
        Array::Bool(flags) => flags
            .iter()
            .map(|flag| json!({ "boolValue": flag }))
            .collect(),
        Array::I64(ints) => ints
            .iter()
            .map(|int| json!({ "intValue": int.to_string() }))
            .collect(),
        Array::F64(floats) => floats
            .iter()
            .map(|float| json!({ "doubleValue": float }))
            .collect(),
        Array::String(texts) => texts
            .iter()
            .map(|text| json!({ "stringValue": text.as_str() }))
            .collect(),
        other => vec![json!({ "stringValue": format!("{other:?}") })],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2^64 ns after the epoch is written 0, a nanosecond less as its digits.
    #[test]
    fn a_time_past_64_bits_of_nanoseconds_is_written_as_0() {
        let last = UNIX_EPOCH + Duration::from_nanos(u64::MAX);
        assert_eq!(nanos(last), "18446744073709551615");
        assert_eq!(nanos(last + Duration::from_nanos(1)), "0");
    }
}
