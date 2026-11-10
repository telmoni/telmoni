//! A trace recorded after it ended: `record_trace` and
//! `sdk/fixtures/recorded.json`.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{
    Collected, Failure, FakeTransport, Harness, Outcome, Sleeps, Warnings, attributes_of, field,
    hooks, int, keyed, load_fixture, lock, mode_on, name_of, spans_of, text, trace_span,
};
use opentelemetry::trace::{Link, SpanKind, TraceContextExt as _, TraceId};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::{SamplingDecision, SamplingResult, ShouldSample};
use serde_json::{Value, json};
use telmoni::{
    Batching, Config, Content, Error, ModelCallRecord, RecordItem, StepRecord, Telmoni, Text,
    ToolCallRecord, TraceOptions, TraceRecord, Usage,
};
use uuid::Uuid;

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

fn item_from(spec: &Value) -> Result<RecordItem, Failure> {
    if let Some(step) = spec.get("step") {
        let calls = step
            .get("calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(item_from)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(RecordItem::Step(StepRecord {
            name: text(step, "name")?.to_owned(),
            input_messages: None,
            output_messages: None,
            start: at(text(step, "start")?)?,
            end: at(text(step, "end")?)?,
            calls,
            error_type: optional_text(step, "error_type")?,
            cancelled: flag(step, "cancelled")?,
            scores: Vec::new(),
        }));
    }
    if let Some(call) = spec.get("model_call") {
        let usage = call
            .get("usage")
            .map(|usage| -> Result<Usage, Failure> {
                Ok(Usage {
                    input_tokens: Some(int(usage, "input_tokens")?),
                    output_tokens: Some(int(usage, "output_tokens")?),
                    cache_read_input_tokens: Some(int(usage, "cache_read_input_tokens")?),
                    cache_write_input_tokens: Some(int(usage, "cache_write_input_tokens")?),
                    reasoning_output_tokens: Some(int(usage, "reasoning_output_tokens")?),
                    cost: usage.get("cost").and_then(Value::as_f64),
                })
            })
            .transpose()?;
        return Ok(RecordItem::ModelCall(ModelCallRecord {
            model: text(call, "model")?.to_owned(),
            provider: optional_text(call, "provider")?,
            operation: optional_text(call, "operation")?,
            reasoning_effort: optional_text(call, "reasoning_effort")?,
            start: at(text(call, "start")?)?,
            end: at(text(call, "end")?)?,
            usage,
            input_messages: call.get("input_messages").cloned().map(Content::from),
            output_messages: None,
            error_type: optional_text(call, "error_type")?,
            cancelled: flag(call, "cancelled")?,
            scores: Vec::new(),
        }));
    }
    let tool = field(spec, "tool_call")?;
    Ok(RecordItem::ToolCall(ToolCallRecord {
        name: text(tool, "name")?.to_owned(),
        arguments: None,
        result: None,
        start: at(text(tool, "start")?)?,
        end: at(text(tool, "end")?)?,
        error_type: optional_text(tool, "error_type")?,
        cancelled: flag(tool, "cancelled")?,
        scores: Vec::new(),
    }))
}

/// The scenario's record, field by field.
fn record_from(spec: &Value) -> Result<TraceRecord, Failure> {
    let inside = field(spec, "inside")?
        .as_array()
        .ok_or("inside")?
        .iter()
        .map(item_from)
        .collect::<Result<Vec<_>, _>>()?;
    let tags = spec
        .get("tags")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| Ok((key.clone(), value.as_str().ok_or("a tag")?.to_owned())))
        .collect::<Result<BTreeMap<_, _>, Failure>>()?;
    Ok(TraceRecord {
        trace_id: text(spec, "trace_id")?.to_owned(),
        agent: text(spec, "agent")?.to_owned(),
        user_id: optional_text(spec, "user_id")?,
        session_id: optional_text(spec, "session_id")?,
        tags,
        input_messages: None,
        output_messages: None,
        start: at(text(spec, "start")?)?,
        end: at(text(spec, "end")?)?,
        inside,
        error_type: optional_text(spec, "error_type")?,
        cancelled: flag(spec, "cancelled")?,
        scores: Vec::new(),
    })
}

fn the_record() -> Result<TraceRecord, Failure> {
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

/// The spans of the trace `trace_id`, in the order they were sent.
fn of_trace<'a>(spans: &'a [Value], trace_id: &str) -> Vec<&'a Value> {
    spans
        .iter()
        .filter(|span| span.get("traceId") == Some(&json!(trace_id)))
        .collect()
}

#[test]
fn the_record_matches_the_golden_export() -> Outcome {
    let fixture = load_fixture("recorded.json")?;
    let harness = Harness::new()?;
    harness
        .telmoni
        .record_trace(record_from(field(field(&fixture, "scenario")?, "record")?)?)?;
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
    // The record's one call of an operation ingest reads as no model call goes
    // as chat.
    assert_eq!(
        harness.warnings.all(),
        [
            "telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat"
        ]
    );
    assert_eq!(harness.warnings.kinds(), ["input"]);
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
        harness.telmoni.record_trace(record.clone())?;
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
    let live = harness.telmoni.trace("live", TraceOptions::default())?;
    let live_id = live.trace_id().to_owned();
    let live_span = live.context().span().span_context().clone();
    {
        let _current = live.attach();
        // Written in capitals, the id is still the trace's.
        harness.telmoni.record_trace(TraceRecord {
            trace_id: record.trace_id.to_uppercase(),
            ..record.clone()
        })?;
        assert_eq!(
            Context::current().span().span_context(),
            &live_span,
            "the live trace is still the current span"
        );
    }
    live.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let recorded = of_trace(&spans, &record.trace_id);
    assert_eq!(recorded.len(), 10, "every span of the record, by its id");
    assert!(
        trace_span(&spans, &record.trace_id)?
            .get("parentSpanId")
            .is_none(),
        "the record's agent span is the root of its trace"
    );
    assert_ne!(
        trace_span(&spans, &live_id)?.get("traceId"),
        Some(&json!(record.trace_id))
    );
    Ok(())
}

#[test]
fn a_failed_trace_is_marked_on_its_own_span() -> Outcome {
    let harness = Harness::new()?;
    let record = TraceRecord {
        error_type: Some("RateLimitError".to_owned()),
        cancelled: false,
        inside: Vec::new(),
        ..the_record()?
    };
    harness.telmoni.record_trace(record.clone())?;
    harness.flush()?;

    let spans = harness.spans()?;
    let trace = trace_span(&spans, &record.trace_id)?;
    assert_eq!(trace.get("status"), Some(&json!({ "code": 2 })));
    let attributes = attributes_of(trace)?;
    assert_eq!(attributes.get("error.type"), Some(&json!("RateLimitError")));
    assert!(attributes.get("telmoni.cancelled").is_none());
    assert_eq!(
        spans.iter().map(name_of).collect::<Vec<_>>(),
        ["invoke_agent claude-code"]
    );
    Ok(())
}

/// A record's content, its trace's, steps' and calls', leaves only with
/// `capture_content` and the mode `on`.
/// A call's reasoning effort, which is no content, leaves whatever the mode.
#[test]
fn a_records_content_leaves_only_when_asked_and_the_project_allows() -> Outcome {
    let content = [
        (
            "invoke_agent claude-code",
            vec![
                ("gen_ai.input.messages", r#"["asked"]"#),
                ("gen_ai.output.messages", r#"{"said":"answered"}"#),
            ],
        ),
        (
            "subagent",
            vec![
                ("gen_ai.input.messages", r#"["the task"]"#),
                ("gen_ai.output.messages", r#"["the answer"]"#),
            ],
        ),
        (
            "its subagent",
            vec![
                ("gen_ai.input.messages", r#"["its task"]"#),
                ("gen_ai.output.messages", r#"["its answer"]"#),
            ],
        ),
        (
            "chat m",
            vec![
                ("gen_ai.input.messages", r#"["the question"]"#),
                ("gen_ai.output.messages", r#"{"said":"it"}"#),
            ],
        ),
        (
            "execute_tool t",
            vec![
                ("gen_ai.tool.call.arguments", r#"{"q":1}"#),
                ("gen_ai.tool.call.result", "found"),
            ],
        ),
    ];
    // The project's mode is off where no answer names it on.
    for (capture_content, answers) in [(true, 2), (false, 2), (true, 0)] {
        let harness = Harness::answering(
            Config {
                capture_content,
                ..keyed()
            },
            std::iter::repeat_n(Ok(mode_on()), answers),
        )?;
        harness
            .telmoni
            .trace("warm", TraceOptions::default())?
            .end();
        harness.flush()?;
        let at = the_record()?.start;
        let record = TraceRecord {
            input_messages: Some(json!(["asked"]).into()),
            output_messages: Some(json!({ "said": "answered" }).into()),
            inside: vec![
                StepRecord {
                    input_messages: Some(json!(["the task"]).into()),
                    output_messages: Some(json!(["the answer"]).into()),
                    calls: vec![
                        StepRecord {
                            input_messages: Some(json!(["its task"]).into()),
                            output_messages: Some(json!(["its answer"]).into()),
                            calls: vec![
                                ModelCallRecord {
                                    reasoning_effort: Some("high".to_owned()),
                                    input_messages: Some(json!(["the question"]).into()),
                                    output_messages: Some(json!({ "said": "it" }).into()),
                                    ..ModelCallRecord::new("m", at, at)
                                }
                                .into(),
                            ],
                            ..StepRecord::new("its subagent", at, at)
                        }
                        .into(),
                    ],
                    ..StepRecord::new("subagent", at, at)
                }
                .into(),
                ToolCallRecord {
                    arguments: Some(json!({ "q": 1 }).into()),
                    result: Some("found".into()),
                    ..ToolCallRecord::new("t", at, at)
                }
                .into(),
            ],
            ..the_record()?
        };
        harness.telmoni.record_trace(record)?;
        harness.flush()?;
        let spans = harness.spans()?;
        let sends = capture_content && answers > 0;
        for (name, fields) in &content {
            let span = spans
                .iter()
                .find(|span| name_of(span) == *name)
                .ok_or(*name)?;
            let sent = attributes_of(span)?;
            for &(key, value) in fields {
                if sends {
                    assert_eq!(sent.get(key), Some(&json!(value)), "{name} {key}");
                } else {
                    assert!(sent.get(key).is_none(), "{name} {key}");
                }
            }
        }
        let call = spans
            .iter()
            .find(|span| name_of(span) == "chat m")
            .ok_or("chat m")?;
        assert_eq!(
            attributes_of(call)?.get("gen_ai.request.reasoning.level"),
            Some(&json!("high"))
        );
    }
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
            TraceRecord {
                agent: " ".to_owned(),
                ..record.clone()
            },
            Error::EmptyAgent,
        ),
        (
            TraceRecord {
                trace_id: "trace-1".to_owned(),
                ..record.clone()
            },
            Error::TraceId,
        ),
        (
            TraceRecord {
                trace_id: "3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d".to_owned(),
                ..record.clone()
            },
            Error::TraceId,
        ),
        (
            TraceRecord {
                trace_id: "00000000000000000000000000000000".to_owned(),
                ..record.clone()
            },
            Error::TraceId,
        ),
        (
            TraceRecord {
                end: start - second,
                ..record.clone()
            },
            Error::EndBeforeStart,
        ),
        (
            TraceRecord {
                inside: vec![StepRecord::new("s", start + second, start).into()],
                ..record.clone()
            },
            Error::EndBeforeStart,
        ),
        (
            TraceRecord {
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
        assert_eq!(harness.telmoni.record_trace(refused), Err(error));
    }
    harness.flush()?;
    assert_eq!(harness.transport.count(), 0, "nothing of a refused record");
    Ok(())
}

/// A record with a time ingest cannot read, the trace's, a step's or a call's, is refused whole.
#[test]
fn a_record_timed_where_ingest_cannot_read_is_refused() -> Outcome {
    assert_eq!(
        Error::RecordedTime.to_string(),
        "a recorded time must fall after the Unix epoch and less than 2^64 nanoseconds after it"
    );
    let harness = Harness::new()?;
    let golden = the_record()?;
    let at = golden.start;
    let epoch = UNIX_EPOCH;
    let before = epoch.checked_sub(Duration::from_secs(1)).ok_or("1969")?;
    let last = epoch
        .checked_add(Duration::from_nanos(u64::MAX))
        .ok_or("2554")?;
    let past = last.checked_add(Duration::from_nanos(1)).ok_or("2554")?;
    let refused = [
        TraceRecord {
            start: epoch,
            ..golden.clone()
        },
        TraceRecord {
            end: past,
            ..golden.clone()
        },
        TraceRecord {
            inside: vec![StepRecord::new("s", before, at).into()],
            ..golden.clone()
        },
        TraceRecord {
            inside: vec![
                StepRecord {
                    calls: vec![ToolCallRecord::new("t", at, past).into()],
                    ..StepRecord::new("s", at, at)
                }
                .into(),
            ],
            ..golden.clone()
        },
        TraceRecord {
            inside: vec![ModelCallRecord::new("m", epoch, at).into()],
            ..golden.clone()
        },
    ];
    for record in refused {
        assert_eq!(
            harness.telmoni.record_trace(record),
            Err(Error::RecordedTime)
        );
    }
    harness.flush()?;
    assert_eq!(harness.transport.count(), 0, "nothing of a refused record");

    let first = epoch.checked_add(Duration::from_nanos(1)).ok_or("1970")?;
    harness.telmoni.record_trace(TraceRecord {
        start: first,
        end: last,
        inside: Vec::new(),
        ..golden.clone()
    })?;
    harness.flush()?;
    let spans = harness.spans()?;
    let trace = trace_span(&spans, &golden.trace_id)?;
    assert_eq!(text(trace, "startTimeUnixNano")?, "1");
    assert_eq!(text(trace, "endTimeUnixNano")?, u64::MAX.to_string());
    Ok(())
}

/// A record with an empty step name or model is refused whole; a blank one, or a tool's empty name, is sent.
#[test]
fn a_record_with_an_empty_name_is_refused() -> Outcome {
    assert_eq!(
        Error::EmptyName.to_string(),
        "a recorded step's name or model call's model cannot be empty"
    );
    let harness = Harness::new()?;
    let golden = the_record()?;
    let at = golden.start;
    let step = |name: &str, calls: Vec<RecordItem>| -> RecordItem {
        StepRecord {
            calls,
            ..StepRecord::new(name, at, at)
        }
        .into()
    };
    let refused = [
        step("", Vec::new()),
        ModelCallRecord::new("", at, at).into(),
        step("s", vec![ModelCallRecord::new("", at, at).into()]),
        step("s", vec![step("", Vec::new())]),
    ];
    for item in refused {
        let record = TraceRecord {
            inside: vec![item],
            ..golden.clone()
        };
        assert_eq!(harness.telmoni.record_trace(record), Err(Error::EmptyName));
    }
    harness.flush()?;
    assert_eq!(harness.transport.count(), 0, "nothing of a refused record");

    // A tool's empty name ingest reads as none, keeping the call.
    harness.telmoni.record_trace(TraceRecord {
        inside: vec![step(
            " ",
            vec![
                ToolCallRecord::new(" ", at, at).into(),
                ToolCallRecord::new("", at, at).into(),
                ModelCallRecord::new(" ", at, at).into(),
            ],
        )],
        ..golden
    })?;
    harness.flush()?;
    let names: BTreeSet<String> = harness
        .spans()?
        .iter()
        .map(|span| name_of(span).to_owned())
        .collect();
    for blank in [" ", "execute_tool  ", "execute_tool ", "chat  "] {
        assert!(names.contains(blank), "{blank:?} in {names:?}");
    }
    Ok(())
}

/// What a refusal names, its cap, and a record holding that text at the length given.
type Capped<'a> = (Text, usize, &'a dyn Fn(usize) -> TraceRecord);

/// Each text a record holds is sent at ingest's cap and refused one past it, counted in characters.
#[test]
fn a_record_with_a_text_past_ingests_cap_is_refused() -> Outcome {
    assert_eq!(
        Error::TooLong {
            what: Text::TraceAgent,
            max: 128
        }
        .to_string(),
        "a recorded trace's agent must be up to 128 characters"
    );
    let harness = Harness::new()?;
    let golden = TraceRecord {
        inside: Vec::new(),
        ..the_record()?
    };
    let at = golden.start;
    // Two bytes each, so a cap counted in bytes would refuse every text at it.
    let text = |n: usize| "é".repeat(n);
    let inside = |item: RecordItem| TraceRecord {
        inside: vec![item],
        ..golden.clone()
    };
    let in_step = |item: RecordItem| {
        inside(
            StepRecord {
                calls: vec![item],
                ..StepRecord::new("s", at, at)
            }
            .into(),
        )
    };
    let model = |n: usize| ModelCallRecord::new(text(n), at, at);
    let tool = |n: usize| ToolCallRecord::new(text(n), at, at);
    let cases: [Capped<'_>; 12] = [
        (Text::TraceAgent, 128, &|n| TraceRecord {
            agent: text(n),
            ..golden.clone()
        }),
        (Text::TraceUserId, 128, &|n| TraceRecord {
            user_id: Some(text(n)),
            ..golden.clone()
        }),
        (Text::TraceSessionId, 200, &|n| TraceRecord {
            session_id: Some(text(n)),
            ..golden.clone()
        }),
        (Text::TraceErrorType, 128, &|n| TraceRecord {
            error_type: Some(text(n)),
            ..golden.clone()
        }),
        (Text::StepName, 145, &|n| {
            inside(StepRecord::new(text(n), at, at).into())
        }),
        (Text::StepErrorType, 128, &|n| {
            inside(
                StepRecord {
                    error_type: Some(text(n)),
                    ..StepRecord::new("s", at, at)
                }
                .into(),
            )
        }),
        // The longest operation and a model at its cap keep the call its name.
        (Text::ModelCallModel, 128, &|n| {
            inside(
                ModelCallRecord {
                    operation: Some("generate_content".to_owned()),
                    ..model(n)
                }
                .into(),
            )
        }),
        (Text::ModelCallProvider, 128, &|n| {
            in_step(
                ModelCallRecord {
                    provider: Some(text(n)),
                    ..model(1)
                }
                .into(),
            )
        }),
        // In a step inside a step, which is checked as the trace's own are.
        (Text::ModelCallReasoningEffort, 1024, &|n| {
            in_step(
                StepRecord {
                    calls: vec![
                        ModelCallRecord {
                            reasoning_effort: Some(text(n)),
                            ..model(1)
                        }
                        .into(),
                    ],
                    ..StepRecord::new("inner", at, at)
                }
                .into(),
            )
        }),
        (Text::ModelCallErrorType, 128, &|n| {
            inside(
                ModelCallRecord {
                    error_type: Some(text(n)),
                    ..model(1)
                }
                .into(),
            )
        }),
        (Text::ToolCallName, 145, &|n| inside(tool(n).into())),
        (Text::ToolCallErrorType, 128, &|n| {
            in_step(
                ToolCallRecord {
                    error_type: Some(text(n)),
                    ..tool(1)
                }
                .into(),
            )
        }),
    ];
    for &(what, max, record) in &cases {
        harness.telmoni.record_trace(record(max))?;
        assert_eq!(
            harness.telmoni.record_trace(record(max + 1)),
            Err(Error::TooLong { what, max }),
            "{what}"
        );
    }
    harness.flush()?;
    let runs = harness
        .spans()?
        .iter()
        .filter(|span| name_of(span).starts_with("invoke_agent "))
        .count();
    assert_eq!(runs, cases.len(), "each record at its cap, and no other");
    Ok(())
}

/// `depth` steps, each inside the one before.
fn nested(depth: usize, at: SystemTime) -> RecordItem {
    let mut step = StepRecord::new("s", at, at);
    for n in 2..=depth {
        step = StepRecord {
            calls: vec![step.into()],
            ..StepRecord::new(format!("s{n}"), at, at)
        };
    }
    step.into()
}

/// A step inside a step goes under its span, each span at its place depth first, each step sent after what it holds.
#[test]
fn a_step_inside_a_step_goes_under_its_span() -> Outcome {
    let harness = Harness::new()?;
    let golden = the_record()?;
    let at = golden.start;
    let trace = Uuid::parse_str(&golden.trace_id)?;
    let call = |name: &str| -> RecordItem { ToolCallRecord::new(name, at, at).into() };
    let step = |name: &str, calls: Vec<RecordItem>| -> RecordItem {
        StepRecord {
            calls,
            ..StepRecord::new(name, at, at)
        }
        .into()
    };
    harness.telmoni.record_trace(TraceRecord {
        agent: "bot".to_owned(),
        inside: vec![
            step(
                "outer",
                vec![
                    call("a"),
                    step("middle", vec![step("inner", vec![call("b")])]),
                    call("c"),
                ],
            ),
            call("d"),
        ],
        ..golden
    })?;
    harness.flush()?;
    let spans = harness.spans()?;
    let sent: Vec<&str> = spans.iter().map(name_of).collect();
    assert_eq!(
        sent,
        [
            "execute_tool a",
            "execute_tool b",
            "inner",
            "middle",
            "execute_tool c",
            "outer",
            "execute_tool d",
            "invoke_agent bot",
        ]
    );
    let places = [
        "invoke_agent bot",
        "outer",
        "execute_tool a",
        "middle",
        "inner",
        "execute_tool b",
        "execute_tool c",
        "execute_tool d",
    ];
    let id_of = |name: &str| -> Result<String, Failure> {
        let place = places.iter().position(|each| *each == name).ok_or(name)?;
        let id = Uuid::new_v5(&trace, place.to_string().as_bytes());
        Ok(id
            .as_bytes()
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect())
    };
    let parents = BTreeMap::from([
        ("outer", "invoke_agent bot"),
        ("execute_tool a", "outer"),
        ("middle", "outer"),
        ("inner", "middle"),
        ("execute_tool b", "inner"),
        ("execute_tool c", "outer"),
        ("execute_tool d", "invoke_agent bot"),
    ]);
    for span in &spans {
        let name = name_of(span);
        assert_eq!(span.get("spanId"), Some(&json!(id_of(name)?)), "{name}");
        let parent = parents.get(name).map(|parent| id_of(parent)).transpose()?;
        assert_eq!(
            span.get("parentSpanId").and_then(Value::as_str),
            parent.as_deref(),
            "{name}"
        );
    }
    Ok(())
}

/// Steps nested 16 deep are sent, each under the next, and one deeper is refused whole.
#[test]
fn steps_nest_16_deep_and_no_deeper() -> Outcome {
    assert_eq!(
        Error::TooDeep { max: 16 }.to_string(),
        "a recorded step must be nested at most 16 deep"
    );
    let harness = Harness::new()?;
    let golden = the_record()?;
    let at = golden.start;
    assert_eq!(
        harness.telmoni.record_trace(TraceRecord {
            inside: vec![nested(17, at)],
            ..golden.clone()
        }),
        Err(Error::TooDeep { max: 16 })
    );
    harness.telmoni.record_trace(TraceRecord {
        inside: vec![nested(16, at)],
        ..golden
    })?;
    harness.flush()?;
    let spans = harness.spans()?;
    assert_eq!(
        spans.len(),
        17,
        "the agent span and its 16 steps, and no more"
    );
    for (inner, outer) in spans.iter().zip(spans.iter().skip(1)) {
        assert_eq!(
            inner.get("parentSpanId"),
            outer.get("spanId"),
            "{} under {}",
            name_of(inner),
            name_of(outer)
        );
    }
    Ok(())
}

/// Tags past the first 32 by key and negative token counts are left off a record, each warned once.
#[test]
fn a_record_leaves_off_tags_past_32_and_negative_counts() -> Outcome {
    let harness = Harness::new()?;
    let golden = the_record()?;
    let at = golden.start;
    let tags: BTreeMap<String, String> =
        (0..40).map(|n| (format!("k{n}"), n.to_string())).collect();
    let negative = Usage {
        input_tokens: Some(-1),
        output_tokens: Some(5),
        ..Usage::default()
    };
    let record = TraceRecord {
        tags: tags.clone(),
        inside: vec![
            ModelCallRecord {
                usage: Some(negative),
                ..ModelCallRecord::new("m", at, at)
            }
            .into(),
            StepRecord {
                calls: vec![
                    ModelCallRecord {
                        usage: Some(Usage {
                            cache_write_input_tokens: Some(-3),
                            ..negative
                        }),
                        ..ModelCallRecord::new("m", at, at)
                    }
                    .into(),
                ],
                ..StepRecord::new("s", at, at)
            }
            .into(),
        ],
        ..golden
    };
    harness.telmoni.record_trace(record.clone())?;
    harness.telmoni.record_trace(record.clone())?;
    harness.flush()?;

    let spans = harness.spans()?;
    let sent: BTreeSet<String> = attributes_of(trace_span(&spans, &record.trace_id)?)?
        .as_object()
        .ok_or("attributes")?
        .keys()
        .filter(|key| key.starts_with("telmoni.tag."))
        .cloned()
        .collect();
    let first: BTreeSet<String> = tags
        .keys()
        .take(32)
        .map(|key| format!("telmoni.tag.{key}"))
        .collect();
    assert_eq!(sent, first);
    let calls: Vec<&Value> = spans
        .iter()
        .filter(|span| name_of(span) == "chat m")
        .collect();
    assert_eq!(calls.len(), 4, "two calls in each of two sends");
    for call in calls {
        let attributes = attributes_of(call)?;
        assert_eq!(attributes.get("gen_ai.usage.input_tokens"), None);
        assert_eq!(
            attributes.get("gen_ai.usage.cache_creation.input_tokens"),
            None
        );
        assert_eq!(
            attributes.get("gen_ai.usage.output_tokens"),
            Some(&json!(5))
        );
    }
    let mut warned = harness.warnings.all();
    warned.sort();
    let mut expected = [
        "telmoni: a trace carries up to 32 tags, the most ingest keeps; the rest are not sent",
        "telmoni: gen_ai.usage.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
        "telmoni: gen_ai.usage.cache_creation.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
    ];
    expected.sort_unstable();
    assert_eq!(warned, expected);
    Ok(())
}

/// A recorded tag ingest would drop is left off with one warning and takes no place among the 32.
#[test]
fn a_record_leaves_off_a_tag_ingest_would_drop_before_its_32() -> Outcome {
    let harness = Harness::new()?;
    let kept: BTreeMap<String, String> = (0..32)
        .map(|n| (format!("k{n:02}"), "é".repeat(256)))
        .collect();
    let mut tags = kept.clone();
    tags.insert(String::new(), "v".to_owned());
    tags.insert("k00a".to_owned(), "v".repeat(257));
    tags.insert("k".repeat(65), "v".to_owned());
    let record = TraceRecord {
        tags,
        ..the_record()?
    };
    harness.telmoni.record_trace(record.clone())?;
    harness.flush()?;

    let spans = harness.spans()?;
    let sent: BTreeMap<String, Value> = attributes_of(trace_span(&spans, &record.trace_id)?)?
        .as_object()
        .ok_or("attributes")?
        .iter()
        .filter_map(|(key, value)| {
            Some((key.strip_prefix("telmoni.tag.")?.to_owned(), value.clone()))
        })
        .collect();
    let expected: BTreeMap<String, Value> = kept
        .into_iter()
        .map(|(key, value)| (key, json!(value)))
        .collect();
    assert_eq!(sent, expected);
    assert_eq!(
        harness.warnings.all(),
        [
            "telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent",
            "telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat",
        ]
    );
    Ok(())
}

/// A record's warnings come before its hold, so a warning sink can record a trace itself.
#[test]
fn a_records_warnings_come_before_its_hold() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    // A warning records a trace on another thread, which a hold taken would stall.
    let client: Arc<Mutex<Option<Telmoni>>> = Arc::default();
    let through = Collected::<bool>::default();
    let (shared, noted, warned) = (Arc::clone(&client), through.clone(), warnings.clone());
    // No call in it, so the probe gives no warning of its own.
    let probe = TraceRecord {
        inside: Vec::new(),
        ..the_record()?
    };
    hooks.warn = Box::new(move |text, warning| {
        warned.push(text, warning);
        let Some(telmoni) = lock(&shared).clone() else {
            return;
        };
        let probe = probe.clone();
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(telmoni.record_trace(probe));
        });
        noted.push(finished.recv_timeout(Duration::from_secs(5)).is_ok());
    });
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    *lock(&client) = Some(telmoni.clone());
    let golden = the_record()?;
    let at = golden.start;
    let mut tags: BTreeMap<String, String> =
        (0..33).map(|n| (format!("k{n}"), String::new())).collect();
    tags.insert(String::new(), String::new());
    let record = TraceRecord {
        trace_id: "0f8fad5bd9cb469fa16570867728950e".to_owned(),
        tags,
        inside: vec![
            ModelCallRecord {
                usage: Some(Usage {
                    input_tokens: Some(-1),
                    ..Usage::default()
                }),
                ..ModelCallRecord::new("m", at, at)
            }
            .into(),
        ],
        ..golden
    };
    let recorded = telmoni.record_trace(record);
    *lock(&client) = None;
    recorded?;
    assert!(telmoni.flush());
    telmoni.shutdown();

    assert_eq!(warnings.all().len(), 3, "{:?}", warnings.all());
    assert_eq!(
        through.all(),
        [true, true, true],
        "a warning was given while the record was held"
    );
    Ok(())
}

/// A record made once the client is shut down is refused, never answered as
/// sent when it went nowhere.
#[test]
fn a_record_after_shutdown_is_refused() -> Outcome {
    let harness = Harness::new()?;
    harness.telmoni.shutdown();
    assert_eq!(
        harness.telmoni.record_trace(the_record()?),
        Err(Error::Shutdown)
    );
    assert_eq!(harness.transport.count(), 0);
    Ok(())
}

/// A record the queue has no room for is refused whole, before anything is
/// queued, since its tail — the agent span first — would otherwise be
/// dropped; drained, the queue takes it whole.
#[test]
fn a_record_the_queue_has_no_room_for_is_refused_whole() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    // Room for four spans, and a worker that will not wake until flushed.
    hooks.batching = Batching {
        max_queue_size: 4,
        max_batch_size: 100,
        schedule_delay: Duration::from_secs(60),
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    let golden = the_record()?;
    let at = golden.start;
    let record = TraceRecord {
        inside: vec![
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
        ],
        ..golden
    };

    // Two steps hold two of the four slots, the trace's opening gone first.
    let trace = telmoni.trace("bot", TraceOptions::default())?;
    assert!(
        transport.wait_for(1),
        "the trace's opening, out of the queue"
    );
    for n in 0..2 {
        let step = trace.step(&format!("s{n}"))?;
        step.end();
    }
    let refused = telmoni.record_trace(record.clone());
    assert_eq!(
        refused,
        Err(Error::QueueFull { spans: 3, queue: 4 }),
        "room for two of the record's three spans"
    );
    assert!(refused.err().is_some_and(|error| {
        error
            .to_string()
            .ends_with("flush or wait, then record it again")
    }));
    trace.end();
    assert!(telmoni.flush());
    telmoni.record_trace(record)?;
    assert!(telmoni.flush());
    telmoni.shutdown();

    let names: Vec<String> = transport
        .spans()?
        .iter()
        .map(|span| name_of(span).to_owned())
        .collect();
    let calls = names
        .iter()
        .filter(|name| *name == "execute_tool t")
        .count();
    assert!(
        calls == 2 && names.len() == 6,
        "the live trace's three spans and the record's three: {names:?}"
    );
    assert!(warnings.all().is_empty(), "{:?}", warnings.all());
    Ok(())
}

/// A record of more spans than the queue holds, which no flush makes room for, names the bound to raise.
#[test]
fn a_record_past_the_whole_queue_names_the_bound() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.batching = Batching {
        max_queue_size: 2,
        ..Batching::default()
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    let golden = the_record()?;
    let at = golden.start;
    let record = TraceRecord {
        inside: vec![
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
        ],
        ..golden
    };
    let refused = telmoni.record_trace(record);
    assert_eq!(refused, Err(Error::QueueFull { spans: 3, queue: 2 }));
    assert_eq!(
        refused.err().map(|error| error.to_string()).as_deref(),
        Some(
            "the export queue has no room for the record's 3 spans; the queue holds 2, so raise its size to record it"
        )
    );
    telmoni.shutdown();
    assert_eq!(transport.count(), 0);
    Ok(())
}

/// The traces an export's spans are in.
fn traces_in(body: &[u8]) -> Result<BTreeSet<String>, Failure> {
    let body: Value = serde_json::from_slice(body)?;
    spans_of(&body)
        .iter()
        .map(|span| Ok(text(span, "traceId")?.to_owned()))
        .collect()
}

/// A span queued before a record or ending after it, and a record beside it,
/// go in other exports, so the record is the same bytes as when sent alone.
#[test]
fn a_record_travels_in_exports_of_its_own() -> Outcome {
    let harness = Harness::new()?;
    let record = the_record()?;
    let beside = TraceRecord {
        trace_id: "0191a3b422224aaa8bbb000000000002".to_owned(),
        ..record.clone()
    };
    let live = harness.telmoni.trace("live", TraceOptions::default())?;
    live.step("live step")?.end();
    harness.telmoni.record_trace(record.clone())?;
    harness.telmoni.record_trace(beside.clone())?;
    live.end();
    harness.flush()?;
    let sent = harness.transport.count();
    harness.telmoni.record_trace(record.clone())?;
    harness.flush()?;
    harness.telmoni.shutdown();

    let bodies: Vec<Vec<u8>> = harness
        .transport
        .requests()
        .into_iter()
        .map(|request| request.body)
        .collect();
    let trace_of = |record: &TraceRecord| record.trace_id.clone();
    let mut first = Vec::new();
    let mut other = 0;
    for (n, body) in bodies.iter().enumerate() {
        let traces = traces_in(body)?;
        assert_eq!(traces.len(), 1, "export {n} is in traces {traces:?}");
        if n < sent && traces.contains(&trace_of(&record)) {
            first.push(body);
        }
        if traces.contains(&trace_of(&beside)) {
            other += 1;
        }
    }
    assert_eq!(bodies.len(), sent + 1);
    assert_eq!(
        first,
        [&bodies[sent]],
        "the record sent beside a live trace is the bytes it is sent again as"
    );
    assert_eq!(
        other, 1,
        "the record beside it went in one export of its own"
    );
    Ok(())
}

/// A record larger than a batch is cut into batches counted from its own
/// start, wherever it lands in the queue.
#[test]
fn a_record_larger_than_a_batch_is_cut_from_its_own_start() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.batching = Batching {
        max_queue_size: 2048,
        max_batch_size: 3,
        schedule_delay: Duration::from_secs(60),
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    let golden = the_record()?;
    let at = golden.start;
    let record = TraceRecord {
        inside: vec![
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
        ],
        ..golden
    };
    let trace = telmoni.trace("bot", TraceOptions::default())?;
    assert!(transport.wait_for(1), "the trace's opening, at once");
    for n in 0..2 {
        trace.step(&format!("s{n}"))?.end();
    }
    telmoni.record_trace(record)?;
    trace.end();
    assert!(telmoni.flush());
    telmoni.shutdown();

    let exports: Vec<Vec<String>> = transport
        .bodies()?
        .iter()
        .map(|body| {
            spans_of(body)
                .iter()
                .map(|span| name_of(span).to_owned())
                .collect()
        })
        .collect();
    assert_eq!(
        exports,
        [
            vec!["telmoni.trace.start"],
            vec!["s0", "s1"],
            vec!["execute_tool t", "execute_tool t", "execute_tool t"],
            vec!["invoke_agent claude-code"],
            vec!["invoke_agent bot"],
        ]
    );
    Ok(())
}

/// A live step ending on another thread while a record is built is in no
/// export of the record's. The queue is large, so the thread never fills it.
#[test]
fn a_span_ending_while_a_record_is_built_travels_apart() -> Outcome {
    let record = the_record()?;
    for _ in 0..20 {
        let transport = FakeTransport::new([])?;
        let warnings = Warnings::default();
        let sleeps = Sleeps::default();
        let mut hooks = hooks(&transport, &warnings, &sleeps);
        hooks.batching = Batching {
            max_queue_size: 1 << 16,
            ..Batching::default()
        };
        let telmoni = Telmoni::with_hooks(keyed(), hooks);
        let live = telmoni.trace("live", TraceOptions::default())?;
        let stop = AtomicBool::new(false);
        let recorded = std::thread::scope(|scope| {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(step) = live.step("live step") {
                        step.end();
                    }
                }
            });
            let recorded = telmoni.record_trace(record.clone());
            stop.store(true, Ordering::Relaxed);
            recorded
        });
        recorded?;
        live.end();
        assert!(telmoni.flush());
        telmoni.shutdown();
        for (n, request) in transport.requests().iter().enumerate() {
            let traces = traces_in(&request.body)?;
            assert_eq!(traces.len(), 1, "export {n} is in traces {traces:?}");
        }
    }
    Ok(())
}

type Hook = Box<dyn FnOnce() + Send>;

/// Calls its hook once, as the next span with no parent starts: a record's
/// own span, mid-build.
#[derive(Clone, Default)]
struct Hooked(Arc<Mutex<Option<Hook>>>);

impl std::fmt::Debug for Hooked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hooked")
    }
}

impl ShouldSample for Hooked {
    fn should_sample(
        &self,
        _parent_context: Option<&Context>,
        _trace_id: TraceId,
        _name: &str,
        _span_kind: &SpanKind,
        _attributes: &[KeyValue],
        _links: &[Link],
    ) -> SamplingResult {
        let hook = lock(&self.0).take();
        if let Some(hook) = hook {
            hook();
        }
        SamplingResult {
            decision: SamplingDecision::RecordAndSample,
            attributes: Vec::new(),
            trace_state: Default::default(),
        }
    }
}

/// While a record is built its room in the queue is kept: a live step ending
/// meanwhile is dropped past it, so the queue never holds more than its size.
#[test]
fn a_records_room_is_kept_while_it_is_built() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    // Room for four spans, and a worker that will not wake until flushed.
    hooks.batching = Batching {
        max_queue_size: 4,
        max_batch_size: 100,
        schedule_delay: Duration::from_secs(60),
    };
    let sampler = Hooked::default();
    let config = Config {
        sampler: Some(Box::new(sampler.clone())),
        ..keyed()
    };
    let telmoni = Telmoni::with_hooks(config, hooks);
    let golden = the_record()?;
    let at = golden.start;
    let record = TraceRecord {
        inside: vec![
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
        ],
        ..golden
    };
    let live = Arc::new(telmoni.trace("live", TraceOptions::default())?);
    assert!(
        transport.wait_for(1),
        "the trace's opening, out of the queue"
    );
    // Room for one step beside the record's three spans.
    let stepping = Arc::clone(&live);
    *lock(&sampler.0) = Some(Box::new(move || {
        for name in ["s0", "s1"] {
            if let Ok(step) = stepping.step(name) {
                step.end();
            }
        }
    }));
    telmoni.record_trace(record)?;
    assert!(telmoni.flush());
    Arc::try_unwrap(live)
        .map_err(|_| "the live trace is still shared")?
        .end();
    assert!(telmoni.flush());
    telmoni.shutdown();

    let mut names: Vec<String> = transport
        .spans()?
        .iter()
        .map(|span| name_of(span).to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "execute_tool t",
            "execute_tool t",
            "invoke_agent claude-code",
            "invoke_agent live",
            "s0",
        ]
    );
    assert_eq!(
        warnings.all(),
        ["telmoni: the export queue was full; 1 spans were dropped"]
    );
    Ok(())
}

/// A record queued behind a live step goes in the export after the step's,
/// with no flush.
#[test]
fn a_record_goes_at_once() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.batching = Batching {
        schedule_delay: Duration::from_secs(60),
        ..Batching::default()
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    let live = telmoni.trace("live", TraceOptions::default())?;
    assert!(transport.wait_for(1), "the trace's opening, at once");
    live.step("live step")?.end();
    telmoni.record_trace(the_record()?)?;
    assert!(transport.wait_for(3), "the record waited for the delay");
    live.end();
    telmoni.shutdown();
    Ok(())
}
