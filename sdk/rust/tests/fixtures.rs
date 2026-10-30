//! The golden fixtures in `sdk/fixtures`: the export every SDK must send for
//! the same run, and what each must do with the server's answers.

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use common::{
    Collected, Failure, Harness, Outcome, Sleeps, Warnings, answer, answer_names, field, fields_of,
    float, int, is_hex, is_uuid_v4, keyed, load_fixture, name_of, spans_of, text, warning_of,
};
use serde_json::{Value, json};
use telmoni::transport::{Answer, TransportError};
use telmoni::{
    Config, ContentMode, Hooks, ModelCallOptions, Reply, RunOptions, Telmoni, ToolCallOptions,
    Usage,
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

    let mut run_ids = HashSet::new();
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
        for attribute in attributes.iter_mut() {
            if key_of(attribute) != "telmoni.run_id" {
                continue;
            }
            let value = attribute
                .get_mut("value")
                .and_then(Value::as_object_mut)
                .ok_or("a value")?;
            if let Some(Value::String(id)) = value.get("stringValue") {
                run_ids.insert(id.clone());
            }
            value.insert("stringValue".to_owned(), json!("<uuid>"));
        }
    }
    assert_eq!(run_ids.len(), 1, "one run: {run_ids:?}");
    assert!(run_ids.iter().all(|id| is_uuid_v4(id)), "{run_ids:?}");

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
    let run_spec = field(scenario, "run")?;
    let tags = field(run_spec, "tags")?
        .as_object()
        .ok_or("tags")?
        .iter()
        .map(|(key, value)| Ok((key.clone(), value.as_str().ok_or("a tag")?.to_owned())))
        .collect::<Result<BTreeMap<_, _>, Failure>>()?;
    let run = harness.telmoni.run(
        text(run_spec, "agent")?,
        RunOptions {
            user_id: Some(text(run_spec, "user_id")?.to_owned()),
            session_id: Some(text(run_spec, "session_id")?.to_owned()),
            tags,
            sample_rate: Some(float(run_spec, "sample_rate")?),
            ..RunOptions::default()
        },
    )?;

    let inside = field(run_spec, "inside")?.as_array().ok_or("inside")?;
    let step_spec = inside.first().ok_or("a step")?;
    let step = run.step(text(step_spec, "step")?)?;
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
            cost: Some(field(usage, "cost")?.as_f64().ok_or("cost")?),
        }),
        output_messages: Some(field(call_spec, "output_messages")?.clone().into()),
    });
    call.end();
    step.end();

    let tool_spec = field(inside.get(1).ok_or("a tool call")?, "tool_call")?;
    let tool = run.tool_call(text(tool_spec, "name")?, ToolCallOptions::default())?;
    tool.fail(text(tool_spec, "fails_with")?);
    tool.end();
    run.end();
    harness.flush()
}

#[test]
fn the_run_matches_the_golden_export() -> Outcome {
    let fixture = load_fixture("run.json")?;
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
    let mut gathered = bodies.first().ok_or("the run was not sent")?.clone();
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

/// In nanoseconds, a retry its own: ingest moves a run by it.
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
    telmoni.run("bot", RunOptions::default())?.end();
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

/// One empty run, sent in an export of its own.
fn a_run(harness: &Harness) -> Outcome {
    harness.telmoni.run("bot", RunOptions::default())?.end();
    harness.flush()
}

#[test]
fn kept_logs_nothing_and_sets_the_mode() -> Outcome {
    let harness = Harness::answering(keyed(), [Ok(answer("kept")?)])?;
    assert_eq!(harness.telmoni.content_mode(), None);
    a_run(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), Vec::<String>::new());
    assert_eq!(harness.telmoni.content_mode(), Some(ContentMode::Off));
    assert!(harness.sleeps.all().is_empty());
    Ok(())
}

#[test]
fn dropped_is_warned_once() -> Outcome {
    let harness = Harness::answering(keyed(), std::iter::repeat_n(Ok(answer("dropped")?), 2))?;
    a_run(&harness)?;
    a_run(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("dropped")?);
    assert_eq!(harness.warnings.fields(), fields_of("dropped")?);
    Ok(())
}

#[test]
fn unlisted_names_the_tag_that_would_keep_it() -> Outcome {
    let harness = Harness::answering(keyed(), [Ok(answer("unlisted")?)])?;
    a_run(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("unlisted")?);
    assert_eq!(harness.warnings.fields(), fields_of("unlisted")?);
    Ok(())
}

#[test]
fn a_tag_past_the_cardinality_cap_is_warned_with_what_keeps_it() -> Outcome {
    let harness = Harness::answering(keyed(), [Ok(answer("cardinality")?)])?;
    a_run(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("cardinality")?);
    assert_eq!(harness.warnings.fields(), fields_of("cardinality")?);
    Ok(())
}

#[test]
fn refused_is_warned_once_and_never_retried() -> Outcome {
    let harness = Harness::answering(keyed(), std::iter::repeat_n(Ok(answer("refused")?), 2))?;
    a_run(&harness)?;
    a_run(&harness)?;
    harness.telmoni.shutdown();
    assert_eq!(harness.warnings.all(), warning_of("refused")?);
    assert_eq!(harness.warnings.fields(), fields_of("refused")?);
    assert_eq!(
        harness.transport.count(),
        2,
        "one export each run, neither sent again"
    );
    Ok(())
}

#[test]
fn rate_limited_waits_retry_after_and_sends_the_same_bytes() -> Outcome {
    let harness = Harness::answering(keyed(), [Ok(answer("rate-limited")?), Ok(answer("kept")?)])?;
    a_run(&harness)?;
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
    a_run(&harness)?;
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
        a_run(&harness)?;
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
    a_run(&harness)?;
    a_run(&harness)?;
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
        "three for the first run, then the next run's own export"
    );
    Ok(())
}

#[test]
fn forbidden_is_not_retried_and_the_next_batch_is_still_sent() -> Outcome {
    let forbidden = answer("forbidden")?;
    let harness = Harness::answering(keyed(), [Ok(forbidden.clone()), Ok(forbidden)])?;
    a_run(&harness)?;
    a_run(&harness)?;
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
        a_run(&harness)?;
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
    a_run(&harness)?;
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
    a_run(&harness)?;
    a_run(&harness)?;
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
        telmoni.run("bot", RunOptions::default())?.end();
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
