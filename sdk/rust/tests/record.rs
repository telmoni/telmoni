//! A run recorded after it ended: `record_run` and
//! `sdk/fixtures/recorded.json`.

mod common;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{
    Failure, FakeTransport, Harness, Outcome, Sleeps, Warnings, attributes_of, field, hooks, int,
    keyed, load_fixture, lock, name_of, run_span, text,
};
use opentelemetry::Context;
use opentelemetry::trace::TraceContextExt as _;
use serde_json::{Value, json};
use telmoni_sdk::{
    CallRecord, Config, Error, ModelCallRecord, RecordItem, RunOptions, RunRecord, StepRecord,
    Telmoni, ToolCallRecord, Usage,
};

/// A time as the fixture writes it, RFC 3339 in UTC to the millisecond:
/// `2026-10-12T09:00:00.100Z`.
fn at(time: &str) -> Result<SystemTime, Failure> {
    let parsed = (|| {
        let (date, clock) = time.strip_suffix('Z')?.split_once('T')?;
        let (clock, millis) = clock.split_once('.')?;
        let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
        let mut clock = clock.split(':').map(|part| part.parse::<i64>().ok());
        let (year, month, day) = (date.next()??, date.next()??, date.next()??);
        let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
        let seconds =
            days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
        Some(
            UNIX_EPOCH
                + Duration::from_secs(u64::try_from(seconds).ok()?)
                + Duration::from_millis(millis.parse().ok()?),
        )
    })();
    parsed.ok_or_else(|| format!("not a UTC time to the millisecond: {time}").into())
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar, as
/// Howard Hinnant's `days_from_civil` counts them.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `key` as a string, `None` when the spec leaves it out.
fn optional_text(spec: &Value, key: &str) -> Result<Option<String>, Failure> {
    spec.get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("`{key}` is not a string").into())
        })
        .transpose()
}

/// `key` as a flag, `false` when the spec leaves it out.
fn flag(spec: &Value, key: &str) -> Result<bool, Failure> {
    spec.get(key).map_or(Ok(false), |value| {
        value
            .as_bool()
            .ok_or_else(|| format!("`{key}` is not a boolean").into())
    })
}

fn call_from(spec: &Value) -> Result<CallRecord, Failure> {
    if let Some(call) = spec.get("model_call") {
        let usage = call
            .get("usage")
            .map(|usage| -> Result<Usage, Failure> {
                Ok(Usage {
                    input_tokens: Some(int(usage, "input_tokens")?),
                    output_tokens: Some(int(usage, "output_tokens")?),
                    cache_read_input_tokens: Some(int(usage, "cache_read_input_tokens")?),
                    cache_write_input_tokens: Some(int(usage, "cache_write_input_tokens")?),
                })
            })
            .transpose()?;
        return Ok(CallRecord::Model(ModelCallRecord {
            model: text(call, "model")?.to_owned(),
            provider: optional_text(call, "provider")?,
            operation: optional_text(call, "operation")?,
            start: at(text(call, "start")?)?,
            end: at(text(call, "end")?)?,
            usage,
            error_type: optional_text(call, "error_type")?,
            cancelled: flag(call, "cancelled")?,
        }));
    }
    let tool = field(spec, "tool_call")?;
    Ok(CallRecord::Tool(ToolCallRecord {
        name: text(tool, "name")?.to_owned(),
        start: at(text(tool, "start")?)?,
        end: at(text(tool, "end")?)?,
        error_type: optional_text(tool, "error_type")?,
        cancelled: flag(tool, "cancelled")?,
    }))
}

/// The scenario's record, field by field.
fn record_from(spec: &Value) -> Result<RunRecord, Failure> {
    let mut inside = Vec::new();
    for item in field(spec, "inside")?.as_array().ok_or("inside")? {
        let Some(step) = item.get("step") else {
            inside.push(call_from(item)?.into());
            continue;
        };
        let calls = step
            .get("calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(call_from)
            .collect::<Result<Vec<_>, _>>()?;
        inside.push(RecordItem::Step(StepRecord {
            name: text(step, "name")?.to_owned(),
            start: at(text(step, "start")?)?,
            end: at(text(step, "end")?)?,
            calls,
            error_type: optional_text(step, "error_type")?,
            cancelled: flag(step, "cancelled")?,
        }));
    }
    let tags = spec
        .get("tags")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| Ok((key.clone(), value.as_str().ok_or("a tag")?.to_owned())))
        .collect::<Result<BTreeMap<_, _>, Failure>>()?;
    Ok(RunRecord {
        run_id: text(spec, "run_id")?.to_owned(),
        agent: text(spec, "agent")?.to_owned(),
        user_id: optional_text(spec, "user_id")?,
        session_id: optional_text(spec, "session_id")?,
        tags,
        start: at(text(spec, "start")?)?,
        end: at(text(spec, "end")?)?,
        inside,
        error_type: optional_text(spec, "error_type")?,
        cancelled: flag(spec, "cancelled")?,
    })
}

fn the_record() -> Result<RunRecord, Failure> {
    record_from(field(
        field(&load_fixture("recorded.json")?, "scenario")?,
        "record",
    )?)
}

/// The fixture's normalization, as its README sets it out: every attribute
/// list sorted by key, the language and the versions replaced; nothing else.
fn normalize(body: &Value) -> Result<Value, Failure> {
    let mut body = body.clone();
    let resource_spans = body
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
    for span in scope_span
        .get_mut("spans")
        .and_then(Value::as_array_mut)
        .ok_or("spans")?
    {
        span.get_mut("attributes")
            .and_then(Value::as_array_mut)
            .ok_or("attributes")?
            .sort_by(|a, b| key_of(a).cmp(key_of(b)));
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
    Ok(body)
}

fn key_of(attribute: &Value) -> &str {
    attribute.get("key").and_then(Value::as_str).unwrap_or("")
}

/// The spans carrying `run_id`, in the order they were sent.
fn of_run<'a>(spans: &'a [Value], run_id: &str) -> Vec<&'a Value> {
    spans
        .iter()
        .filter(|span| {
            attributes_of(span)
                .ok()
                .and_then(|attributes| attributes.get("telmoni.run_id").cloned())
                == Some(json!(run_id))
        })
        .collect()
}

#[test]
fn the_record_matches_the_golden_export() -> Outcome {
    let fixture = load_fixture("recorded.json")?;
    let harness = Harness::new()?;
    harness
        .telmoni
        .record_run(record_from(field(field(&fixture, "scenario")?, "record")?)?)?;
    harness.flush()?;

    let bodies = harness.transport.bodies()?;
    assert_eq!(bodies.len(), 1, "a record travels in one export");
    let normalized = normalize(bodies.first().ok_or("a body")?)?;
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
        assert!(
            request
                .header("User-Agent")
                .is_some_and(|agent| agent.starts_with("telmoni-rust/"))
        );
    }
    Ok(())
}

#[test]
fn the_same_record_sent_again_is_the_same_bytes() -> Outcome {
    let record = the_record()?;
    let mut sent = Vec::new();
    // Again and again, since a worker that took a batch mid-record would split
    // it only now and then.
    for _ in 0..20 {
        let harness = Harness::new()?;
        harness.telmoni.record_run(record.clone())?;
        harness.flush()?;
        harness.telmoni.shutdown();
        let bodies: Vec<Vec<u8>> = harness
            .transport
            .requests()
            .into_iter()
            .map(|request| request.body)
            .collect();
        sent.push(bodies);
    }
    let first = sent.first().ok_or("a first send")?;
    assert_eq!(first.len(), 1, "a record travels in one export");
    assert!(sent.iter().all(|bodies| bodies == first));
    Ok(())
}

#[test]
fn a_record_takes_a_trace_of_its_own_and_leaves_the_context_alone() -> Outcome {
    let harness = Harness::new()?;
    let record = the_record()?;
    let live = harness.telmoni.run("live", RunOptions::default())?;
    let live_id = live.run_id().to_owned();
    let live_span = live.context().span().span_context().clone();
    {
        let _current = live.attach();
        // Written in capitals, the id is still the run's.
        harness.telmoni.record_run(RunRecord {
            run_id: record.run_id.to_uppercase(),
            ..record.clone()
        })?;
        assert_eq!(
            Context::current().span().span_context(),
            &live_span,
            "the live run is still the current span"
        );
    }
    live.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let recorded = of_run(&spans, &record.run_id);
    assert_eq!(recorded.len(), 9, "every span of the record, by its id");
    let trace = json!(record.run_id.replace('-', ""));
    assert!(
        recorded
            .iter()
            .all(|span| span.get("traceId") == Some(&trace)),
        "{recorded:?}"
    );
    assert!(
        run_span(&spans, &record.run_id)?
            .get("parentSpanId")
            .is_none(),
        "the record's run is the root of its trace"
    );
    assert_ne!(run_span(&spans, &live_id)?.get("traceId"), Some(&trace));
    Ok(())
}

#[test]
fn a_failed_run_is_marked_on_its_own_span() -> Outcome {
    let harness = Harness::new()?;
    let record = RunRecord {
        error_type: Some("RateLimitError".to_owned()),
        cancelled: false,
        inside: Vec::new(),
        ..the_record()?
    };
    harness.telmoni.record_run(record.clone())?;
    harness.flush()?;

    let spans = harness.spans()?;
    let run = run_span(&spans, &record.run_id)?;
    assert_eq!(run.get("status"), Some(&json!({ "code": 2 })));
    let attributes = attributes_of(run)?;
    assert_eq!(attributes.get("error.type"), Some(&json!("RateLimitError")));
    assert!(attributes.get("telmoni.cancelled").is_none());
    assert_eq!(
        spans.iter().map(name_of).collect::<Vec<_>>(),
        ["telmoni.run.start", "invoke_agent claude-code"]
    );
    Ok(())
}

#[test]
fn the_sample_rate_drops_a_record_whole() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let draws = Arc::new(Mutex::new(VecDeque::from([0.9, 0.1])));
    let source = Arc::clone(&draws);
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.random = Box::new(move || lock(&source).pop_front().unwrap_or(0.0));
    let telmoni = Telmoni::with_hooks(
        Config {
            sample_rate: 0.25,
            ..keyed()
        },
        hooks,
    );

    let dropped = the_record()?;
    telmoni.record_run(dropped.clone())?;
    assert!(telmoni.flush());
    assert_eq!(
        transport.count(),
        0,
        "a span of the record dropped was sent"
    );
    let kept = RunRecord {
        run_id: "0f8fad5b-d9cb-469f-a165-70867728950e".to_owned(),
        ..dropped
    };
    telmoni.record_run(kept.clone())?;
    assert!(telmoni.flush());
    telmoni.shutdown();

    let spans = transport.spans()?;
    assert_eq!(
        of_run(&spans, &kept.run_id).len(),
        spans.len(),
        "only the record kept"
    );
    assert_eq!(spans.len(), 9, "the record kept is sent whole");
    assert_eq!(
        attributes_of(run_span(&spans, &kept.run_id)?)?.get("telmoni.sample_rate"),
        Some(&json!(0.25))
    );
    assert!(lock(&draws).is_empty(), "one draw per record");
    Ok(())
}

#[test]
fn a_record_refused_sends_nothing() -> Outcome {
    let harness = Harness::new()?;
    let record = the_record()?;
    let start = record.start;
    let second = Duration::from_secs(1);
    let refused = [
        (
            RunRecord {
                agent: " ".to_owned(),
                ..record.clone()
            },
            Error::EmptyAgent,
        ),
        (
            RunRecord {
                run_id: "run-1".to_owned(),
                ..record.clone()
            },
            Error::RunId,
        ),
        (
            RunRecord {
                run_id: "00000000-0000-0000-0000-000000000000".to_owned(),
                ..record.clone()
            },
            Error::RunId,
        ),
        (
            RunRecord {
                end: start - second,
                ..record.clone()
            },
            Error::EndBeforeStart,
        ),
        (
            RunRecord {
                inside: vec![StepRecord::new("s", start + second, start).into()],
                ..record.clone()
            },
            Error::EndBeforeStart,
        ),
        (
            RunRecord {
                inside: vec![
                    StepRecord {
                        calls: vec![ToolCallRecord::new("t", start + second, start).into()],
                        ..StepRecord::new("s", start, start + 2 * second)
                    }
                    .into(),
                ],
                ..record.clone()
            },
            Error::EndBeforeStart,
        ),
    ];
    for (refused, error) in refused {
        assert_eq!(harness.telmoni.record_run(refused), Err(error));
    }
    harness.flush()?;
    assert_eq!(harness.transport.count(), 0, "nothing of a refused record");

    let misconfigured = Harness::with(Config {
        sample_rate: 0.0,
        ..keyed()
    })?;
    assert!(matches!(
        misconfigured.telmoni.record_run(record),
        Err(Error::SampleRate(_))
    ));
    misconfigured.flush()?;
    assert_eq!(misconfigured.transport.count(), 0);
    Ok(())
}
