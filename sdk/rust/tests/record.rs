//! A run recorded after it ended: `record_run` and
//! `sdk/fixtures/recorded.json`.

mod common;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{
    Collected, Failure, FakeTransport, Harness, Outcome, Sleeps, Warnings, attributes_of, field,
    hooks, int, keyed, load_fixture, lock, name_of, run_span, spans_of, text,
};
use opentelemetry::trace::{Link, SpanKind, TraceContextExt as _, TraceId};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::{SamplingDecision, SamplingResult, ShouldSample};
use serde_json::{Value, json};
use telmoni_sdk::{
    Batching, CallRecord, Config, Error, ModelCallRecord, RecordItem, RunOptions, RunRecord,
    StepRecord, Telmoni, ToolCallRecord, Usage,
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

/// A record with a time ingest cannot read, the run's, a step's or a call's, is refused whole.
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
        RunRecord {
            start: epoch,
            ..golden.clone()
        },
        RunRecord {
            end: past,
            ..golden.clone()
        },
        RunRecord {
            inside: vec![StepRecord::new("s", before, at).into()],
            ..golden.clone()
        },
        RunRecord {
            inside: vec![
                StepRecord {
                    calls: vec![ToolCallRecord::new("t", at, past).into()],
                    ..StepRecord::new("s", at, at)
                }
                .into(),
            ],
            ..golden.clone()
        },
        RunRecord {
            inside: vec![ModelCallRecord::new("m", epoch, at).into()],
            ..golden.clone()
        },
    ];
    for record in refused {
        assert_eq!(harness.telmoni.record_run(record), Err(Error::RecordedTime));
    }
    harness.flush()?;
    assert_eq!(harness.transport.count(), 0, "nothing of a refused record");

    let first = epoch.checked_add(Duration::from_nanos(1)).ok_or("1970")?;
    harness.telmoni.record_run(RunRecord {
        start: first,
        end: last,
        inside: Vec::new(),
        ..golden.clone()
    })?;
    harness.flush()?;
    let spans = harness.spans()?;
    let run = run_span(&spans, &golden.run_id)?;
    assert_eq!(text(run, "startTimeUnixNano")?, "1");
    assert_eq!(text(run, "endTimeUnixNano")?, u64::MAX.to_string());
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
    let step = |name: &str, calls: Vec<CallRecord>| -> RecordItem {
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
    ];
    for item in refused {
        let record = RunRecord {
            inside: vec![item],
            ..golden.clone()
        };
        assert_eq!(harness.telmoni.record_run(record), Err(Error::EmptyName));
    }
    harness.flush()?;
    assert_eq!(harness.transport.count(), 0, "nothing of a refused record");

    // A tool's empty name ingest reads as none, keeping the call.
    harness.telmoni.record_run(RunRecord {
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
type Capped<'a> = (&'static str, usize, &'a dyn Fn(usize) -> RunRecord);

/// Each text a record holds is sent at ingest's cap and refused one past it, counted in characters.
#[test]
fn a_record_with_a_text_past_ingests_cap_is_refused() -> Outcome {
    assert_eq!(
        Error::TooLong {
            what: "run's agent",
            max: 128
        }
        .to_string(),
        "a recorded run's agent must be up to 128 characters"
    );
    let harness = Harness::new()?;
    let golden = RunRecord {
        inside: Vec::new(),
        ..the_record()?
    };
    let at = golden.start;
    // Two bytes each, so a cap counted in bytes would refuse every text at it.
    let text = |n: usize| "é".repeat(n);
    let inside = |item: RecordItem| RunRecord {
        inside: vec![item],
        ..golden.clone()
    };
    let in_step = |call: CallRecord| {
        inside(
            StepRecord {
                calls: vec![call],
                ..StepRecord::new("s", at, at)
            }
            .into(),
        )
    };
    let model = |n: usize| ModelCallRecord::new(text(n), at, at);
    let tool = |n: usize| ToolCallRecord::new(text(n), at, at);
    let cases: [Capped<'_>; 13] = [
        ("run's agent", 128, &|n| RunRecord {
            agent: text(n),
            ..golden.clone()
        }),
        ("run's user_id", 128, &|n| RunRecord {
            user_id: Some(text(n)),
            ..golden.clone()
        }),
        ("run's session_id", 200, &|n| RunRecord {
            session_id: Some(text(n)),
            ..golden.clone()
        }),
        ("run's error_type", 128, &|n| RunRecord {
            error_type: Some(text(n)),
            ..golden.clone()
        }),
        ("step's name", 145, &|n| {
            inside(StepRecord::new(text(n), at, at).into())
        }),
        ("step's error_type", 128, &|n| {
            inside(
                StepRecord {
                    error_type: Some(text(n)),
                    ..StepRecord::new("s", at, at)
                }
                .into(),
            )
        }),
        // The longest operation and a model at its cap keep the call its name.
        ("model call's model", 128, &|n| {
            inside(
                ModelCallRecord {
                    operation: Some("generate_content".to_owned()),
                    ..model(n)
                }
                .into(),
            )
        }),
        ("model call's provider", 128, &|n| {
            in_step(
                ModelCallRecord {
                    provider: Some(text(n)),
                    ..model(1)
                }
                .into(),
            )
        }),
        ("model call's error_type", 128, &|n| {
            inside(
                ModelCallRecord {
                    error_type: Some(text(n)),
                    ..model(1)
                }
                .into(),
            )
        }),
        // An operation of its own, since the longest ingest reads leaves room.
        ("model call's operation and model", 144, &|n| {
            in_step(
                ModelCallRecord {
                    operation: Some(text(20)),
                    ..model(n - 20)
                }
                .into(),
            )
        }),
        ("model call's operation and model", 144, &|n| {
            inside(
                ModelCallRecord {
                    operation: Some(text(27)),
                    ..model(n - 27)
                }
                .into(),
            )
        }),
        ("tool call's name", 145, &|n| inside(tool(n).into())),
        ("tool call's error_type", 128, &|n| {
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
        harness.telmoni.record_run(record(max))?;
        assert_eq!(
            harness.telmoni.record_run(record(max + 1)),
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
    let record = RunRecord {
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
    harness.telmoni.record_run(record.clone())?;
    harness.telmoni.record_run(record.clone())?;
    harness.flush()?;

    let spans = harness.spans()?;
    let sent: BTreeSet<String> = attributes_of(run_span(&spans, &record.run_id)?)?
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
            attributes.get("gen_ai.usage.cache_write.input_tokens"),
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
        "telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent",
        "telmoni: gen_ai.usage.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
        "telmoni: gen_ai.usage.cache_write.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
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
    let record = RunRecord {
        tags,
        ..the_record()?
    };
    harness.telmoni.record_run(record.clone())?;
    harness.flush()?;

    let spans = harness.spans()?;
    let sent: BTreeMap<String, Value> = attributes_of(run_span(&spans, &record.run_id)?)?
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
            "telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent"
        ]
    );
    Ok(())
}

/// A record's warnings come before its hold, so a warning sink can record a run itself.
#[test]
fn a_records_warnings_come_before_its_hold() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    // A warning records a run on another thread, which a hold taken would stall.
    let client: Arc<Mutex<Option<Telmoni>>> = Arc::default();
    let through = Collected::<bool>::default();
    let (shared, noted, warned) = (Arc::clone(&client), through.clone(), warnings.clone());
    let probe = the_record()?;
    hooks.warn = Box::new(move |text| {
        warned.push(text.to_owned());
        let Some(telmoni) = lock(&shared).clone() else {
            return;
        };
        let probe = probe.clone();
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(telmoni.record_run(probe));
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
    let record = RunRecord {
        run_id: "0f8fad5b-d9cb-469f-a165-70867728950e".to_owned(),
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
    let recorded = telmoni.record_run(record);
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

/// A record the queue has no room for is refused whole, before anything is
/// queued, since its tail — the run's own span first — would otherwise be
/// dropped; drained, the queue takes it whole.
#[test]
fn a_record_the_queue_has_no_room_for_is_refused_whole() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    // Room for five spans, and a worker that will not wake until flushed.
    hooks.batching = Batching {
        max_queue_size: 5,
        max_batch_size: 100,
        schedule_delay: Duration::from_secs(60),
    };
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    let golden = the_record()?;
    let at = golden.start;
    let record = RunRecord {
        inside: vec![
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
        ],
        ..golden
    };

    // The start's export is held at the gate, the worker inside it; two
    // steps then hold two of the five slots.
    transport.close_gate();
    let run = telmoni.run("bot", RunOptions::default())?;
    assert!(transport.wait_for(1));
    for n in 0..2 {
        let step = run.step(&format!("s{n}"));
        step.end();
    }
    assert_eq!(
        telmoni.record_run(record.clone()),
        Err(Error::QueueFull { spans: 4 }),
        "room for three of the record's four spans"
    );
    transport.open_gate();
    run.end();
    assert!(telmoni.flush());
    telmoni.record_run(record)?;
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
        calls == 2 && names.len() == 8,
        "the live run's four spans and the record's four: {names:?}"
    );
    assert!(warnings.all().is_empty(), "{:?}", warnings.all());
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
    let beside = RunRecord {
        run_id: "0191a3b4-2222-4aaa-8bbb-000000000002".to_owned(),
        ..record.clone()
    };
    let live = harness.telmoni.run("live", RunOptions::default())?;
    assert!(
        harness.transport.wait_for(1),
        "the live run's start goes alone"
    );
    live.step("live step").end();
    harness.telmoni.record_run(record.clone())?;
    harness.telmoni.record_run(beside.clone())?;
    live.end();
    harness.flush()?;
    let sent = harness.transport.count();
    harness.telmoni.record_run(record.clone())?;
    harness.flush()?;
    harness.telmoni.shutdown();

    let bodies: Vec<Vec<u8>> = harness
        .transport
        .requests()
        .into_iter()
        .map(|request| request.body)
        .collect();
    let trace_of = |run: &RunRecord| run.run_id.replace('-', "");
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
        "the record sent beside a live run is the bytes it is sent again as"
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
    let record = RunRecord {
        inside: vec![
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
        ],
        ..golden
    };
    let run = telmoni.run("bot", RunOptions::default())?;
    assert!(transport.wait_for(1));
    for n in 0..2 {
        run.step(&format!("s{n}")).end();
    }
    telmoni.record_run(record)?;
    run.end();
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
            vec!["telmoni.run.start"],
            vec!["s0", "s1"],
            vec!["telmoni.run.start", "execute_tool t", "execute_tool t"],
            vec!["execute_tool t", "invoke_agent claude-code"],
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
        let live = telmoni.run("live", RunOptions::default())?;
        let stop = AtomicBool::new(false);
        let recorded = std::thread::scope(|scope| {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    live.step("live step").end();
                }
            });
            let recorded = telmoni.record_run(record.clone());
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
    // Room for five spans, and a worker that will not wake until flushed.
    hooks.batching = Batching {
        max_queue_size: 5,
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
    let record = RunRecord {
        inside: vec![
            ToolCallRecord::new("t", at, at).into(),
            ToolCallRecord::new("t", at, at).into(),
        ],
        ..golden
    };
    let live = Arc::new(telmoni.run("live", RunOptions::default())?);
    assert!(transport.wait_for(1));
    // Room for one step beside the record's four spans.
    let stepping = Arc::clone(&live);
    *lock(&sampler.0) = Some(Box::new(move || {
        stepping.step("s0").end();
        stepping.step("s1").end();
    }));
    telmoni.record_run(record)?;
    assert!(telmoni.flush());
    Arc::try_unwrap(live)
        .map_err(|_| "the live run is still shared")?
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
            "telmoni.run.start",
            "telmoni.run.start",
        ]
    );
    assert_eq!(
        warnings.all(),
        ["telmoni: the export queue was full; 1 spans were dropped"]
    );
    Ok(())
}

/// A record queued behind a live step goes in the export after the step's, as
/// a live run's start would, with no flush.
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
    let live = telmoni.run("live", RunOptions::default())?;
    assert!(transport.wait_for(1));
    live.step("live step").end();
    telmoni.record_run(the_record()?)?;
    assert!(transport.wait_for(3), "the record waited for the delay");
    live.end();
    telmoni.shutdown();
    Ok(())
}
