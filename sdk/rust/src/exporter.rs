//! The exporter: OTLP's JSON over the transport, content off unless asked and allowed,
//! each image or file a message carries put by its hash where content leaves,
//! each attempt stamped with this clock, each answer's content mode and drops read.

use std::borrow::Cow;
use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use opentelemetry::trace::{SpanId, SpanKind, Status};
use opentelemetry::{Array, Key, KeyValue, Value};
use opentelemetry_sdk::resource::{ResourceDetector as _, TelemetryResourceDetector};
use opentelemetry_sdk::trace::SpanData;
use serde::Serialize;
use serde_json::json;

use crate::config::{Config, Mask};
use crate::contract;
use crate::scrub::{self, Counts, Switches};
use crate::transport::{Answer, Method, Transport, refusal};
use crate::warnings::{Warner, Warning, WarningKind};
use crate::{VERSION, lock};

/// How many times one batch is sent before it is dropped.
const ATTEMPTS: u32 = 3;
/// OTLP's retryable statuses: a 502 or a 504 is a load balancer's, a server's
/// rollout among its causes.
const RETRIED: [u16; 4] = [429, 502, 503, 504];
/// The longest a `Retry-After` is honoured for.
const RETRY_AFTER_MAX_SECS: u64 = 60;
/// How many files put are remembered before the set starts over: an image a
/// conversation repeats in every call is put once.
const UPLOADED_MAX: usize = 4096;
/// Where a dropped content field's warning sends the reader.
const CONTENT_DOCS: &str = "https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door";

/// A project's content mode, as ingest's answers name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContentMode {
    /// Content is dropped at the door, so it stays home.
    Off,
    /// Content is kept, so it leaves where `capture_content` asks.
    On,
}

pub(crate) struct Exporter {
    url: String,
    media_url: String,
    scores_url: String,
    /// Why nothing is sent to the endpoint ([`refusal`]), which the first
    /// export says.
    refusal: Option<String>,
    bearer: Option<String>,
    capture_content: bool,
    mask: Option<Mask>,
    scrubbers: Switches,
    /// What scrubs a field: [`scrub::scrub`], which a test swaps for one that fails.
    scrubber: fn(&str, Switches) -> Option<(String, Counts)>,
    transport: Box<dyn Transport>,
    sleep: Box<dyn Fn(Duration) + Send + Sync>,
    warner: Arc<Warner>,
    content_mode: Mutex<Option<ContentMode>>,
    /// The resource's attributes, encoded once: the exporter's own list,
    /// whatever the provider's resource holds.
    resource: Vec<serde_json::Value>,
    /// Set by a shutdown past its deadline: no attempt follows, nor a wait.
    given_up: AtomicBool,
    /// The hashes of the files put.
    uploaded: Mutex<HashSet<String>>,
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
            media_url: format!("{}{}", config.endpoint, contract::MEDIA_PATH),
            scores_url: format!("{}{}", config.endpoint, contract::SCORES_PATH),
            refusal: refusal(&config.endpoint),
            bearer: config.api_key.clone(),
            capture_content: config.capture_content,
            mask: config.mask.clone(),
            scrubbers: config.scrubbers(),
            scrubber: scrub::scrub,
            transport,
            sleep,
            warner,
            content_mode: Mutex::new(None),
            resource,
            given_up: AtomicBool::new(false),
            uploaded: Mutex::new(HashSet::new()),
        }
    }

    /// The project's mode, as the last answer named it; `None` before one.
    pub(crate) fn content_mode(&self) -> Option<ContentMode> {
        *lock(&self.content_mode)
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
        // A score sent once its span ended rides the queue on a span of its
        // own, which leaves as `/v1/scores`' item and never as a span.
        if self.refusal.is_none() && spans.iter().any(|span| score_sent(span).is_some()) {
            let items: Vec<&str> = spans.iter().filter_map(score_sent).collect();
            let rest: Vec<SpanData> = spans
                .iter()
                .filter(|span| score_sent(span).is_none())
                .cloned()
                .collect();
            self.send_scores(&items);
            return self.export(&rest);
        }
        if let Some(refusal) = &self.refusal {
            self.warner.once(
                "endpoint".to_owned(),
                &format!("telmoni: {refusal}; nothing is sent"),
                WarningKind::Refused.into(),
            );
            return false;
        }
        // Yet to hear the project's mode, a client asks it first, so a process
        // that sends once, as a hook does, still sends its content.
        if self.capture_content && self.content_mode().is_none() && spans.iter().any(holds_content)
        {
            self.ask();
        }
        // Decided once, so no answer met halfway sends content the mask never saw.
        let keep = self.keeps_content();
        let spans = if keep {
            let masked = self.mask(spans);
            let lifted = match self.lift(&masked) {
                Cow::Owned(lifted) => Some(lifted),
                Cow::Borrowed(_) => None,
            };
            // As sent, its images out: a rule could misread their base64.
            self.scrub(lifted.map_or(masked, Cow::Owned))
        } else {
            Cow::Borrowed(spans)
        };
        self.export_lifted(&spans, keep)
    }

    /// Asks the project's content mode with an export of no spans, which
    /// ingest answers as the mode is now: once and silently, since the export
    /// after it retries and warns of what it meets, its content held back
    /// meanwhile.
    fn ask(&self) {
        if self.gave_up() {
            return;
        }
        let mut sent = self.headers();
        sent.push((
            contract::SENT_AT_HEADER.to_owned(),
            nanos(std::time::SystemTime::now()),
        ));
        // Unheard, the mode keeps the content back, as no answer does.
        if let Ok(answer) = self.transport.send(Method::Post, &self.url, &sent, ASK)
            && (200..300).contains(&answer.status)
        {
            self.read(&answer);
        }
    }

    fn headers(&self) -> Vec<(String, String)> {
        self.headers_for("application/json")
    }

    /// A request's headers to the platform: its body's type, the SDK's name
    /// and the key.
    fn headers_for(&self, content_type: &str) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Content-Type".to_owned(), content_type.to_owned()),
            ("User-Agent".to_owned(), format!("telmoni-rust/{VERSION}")),
        ];
        if let Some(bearer) = &self.bearer {
            headers.push(("Authorization".to_owned(), format!("Bearer {bearer}")));
        }
        headers
    }

    fn export_lifted(&self, spans: &[SpanData], keep: bool) -> bool {
        // Ingest refuses an export past 1,000 spans whole, as one past 1 MiB below.
        if spans.len() > contract::BATCH_SPANS_MAX {
            return self.halve(spans, keep);
        }
        let Ok(mut body) = serde_json::to_vec(&self.encode(spans, keep, false, &[])) else {
            self.warner.once(
                "encode".to_owned(),
                "telmoni: an export could not be encoded; the batch was dropped",
                WarningKind::ExportError.into(),
            );
            return true;
        };
        if body.len() > contract::BATCH_MAX_BYTES {
            // Ingest refuses it whole, its 413 maybe breaking the upload still
            // going out, which reads as no answer and is sent again: halved.
            if spans.len() > 1 {
                return self.halve(spans, keep);
            }
            // Content sent goes first, the longest field first, each named; else a
            // span alone past it goes without what ingest drops for its length,
            // and then fits, unless its own name does not.
            if let Some(cut) = self.cut(spans, keep) {
                self.warner.once(
                    "content-cut".to_owned(),
                    "telmoni: a span past the 1 MiB an export may hold was sent without its longest content, named in telmoni.content_cut",
                    WarningKind::Trimmed.into(),
                );
                body = cut;
            } else {
                match serde_json::to_vec(&self.encode(spans, keep, true, &[])) {
                    Ok(trimmed) if trimmed.len() <= contract::BATCH_MAX_BYTES => {
                        self.warner.once(
                        "trimmed".to_owned(),
                        "telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length",
                        WarningKind::Trimmed.into(),
                    );
                        body = trimmed;
                    }
                    _ => {
                        self.warner.once(
                            "too-large".to_owned(),
                            "telmoni: a span past the 1 MiB an export may hold was dropped",
                            WarningKind::TooLarge.into(),
                        );
                        return true;
                    }
                }
            }
        }
        match self.post(&self.url, &body) {
            Posted::Kept(answer) => {
                self.read(&answer);
                true
            }
            Posted::GaveUp => false,
            Posted::Refused(status, problem) => {
                self.warner.once(
                    format!("status:{status}"),
                    &format!(
                        "telmoni: export refused with {}; the batch was dropped",
                        named(status, &problem),
                    ),
                    answered(WarningKind::Refused, Some((status, problem))),
                );
                // Refused for what it holds, the rest of a batch may still go.
                status == 400 || status == 413
            }
            Posted::Failed(failure, last) => {
                self.warner.once(
                    format!("failed:{failure}"),
                    &format!(
                        "telmoni: export failed after {ATTEMPTS} attempts ({failure}); the batch was dropped"
                    ),
                    answered(WarningKind::Failed, last),
                );
                false
            }
        }
    }

    /// Posts `body` to `url` as JSON, each attempt stamped with this clock,
    /// retrying what a busy or restarting platform answers.
    fn post(&self, url: &str, body: &[u8]) -> Posted {
        let headers = self.headers();
        let mut failure = String::new();
        // The last attempt's alone, as the line names it.
        let mut last: Option<(u16, String)> = None;
        for attempt in 0..ATTEMPTS {
            if self.gave_up() {
                return Posted::GaveUp;
            }
            // This clock as the attempt leaves, the spans' own.
            let mut sent = headers.clone();
            sent.push((
                contract::SENT_AT_HEADER.to_owned(),
                nanos(std::time::SystemTime::now()),
            ));
            let answer = match self.transport.send(Method::Post, url, &sent, body) {
                Ok(answer) => answer,
                Err(error) => {
                    failure = error.name;
                    last = None;
                    self.wait(None, attempt);
                    continue;
                }
            };
            if (200..300).contains(&answer.status) {
                return Posted::Kept(answer);
            }
            let problem = problem_type(&answer.body);
            if RETRIED.contains(&answer.status) {
                failure = named(answer.status, &problem);
                last = Some((answer.status, problem));
                self.wait(answer.header("Retry-After"), attempt);
                continue;
            }
            return Posted::Refused(answer.status, problem);
        }
        Posted::Failed(failure, last)
    }

    /// Sends the scores `items` hold to `/v1/scores`, a hundred a body; a
    /// body refused or failed is dropped with one warning, and the rest go.
    fn send_scores(&self, items: &[&str]) {
        for chunk in items.chunks(contract::SCORES_SENT_MAX) {
            let scores: Vec<serde_json::Value> = chunk
                .iter()
                .filter_map(|item| serde_json::from_str(item).ok())
                .collect();
            let Ok(body) = serde_json::to_vec(&json!({ "scores": scores })) else {
                continue;
            };
            match self.post(&self.scores_url, &body) {
                Posted::Kept(_) => {}
                Posted::GaveUp => return,
                Posted::Refused(status, problem) => self.warner.once(
                    format!("scores-status:{status}"),
                    &format!(
                        "telmoni: scores refused with {}; they were dropped",
                        named(status, &problem),
                    ),
                    answered(WarningKind::Refused, Some((status, problem))),
                ),
                Posted::Failed(failure, last) => self.warner.once(
                    format!("scores-failed:{failure}"),
                    &format!(
                        "telmoni: scores failed after {ATTEMPTS} attempts ({failure}); they were dropped"
                    ),
                    answered(WarningKind::Failed, last),
                ),
            }
        }
    }

    /// Exports each half of `spans`, the second only where the first went.
    fn halve(&self, spans: &[SpanData], keep: bool) -> bool {
        let (head, tail) = spans.split_at(spans.len() / 2);
        self.export_lifted(head, keep) && self.export_lifted(tail, keep)
    }

    /// `spans` with every content field, event attribute and status message as
    /// the code's mask answers it, before a part it takes out could be put.
    fn mask<'a>(&self, spans: &'a [SpanData]) -> Cow<'a, [SpanData]> {
        let Some(mask) = &self.mask else {
            return Cow::Borrowed(spans);
        };
        let masked = spans
            .iter()
            .cloned()
            .map(|mut span| {
                content_fields(&mut span, |attribute| self.masked(mask, attribute));
                span
            })
            .collect();
        Cow::Owned(masked)
    }

    /// `spans` with every field the mask is handed as the scrubbers leave it; a
    /// span they changed counts what in `telmoni.scrubbed`.
    fn scrub<'a>(&self, spans: Cow<'a, [SpanData]>) -> Cow<'a, [SpanData]> {
        if !self.scrubbers.any() {
            return spans;
        }
        let scrubbed = spans
            .into_owned()
            .into_iter()
            .map(|mut span| {
                let mut counts = Counts::new();
                content_fields(&mut span, |attribute| self.scrubbed(attribute, &mut counts));
                if !counts.is_empty() {
                    // A `BTreeMap` writes its keys sorted.
                    let counts = serde_json::to_string(&counts).unwrap_or_default();
                    span.attributes
                        .push(KeyValue::new(contract::SCRUBBED, counts));
                }
                span
            })
            .collect();
        Cow::Owned(scrubbed)
    }

    /// One field as the scrubbers leave it, each kind replaced counted; `None` where
    /// they fail, counted as `dropped:<key>`, so it never leaves as it was.
    fn scrubbed(&self, attribute: KeyValue, counts: &mut Counts) -> Option<KeyValue> {
        let given = as_text(&attribute.value);
        let Ok(Some((text, found))) =
            panic::catch_unwind(AssertUnwindSafe(|| (self.scrubber)(&given, self.scrubbers)))
        else {
            *counts
                .entry(format!("dropped:{}", attribute.key.as_str()))
                .or_default() += 1;
            return None;
        };
        if found.is_empty() {
            return Some(attribute);
        }
        for (kind, count) in found {
            *counts.entry(kind).or_default() += count;
        }
        Some(KeyValue::new(attribute.key, text))
    }

    /// One field as the mask answers it, handed as a string, any other value as
    /// its JSON; `None` to send the span without it. An answer unchanged keeps
    /// the value as it was.
    fn masked(&self, mask: &Mask, attribute: KeyValue) -> Option<KeyValue> {
        let given = as_text(&attribute.value);
        let key = attribute.key.as_str();
        let Ok(answer) = panic::catch_unwind(AssertUnwindSafe(|| mask(key, &given))) else {
            // The key alone: a panic's words may hold the content.
            self.warner.once(
                format!("mask:{key}"),
                &format!("telmoni: the mask failed on {key}; the span was sent without it"),
                WarningKind::MaskFailed.into(),
            );
            return None;
        };
        match answer? {
            same if same == given => Some(attribute),
            changed => Some(KeyValue::new(attribute.key, changed)),
        }
    }

    /// `spans` with every `blob` part of their messages lifted out: a blob
    /// repeated in every span would bloat each, past 1 MiB maybe.
    fn lift<'a>(&self, spans: &'a [SpanData]) -> Cow<'a, [SpanData]> {
        let mut lifted: Option<Vec<SpanData>> = None;
        for (i, span) in spans.iter().enumerate() {
            for (j, attribute) in span.attributes.iter().enumerate() {
                if !contract::MEDIA_FIELDS.contains(&attribute.key.as_str()) {
                    continue;
                }
                let Value::String(text) = &attribute.value else {
                    continue;
                };
                let Some(text) = self.lift_text(text.as_str()) else {
                    continue;
                };
                let copy = lifted.get_or_insert_with(|| spans.to_vec());
                if let Some(slot) = copy.get_mut(i).and_then(|span| span.attributes.get_mut(j)) {
                    *slot = KeyValue::new(attribute.key.clone(), text);
                }
            }
        }
        lifted.map_or(Cow::Borrowed(spans), Cow::Owned)
    }

    /// A field's JSON with each `blob` part lifted out; `None` where it holds
    /// none, or is no JSON array.
    fn lift_text(&self, text: &str) -> Option<String> {
        let mut decoded: serde_json::Value = serde_json::from_str(text).ok()?;
        if !self.lift_parts(decoded.as_array_mut()?) {
            return None;
        }
        serde_json::to_string(&decoded).ok()
    }

    /// Replaces each `blob` part in place, a message's parts too; whether any was.
    fn lift_parts(&self, parts: &mut [serde_json::Value]) -> bool {
        let mut lifted = false;
        for part in parts {
            if let Some(inner) = part
                .get_mut("parts")
                .and_then(serde_json::Value::as_array_mut)
            {
                lifted |= self.lift_parts(inner);
                continue;
            }
            if let Some(file) = self.file_part(part) {
                *part = file;
                lifted = true;
            }
        }
        lifted
    }

    /// The `file` part naming a `blob` part's bytes, put first unless past the
    /// platform's cap or put already; `None` for any other part.
    fn file_part(&self, part: &serde_json::Value) -> Option<serde_json::Value> {
        let blob = part.as_object()?;
        if blob.get("type").and_then(serde_json::Value::as_str) != Some("blob") {
            return None;
        }
        let data = base64_decode(blob.get("content")?.as_str()?)?;
        let hash = sha256_hex(&data);
        if data.len() <= contract::MEDIA_MAX_BYTES && !lock(&self.uploaded).contains(&hash) {
            self.upload(&hash, &data, blob.get("mime_type"));
        }
        // Named whatever became of it: the platform says itself what it holds.
        let mut file = serde_json::Map::new();
        file.insert("type".to_owned(), "file".into());
        for name in ["modality", "mime_type"] {
            if let Some(value) = blob.get(name) {
                file.insert(name.to_owned(), value.clone());
            }
        }
        file.insert(
            "file_id".to_owned(),
            format!("{}{hash}", contract::FILE_ID_PREFIX).into(),
        );
        Some(serde_json::Value::Object(file))
    }

    /// Puts one file, once: a failure is warned of, never the export's.
    fn upload(&self, hash: &str, data: &[u8], mime_type: Option<&serde_json::Value>) {
        if self.gave_up() {
            return;
        }
        let content_type = mime_type
            .and_then(serde_json::Value::as_str)
            .filter(|given| header_safe(given))
            .unwrap_or(contract::MEDIA_DEFAULT_TYPE);
        let headers = self.headers_for(content_type);
        let url = format!("{}{hash}", self.media_url);
        // A transport that panics costs the file alone.
        let sent = panic::catch_unwind(AssertUnwindSafe(|| {
            self.transport.send(Method::Put, &url, &headers, data)
        }));
        let (failure, status) = match sent {
            Ok(Ok(answer)) if (200..300).contains(&answer.status) => {
                let mut uploaded = lock(&self.uploaded);
                if uploaded.len() >= UPLOADED_MAX {
                    uploaded.clear();
                }
                uploaded.insert(hash.to_owned());
                return;
            }
            Ok(Ok(answer)) => (answer.status.to_string(), Some(answer.status)),
            Ok(Err(error)) => (error.name, None),
            Err(_) => ("panic".to_owned(), None),
        };
        self.warner.once(
            format!("upload:{failure}"),
            &format!(
                "telmoni: an image or file in a message could not be uploaded ({failure}); its part was sent naming it, which the platform may not hold"
            ),
            Warning {
                kind: WarningKind::UploadFailed,
                status,
                problem_type: None,
            },
        );
    }

    /// The one span's body without its longest content, as few fields as fit it
    /// in an export: a content field, or any past ingest's caps, which on a
    /// project that keeps content is content too; `None` where it sends no
    /// content, or fits without all of it.
    fn cut(&self, spans: &[SpanData], keep: bool) -> Option<Vec<u8>> {
        if !keep {
            return None;
        }
        let mut content: Vec<(&str, usize)> = spans
            .iter()
            .flat_map(|span| &span.attributes)
            .map(|attribute| (attribute.key.as_str(), &attribute.value))
            .filter(|(key, value)| {
                *key != contract::CONTENT_CUT
                    && (contract::is_content(key) || past_caps(key, value))
            })
            .map(|(key, value)| (key, ingest_length(value)))
            .collect();
        content.sort_by_key(|(_, length)| std::cmp::Reverse(*length));
        let mut cut: Vec<&str> = Vec::new();
        for (key, _) in content {
            if cut.contains(&key) {
                continue;
            }
            cut.push(key);
            if let Ok(body) = serde_json::to_vec(&self.encode(spans, keep, false, &cut))
                && body.len() <= contract::BATCH_MAX_BYTES
            {
                return Some(body);
            }
        }
        None
    }

    /// Whether content leaves: the code asked, and the project's mode allows.
    fn keeps_content(&self) -> bool {
        self.capture_content && self.content_mode() == Some(ContentMode::On)
    }

    /// OTLP's `ExportTraceServiceRequest`, in its JSON encoding; `keep_content`,
    /// with its content; `trimmed`, without the attributes ingest drops for their
    /// length; `cut`, without those content fields, named in `telmoni.content_cut`.
    fn encode<'a>(
        &'a self,
        spans: &'a [SpanData],
        keep_content: bool,
        trimmed: bool,
        cut: &'a [&'a str],
    ) -> Request<'a> {
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
                        .map(|span| self.span(span, trimmed, keep_content, cut))
                        .collect(),
                }],
            }],
        }
    }

    fn span<'a>(
        &self,
        span: &'a SpanData,
        trimmed: bool,
        keep_content: bool,
        cut: &'a [&'a str],
    ) -> OtlpSpan<'a> {
        // The last value set under a key is the one that stands, as a map
        // would have it: OpenTelemetry's span keeps every set in a list.
        let mut attributes: Vec<(&str, &Value)> = Vec::new();
        for attribute in &span.attributes {
            let key = attribute.key.as_str();
            if (!keep_content && contract::is_content(key))
                || cut.contains(&key)
                || (!cut.is_empty() && key == contract::CONTENT_CUT)
            {
                continue;
            }
            match attributes.iter_mut().find(|(seen, _)| *seen == key) {
                Some(slot) => slot.1 = &attribute.value,
                None => attributes.push((key, &attribute.value)),
            }
        }
        let mut attributes: Vec<OtlpAttribute<'a>> = attributes
            .into_iter()
            .filter(|(key, value)| !(trimmed && past_caps(key, value)))
            .map(|(key, value)| OtlpAttribute {
                key,
                value: any_value(value),
            })
            .collect();
        if !cut.is_empty() {
            attributes.push(OtlpAttribute {
                key: contract::CONTENT_CUT,
                value: AnyValue::Array {
                    values: cut
                        .iter()
                        .map(|key| AnyValue::String(Cow::Borrowed(key)))
                        .collect(),
                },
            });
        }
        // Events and the status message carry an exception's message: content,
        // and the first to go from a span trimmed to fit. A score's own keys
        // are metadata, sent in every mode.
        let kept = keep_content && !trimmed;
        let events = span
            .events
            .iter()
            .filter(|event| kept || is_score(event))
            .map(|event| OtlpEvent {
                time_unix_nano: nanos(event.timestamp),
                name: &event.name,
                attributes: event
                    .attributes
                    .iter()
                    .filter(|attribute| kept || is_score_key(attribute))
                    .map(|attribute| OtlpAttribute {
                        key: attribute.key.as_str(),
                        value: any_value(&attribute.value),
                    })
                    .collect(),
            })
            .collect();
        let status = match &span.status {
            Status::Error { description } => Some(ErrorStatus {
                code: 2,
                message: (kept && !description.is_empty()).then_some(description.as_ref()),
            }),
            _ => None,
        };
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
            events,
            status,
        }
    }

    fn read(&self, answer: &Answer) {
        if let Some(mode) = answer
            .header(contract::CONTENT_MODE_HEADER)
            .map(str::trim)
            .filter(|mode| !mode.is_empty())
        {
            // A word ingest does not send reads as none, which keeps content home.
            *lock(&self.content_mode) = match mode {
                "off" => Some(ContentMode::Off),
                "on" => Some(ContentMode::On),
                _ => None,
            };
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
                self.warner.once(
                    format!("dropped:{attribute}:{reason}"),
                    &text,
                    WarningKind::Dropped.into(),
                );
                continue;
            }
            if let Some(why) = refused(line) {
                self.warner.once(
                    format!("refused:{why}"),
                    &format!("telmoni: the server refused a span: {why}"),
                    WarningKind::SpanRefused.into(),
                );
                continue;
            }
            if line.starts_with("and ") || line.is_empty() {
                continue;
            }
            self.warner.once(
                format!("line:{line}"),
                &format!("telmoni: the server said: {line}"),
                WarningKind::Said.into(),
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

/// What a post came to: kept with its answer, refused with its status and
/// problem type, failed past its attempts, or given up for a shutdown.
enum Posted {
    Kept(Answer),
    Refused(u16, String),
    Failed(String, Option<(u16, String)>),
    GaveUp,
}

/// Each field of `span` the content lock decides on — a content field, an
/// event's attribute but a score's own, the status message — as `each` answers it.
fn content_fields(span: &mut SpanData, mut each: impl FnMut(KeyValue) -> Option<KeyValue>) {
    span.attributes = std::mem::take(&mut span.attributes)
        .into_iter()
        .filter_map(|attribute| {
            let key = attribute.key.as_str();
            if contract::is_content(key) && key != contract::CONTENT_CUT {
                each(attribute)
            } else {
                Some(attribute)
            }
        })
        .collect();
    for event in &mut span.events.events {
        let score = is_score(event);
        event.attributes = std::mem::take(&mut event.attributes)
            .into_iter()
            .filter_map(|attribute| {
                if score && is_score_key(&attribute) {
                    Some(attribute)
                } else {
                    each(attribute)
                }
            })
            .collect();
    }
    if let Status::Error { description } = &span.status
        && !description.is_empty()
    {
        let given = KeyValue::new(contract::STATUS_DESCRIPTION, description.to_string());
        span.status = Status::Error {
            description: each(given)
                .map(|kept| kept.value.as_str().into_owned().into())
                .unwrap_or_default(),
        };
    }
}

/// Whether `event` is a score, whose own keys are metadata where every other
/// event attribute is content.
fn is_score(event: &opentelemetry::trace::Event) -> bool {
    event.name == contract::EVALUATION_EVENT
}

/// Whether `attribute` is one of a score's own keys.
fn is_score_key(attribute: &KeyValue) -> bool {
    contract::SCORE_KEYS.contains(&attribute.key.as_str())
}

/// The `/v1/scores` item a score's span carries, where it is one: the
/// client's own root span, so no application's attribute diverts a span.
fn score_sent(span: &SpanData) -> Option<&str> {
    if span.name != contract::SCORE_SPAN || span.parent_span_id != SpanId::INVALID {
        return None;
    }
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == contract::SCORE_SENT)
        .and_then(|attribute| match &attribute.value {
            Value::String(item) => Some(item.as_str()),
            _ => None,
        })
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
        // platform's to say, and only the organization's owner switches one.
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

/// Whether a part's type can go as a header: printable ASCII, not blank.
fn header_safe(value: &str) -> bool {
    !value.trim_matches(' ').is_empty() && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
}

/// Base64 as Python's `validate=True` reads it: padded, nothing but the
/// alphabet, and a last symbol's spare bits ignored, as Python ignores them.
const PADDED: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::RequireCanonical)
        .with_decode_allow_trailing_bits(true),
);

/// Base64's bytes, read as [`PADDED`] reads them; `None` for any other text.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    PADDED.decode(text).ok()
}

/// The lowercase hex SHA-256 of `data`.
fn sha256_hex(data: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, data)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// An export's own failure, with the status and problem type it last met,
/// an empty problem type none.
fn answered(kind: WarningKind, last: Option<(u16, String)>) -> Warning {
    let (status, problem_type) = match last {
        Some((status, problem)) => (Some(status), Some(problem).filter(|p| !p.is_empty())),
        None => (None, None),
    };
    Warning {
        kind,
        status,
        problem_type,
    }
}

/// The problem type a refusal's body opens with, else nothing.
/// An export of no spans: how a client asks the project's content mode.
const ASK: &[u8] = br#"{"resourceSpans":[]}"#;

/// Whether a span holds anything the content lock decides on: a content
/// field, an event or a status message.
fn holds_content(span: &SpanData) -> bool {
    span.attributes
        .iter()
        .any(|kv| contract::is_content(kv.key.as_str()))
        || span
            .events
            .iter()
            .any(|event| !is_score(event) || event.attributes.iter().any(|kv| !is_score_key(kv)))
        || matches!(&span.status, Status::Error { description } if !description.is_empty())
}

/// An answer as a warning names it: its status, then the problem type its
/// body named, where it named one.
fn named(status: u16, problem: &str) -> String {
    if problem.is_empty() {
        status.to_string()
    } else {
        format!("{status} {problem}")
    }
}

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
                WarningKind::Input.into(),
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    events: Vec<OtlpEvent<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<ErrorStatus<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OtlpEvent<'a> {
    time_unix_nano: String,
    name: &'a str,
    attributes: Vec<OtlpAttribute<'a>>,
}

#[derive(Serialize)]
struct ErrorStatus<'a> {
    code: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
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
/// characters, or a value past 1,024 as ingest writes it.
fn past_caps(key: &str, value: &Value) -> bool {
    key.chars().count() > contract::ATTRIBUTE_KEY_MAX
        || ingest_length(value) > contract::ATTRIBUTE_VALUE_MAX
}

/// A value's length in characters as ingest writes it.
fn ingest_length(value: &Value) -> usize {
    match value {
        Value::String(text) => text.as_str().chars().count(),
        other => as_text(other).chars().count(),
    }
}

/// A value as ingest writes it, a list as its JSON.
fn as_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.as_str().to_owned(),
        Value::Array(array) => {
            let items: Vec<serde_json::Value> = match array {
                Array::Bool(items) => items.iter().map(|item| json!(item)).collect(),
                Array::I64(items) => items.iter().map(|item| json!(item)).collect(),
                Array::F64(items) => items.iter().map(|item| json!(item)).collect(),
                Array::String(items) => items.iter().map(|item| json!(item.as_str())).collect(),
                _ => Vec::new(),
            };
            serde_json::Value::Array(items).to_string()
        }
        other => other.to_string(),
    }
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
            _method: Method,
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

    /// Each warning's line and what it describes.
    type Warned = Arc<Mutex<Vec<(String, Warning)>>>;

    fn exporter(kept: &Kept, warnings: &Warned) -> Exporter {
        let warned = Arc::clone(warnings);
        let config = Config {
            api_key: Some("k".to_owned()),
            ..Config::default()
        };
        Exporter::new(
            &config,
            Box::new(kept.clone()),
            Box::new(|_| {}),
            Arc::new(Warner::new(Box::new(move |text, warning| {
                lock(&warned).push((text.to_owned(), warning.clone()));
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

    /// A file is put up to the platform's cap, which `media.json` names too.
    #[test]
    fn a_file_is_put_up_to_the_platforms_cap() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/media.json");
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("media.json"))
                .expect("media.json is JSON");
        assert_eq!(fixture["max_bytes"], contract::MEDIA_MAX_BYTES);
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
            [(
                "telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length".to_owned(),
                WarningKind::Trimmed.into()
            )]
        );
    }

    /// With content sent, the longest fields go first, as few as fit the span,
    /// each named in `telmoni.content_cut`; the rest of the span goes whole.
    #[test]
    fn a_span_alone_past_it_goes_without_its_longest_content_named() {
        let (kept, warnings) = (Kept::default(), Arc::default());
        let mut sender = exporter(&kept, &warnings);
        sender.capture_content = true;
        *lock(&sender.content_mode) = Some(ContentMode::On);
        let mut big = spans(&[1]);
        big[0].attributes.extend([
            KeyValue::new("gen_ai.input.messages", "i".repeat(600_000)),
            KeyValue::new("gen_ai.tool.call.result", "r".repeat(700_000)),
            KeyValue::new("gen_ai.output.messages", "o".repeat(2_000)),
            KeyValue::new("app.notes", "n".repeat(800_000)),
            KeyValue::new(
                "telmoni.content_cut",
                Value::Array(vec![opentelemetry::StringValue::from("set by the code")].into()),
            ),
        ]);
        sender.export(&big);
        assert_eq!(
            sent(&kept),
            [
                "s0:app.kept,gen_ai.input.messages,gen_ai.output.messages,telmoni.content_cut,telmoni.tag.big"
            ]
        );
        let body: serde_json::Value = serde_json::from_slice(&lock(&kept.0)[0]).unwrap();
        let cut = body["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|each| each["key"] == contract::CONTENT_CUT)
            .map(|each| each["value"].clone());
        assert_eq!(
            cut,
            Some(json!({ "arrayValue": { "values": [
                    { "stringValue": "app.notes" },
                    { "stringValue": "gen_ai.tool.call.result" }
                ] } }))
        );
        assert_eq!(
            *lock(&warnings),
            [(
                "telmoni: a span past the 1 MiB an export may hold was sent without its longest content, named in telmoni.content_cut".to_owned(),
                WarningKind::Trimmed.into()
            )]
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
            [(
                "telmoni: a span past the 1 MiB an export may hold was dropped".to_owned(),
                WarningKind::TooLarge.into()
            )]
        );
    }

    /// A field the scrubber fails on is left out, never sent raw, and named in `telmoni.scrubbed`.
    #[test]
    fn a_field_the_scrubber_fails_on_is_left_out_and_named() {
        let (kept, warnings) = (Kept::default(), Arc::default());
        let mut sender = exporter(&kept, &warnings);
        sender.capture_content = true;
        *lock(&sender.content_mode) = Some(ContentMode::On);
        sender.scrubbers = Switches {
            secrets: true,
            pii: false,
        };
        sender.scrubber = |text, on| {
            if text.contains("fails") {
                panic!("the scrubber failed");
            }
            scrub::scrub(text, on)
        };
        let mut failing = spans(&[1]);
        failing[0].attributes.extend([
            KeyValue::new("gen_ai.input.messages", "it fails on AKIAIOSFODNN7EXAMPLE"),
            KeyValue::new("gen_ai.output.messages", "key AKIAIOSFODNN7EXAMPLE"),
        ]);
        failing[0].status = Status::error("it fails here");
        sender.export(&failing);
        let body = lock(&kept.0)[0].clone();
        assert!(!String::from_utf8_lossy(&body).contains("fails"));
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let sent = &body["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        let attributes: BTreeMap<&str, &serde_json::Value> = sent["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|each| (each["key"].as_str().unwrap(), &each["value"]["stringValue"]))
            .collect();
        assert!(!attributes.contains_key("gen_ai.input.messages"));
        assert_eq!(
            attributes["gen_ai.output.messages"],
            "key [secret: aws_key]"
        );
        let scrubbed: serde_json::Value =
            serde_json::from_str(attributes[contract::SCRUBBED].as_str().unwrap()).unwrap();
        assert_eq!(
            scrubbed,
            json!({
                "dropped:gen_ai.input.messages": 1,
                "dropped:otel.status_description": 1,
                "secret:aws_key": 1
            })
        );
        assert_eq!(sent["status"], json!({ "code": 2 }));
        assert!(lock(&warnings).is_empty());
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
            _method: Method,
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
        let failed = Warning::from(WarningKind::Failed);
        for (answers, sent, warned, described) in [
            (
                vec![None; 9],
                3,
                "telmoni: export failed after 3 attempts (Connect); the batch was dropped",
                failed.clone(),
            ),
            // The fields are the last attempt's, as the line is.
            (
                vec![Some(503), Some(503), None],
                3,
                "telmoni: export failed after 3 attempts (Connect); the batch was dropped",
                failed,
            ),
            (
                vec![Some(400)],
                3,
                "telmoni: export refused with 400; the batch was dropped",
                Warning {
                    kind: WarningKind::Refused,
                    status: Some(400),
                    problem_type: None,
                },
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
                Arc::new(Warner::new(Box::new(move |text, warning| {
                    lock(&warned_to).push((text.to_owned(), warning.clone()));
                }))),
            );
            exporter.export(&spans(&[1; 2001]));
            assert_eq!(*lock(&transport.sent), sent);
            assert_eq!(*lock(&warnings), [(warned.to_owned(), described)]);
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

    /// FIPS 180-4's vectors, one and two blocks and one of many, in lowercase hex.
    #[test]
    fn sha256_matches_the_standards_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            sha256_hex(&[b'a'; 1000]),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    /// Padded base64 alone, as Python's `validate=True` reads it, spare bits and all.
    #[test]
    fn base64_is_read_padded_and_nothing_else() {
        assert_eq!(base64_decode(""), Some(Vec::new()));
        assert_eq!(base64_decode("QQ=="), Some(b"A".to_vec()));
        assert_eq!(base64_decode("QR=="), Some(b"A".to_vec()));
        assert_eq!(base64_decode("QUI="), Some(b"AB".to_vec()));
        assert_eq!(base64_decode("+/+/"), Some(vec![0xfb, 0xff, 0xbf]));
        for bad in [
            "QQ",
            "QQ=",
            "Q===",
            "QQ==QQ==",
            "QQ\nQ",
            "not base64!",
            "QUJD\u{e9}",
        ] {
            assert_eq!(base64_decode(bad), None, "{bad}");
        }
    }
}
