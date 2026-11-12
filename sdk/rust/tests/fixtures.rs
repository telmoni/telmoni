//! The golden fixtures in `sdk/fixtures`: the export every SDK must send for
//! the same trace, and what each must do with the server's answers.

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use common::{
    Collected, Failure, FakeTransport, Harness, Outcome, Sleeps, Warnings, answer, answer_names,
    attributes_of, field, fields_of, float, hooks, int, is_hex, keyed, load_fixture, name_of,
    spans_of, text, warning_of,
};
use opentelemetry::KeyValue;
use opentelemetry::trace::TraceContextExt as _;
use serde_json::{Value, json};
use telmoni::transport::{Answer, TransportError};
use telmoni::{
    Batching, Config, ContentMode, Hooks, ModelCallOptions, Reply, RetrievalOptions, Telmoni,
    ToolCallOptions, ToolCallRecord, TraceOptions, TraceRecord, Usage,
};

/// The fixture's normalization, as its README sets it out.
fn normalize(export: &Value) -> Result<Value, Failure> {
    let mut export = export.clone();
    let resource_spans = export
        .get_mut("resourceSpans")
        .and_then(Value::as_array_mut)
        .ok_or("resourceSpans")?;
    assert_eq!(resource_spans.len(), 1, "one resourceSpans");
    let resource_span = resource_spans.first_mut().ok_or("a resourceSpans")?;
    let scope_spans = resource_span
        .get_mut("scopeSpans")
        .and_then(Value::as_array_mut)
        .ok_or("scopeSpans")?;
    assert_eq!(scope_spans.len(), 1, "one scopeSpans");
    let scope_span = scope_spans.first_mut().ok_or("a scopeSpans")?;
    let spans = scope_span
        .get_mut("spans")
        .and_then(Value::as_array_mut)
        .ok_or("spans")?;
    spans.sort_by(|a, b| name_of(a).cmp(name_of(b)));

    let trace_ids: HashSet<String> = spans
        .iter()
        .map(|span| text(span, "traceId").map(str::to_owned))
        .collect::<Result<_, _>>()?;
    assert_eq!(trace_ids.len(), 1, "one trace");
    assert!(trace_ids.iter().all(|id| is_hex(id, 32)), "{trace_ids:?}");
    let names: HashMap<String, String> = spans
        .iter()
        .map(|span| Ok((text(span, "spanId")?.to_owned(), name_of(span).to_owned())))
        .collect::<Result<_, Failure>>()?;
    assert!(names.keys().all(|id| is_hex(id, 16)), "{names:?}");

    for span in spans.iter_mut() {
        let name = name_of(span).to_owned();
        let span_id = text(span, "spanId")?.to_owned();
        let parent = span
            .get("parentSpanId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let start: u128 = text(span, "startTimeUnixNano")?.parse()?;
        let end: u128 = text(span, "endTimeUnixNano")?.parse()?;
        assert!(end >= start, "{name} ends before it starts");
        let object = span.as_object_mut().ok_or("a span object")?;
        object.insert("traceId".to_owned(), json!("<trace>"));
        let own = names.get(&span_id).ok_or("its own id")?;
        object.insert("spanId".to_owned(), json!(format!("<span:{own}>")));
        if let Some(parent) = parent {
            let parent_name = names
                .get(&parent)
                .ok_or_else(|| format!("{name} names a parent outside the export"))?;
            object.insert(
                "parentSpanId".to_owned(),
                json!(format!("<span:{parent_name}>")),
            );
        }
        object.insert("startTimeUnixNano".to_owned(), json!("<time>"));
        object.insert("endTimeUnixNano".to_owned(), json!("<time>"));
        let attributes = object
            .get_mut("attributes")
            .and_then(Value::as_array_mut)
            .ok_or("attributes")?;
        attributes.sort_by(|a, b| key_of(a).cmp(key_of(b)));
    }

    scope_span
        .get_mut("scope")
        .and_then(Value::as_object_mut)
        .ok_or("scope")?
        .insert("version".to_owned(), json!("<version>"));
    let resource_attributes = resource_span
        .get_mut("resource")
        .and_then(|resource| resource.get_mut("attributes"))
        .and_then(Value::as_array_mut)
        .ok_or("resource attributes")?;
    resource_attributes.sort_by(|a, b| key_of(a).cmp(key_of(b)));
    for attribute in resource_attributes.iter_mut() {
        let replacement = match key_of(attribute) {
            "telemetry.sdk.language" => "<language>",
            "telemetry.sdk.version" | "telemetry.distro.version" => "<version>",
            _ => continue,
        };
        attribute
            .get_mut("value")
            .and_then(Value::as_object_mut)
            .ok_or("a value")?
            .insert("stringValue".to_owned(), json!(replacement));
    }
    Ok(export)
}

fn key_of(attribute: &Value) -> &str {
    attribute.get("key").and_then(Value::as_str).unwrap_or("")
}

/// The scenario, through the API, as the README describes it.
fn perform(harness: &Harness, scenario: &Value) -> Outcome {
    let trace_spec = field(scenario, "trace")?;
    let tags = field(trace_spec, "tags")?
        .as_object()
        .ok_or("tags")?
        .iter()
        .map(|(key, value)| Ok((key.clone(), value.as_str().ok_or("a tag")?.to_owned())))
        .collect::<Result<BTreeMap<_, _>, Failure>>()?;
    let trace = harness.telmoni.trace(
        text(trace_spec, "agent")?,
        TraceOptions {
            user_id: Some(text(trace_spec, "user_id")?.to_owned()),
            session_id: Some(text(trace_spec, "session_id")?.to_owned()),
            tags,
            ..TraceOptions::default()
        },
    )?;

    let inside = field(trace_spec, "inside")?.as_array().ok_or("inside")?;
    let step_spec = inside.first().ok_or("a step")?;
    let step = trace.step(text(step_spec, "step")?)?;
    let call_spec = field(
        field(step_spec, "inside")?
            .as_array()
            .and_then(|inside| inside.first())
            .ok_or("a model call")?,
        "model_call",
    )?;
    let call = step.model_call(
        text(call_spec, "model")?,
        ModelCallOptions {
            provider: Some(text(call_spec, "provider")?.to_owned()),
            prompt: Some(text(call_spec, "prompt")?.to_owned()),
            prompt_version: Some(text(call_spec, "prompt_version")?.to_owned()),
            input_messages: Some(field(call_spec, "input_messages")?.clone().into()),
            ..ModelCallOptions::default()
        },
    )?;
    let usage = field(call_spec, "usage")?;
    call.answered(Reply {
        response_model: Some(text(call_spec, "response_model")?.to_owned()),
        usage: Some(Usage {
            input_tokens: Some(int(usage, "input_tokens")?),
            output_tokens: Some(int(usage, "output_tokens")?),
            cache_read_input_tokens: Some(int(usage, "cache_read_input_tokens")?),
            cache_write_input_tokens: Some(int(usage, "cache_write_input_tokens")?),
            reasoning_output_tokens: Some(int(usage, "reasoning_output_tokens")?),
            cost: Some(field(usage, "cost")?.as_f64().ok_or("cost")?),
        }),
        output_messages: Some(field(call_spec, "output_messages")?.clone().into()),
        time_to_first_token: Some(Duration::from_secs_f64(float(
            call_spec,
            "time_to_first_token",
        )?)),
    });
    call.end();
    step.end();

    let tool_spec = field(inside.get(1).ok_or("a tool call")?, "tool_call")?;
    let tool = trace.tool_call(text(tool_spec, "name")?, ToolCallOptions::default())?;
    tool.fail(text(tool_spec, "fails_with")?);
    tool.end();
    trace.end();
    harness.flush()
}

#[test]
fn the_trace_matches_the_golden_export() -> Outcome {
    let fixture = load_fixture("trace.json")?;
    let scenario = field(&fixture, "scenario")?;
    let harness = Harness::with(Config {
        capture_content: field(scenario, "capture_content")?
            .as_bool()
            .ok_or("capture_content")?,
        environment: Some(text(scenario, "environment")?.to_owned()),
        release: Some(text(scenario, "release")?.to_owned()),
        ..keyed()
    })?;
    perform(&harness, scenario)?;

    let bodies = harness.transport.bodies()?;
    let mut gathered = bodies.first().ok_or("the trace was not sent")?.clone();
    let every_span: Vec<Value> = bodies.iter().flat_map(spans_of).collect();
    *gathered
        .get_mut("resourceSpans")
        .and_then(Value::as_array_mut)
        .and_then(|resource_spans| resource_spans.first_mut())
        .and_then(|resource_spans| resource_spans.get_mut("scopeSpans"))
        .and_then(Value::as_array_mut)
        .and_then(|scope_spans| scope_spans.first_mut())
        .and_then(|scope_spans| scope_spans.get_mut("spans"))
        .ok_or("the first body's spans")? = json!(every_span);

    let normalized = normalize(&gathered)?;
    let expected = field(&fixture, "export")?;
    assert!(
        normalized == *expected,
        "sent:\n{}\nexpected:\n{}",
        serde_json::to_string_pretty(&normalized)?,
        serde_json::to_string_pretty(expected)?
    );
    for request in harness.transport.requests() {
        assert_eq!(
            request.header("Authorization"),
            Some("Bearer telmoni_test_key")
        );
        assert_eq!(request.header("Content-Type"), Some("application/json"));
        assert!(
            request
                .header("User-Agent")
                .is_some_and(|agent| agent.starts_with("telmoni-rust/"))
        );
        assert!(
            request
                .header("Telmoni-Sent-At")
                .is_some_and(|at| !at.is_empty() && at.bytes().all(|b| b.is_ascii_digit()))
        );
        assert!(request.url.ends_with("/otlp/v1/traces"));
    }
    Ok(())
}

#[test]
fn the_retrievals_match_the_golden_export() -> Outcome {
    let fixture = load_fixture("retrieval.json")?;
    let spec = fixture
        .pointer("/scenario/trace")
        .ok_or("the scenario's trace")?;
    for case in field(&fixture, "cases")?.as_array().ok_or("cases")? {
        let mode = Answer::new(
            200,
            vec![(
                "Telmoni-Content-Mode".to_owned(),
                text(case, "content_mode")?.to_owned(),
            )],
            b"{}".to_vec(),
        );
        let harness = Harness::answering(
            Config {
                capture_content: field(case, "capture_content")?
                    .as_bool()
                    .ok_or("capture_content")?,
                ..keyed()
            },
            std::iter::repeat_n(Ok(mode), 4),
        )?;
        let trace = harness.telmoni.trace(
            text(spec, "agent")?,
            TraceOptions {
                user_id: Some(text(spec, "user_id")?.to_owned()),
                session_id: Some(text(spec, "session_id")?.to_owned()),
                ..TraceOptions::default()
            },
        )?;
        for item in field(spec, "inside")?.as_array().ok_or("inside")? {
            let given = field(item, "retrieval")?;
            let found = trace.retrieval(
                text(given, "source")?,
                RetrievalOptions {
                    query: Some(text(given, "query")?.into()),
                    top_k: Some(u32::try_from(int(given, "top_k")?)?),
                    memory: given.get("memory").and_then(Value::as_bool) == Some(true),
                },
            )?;
            match given.get("documents").and_then(Value::as_array) {
                Some(documents) => found.returned(documents),
                None => found.fail(text(given, "fails_with")?),
            }
            found.end();
        }
        trace.end();
        harness.flush()?;

        // The ask before the first content holds no spans.
        let bodies: Vec<Value> = harness
            .transport
            .bodies()?
            .into_iter()
            .filter(|body| !spans_of(body).is_empty())
            .collect();
        let mut gathered = bodies.first().ok_or("the trace was not sent")?.clone();
        let every_span: Vec<Value> = bodies.iter().flat_map(spans_of).collect();
        *gathered
            .get_mut("resourceSpans")
            .and_then(Value::as_array_mut)
            .and_then(|resource_spans| resource_spans.first_mut())
            .and_then(|resource_spans| resource_spans.get_mut("scopeSpans"))
            .and_then(Value::as_array_mut)
            .and_then(|scope_spans| scope_spans.first_mut())
            .and_then(|scope_spans| scope_spans.get_mut("spans"))
            .ok_or("the first body's spans")? = json!(every_span);
        let normalized = normalize(&gathered)?;
        let expected = field(&fixture, text(case, "sends")?)?;
        assert!(
            normalized == *expected,
            "sent:\n{}\nexpected:\n{}",
            serde_json::to_string_pretty(&normalized)?,
            serde_json::to_string_pretty(expected)?
        );
    }
    Ok(())
}

#[test]
fn a_trace_is_sent_from_its_start() -> Outcome {
    let fixture = load_fixture("trace-start.json")?;
    let scenario = field(&fixture, "scenario")?;
    let spec = field(scenario, "trace")?;
    let agent = text(spec, "agent")?;
    let transport = FakeTransport::new([])?;
    let mut hooks = hooks(&transport, &Warnings::default(), &Sleeps::default());
    hooks.batching = Batching {
        schedule_delay: Duration::from_millis(
            field(scenario, "schedule_delay_ms")?
                .as_u64()
                .ok_or("schedule_delay_ms")?,
        ),
        ..Batching::default()
    };
    let telmoni = Telmoni::with_hooks(
        Config {
            environment: Some(text(scenario, "environment")?.to_owned()),
            release: Some(text(scenario, "release")?.to_owned()),
            ..keyed()
        },
        hooks,
    );
    let tags = field(spec, "tags")?
        .as_object()
        .ok_or("tags")?
        .iter()
        .map(|(key, value)| Ok((key.clone(), value.as_str().ok_or("a tag")?.to_owned())))
        .collect::<Result<BTreeMap<_, _>, Failure>>()?;
    let trace = telmoni.trace(
        agent,
        TraceOptions {
            user_id: Some(text(spec, "user_id")?.to_owned()),
            session_id: Some(text(spec, "session_id")?.to_owned()),
            tags,
            ..TraceOptions::default()
        },
    )?;
    let within = Duration::from_millis(
        field(scenario, "sent_within_ms")?
            .as_u64()
            .ok_or("sent_within_ms")?,
    );
    let deadline = Instant::now() + within;
    while transport.count() == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    let first = transport.bodies()?;
    assert_eq!(first.len(), 1, "one body, before the trace ends");
    let mut body = first.first().ok_or("the opening was not sent")?.clone();
    let [start] = spans_of(&body).try_into().map_err(|_| "one span")?;
    assert_eq!(
        text(&start, "startTimeUnixNano")?,
        text(&start, "endTimeUnixNano")?,
        "of no length"
    );
    let parent = text(&start, "parentSpanId")?.to_owned();
    let spans = "/resourceSpans/0/scopeSpans/0/spans/0";
    // Its parent is not in the export, which normalization would refuse.
    body.pointer_mut(spans)
        .and_then(Value::as_object_mut)
        .ok_or("the one span")?
        .remove("parentSpanId");
    let mut normalized = normalize(&body)?;
    normalized
        .pointer_mut(spans)
        .and_then(Value::as_object_mut)
        .ok_or("the one span")?
        .insert("parentSpanId".to_owned(), json!("<span:agent>"));
    let expected = field(&fixture, "first_export")?;
    assert!(
        normalized == *expected,
        "sent:\n{}\nexpected:\n{}",
        serde_json::to_string_pretty(&normalized)?,
        serde_json::to_string_pretty(expected)?
    );

    trace.end();
    assert!(telmoni.flush(), "the flush timed out");
    let name = format!("invoke_agent {agent}");
    let agents: Vec<Value> = transport
        .spans()?
        .into_iter()
        .filter(|span| name_of(span) == name)
        .collect();
    let [own] = agents.try_into().map_err(|_| "one agent span")?;
    assert_eq!(text(&own, "spanId")?, parent);
    assert_eq!(
        text(&own, "startTimeUnixNano")?,
        text(&start, "startTimeUnixNano")?
    );
    assert_eq!(text(&own, "traceId")?, text(&start, "traceId")?);
    telmoni.shutdown();
    Ok(())
}

/// `failure.json`'s error, its type named as Rust names one.
#[derive(Debug)]
struct RateLimited(String);

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<telmoni::Error> for RateLimited {
    fn from(error: telmoni::Error) -> Self {
        Self(error.to_string())
    }
}

/// The agent span as `failure.json`'s README normalizes it.
fn normalize_failure(span: &Value) -> Result<Value, Failure> {
    let error_type = attributes_of(span)?
        .get("error.type")
        .cloned()
        .ok_or("error.type")?;
    let mut sent = serde_json::Map::new();
    sent.insert("status".to_owned(), field(span, "status")?.clone());
    let Some(events) = span.get("events") else {
        return Ok(Value::Object(sent));
    };
    let start: u64 = text(span, "startTimeUnixNano")?.parse()?;
    let end: u64 = text(span, "endTimeUnixNano")?.parse()?;
    let mut events = events.clone();
    for event in events.as_array_mut().ok_or("events")? {
        let at: u64 = text(event, "timeUnixNano")?.parse()?;
        assert!((start..=end).contains(&at), "an event outside its span");
        event["timeUnixNano"] = json!("<time>");
        let attributes = event
            .get_mut("attributes")
            .and_then(Value::as_array_mut)
            .ok_or("an event's attributes")?;
        attributes.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
        for attribute in attributes.iter_mut() {
            let key = attribute.get("key").and_then(Value::as_str);
            assert_ne!(
                key,
                Some("exception.stacktrace"),
                "Rust sends no stack trace"
            );
            if key == Some("exception.type") {
                let value = attribute
                    .pointer_mut("/value/stringValue")
                    .ok_or("exception.type's value")?;
                assert_eq!(*value, error_type);
                *value = json!("<error.type>");
            }
        }
    }
    sent.insert("events".to_owned(), events);
    Ok(Value::Object(sent))
}

#[test]
fn a_failure_leaves_in_full_only_under_both_locks() -> Outcome {
    let fixture = load_fixture("failure.json")?;
    let scenario = field(&fixture, "scenario")?;
    let agent = text(scenario, "agent")?;
    let message = text(scenario, "message")?;
    let event = field(scenario, "event")?;
    let event_name = text(event, "name")?.to_owned();
    let event_attributes: Vec<KeyValue> = field(event, "attributes")?
        .as_object()
        .ok_or("the event's attributes")?
        .iter()
        .map(|(key, value)| match value {
            Value::Number(n) => KeyValue::new(key.clone(), n.as_i64().unwrap_or_default()),
            other => KeyValue::new(key.clone(), other.as_str().unwrap_or_default().to_owned()),
        })
        .collect();
    for case in field(&fixture, "cases")?.as_array().ok_or("cases")? {
        let mode = text(case, "content_mode")?;
        let harness = Harness::answering(
            Config {
                capture_content: field(case, "capture_content")?
                    .as_bool()
                    .ok_or("capture_content")?,
                ..keyed()
            },
            // The mode to each export, a trace's opening its own among them.
            std::iter::repeat_n(
                Ok(Answer::new(
                    200,
                    vec![("Telmoni-Content-Mode".to_owned(), mode.to_owned())],
                    b"{}".to_vec(),
                )),
                4,
            ),
        )?;
        harness.telmoni.trace(agent, TraceOptions::default())?.end();
        harness.flush()?;
        let result: Result<(), RateLimited> =
            harness
                .telmoni
                .try_trace_with(agent, TraceOptions::default(), |trace| {
                    trace
                        .context()
                        .span()
                        .add_event(event_name.clone(), event_attributes.clone());
                    Err(RateLimited(message.to_owned()))
                });
        assert!(result.is_err());
        harness.flush()?;
        let spans = harness.spans()?;
        let failed = spans.last().ok_or("the failed trace")?;
        let sends = text(case, "sends")?;
        assert_eq!(
            &normalize_failure(failed)?,
            field(&fixture, sends)?,
            "{case}"
        );
        if sends == "withheld" {
            let sent = harness.transport.text();
            assert!(
                !sent.contains(message) && !sent.contains(&event_name),
                "{case}"
            );
        }
    }
    Ok(())
}

/// Base64, padded: what `media.json` spells a file's bytes in.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |word, (i, byte)| {
            word | u32::from(*byte) << (16 - 8 * i)
        });
        for i in 0..4 {
            if i <= chunk.len() {
                let at = usize::try_from(word >> (18 - 6 * i) & 63).unwrap_or(0);
                out.push(char::from(ALPHABET.get(at).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn the_mode_is_asked_before_the_first_content_it_would_send() -> Outcome {
    let fixture = load_fixture("ask.json")?;
    for case in field(&fixture, "cases")?.as_array().ok_or("cases")? {
        let flag = |name: &'static str| -> Result<bool, Failure> {
            Ok(field(case, name)?.as_bool().ok_or(name)?)
        };
        let answers = field(case, "answers")?
            .as_array()
            .ok_or("answers")?
            .iter()
            .map(|given| match given {
                Value::String(mode) => Ok(Ok(Answer::new(
                    200,
                    vec![("Telmoni-Content-Mode".to_owned(), mode.clone())],
                    b"{}".to_vec(),
                ))),
                _ => {
                    let status = given.as_u64().ok_or("an answer")?;
                    Ok(Ok(Answer::new(
                        u16::try_from(status)?,
                        Vec::new(),
                        Vec::new(),
                    )))
                }
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        // Each trace's opening, waited for so it goes alone, is answered naming
        // no mode; the case's answers are the first trace's other exports.
        let neutral = || Ok(Answer::new(200, Vec::new(), b"{}".to_vec()));
        let answers = std::iter::once(neutral())
            .chain(answers)
            .chain(std::iter::once(neutral()));
        let harness = Harness::answering(
            Config {
                capture_content: flag("capture_content")?,
                api_key: Some("k".to_owned()),
                ..keyed()
            },
            answers,
        )?;
        for _ in 0..2 {
            let sent = harness.transport.count();
            let trace = harness
                .telmoni
                .trace(text(&fixture, "agent")?, TraceOptions::default())?;
            assert!(harness.transport.wait_for(sent + 1), "the trace's opening");
            let options = if flag("content")? {
                ModelCallOptions {
                    input_messages: Some(field(&fixture, "input_messages")?.clone().into()),
                    ..ModelCallOptions::default()
                }
            } else {
                ModelCallOptions::default()
            };
            trace.model_call(text(&fixture, "model")?, options)?.end();
            trace.end();
            harness.flush()?;
        }
        let opening = |body: &Value| {
            let spans = spans_of(body);
            !spans.is_empty()
                && spans
                    .iter()
                    .all(|span| name_of(span) == "telmoni.trace.start")
        };
        let (openings, bodies): (Vec<Value>, Vec<Value>) =
            harness.transport.bodies()?.into_iter().partition(opening);
        assert_eq!(openings.len(), 2, "{case}");
        assert_eq!(
            Some(u64::try_from(bodies.len())?),
            field(case, "requests")?.as_u64(),
            "{case}"
        );
        let (asks, exports): (Vec<&Value>, Vec<&Value>) =
            bodies.iter().partition(|body| spans_of(body).is_empty());
        if flag("asked")? {
            assert_eq!(asks, [field(&fixture, "body")?], "{case}");
            assert_eq!(bodies.first(), Some(field(&fixture, "body")?), "{case}");
            let requests = harness.transport.requests();
            let ask = requests
                .iter()
                .find(|request| {
                    serde_json::from_slice::<Value>(&request.body).is_ok_and(|body| !opening(&body))
                })
                .ok_or("the ask")?;
            assert_eq!(ask.header("Authorization"), Some("Bearer k"), "{case}");
        } else {
            assert!(asks.is_empty(), "{case}");
        }
        let first = exports.first().ok_or("the first trace's export")?;
        let spans = spans_of(first);
        let call = spans
            .iter()
            .find(|span| name_of(span).starts_with("chat "))
            .ok_or("its model call")?;
        let sent = attributes_of(call)?.get("gen_ai.input.messages").is_some();
        assert_eq!(sent, flag("sent")?, "{case}");
    }
    Ok(())
}

#[test]
fn images_are_lifted_out_of_messages_only_under_both_locks() -> Outcome {
    let fixture = load_fixture("media.json")?;
    let scenario = field(&fixture, "scenario")?;
    let sent_fields = field(&fixture, "sent")?.as_object().ok_or("sent")?;
    for case in field(&fixture, "cases")?.as_array().ok_or("cases")? {
        let mode = Answer::new(
            200,
            vec![(
                "Telmoni-Content-Mode".to_owned(),
                text(case, "content_mode")?.to_owned(),
            )],
            b"{}".to_vec(),
        );
        let harness = Harness::answering(
            Config {
                capture_content: field(case, "capture_content")?
                    .as_bool()
                    .ok_or("capture_content")?,
                api_key: Some("k".to_owned()),
                ..keyed()
            },
            // The mode to each export, a trace's opening its own among them.
            std::iter::repeat_n(Ok(mode), 6),
        )?;
        harness.telmoni.trace("bot", TraceOptions::default())?.end();
        harness.flush()?;
        // Twice, in two exports: each file is put once.
        for _ in 0..2 {
            let trace = harness
                .telmoni
                .trace(text(scenario, "agent")?, TraceOptions::default())?;
            let call = trace.model_call(
                text(scenario, "model")?,
                ModelCallOptions {
                    provider: Some(text(scenario, "provider")?.to_owned()),
                    input_messages: Some(field(scenario, "input_messages")?.clone().into()),
                    system_instructions: Some(
                        field(scenario, "system_instructions")?.clone().into(),
                    ),
                    ..ModelCallOptions::default()
                },
            )?;
            call.answered(Reply {
                output_messages: Some(field(scenario, "output_messages")?.clone().into()),
                ..Reply::default()
            });
            call.end();
            trace.end();
            harness.flush()?;
        }
        let spans = harness.spans()?;
        let calls: Vec<&Value> = spans
            .iter()
            .filter(|span| name_of(span).starts_with("chat "))
            .collect();
        assert_eq!(calls.len(), 2, "{case}");
        let mut uploads = harness.transport.uploads();
        if field(case, "sends")? == &json!(false) {
            assert!(uploads.is_empty(), "{case}");
            for call in calls {
                let attributes = attributes_of(call)?;
                assert!(
                    sent_fields.keys().all(|key| attributes.get(key).is_none()),
                    "{case}"
                );
            }
            continue;
        }
        uploads.sort_by(|a, b| a.url.cmp(&b.url));
        let put: Vec<Value> = uploads
            .iter()
            .map(|upload| {
                assert_eq!(upload.header("Authorization"), Some("Bearer k"));
                json!({
                    "path": upload.url.strip_prefix(common::ENDPOINT).unwrap_or(&upload.url),
                    "content_type": upload.header("Content-Type"),
                    "body": base64(&upload.body),
                })
            })
            .collect();
        assert_eq!(&Value::Array(put), field(&fixture, "uploads")?, "{case}");
        for call in calls {
            let attributes = attributes_of(call)?;
            let mut sent = serde_json::Map::new();
            for key in sent_fields.keys() {
                let given = attributes
                    .get(key)
                    .and_then(Value::as_str)
                    .ok_or("a field")?;
                sent.insert(key.clone(), serde_json::from_str(given)?);
            }
            assert_eq!(&Value::Object(sent), field(&fixture, "sent")?, "{case}");
        }
    }
    Ok(())
}

/// `mask.json`'s error, its type named as Rust names one.
#[derive(Debug)]
struct Declined(String);

impl From<telmoni::Error> for Declined {
    fn from(error: telmoni::Error) -> Self {
        Self(error.to_string())
    }
}

impl std::fmt::Display for Declined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[test]
fn the_mask_answers_each_content_field_only_under_both_locks() -> Outcome {
    let fixture = load_fixture("mask.json")?;
    let scenario = field(&fixture, "scenario")?;
    let agent = text(scenario, "agent")?;
    let event = field(scenario, "event")?;
    let event_name = text(event, "name")?.to_owned();
    let event_attributes: Vec<KeyValue> = field(event, "attributes")?
        .as_object()
        .ok_or("the event's attributes")?
        .iter()
        .map(|(key, value)| match value {
            Value::Number(n) => KeyValue::new(key.clone(), n.as_i64().unwrap_or_default()),
            other => KeyValue::new(key.clone(), other.as_str().unwrap_or_default().to_owned()),
        })
        .collect();
    let messages = [
        ("gen_ai.input.messages", "input_messages"),
        ("gen_ai.output.messages", "output_messages"),
        ("gen_ai.system_instructions", "system_instructions"),
    ];
    for case in field(&fixture, "cases")?.as_array().ok_or("cases")? {
        let handed: Collected<(String, String)> = Collected::default();
        // As `mask.json`'s rules say, keeping what it was handed.
        let rules = field(&fixture, "mask")?.clone();
        let kept = handed.clone();
        let mask: telmoni::Mask = Arc::new(move |key: &str, value: &str| {
            kept.push((key.to_owned(), value.to_owned()));
            let Some(rule) = rules.get(key) else {
                return Some(value.to_owned());
            };
            if rule.get("fail").is_some() {
                std::panic::panic_any(format!("the mask failed on {value}"));
            }
            if rule.get("drop").is_some() {
                return None;
            }
            rule.get("replace")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let mode = Answer::new(
            200,
            vec![(
                "Telmoni-Content-Mode".to_owned(),
                text(case, "content_mode")?.to_owned(),
            )],
            b"{}".to_vec(),
        );
        let harness = Harness::answering(
            Config {
                capture_content: field(case, "capture_content")?
                    .as_bool()
                    .ok_or("capture_content")?,
                mask: Some(mask),
                ..keyed()
            },
            std::iter::repeat_n(Ok(mode), 2),
        )?;
        a_trace(&harness)?;
        let options = TraceOptions {
            user_id: Some(text(scenario, "user")?.to_owned()),
            session_id: Some(text(scenario, "session")?.to_owned()),
            tags: serde_json::from_value(field(scenario, "tags")?.clone())?,
            ..TraceOptions::default()
        };
        let model = text(scenario, "model")?;
        let call_options = ModelCallOptions {
            provider: Some(text(scenario, "provider")?.to_owned()),
            input_messages: Some(field(scenario, "input_messages")?.clone().into()),
            system_instructions: Some(field(scenario, "system_instructions")?.clone().into()),
            ..ModelCallOptions::default()
        };
        let reply = Reply {
            output_messages: Some(field(scenario, "output_messages")?.clone().into()),
            ..Reply::default()
        };
        let message = text(scenario, "message")?;
        let result: Result<(), Declined> =
            harness.telmoni.try_trace_with(agent, options, |trace| {
                let call = trace.model_call(model, call_options)?;
                call.answered(reply);
                call.end();
                trace
                    .context()
                    .span()
                    .add_event(event_name.clone(), event_attributes.clone());
                Err(Declined(message.to_owned()))
            });
        assert!(result.is_err(), "{case}");
        harness.flush()?;
        let spans = harness.spans()?;
        let calls: Vec<&Value> = spans
            .iter()
            .filter(|span| name_of(span).starts_with("chat "))
            .collect();
        assert_eq!(calls.len(), 1, "{case}");
        assert!(harness.transport.uploads().is_empty(), "{case}");
        assert!(!harness.transport.text().contains("4242 "), "{case}");
        assert!(
            !harness.warnings.all().join("\n").contains("4242 "),
            "{case}"
        );
        let attributes = attributes_of(calls.first().ok_or("the call")?)?;
        let mut sent = serde_json::Map::new();
        for (key, _) in messages {
            if let Some(given) = attributes.get(key).and_then(Value::as_str) {
                sent.insert(key.to_owned(), serde_json::from_str(given)?);
            }
        }
        let failed = spans.last().ok_or("the failed trace")?;
        let handed: BTreeMap<String, String> = handed.all().into_iter().collect();
        if field(case, "masks")? == &json!(false) {
            assert!(handed.is_empty(), "{case}: handed {handed:?}");
            assert!(sent.is_empty(), "{case}");
            assert_eq!(
                &normalize_failure(failed)?,
                field(&fixture, "withheld")?,
                "{case}"
            );
            assert!(harness.warnings.all().is_empty(), "{case}");
            continue;
        }
        assert_eq!(&Value::Object(sent), field(&fixture, "sent")?, "{case}");
        assert_eq!(
            &normalize_failure(failed)?,
            field(&fixture, "kept")?,
            "{case}"
        );
        assert_eq!(
            json!(harness.warnings.all()),
            *field(&fixture, "warnings")?,
            "{case}"
        );
        assert_eq!(
            harness.warnings.fields(),
            vec![json!({"kind": "mask_failed"})],
            "{case}"
        );
        assert_eq!(
            json!(handed.keys().collect::<Vec<_>>()),
            *field(&fixture, "handed")?,
            "{case}"
        );
        for (key, value) in field(&fixture, "handed_text")?
            .as_object()
            .ok_or("handed_text")?
        {
            assert_eq!(handed.get(key).map(String::as_str), value.as_str(), "{key}");
        }
        // As the code gave them, blob parts and all: before any was put.
        for (key, given) in messages {
            let value: Value = serde_json::from_str(handed.get(key).ok_or(key)?)?;
            assert_eq!(&value, field(scenario, given)?, "{key}");
        }
    }
    Ok(())
}

#[test]
fn a_mask_that_panics_is_warned_of_once_and_asked_each_export() -> Outcome {
    let asked = Collected::default();
    let counted = asked.clone();
    let on = Answer::new(
        200,
        vec![("Telmoni-Content-Mode".to_owned(), "on".to_owned())],
        b"{}".to_vec(),
    );
    let harness = Harness::answering(
        Config {
            capture_content: true,
            mask: Some(Arc::new(move |key: &str, value: &str| {
                counted.push(key.to_owned());
                std::panic::panic_any(value.to_owned())
            })),
            ..keyed()
        },
        // On to each export, a trace's opening its own among them.
        std::iter::repeat_n(Ok(on), 5),
    )?;
    a_trace(&harness)?;
    for _ in 0..2 {
        let trace = harness
            .telmoni
            .trace("support-bot", TraceOptions::default())?;
        let call = trace.model_call(
            "gpt-4o",
            ModelCallOptions {
                input_messages: Some(json!("a secret").into()),
                ..ModelCallOptions::default()
            },
        )?;
        call.end();
        trace.end();
        harness.flush()?;
    }
    assert_eq!(asked.all(), vec!["gen_ai.input.messages"; 2]);
    assert_eq!(
        harness.warnings.all(),
        vec!["telmoni: the mask failed on gen_ai.input.messages; the span was sent without it"]
    );
    assert!(!harness.transport.text().contains("a secret"));
    Ok(())
}

#[test]
fn a_mask_answering_a_value_unchanged_keeps_its_type() -> Outcome {
    let on = Answer::new(
        200,
        vec![("Telmoni-Content-Mode".to_owned(), "on".to_owned())],
        b"{}".to_vec(),
    );
    let harness = Harness::answering(
        Config {
            capture_content: true,
            mask: Some(Arc::new(|_: &str, value: &str| {
                Some(if value == "true" { "yes" } else { value }.to_owned())
            })),
            ..keyed()
        },
        std::iter::repeat_n(Ok(on), 2),
    )?;
    a_trace(&harness)?;
    let trace = harness
        .telmoni
        .trace("support-bot", TraceOptions::default())?;
    trace.context().span().add_event(
        "checked",
        vec![KeyValue::new("count", 3_i64), KeyValue::new("found", true)],
    );
    trace.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let mut attributes = spans
        .last()
        .and_then(|span| span.pointer("/events/0/attributes"))
        .and_then(Value::as_array)
        .ok_or("the event's attributes")?
        .clone();
    attributes.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    assert_eq!(
        Value::Array(attributes),
        json!([
            {"key": "count", "value": {"intValue": "3"}},
            {"key": "found", "value": {"stringValue": "yes"}},
        ])
    );
    Ok(())
}

#[test]
fn a_configuration_says_whether_a_mask_is_set_and_nothing_of_it() {
    let config = Config {
        mask: Some(Arc::new(|_: &str, value: &str| Some(value.to_owned()))),
        ..keyed()
    };
    assert!(format!("{config:?}").contains(r#"mask: Some("<set>")"#));
    assert!(format!("{:?}", keyed()).contains("mask: None"));
}

/// Content leaving, live or recorded, holds the canary's markers, counted in `telmoni.scrubbed`.
#[test]
fn content_leaves_with_the_canary_values_scrubbed_and_counted() -> Outcome {
    let fixture = load_fixture("canary.json")?;
    let cases = field(&fixture, "cases")?.as_array().ok_or("cases")?;
    let prose = cases.first().ok_or("the prose case")?;
    let arguments = cases.get(2).ok_or("the arguments case")?;
    let expected = |case: &Value| -> Result<(Value, Value), Failure> {
        let mode = field(field(case, "expect")?, "plugin_default")?;
        Ok((
            field(mode, "text")?.clone(),
            field(mode, "scrubbed")?.clone(),
        ))
    };
    let harness = Harness::answering(
        Config {
            capture_content: true,
            scrub_secrets: Some(true),
            scrub_pii: Some(true),
            ..keyed()
        },
        std::iter::repeat_n(Ok(common::mode_on()), 3),
    )?;
    let message =
        json!([{ "role": "user", "parts": [{ "type": "text", "content": text(prose, "text")? }] }]);
    let trace = harness
        .telmoni
        .trace("support-bot", TraceOptions::default())?;
    let call = trace.model_call(
        "gpt-4o",
        ModelCallOptions {
            input_messages: Some(message.into()),
            ..ModelCallOptions::default()
        },
    )?;
    call.end();
    trace.end();
    let at = std::time::SystemTime::now();
    harness.telmoni.record_trace(TraceRecord {
        inside: vec![
            ToolCallRecord {
                arguments: Some(text(arguments, "text")?.into()),
                ..ToolCallRecord::new("lookup", at, at)
            }
            .into(),
        ],
        ..TraceRecord::new("3d6f8a1c5b2e4f7a9c0d8e1f2a3b4c5d", "support-bot", at, at)
    })?;
    harness.flush()?;

    let spans = harness.spans()?;
    // Compared as JSON, as every SDK's serializer writes it.
    let scrubbed = |span: &Value| -> Result<Value, Failure> {
        Ok(serde_json::from_str(text(
            &attributes_of(span)?,
            "telmoni.scrubbed",
        )?)?)
    };
    let call = common::only(&spans, "chat ")?;
    let messages: Value =
        serde_json::from_str(text(&attributes_of(call)?, "gen_ai.input.messages")?)?;
    let (want, counts) = expected(prose)?;
    assert_eq!(messages.pointer("/0/parts/0/content"), Some(&want));
    assert_eq!(scrubbed(call)?, counts);
    let tool = common::only(&spans, "execute_tool ")?;
    let (want, counts) = expected(arguments)?;
    assert_eq!(
        attributes_of(tool)?.get("gen_ai.tool.call.arguments"),
        Some(&want)
    );
    assert_eq!(scrubbed(tool)?, counts);
    for agent in common::starting_with(&spans, "invoke_agent ") {
        assert!(attributes_of(agent)?.get("telmoni.scrubbed").is_none());
    }
    let sent = harness.transport.text();
    for (name, planted) in field(&fixture, "planted")?.as_object().ok_or("planted")? {
        assert!(
            !sent.contains(planted.as_str().ok_or(name.as_str())?),
            "{name}"
        );
    }
    Ok(())
}

/// In nanoseconds, a retry its own: ingest moves a trace by it.
#[test]
fn each_attempt_carries_this_clock_as_it_leaves() -> Outcome {
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos())
    };
    let transport = common::FakeTransport::new([Ok(answer("unavailable")?)])?;
    let telmoni = Telmoni::with_hooks(
        keyed(),
        Hooks {
            transport: Box::new(transport.clone()),
            sleep: Box::new(|_| thread::sleep(Duration::from_millis(5))),
            warn: Box::new(|_, _| {}),
            ..Hooks::default()
        },
    );
    let before = now();
    telmoni.trace("bot", TraceOptions::default())?.end();
    telmoni.shutdown();
    let after = now();
    let requests = transport.requests();
    let sent = requests
        .iter()
        .map(|request| {
            request
                .header("Telmoni-Sent-At")
                .and_then(|at| at.parse::<u128>().ok())
                .ok_or("a Telmoni-Sent-At in nanoseconds")
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        sent.iter().all(|at| (before..=after).contains(at)),
        "{before} {sent:?} {after}"
    );
    let (first, again) = (requests.first(), requests.get(1));
    assert!(
        first
            .zip(again)
            .is_some_and(|(first, again)| first.body == again.body),
        "the same export, sent again"
    );
    assert!(sent.get(1) > sent.first(), "{sent:?}");
    Ok(())
}

/// One empty trace, recorded, so it goes in exactly one export: a live
/// trace's opening may go in one of its own.
fn a_trace(harness: &Harness) -> Outcome {
    let at = std::time::SystemTime::now();
    harness.telmoni.record_trace(TraceRecord::new(
        "3d6f8a1c5b2e4f7a9c0d8e1f2a3b4c5d",
        "bot",
        at,
        at,
    ))?;
    harness.flush()
}

#[test]
fn kept_logs_nothing_and_sets_the_mode() -> Outcome {
    let harness = Harness::answering(keyed(), [Ok(answer("kept")?)])?;
    assert_eq!(harness.telmoni.content_mode(), None);
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), Vec::<String>::new());
    assert_eq!(harness.telmoni.content_mode(), Some(ContentMode::Off));
    assert!(harness.sleeps.all().is_empty());
    Ok(())
}

#[test]
fn dropped_is_warned_once() -> Outcome {
    let harness = Harness::answering(keyed(), std::iter::repeat_n(Ok(answer("dropped")?), 2))?;
    a_trace(&harness)?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("dropped")?);
    assert_eq!(harness.warnings.fields(), fields_of("dropped")?);
    Ok(())
}

#[test]
fn unlisted_names_the_tag_that_would_keep_it() -> Outcome {
    let harness = Harness::answering(keyed(), [Ok(answer("unlisted")?)])?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("unlisted")?);
    assert_eq!(harness.warnings.fields(), fields_of("unlisted")?);
    Ok(())
}

#[test]
fn a_tag_past_the_cardinality_cap_is_warned_with_what_keeps_it() -> Outcome {
    let harness = Harness::answering(keyed(), [Ok(answer("cardinality")?)])?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("cardinality")?);
    assert_eq!(harness.warnings.fields(), fields_of("cardinality")?);
    Ok(())
}

#[test]
fn refused_is_warned_once_and_never_retried() -> Outcome {
    let harness = Harness::answering(keyed(), std::iter::repeat_n(Ok(answer("refused")?), 2))?;
    a_trace(&harness)?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("refused")?);
    assert_eq!(harness.warnings.fields(), fields_of("refused")?);
    assert_eq!(
        harness.transport.count(),
        2,
        "one export each trace, neither sent again"
    );
    Ok(())
}

#[test]
fn rate_limited_waits_retry_after_and_sends_the_same_bytes() -> Outcome {
    let harness = Harness::answering(keyed(), [Ok(answer("rate-limited")?), Ok(answer("kept")?)])?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.sleeps.all(), [Duration::from_secs(2)]);
    let requests = harness.transport.requests();
    assert_eq!(
        requests.first().map(|request| &request.body),
        requests.get(1).map(|request| &request.body),
        "the same bytes again"
    );
    assert_eq!(harness.warnings.all(), Vec::<String>::new());
    Ok(())
}

#[test]
fn a_retry_after_past_the_cap_waits_the_cap() -> Outcome {
    let harness = Harness::answering(
        keyed(),
        [Ok(answer("rate-limited-long")?), Ok(answer("kept")?)],
    )?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.sleeps.all(), [Duration::from_secs(60)]);
    let requests = harness.transport.requests();
    assert_eq!(
        requests.first().map(|request| &request.body),
        requests.get(1).map(|request| &request.body),
        "the same bytes again"
    );
    assert_eq!(harness.warnings.all(), Vec::<String>::new());
    Ok(())
}

/// `Retry-After` is read as text: ASCII digits, a sign allowed, below 0 none, past 60 the cap however long.
#[test]
fn retry_after_is_read_as_text() -> Outcome {
    let limited = |retry_after: &str| -> Result<Answer, TransportError> {
        Ok(Answer::new(
            429,
            vec![("Retry-After".to_owned(), retry_after.to_owned())],
            b"{}".to_vec(),
        ))
    };
    let harness = Harness::answering(
        keyed(),
        [
            limited("99999999999"),
            limited("1_0"),
            Ok(answer("kept")?),
            limited("-5"),
            limited(" +7\t"),
            Ok(answer("kept")?),
            limited(&"9".repeat(400)),
            Ok(answer("kept")?),
        ],
    )?;
    for _ in 0..3 {
        a_trace(&harness)?;
    }
    harness.telmoni.shutdown();
    let seconds = Duration::from_secs;
    assert_eq!(
        harness.sleeps.all(),
        [seconds(60), seconds(2), seconds(0), seconds(7), seconds(60)]
    );
    assert_eq!(harness.transport.count(), 8);
    assert_eq!(harness.warnings.all(), Vec::<String>::new());
    Ok(())
}

#[test]
fn unavailable_is_retried_three_times_then_dropped() -> Outcome {
    let unavailable = answer("unavailable")?;
    let harness = Harness::answering(
        keyed(),
        [
            Ok(unavailable.clone()),
            Ok(unavailable.clone()),
            Ok(unavailable),
            Ok(answer("kept")?),
        ],
    )?;
    a_trace(&harness)?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(
        harness.sleeps.all(),
        [Duration::from_secs(1), Duration::from_secs(1)]
    );
    assert_eq!(harness.warnings.all(), warning_of("unavailable")?);
    assert_eq!(harness.warnings.fields(), fields_of("unavailable")?);
    assert_eq!(
        harness.transport.count(),
        4,
        "three for the first trace, then the next trace's own export"
    );
    Ok(())
}

#[test]
fn forbidden_is_not_retried_and_the_next_batch_is_still_sent() -> Outcome {
    let forbidden = answer("forbidden")?;
    let harness = Harness::answering(keyed(), [Ok(forbidden.clone()), Ok(forbidden)])?;
    a_trace(&harness)?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("forbidden")?);
    assert_eq!(harness.warnings.fields(), fields_of("forbidden")?);
    assert_eq!(harness.transport.count(), 2);
    Ok(())
}

/// Each answer met on every attempt logs its fixture's line word for word, as the plugins read it.
#[test]
fn each_answer_is_warned_word_for_word() -> Outcome {
    let names = answer_names()?;
    assert!(!names.is_empty(), "no answers");
    for name in names {
        let harness = Harness::answering(keyed(), std::iter::repeat_n(Ok(answer(&name)?), 3))?;
        a_trace(&harness)?;
        harness.telmoni.shutdown();
        assert_eq!(harness.warnings.all(), warning_of(&name)?, "{name}");
        assert_eq!(harness.warnings.fields(), fields_of(&name)?, "{name}");
    }
    Ok(())
}

#[test]
fn a_network_failure_is_retried_with_backoff() -> Outcome {
    let harness = Harness::answering(
        keyed(),
        [
            Err(TransportError::new("ConnectionRefused")),
            Err(TransportError::new("ConnectionRefused")),
            Ok(answer("kept")?),
        ],
    )?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(
        harness.sleeps.all(),
        [Duration::from_secs(1), Duration::from_secs(2)]
    );
    assert_eq!(harness.warnings.all(), Vec::<String>::new());
    Ok(())
}

#[test]
fn a_platform_never_reached_is_warned_once_with_the_errors_name() -> Outcome {
    let harness = Harness::answering(
        keyed(),
        std::iter::repeat_n(Err(TransportError::new("ConnectionRefused")), 6),
    )?;
    a_trace(&harness)?;
    a_trace(&harness)?;
    harness.telmoni.shutdown();
    let warnings = harness.warnings.all();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(
        warnings.first().map(String::as_str),
        Some("telmoni: export failed after 3 attempts (ConnectionRefused); the batch was dropped")
    );
    assert_eq!(harness.warnings.fields(), [json!({ "kind": "failed" })]);
    assert_eq!(harness.transport.count(), 6);
    Ok(())
}

/// A refusal's problem, sent whole before its answer hangs up short of the
/// length it declared.
const PROBLEM: &str = r#"{"message":"/errors/batch-too-large: the batch is too large"}"#;

/// A loopback endpoint answering each export `status` and [`PROBLEM`], and
/// the bodies it was sent.
fn breaking(status: u16) -> Result<(String, Collected<Vec<u8>>), Failure> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let bodies = Collected::default();
    let sent = bodies.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            sent.push(body);
            let _ = write!(
                reader.into_inner(),
                "HTTP/1.1 {status} Answer\r\nContent-Length: 300\r\n\r\n{PROBLEM}"
            );
        }
    });
    Ok((endpoint, bodies))
}

/// The status decides an answer whose body breaks: a refusal is sent once, a 200 kept.
#[test]
fn an_answer_whose_body_breaks_is_decided_by_its_status() -> Outcome {
    for status in [413, 200] {
        let (endpoint, bodies) = breaking(status)?;
        let warnings = Warnings::default();
        let sleeps = Sleeps::default();
        let (warned, slept) = (warnings.clone(), sleeps.clone());
        let telmoni = Telmoni::with_hooks(
            Config::new(endpoint, None, Some("telmoni_k".to_owned())),
            Hooks {
                sleep: Box::new(move |duration| slept.push(duration)),
                warn: Box::new(move |text, warning| warned.push(text, warning)),
                ..Hooks::default()
            },
        );
        telmoni.trace("bot", TraceOptions::default())?.end();
        telmoni.shutdown();
        let sent = bodies.all();
        assert!(!sent.is_empty(), "{status}: nothing was sent");
        assert_eq!(
            sent.iter().collect::<HashSet<_>>().len(),
            sent.len(),
            "{status}: an export was sent again"
        );
        assert_eq!(sleeps.all(), Vec::<Duration>::new(), "{status}");
        let fields = warnings.fields();
        let warnings = warnings.all();
        if status == 200 {
            assert_eq!(warnings, Vec::<String>::new());
        } else {
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert_eq!(
                warnings.first().map(String::as_str),
                Some(
                    "telmoni: export refused with 413 /errors/batch-too-large; the batch was dropped"
                )
            );
            assert_eq!(
                fields,
                [
                    json!({ "kind": "refused", "status": 413, "problem_type": "/errors/batch-too-large" })
                ]
            );
        }
    }
    Ok(())
}
