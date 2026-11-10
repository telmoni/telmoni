//! A trace: the agent's span and attributes, what fails and cancels it, the
//! closures, and what it refuses.

mod common;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::panic::AssertUnwindSafe;

use common::{
    FakeTransport, Harness, Outcome, Sleeps, Warnings, attributes_of, hooks, is_hex, keyed,
    mode_on, name_of, named, only, starting_with, tags, trace_span,
};
use opentelemetry::KeyValue;
use opentelemetry::trace::TraceContextExt as _;
use serde_json::{Value, json};
use telmoni::{Config, Error, ModelCall, Step, Telmoni, ToolCall, Trace, TraceOptions, TraceReply};

/// An application's error, with the SDK's own among its causes so
/// `try_trace_with` can hand a trace that did not open back as one of these.
#[derive(Debug)]
enum Failed {
    Timeout(String),
    Sdk(Error),
}

impl fmt::Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(message) => write!(f, "timed out: {message}"),
            Self::Sdk(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Failed {}

impl From<Error> for Failed {
    fn from(error: Error) -> Self {
        Self::Sdk(error)
    }
}

#[test]
fn a_trace_is_a_span_with_the_contract_attributes() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace(
        "support-bot",
        TraceOptions {
            user_id: Some("user_1".to_owned()),
            session_id: Some("conv_1".to_owned()),
            tags: tags(&[("env", "test"), ("team", "a")]),
            max_steps: None,
            max_spend: None,
            input_messages: None,
        },
    )?;
    assert!(is_hex(trace.trace_id(), 32), "{}", trace.trace_id());
    assert_eq!(trace.agent(), "support-bot");
    let trace_id = trace.trace_id().to_owned();
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let span = only(&spans, "invoke_agent ")?;
    assert_eq!(span.get("kind"), Some(&json!(1)));
    assert_eq!(span.get("traceId"), Some(&json!(trace_id)));
    assert!(span.get("parentSpanId").is_none());
    assert!(span.get("status").is_none());
    assert_eq!(
        attributes_of(span)?,
        json!({
            "gen_ai.operation.name": "invoke_agent",
            "gen_ai.agent.name": "support-bot",
            "telmoni.agent": "support-bot",
            "user.id": "user_1",
            "gen_ai.conversation.id": "conv_1",
            "telmoni.tag.env": "test",
            "telmoni.tag.team": "a",
        })
    );
    Ok(())
}

#[test]
fn an_err_from_the_closure_fails_the_trace_with_its_type_alone() -> Outcome {
    let harness = Harness::new()?;
    let result: Result<(), Failed> =
        harness
            .telmoni
            .try_trace_with("bot", TraceOptions::default(), |_trace| {
                Err(Failed::Timeout(
                    "the message, which is content and never sent".to_owned(),
                ))
            });
    assert!(matches!(result, Err(Failed::Timeout(_))));
    harness.flush()?;
    let spans = harness.spans()?;
    let span = only(&spans, "invoke_agent ")?;
    assert_eq!(span.get("status"), Some(&json!({ "code": 2 })));
    assert_eq!(
        attributes_of(span)?.get("error.type"),
        Some(&json!("Failed")),
        "the type's name, not the variant's"
    );
    assert!(!harness.transport.text().contains("never sent"));
    Ok(())
}

#[test]
fn fail_and_cancel_mark_the_trace() -> Outcome {
    let harness = Harness::new()?;
    let failed = harness.telmoni.trace("bot", TraceOptions::default())?;
    let failed_id = failed.trace_id().to_owned();
    failed.fail("RateLimit");
    failed.end();
    let cancelled = harness.telmoni.trace("bot", TraceOptions::default())?;
    let cancelled_id = cancelled.trace_id().to_owned();
    cancelled.cancel();
    cancelled.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let failed = trace_span(&spans, &failed_id)?;
    assert_eq!(
        attributes_of(failed)?.get("error.type"),
        Some(&json!("RateLimit"))
    );
    assert_eq!(failed.get("status"), Some(&json!({ "code": 2 })));
    let cancelled = trace_span(&spans, &cancelled_id)?;
    assert_eq!(
        attributes_of(cancelled)?.get("telmoni.cancelled"),
        Some(&json!(true))
    );
    assert!(cancelled.get("status").is_none(), "neither ok nor an error");
    Ok(())
}

/// Failed with an empty type, a trace is failed with none, since ingest
/// refuses a span whose `error.type` is empty, and the trace with it.
#[test]
fn a_fail_with_an_empty_type_fails_the_trace_with_none() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let id = trace.trace_id().to_owned();
    trace.fail("");
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let failed = trace_span(&spans, &id)?;
    assert_eq!(attributes_of(failed)?.get("error.type"), None);
    assert_eq!(failed.get("status"), Some(&json!({ "code": 2 })));
    Ok(())
}

#[test]
fn a_panic_unwinding_through_the_guard_fails_it_as_a_panic() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let trace_id = trace.trace_id().to_owned();
    let unwound = std::panic::catch_unwind(AssertUnwindSafe(move || {
        let _trace = trace;
        panic!("the message, which is content and never sent");
    }));
    assert!(unwound.is_err());
    harness.flush()?;
    let spans = harness.spans()?;
    let span = trace_span(&spans, &trace_id)?;
    assert_eq!(span.get("status"), Some(&json!({ "code": 2 })));
    assert_eq!(
        attributes_of(span)?.get("error.type"),
        Some(&json!("panic"))
    );
    assert!(!harness.transport.text().contains("never sent"));
    Ok(())
}

#[test]
fn the_closure_opens_a_trace_per_call_and_returns_its_value() -> Outcome {
    let harness = Harness::new()?;
    let mut seen = Vec::new();
    for question in ["a", "b"] {
        let answer = harness
            .telmoni
            .trace_with("bot", TraceOptions::default(), |trace| {
                seen.push(trace.trace_id().to_owned());
                question.to_uppercase()
            })?;
        assert_eq!(answer, question.to_uppercase());
    }
    harness.flush()?;
    let spans = harness.spans()?;
    assert_eq!(starting_with(&spans, "invoke_agent bot").len(), 2);
    assert_eq!(seen.iter().collect::<HashSet<_>>().len(), 2);
    Ok(())
}

#[test]
fn a_trace_needs_an_agent() -> Outcome {
    let harness = Harness::new()?;
    assert!(matches!(
        harness.telmoni.trace(" ", TraceOptions::default()),
        Err(Error::EmptyAgent)
    ));
    assert!(matches!(
        harness
            .telmoni
            .trace_with(" ", TraceOptions::default(), |_trace| ()),
        Err(Error::EmptyAgent)
    ));
    let result: Result<(), Failed> =
        harness
            .telmoni
            .try_trace_with(" ", TraceOptions::default(), |_trace| Ok(()));
    assert!(matches!(result, Err(Failed::Sdk(Error::EmptyAgent))));

    harness.flush()?;
    assert!(harness.transport.requests().is_empty(), "nothing opened");
    Ok(())
}

/// Only a trace's first 32 tags by key are sent, warned of once, so what its end sets still fits.
#[test]
fn a_trace_sends_its_first_32_tags_and_warns_once_of_the_rest() -> Outcome {
    let harness = Harness::new()?;
    let many: BTreeMap<String, String> =
        (0..200).map(|n| (format!("k{n}"), n.to_string())).collect();
    let mut trace_ids = Vec::new();
    for _ in 0..2 {
        let trace = harness.telmoni.trace(
            "bot",
            TraceOptions {
                tags: many.clone(),
                ..TraceOptions::default()
            },
        )?;
        trace.fail("Overrun");
        trace.cancel();
        trace_ids.push(trace.trace_id().to_owned());
        trace.end();
    }
    let exact = Harness::new()?;
    let trace = exact.telmoni.trace(
        "bot",
        TraceOptions {
            tags: many.clone().into_iter().take(32).collect(),
            ..TraceOptions::default()
        },
    )?;
    let exact_id = trace.trace_id().to_owned();
    trace.end();
    harness.flush()?;
    exact.flush()?;

    let tags_of = |span: &Value| -> Result<BTreeSet<String>, common::Failure> {
        Ok(attributes_of(span)?
            .as_object()
            .ok_or("attributes")?
            .keys()
            .filter(|key| key.starts_with("telmoni.tag."))
            .cloned()
            .collect())
    };
    let first: BTreeSet<String> = many
        .keys()
        .take(32)
        .map(|key| format!("telmoni.tag.{key}"))
        .collect();
    let spans = harness.spans()?;
    for trace_id in &trace_ids {
        let span = trace_span(&spans, trace_id)?;
        let attributes = attributes_of(span)?;
        assert_eq!(attributes.get("error.type"), Some(&json!("Overrun")));
        assert_eq!(tags_of(span)?, first);
        assert_eq!(attributes.get("telmoni.cancelled"), Some(&json!(true)));
    }
    assert_eq!(
        harness.warnings.all(),
        ["telmoni: a trace carries up to 32 tags, the most ingest keeps; the rest are not sent"]
    );
    assert_eq!(tags_of(trace_span(&exact.spans()?, &exact_id)?)?, first);
    assert!(
        exact.warnings.all().is_empty(),
        "{:?}",
        exact.warnings.all()
    );
    Ok(())
}

/// A tag ingest would drop is left off with one warning and takes no place among the 32.
#[test]
fn a_tag_ingest_would_drop_takes_no_place_among_the_32() -> Outcome {
    let harness = Harness::new()?;
    // Two of the 32 at ingest's caps, one of them in two-byte characters.
    let mut kept: BTreeMap<String, String> = (0..30)
        .map(|n| (format!("k{n:02}"), n.to_string()))
        .collect();
    kept.insert("x".repeat(64), String::new());
    kept.insert("é".repeat(64), "é".repeat(256));
    let mut given = kept.clone();
    given.insert(String::new(), "v".to_owned());
    given.insert("k15a".to_owned(), "v".repeat(257));
    given.insert("y".repeat(65), "v".to_owned());
    let mut trace_ids = Vec::new();
    for _ in 0..2 {
        let trace = harness.telmoni.trace(
            "bot",
            TraceOptions {
                tags: given.clone(),
                ..TraceOptions::default()
            },
        )?;
        trace_ids.push(trace.trace_id().to_owned());
        trace.end();
    }
    harness.flush()?;

    let expected: BTreeMap<String, Value> = kept
        .into_iter()
        .map(|(key, value)| (key, json!(value)))
        .collect();
    let spans = harness.spans()?;
    for trace_id in &trace_ids {
        let sent: BTreeMap<String, Value> = attributes_of(trace_span(&spans, trace_id)?)?
            .as_object()
            .ok_or("attributes")?
            .iter()
            .filter_map(|(key, value)| {
                Some((key.strip_prefix("telmoni.tag.")?.to_owned(), value.clone()))
            })
            .collect();
        assert_eq!(sent, expected);
    }
    assert_eq!(
        harness.warnings.all(),
        [
            "telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent"
        ]
    );
    Ok(())
}

/// A warning sink that panics never reaches the agent's call: the trace and its step still go.
#[test]
fn a_warning_sink_that_panics_never_reaches_the_agent() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.warn = Box::new(|text, _| std::panic::panic_any(text.to_owned()));
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    let trace = telmoni.trace(
        "bot",
        TraceOptions {
            tags: (0..40).map(|n| (format!("k{n}"), n.to_string())).collect(),
            ..TraceOptions::default()
        },
    )?;
    let trace_id = trace.trace_id().to_owned();
    trace.step("s")?.end();
    trace.end();
    assert!(telmoni.flush());
    telmoni.shutdown();

    let spans = transport.spans()?;
    assert_eq!(name_of(trace_span(&spans, &trace_id)?), "invoke_agent bot");
    assert!(named(&spans, "s").is_ok(), "the step was not sent");
    Ok(())
}

/// On every span of the trace, any of these past its cap cost ingest the trace whole.
#[test]
fn a_traces_text_past_ingests_caps_is_cut_or_left_off_warned_once() -> Outcome {
    let harness = Harness::new()?;
    let smiles = |n: usize| "😀".repeat(n);
    for _ in 0..2 {
        let trace = harness.telmoni.trace(
            &smiles(129),
            TraceOptions {
                user_id: Some("u".repeat(129)),
                session_id: Some("s".repeat(201)),
                ..TraceOptions::default()
            },
        )?;
        assert_eq!(trace.agent(), smiles(128));
        trace.fail(&"E".repeat(129));
        trace.end();
    }
    harness.flush()?;
    let spans = harness.spans()?;
    for span in &spans {
        let attributes = attributes_of(span)?;
        assert_eq!(attributes.get("telmoni.agent"), Some(&json!(smiles(128))));
        assert_eq!(attributes.get("user.id"), None);
        assert_eq!(attributes.get("gen_ai.conversation.id"), None);
    }
    let agents = starting_with(&spans, "invoke_agent ");
    assert_eq!(agents.len(), 2);
    for span in agents {
        assert_eq!(name_of(span), format!("invoke_agent {}", smiles(128)));
        let attributes = attributes_of(span)?;
        assert_eq!(
            attributes.get("gen_ai.agent.name"),
            Some(&json!(smiles(128)))
        );
        assert_eq!(attributes.get("error.type"), Some(&json!("E".repeat(128))));
    }
    let mut warnings = harness.warnings.all();
    warnings.sort();
    assert_eq!(
        warnings,
        [
            "telmoni: a session id must be up to 200 characters; the trace is sent without it",
            "telmoni: a span's error type must be up to 128 characters; it is sent cut to its first 128",
            "telmoni: a trace's agent must be up to 128 characters; it is sent cut to its first 128",
            "telmoni: a user id must be up to 128 characters; the trace is sent without it",
        ]
    );
    Ok(())
}

/// Counted in characters, as ingest counts them, never in UTF-8's bytes.
#[test]
fn a_traces_text_at_ingests_caps_is_sent_whole() -> Outcome {
    let harness = Harness::new()?;
    let smiles = |n: usize| "😀".repeat(n);
    let trace = harness.telmoni.trace(
        &smiles(128),
        TraceOptions {
            user_id: Some(smiles(128)),
            session_id: Some(smiles(200)),
            ..TraceOptions::default()
        },
    )?;
    let id = trace.trace_id().to_owned();
    trace.fail(&smiles(128));
    trace.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let attributes = attributes_of(trace_span(&spans, &id)?)?;
    for (key, want) in [
        ("telmoni.agent", smiles(128)),
        ("user.id", smiles(128)),
        ("gen_ai.conversation.id", smiles(200)),
        ("error.type", smiles(128)),
    ] {
        assert_eq!(attributes.get(key), Some(&json!(want)), "{key}");
    }
    assert!(harness.warnings.all().is_empty());
    Ok(())
}

#[test]
fn a_trace_is_shared_across_threads() -> Outcome {
    fn send_and_sync<T: Send + Sync>() {}
    send_and_sync::<Telmoni>();
    send_and_sync::<Trace>();
    send_and_sync::<Step>();
    send_and_sync::<ModelCall>();
    send_and_sync::<ToolCall>();

    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let opened =
        std::thread::scope(|scope| scope.spawn(|| trace.step("threaded").map(Step::end)).join());
    assert!(
        matches!(opened, Ok(Ok(()))),
        "the step opened on its thread"
    );
    let trace_id = trace.trace_id().to_owned();
    trace.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let step = named(&spans, "threaded")?;
    assert_eq!(step.get("traceId"), Some(&json!(trace_id)));
    assert_eq!(
        step.get("parentSpanId"),
        only(&spans, "invoke_agent ")?.get("spanId")
    );
    Ok(())
}

/// A failure's message leaves only with `capture_content` and the mode `on`.
#[test]
fn a_failure_leaves_with_its_message_only_when_asked_and_the_project_allows() -> Outcome {
    for capture_content in [true, false] {
        let harness = Harness::answering(
            Config {
                capture_content,
                ..keyed()
            },
            // On to each export, a trace's opening its own among them.
            std::iter::repeat_n(Ok(mode_on()), 4),
        )?;
        harness
            .telmoni
            .trace("warm", TraceOptions::default())?
            .end();
        harness.flush()?;
        let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
        let stepped: Result<(), Failed> =
            trace.try_step_with("plan", |_step| Err(Failed::Timeout("bad plan".to_owned())));
        assert!(stepped.is_err());
        trace.fail("RateLimit");
        trace.end();
        harness.flush()?;
        let spans = harness.spans()?;
        let plan = named(&spans, "plan")?;
        let ran = named(&spans, "invoke_agent bot")?;
        // A type alone carries no message to record.
        assert_eq!(ran.get("status"), Some(&json!({ "code": 2 })));
        assert!(ran.get("events").is_none());
        if capture_content {
            assert_eq!(
                plan.get("status"),
                Some(&json!({ "code": 2, "message": "timed out: bad plan" }))
            );
            let events = plan
                .get("events")
                .and_then(Value::as_array)
                .ok_or("events")?;
            let [event] = events.as_slice() else {
                return Err(format!("one event, not {events:?}").into());
            };
            assert_eq!(event.get("name"), Some(&json!("exception")));
            assert_eq!(
                attributes_of(event)?,
                json!({ "exception.type": "Failed", "exception.message": "timed out: bad plan" })
            );
        } else {
            assert_eq!(plan.get("status"), Some(&json!({ "code": 2 })));
            assert!(plan.get("events").is_none());
            assert!(!harness.transport.text().contains("bad plan"));
        }
        assert_eq!(
            attributes_of(plan)?.get("error.type"),
            Some(&json!("Failed"))
        );
    }
    Ok(())
}

/// An event the application adds leaves only with `capture_content` and the mode `on`.
#[test]
fn an_applications_event_leaves_only_when_asked_and_the_project_allows() -> Outcome {
    for capture_content in [true, false] {
        let harness = Harness::answering(
            Config {
                capture_content,
                ..keyed()
            },
            // On to each export, a trace's opening its own among them.
            std::iter::repeat_n(Ok(mode_on()), 4),
        )?;
        harness
            .telmoni
            .trace("warm", TraceOptions::default())?
            .end();
        harness.flush()?;
        let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
        trace
            .context()
            .span()
            .add_event("cache miss", vec![KeyValue::new("key", "k1")]);
        trace.end();
        harness.flush()?;
        let spans = harness.spans()?;
        let sent = named(&spans, "invoke_agent bot")?.get("events").is_some();
        assert_eq!(sent, capture_content);
        assert_eq!(
            harness.transport.text().contains("cache miss"),
            capture_content
        );
    }
    Ok(())
}

/// A trace's own messages leave only with `capture_content` and the mode `on`.
#[test]
fn a_traces_own_messages_leave_only_when_asked_and_the_project_allows() -> Outcome {
    for capture_content in [true, false] {
        let harness = Harness::answering(
            Config {
                capture_content,
                ..keyed()
            },
            // On to each export, a trace's opening its own among them.
            std::iter::repeat_n(Ok(mode_on()), 4),
        )?;
        harness
            .telmoni
            .trace("warm", TraceOptions::default())?
            .end();
        harness.flush()?;
        let trace = harness.telmoni.trace(
            "bot",
            TraceOptions {
                input_messages: Some(json!([{ "role": "user", "content": "hi" }]).into()),
                ..TraceOptions::default()
            },
        )?;
        trace.answered(TraceReply {
            output_messages: Some("hello".into()),
        });
        trace.end();
        harness.flush()?;
        let spans = harness.spans()?;
        let sent = attributes_of(named(&spans, "invoke_agent bot")?)?;
        if capture_content {
            assert_eq!(
                sent.get("gen_ai.input.messages"),
                Some(&json!(r#"[{"content":"hi","role":"user"}]"#))
            );
            assert_eq!(sent.get("gen_ai.output.messages"), Some(&json!("hello")));
        } else {
            assert!(sent.get("gen_ai.input.messages").is_none());
            assert!(!harness.transport.text().contains("hello"));
        }
    }
    Ok(())
}
