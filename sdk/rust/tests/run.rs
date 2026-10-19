//! A run: its span and attributes, its start sent at once, what fails and
//! cancels it, the closures, the sample rate, and what it refuses.

mod common;

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

use common::{
    FakeTransport, Harness, Outcome, Sleeps, Warnings, attributes_of, hooks, is_uuid_v4, keyed,
    lock, name_of, named, only, run_span, starting_with, tags,
};
use serde_json::{Value, json};
use telmoni_sdk::{Bounds, Config, Error, ModelCall, Run, RunOptions, Step, Telmoni, ToolCall};

/// An application's error, with the SDK's own among its causes so
/// `try_run_with` can hand a run that did not open back as one of these.
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
fn a_run_is_a_span_with_the_contract_attributes() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run(
        "support-bot",
        RunOptions {
            user_id: Some("user_1".to_owned()),
            session_id: Some("conv_1".to_owned()),
            tags: tags(&[("env", "test"), ("team", "a")]),
            bounds: Bounds {
                stall_after_s: Some(30),
                max_steps: Some(5),
                max_cost_usd: Some(0.5),
            },
            sample_rate: None,
        },
    )?;
    assert!(is_uuid_v4(run.run_id()), "{}", run.run_id());
    assert_eq!(run.agent(), "support-bot");
    let run_id = run.run_id().to_owned();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let span = only(&spans, "invoke_agent ")?;
    assert_eq!(span.get("kind"), Some(&json!(1)));
    assert!(span.get("parentSpanId").is_none());
    assert!(span.get("status").is_none());
    assert_eq!(
        attributes_of(span)?,
        json!({
            "gen_ai.operation.name": "invoke_agent",
            "gen_ai.agent.name": "support-bot",
            "telmoni.run_id": run_id,
            "telmoni.agent": "support-bot",
            "user.id": "user_1",
            "gen_ai.conversation.id": "conv_1",
            "telmoni.bound.stall_after_s": 30,
            "telmoni.bound.max_steps": 5,
            "telmoni.bound.max_cost_usd": 0.5,
            "telmoni.sample_rate": 1.0,
            "telmoni.tag.env": "test",
            "telmoni.tag.team": "a",
        })
    );
    Ok(())
}

#[test]
fn the_start_is_sent_at_once_with_the_bounds() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run(
        "bot",
        RunOptions {
            bounds: Bounds {
                max_steps: Some(3),
                ..Bounds::default()
            },
            ..RunOptions::default()
        },
    )?;
    assert!(harness.transport.wait_for(1));
    let spans = harness.spans()?;
    assert_eq!(
        spans.iter().map(name_of).collect::<Vec<_>>(),
        ["telmoni.run.start"]
    );
    let start = spans.first().ok_or("the start")?;
    assert_eq!(
        start.get("startTimeUnixNano"),
        start.get("endTimeUnixNano"),
        "no duration"
    );
    assert_eq!(
        attributes_of(start)?,
        json!({
            "telmoni.run_id": run.run_id(),
            "telmoni.agent": "bot",
            "telmoni.bound.max_steps": 3,
            "telmoni.sample_rate": 1.0,
        })
    );
    run.end();
    harness.flush()?;
    let spans = harness.spans()?;
    assert_eq!(
        only(&spans, "invoke_agent ")?.get("spanId"),
        spans.first().ok_or("the start")?.get("parentSpanId"),
        "the start is the run's child"
    );
    Ok(())
}

#[test]
fn an_err_from_the_closure_fails_the_run_with_its_type_alone() -> Outcome {
    let harness = Harness::new()?;
    let result: Result<(), Failed> =
        harness
            .telmoni
            .try_run_with("bot", RunOptions::default(), |_run| {
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
fn fail_and_cancel_mark_the_run() -> Outcome {
    let harness = Harness::new()?;
    let failed = harness.telmoni.run("bot", RunOptions::default())?;
    let failed_id = failed.run_id().to_owned();
    failed.fail("RateLimit");
    failed.end();
    let cancelled = harness.telmoni.run("bot", RunOptions::default())?;
    let cancelled_id = cancelled.run_id().to_owned();
    cancelled.cancel();
    cancelled.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let failed = run_span(&spans, &failed_id)?;
    assert_eq!(
        attributes_of(failed)?.get("error.type"),
        Some(&json!("RateLimit"))
    );
    assert_eq!(failed.get("status"), Some(&json!({ "code": 2 })));
    let cancelled = run_span(&spans, &cancelled_id)?;
    assert_eq!(
        attributes_of(cancelled)?.get("telmoni.cancelled"),
        Some(&json!(true))
    );
    assert!(cancelled.get("status").is_none(), "neither ok nor an error");
    Ok(())
}

/// Failed with an empty type, a run is failed with none, since ingest
/// refuses a span whose `error.type` is empty, and the run with it.
#[test]
fn a_fail_with_an_empty_type_fails_the_run_with_none() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let id = run.run_id().to_owned();
    run.fail("");
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let failed = run_span(&spans, &id)?;
    assert_eq!(attributes_of(failed)?.get("error.type"), None);
    assert_eq!(failed.get("status"), Some(&json!({ "code": 2 })));
    Ok(())
}

#[test]
fn a_panic_unwinding_through_the_guard_fails_it_as_a_panic() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let run_id = run.run_id().to_owned();
    let unwound = std::panic::catch_unwind(AssertUnwindSafe(move || {
        let _run = run;
        panic!("the message, which is content and never sent");
    }));
    assert!(unwound.is_err());
    harness.flush()?;
    let spans = harness.spans()?;
    let span = run_span(&spans, &run_id)?;
    assert_eq!(span.get("status"), Some(&json!({ "code": 2 })));
    assert_eq!(
        attributes_of(span)?.get("error.type"),
        Some(&json!("panic"))
    );
    assert!(!harness.transport.text().contains("never sent"));
    Ok(())
}

#[test]
fn the_closure_opens_a_run_per_call_and_returns_its_value() -> Outcome {
    let harness = Harness::new()?;
    let mut seen = Vec::new();
    for question in ["a", "b"] {
        let answer = harness
            .telmoni
            .run_with("bot", RunOptions::default(), |run| {
                seen.push(run.run_id().to_owned());
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
fn the_sample_rate_drops_whole_runs_in_the_processor() -> Outcome {
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

    {
        let dropped = telmoni.run("bot", RunOptions::default())?;
        let step = dropped.step("s");
        step.end();
        dropped.end();
    }
    let kept = telmoni.run("bot", RunOptions::default())?;
    let kept_id = kept.run_id().to_owned();
    kept.end();
    assert!(telmoni.flush());
    telmoni.shutdown();

    let spans = transport.spans()?;
    let run_ids: HashSet<_> = spans
        .iter()
        .map(|span| Ok(attributes_of(span)?.get("telmoni.run_id").cloned()))
        .collect::<Result<_, common::Failure>>()?;
    assert_eq!(run_ids, HashSet::from([Some(json!(kept_id))]));
    assert_eq!(
        spans.iter().map(name_of).collect::<HashSet<_>>(),
        HashSet::from(["telmoni.run.start", "invoke_agent bot"])
    );
    assert_eq!(
        attributes_of(spans.first().ok_or("a span")?)?.get("telmoni.sample_rate"),
        Some(&json!(0.25))
    );
    assert!(lock(&draws).is_empty(), "one draw per run");
    Ok(())
}

#[test]
fn a_span_ending_after_its_sampled_out_run_is_dropped_too() -> Outcome {
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

    let dropped = telmoni.run("bot", RunOptions::default())?;
    let late = dropped.step("late");
    dropped.end();
    late.end();
    let kept = telmoni.run("bot", RunOptions::default())?;
    let kept_id = kept.run_id().to_owned();
    kept.end();
    assert!(telmoni.flush());
    telmoni.shutdown();

    let run_ids: HashSet<_> = transport
        .spans()?
        .iter()
        .map(|span| Ok(attributes_of(span)?.get("telmoni.run_id").cloned()))
        .collect::<Result<_, common::Failure>>()?;
    assert_eq!(run_ids, HashSet::from([Some(json!(kept_id))]));
    Ok(())
}

#[test]
fn a_run_needs_an_agent_and_a_valid_rate() -> Outcome {
    let harness = Harness::new()?;
    assert!(matches!(
        harness.telmoni.run(" ", RunOptions::default()),
        Err(Error::EmptyAgent)
    ));
    for rate in [0.0, 1.5, -0.5, f64::NAN] {
        assert!(matches!(
            harness.telmoni.run(
                "bot",
                RunOptions {
                    sample_rate: Some(rate),
                    ..RunOptions::default()
                },
            ),
            Err(Error::SampleRate(_))
        ));
    }
    assert!(matches!(
        harness
            .telmoni
            .run_with(" ", RunOptions::default(), |_run| ()),
        Err(Error::EmptyAgent)
    ));
    let result: Result<(), Failed> =
        harness
            .telmoni
            .try_run_with(" ", RunOptions::default(), |_run| Ok(()));
    assert!(matches!(result, Err(Failed::Sdk(Error::EmptyAgent))));

    let misconfigured = Harness::with(Config {
        sample_rate: 0.0,
        ..keyed()
    })?;
    assert!(matches!(
        misconfigured.telmoni.run("bot", RunOptions::default()),
        Err(Error::SampleRate(_))
    ));

    harness.flush()?;
    assert!(harness.transport.requests().is_empty(), "nothing opened");
    Ok(())
}

#[test]
fn a_cost_bound_that_is_no_finite_number_is_left_off_with_one_warning() -> Outcome {
    let harness = Harness::new()?;
    for usd in [f64::INFINITY, f64::NAN] {
        let run = harness.telmoni.run(
            "bot",
            RunOptions {
                bounds: Bounds {
                    max_steps: Some(5),
                    max_cost_usd: Some(usd),
                    ..Bounds::default()
                },
                ..RunOptions::default()
            },
        )?;
        run.end();
    }
    harness.flush()?;
    let spans = harness.spans()?;
    let runs = starting_with(&spans, "invoke_agent ");
    assert_eq!(runs.len(), 2, "every run sent, and the batch with them");
    for span in runs {
        let attributes = attributes_of(span)?;
        assert_eq!(attributes.get("telmoni.bound.max_cost_usd"), None);
        assert_eq!(attributes.get("telmoni.bound.max_steps"), Some(&json!(5)));
    }
    let warnings = harness.warnings.all();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|w| w.contains("max_cost_usd must be a finite number"))
    );
    Ok(())
}

/// A bound ingest would refuse the run over is left off with one warning per bound.
#[test]
fn a_bound_outside_ingests_range_is_left_off_with_one_warning() -> Outcome {
    let harness = Harness::new()?;
    let refused = [
        Bounds {
            stall_after_s: Some(0),
            max_steps: Some(-1),
            max_cost_usd: Some(0.0),
        },
        Bounds {
            stall_after_s: Some(i64::from(u32::MAX) + 1),
            max_steps: Some(i64::MAX),
            max_cost_usd: Some(-0.0),
        },
        Bounds {
            stall_after_s: Some(i64::MIN),
            max_steps: Some(0),
            max_cost_usd: Some(-2.5),
        },
    ];
    for bounds in refused {
        let run = harness.telmoni.run(
            "bot",
            RunOptions {
                bounds,
                ..RunOptions::default()
            },
        )?;
        run.end();
    }
    let kept = harness.telmoni.run(
        "bot",
        RunOptions {
            bounds: Bounds {
                stall_after_s: Some(1),
                max_steps: Some(i64::from(u32::MAX)),
                max_cost_usd: Some(f64::MIN_POSITIVE),
            },
            ..RunOptions::default()
        },
    )?;
    let kept_id = kept.run_id().to_owned();
    kept.end();
    harness.flush()?;

    let spans = harness.spans()?;
    assert_eq!(spans.len(), 8, "every run sent, with its start");
    for span in &spans {
        let attributes = attributes_of(span)?;
        if attributes.get("telmoni.run_id") == Some(&json!(kept_id)) {
            assert_eq!(
                attributes.get("telmoni.bound.stall_after_s"),
                Some(&json!(1))
            );
            assert_eq!(
                attributes.get("telmoni.bound.max_steps"),
                Some(&json!(4_294_967_295_i64))
            );
            assert!(
                attributes
                    .get("telmoni.bound.max_cost_usd")
                    .and_then(Value::as_f64)
                    .is_some_and(|usd| usd > 0.0)
            );
            continue;
        }
        let bounds: Vec<&String> = attributes
            .as_object()
            .ok_or("attributes")?
            .keys()
            .filter(|key| key.starts_with("telmoni.bound."))
            .collect();
        assert!(bounds.is_empty(), "{bounds:?}");
    }
    assert_eq!(
        harness.warnings.all(),
        [
            "telmoni: a run's stall_after_s must be a whole number from 1 to 4294967295; the run is sent without it",
            "telmoni: a run's max_steps must be a whole number from 1 to 4294967295; the run is sent without it",
            "telmoni: a run's max_cost_usd must be a finite number above 0; the run is sent without it",
        ]
    );
    Ok(())
}

/// Only a run's first 32 tags by key are sent, warned of once, so what its end sets still fits.
#[test]
fn a_run_sends_its_first_32_tags_and_warns_once_of_the_rest() -> Outcome {
    let harness = Harness::new()?;
    let many: BTreeMap<String, String> =
        (0..200).map(|n| (format!("k{n}"), n.to_string())).collect();
    let mut run_ids = Vec::new();
    for _ in 0..2 {
        let run = harness.telmoni.run(
            "bot",
            RunOptions {
                tags: many.clone(),
                ..RunOptions::default()
            },
        )?;
        run.fail("Overrun");
        run.cancel();
        run_ids.push(run.run_id().to_owned());
        run.end();
    }
    let exact = Harness::new()?;
    let run = exact.telmoni.run(
        "bot",
        RunOptions {
            tags: many.clone().into_iter().take(32).collect(),
            ..RunOptions::default()
        },
    )?;
    let exact_id = run.run_id().to_owned();
    run.end();
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
    for run_id in &run_ids {
        let span = run_span(&spans, run_id)?;
        let attributes = attributes_of(span)?;
        assert_eq!(attributes.get("error.type"), Some(&json!("Overrun")));
        assert_eq!(tags_of(span)?, first);
        assert_eq!(attributes.get("telmoni.cancelled"), Some(&json!(true)));
    }
    assert_eq!(
        harness.warnings.all(),
        ["telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent"]
    );
    assert_eq!(tags_of(run_span(&exact.spans()?, &exact_id)?)?, first);
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
    let mut run_ids = Vec::new();
    for _ in 0..2 {
        let run = harness.telmoni.run(
            "bot",
            RunOptions {
                tags: given.clone(),
                ..RunOptions::default()
            },
        )?;
        run_ids.push(run.run_id().to_owned());
        run.end();
    }
    harness.flush()?;

    let expected: BTreeMap<String, Value> = kept
        .into_iter()
        .map(|(key, value)| (key, json!(value)))
        .collect();
    let spans = harness.spans()?;
    for run_id in &run_ids {
        let sent: BTreeMap<String, Value> = attributes_of(run_span(&spans, run_id)?)?
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

/// A warning sink that panics never reaches the agent's call: the run and its step still go.
#[test]
fn a_warning_sink_that_panics_never_reaches_the_agent() -> Outcome {
    let transport = FakeTransport::new([])?;
    let warnings = Warnings::default();
    let sleeps = Sleeps::default();
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.warn = Box::new(|text| std::panic::panic_any(text.to_owned()));
    let telmoni = Telmoni::with_hooks(keyed(), hooks);
    let run = telmoni.run(
        "bot",
        RunOptions {
            tags: (0..40).map(|n| (format!("k{n}"), n.to_string())).collect(),
            bounds: Bounds {
                max_cost_usd: Some(f64::NAN),
                ..Bounds::default()
            },
            ..RunOptions::default()
        },
    )?;
    let run_id = run.run_id().to_owned();
    run.step("s").end();
    run.end();
    assert!(telmoni.flush());
    telmoni.shutdown();

    let spans = transport.spans()?;
    assert_eq!(name_of(run_span(&spans, &run_id)?), "invoke_agent bot");
    assert!(named(&spans, "s").is_ok(), "the step was not sent");
    Ok(())
}

#[test]
fn a_run_is_shared_across_threads() -> Outcome {
    fn send_and_sync<T: Send + Sync>() {}
    send_and_sync::<Telmoni>();
    send_and_sync::<Run>();
    send_and_sync::<Step>();
    send_and_sync::<ModelCall>();
    send_and_sync::<ToolCall>();

    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let step = run.step("threaded");
            step.end();
        });
    });
    let run_id = run.run_id().to_owned();
    run.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let step = named(&spans, "threaded")?;
    assert_eq!(
        attributes_of(step)?.get("telmoni.run_id"),
        Some(&json!(run_id))
    );
    assert_eq!(
        step.get("parentSpanId"),
        only(&spans, "invoke_agent ")?.get("spanId")
    );
    Ok(())
}
