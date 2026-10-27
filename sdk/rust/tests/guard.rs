//! The guard: a run's caps on its steps and spend, counted in this process as
//! the server counts a call's cost, replayed from `sdk/fixtures/guard.json`.

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use common::{
    Failure, FakeTransport, Harness, Outcome, attributes_of, field, int, keyed, load_fixture, lock,
    named, text,
};
use serde_json::{Value, json};
use telmoni::{
    Cap, Config, Error, Hooks, ModelCall, ModelCallOptions, Price, Reply, Run, RunOptions, Telmoni,
    ToolCallOptions, Usage,
};

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12
}

fn prices(fixture: &Value) -> Result<Vec<Price>, Failure> {
    Ok(serde_json::from_value(field(fixture, "prices")?.clone())?)
}

fn optional_int(value: &Value, key: &str) -> Result<Option<i64>, Failure> {
    value.get(key).map(|_| int(value, key)).transpose()
}

/// One of the fixture's answers to a call.
fn reply(answer: &Value) -> Reply {
    let usage = answer.get("usage").map(|usage| Usage {
        input_tokens: optional_int(usage, "input_tokens").ok().flatten(),
        output_tokens: optional_int(usage, "output_tokens").ok().flatten(),
        cache_read_input_tokens: optional_int(usage, "cache_read_input_tokens")
            .ok()
            .flatten(),
        cache_write_input_tokens: optional_int(usage, "cache_write_input_tokens")
            .ok()
            .flatten(),
        cost: usage.get("cost").and_then(Value::as_f64),
    });
    Reply {
        response_model: answer
            .get("response_model")
            .and_then(Value::as_str)
            .map(str::to_owned),
        usage,
        ..Reply::default()
    }
}

/// Opens `spec`'s model call under `run` and answers it as the fixture does.
fn call(run: &Run, spec: &Value) -> Result<(), Error> {
    let model_call = run.model_call(
        spec.get("model")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        ModelCallOptions {
            provider: spec
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_owned),
            ..ModelCallOptions::default()
        },
    )?;
    match spec.get("answers").and_then(Value::as_array) {
        Some(answers) => answers
            .iter()
            .for_each(|answer| model_call.answered(reply(answer))),
        None => model_call.answered(reply(spec)),
    }
    model_call.end();
    Ok(())
}

/// Does `op` under `run`.
fn perform(run: &Run, op: &Value, calls: &[Value]) -> Result<(), Error> {
    match op.as_str() {
        Some("step") => run.step("step").map(telmoni::Step::end),
        Some("tool_call") => run
            .tool_call("lookup", ToolCallOptions::default())
            .map(telmoni::ToolCall::end),
        _ => {
            let n = op
                .get("model_call")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok());
            let spec = n.and_then(|n| calls.get(n)).cloned().unwrap_or(Value::Null);
            call(run, &spec)
        }
    }
}

#[test]
fn each_call_is_counted_as_the_server_counts_it() -> Outcome {
    let fixture = load_fixture("guard.json")?;
    let harness = Harness::with(Config {
        prices: prices(&fixture)?,
        ..keyed()
    })?;
    for spec in field(&fixture, "calls")?.as_array().ok_or("calls")? {
        let run = harness.telmoni.run("guarded", RunOptions::default())?;
        call(&run, spec)?;
        let counted = field(spec, "counted")?.as_f64().ok_or("counted")?;
        // Exactly, as the server rounds each product and each sum.
        assert_eq!(run.spent(), counted, "{}", text(spec, "name")?);
        run.end();
    }
    Ok(())
}

#[test]
fn a_run_past_a_cap_is_refused_what_it_opens_next() -> Outcome {
    let fixture = load_fixture("guard.json")?;
    let calls = field(&fixture, "calls")?.as_array().ok_or("calls")?;
    for spec in field(&fixture, "runs")?.as_array().ok_or("runs")? {
        let name = text(spec, "name")?;
        let harness = Harness::with(Config {
            prices: prices(&fixture)?,
            ..keyed()
        })?;
        let run = harness.telmoni.run(
            "guarded",
            RunOptions {
                max_steps: spec.get("max_steps").and_then(Value::as_u64),
                max_spend: spec.get("max_spend").and_then(Value::as_f64),
                ..RunOptions::default()
            },
        )?;
        let refused = spec.get("refused").filter(|refused| !refused.is_null());
        let at = refused
            .map(|refused| int(refused, "at"))
            .transpose()?
            .and_then(|at| usize::try_from(at).ok());
        let ops = field(spec, "ops")?.as_array().ok_or("ops")?;
        let mut performed = ops.len();
        for (n, op) in ops.iter().enumerate() {
            let result = perform(&run, op, calls);
            if Some(n) != at {
                result.map_err(|e| format!("{name}: op {n} was refused: {e}"))?;
                continue;
            }
            let Err(Error::CapExceeded { cap, limit, used }) = result else {
                return Err(format!("{name}: op {n} opened past its cap: {result:?}").into());
            };
            let refused = refused.ok_or("a refusal")?;
            let want = match text(refused, "cap")? {
                "steps" => Cap::Steps,
                _ => Cap::Spend,
            };
            assert_eq!(cap, want, "{name}");
            assert!(
                near(limit, field(refused, "limit")?.as_f64().ok_or("limit")?),
                "{name}"
            );
            assert!(
                near(used, field(refused, "used")?.as_f64().ok_or("used")?),
                "{name}: {used}"
            );
            performed = n;
            break;
        }
        let spent = field(spec, "spent")?.as_f64().ok_or("spent")?;
        assert!(near(run.spent(), spent), "{name}: spent {}", run.spent());
        run.end();
        harness.flush()?;
        let opened = harness
            .spans()?
            .iter()
            .filter(|span| span.get("name") != Some(&json!("invoke_agent guarded")))
            .count();
        assert_eq!(opened, performed, "{name}: a refused op opens nothing");
    }
    Ok(())
}

#[test]
fn a_refusal_says_what_the_other_sdks_say() -> Outcome {
    let fixture = load_fixture("guard.json")?;
    for spec in field(&fixture, "refusals")?.as_array().ok_or("refusals")? {
        let refusal = Error::CapExceeded {
            cap: match text(spec, "cap")? {
                "steps" => Cap::Steps,
                _ => Cap::Spend,
            },
            limit: field(spec, "limit")?.as_f64().ok_or("limit")?,
            used: field(spec, "used")?.as_f64().ok_or("used")?,
        };
        assert_eq!(refusal.to_string(), text(spec, "says")?);
    }
    Ok(())
}

#[test]
fn a_refusal_carried_out_by_a_try_closure_is_the_callers_error() -> Outcome {
    #[derive(Debug)]
    enum Stopped {
        Cap(Error),
    }

    impl From<Error> for Stopped {
        fn from(error: Error) -> Self {
            Self::Cap(error)
        }
    }

    let harness = Harness::new()?;
    let run = harness.telmoni.run(
        "guarded",
        RunOptions {
            max_steps: Some(1),
            ..RunOptions::default()
        },
    )?;
    let first: Result<u8, Stopped> = run.try_step_with("first", |_step| Ok(1));
    assert!(matches!(first, Ok(1)));
    let second: Result<u8, Stopped> = run.try_step_with("second", |_step| Ok(2));
    assert!(
        matches!(
            second,
            Err(Stopped::Cap(Error::CapExceeded {
                cap: Cap::Steps,
                ..
            }))
        ),
        "{second:?}"
    );
    run.end();
    Ok(())
}

/// A refusal escaping a run fails it with its error's type, `telmoni::Error`.
#[test]
fn a_refusal_escaping_the_run_fails_it_with_its_type() -> Outcome {
    let harness = Harness::new()?;
    let escaped = harness.telmoni.try_run_with(
        "guarded",
        RunOptions {
            max_steps: Some(1),
            ..RunOptions::default()
        },
        |run| {
            run.step("first")?.end();
            run.step("second")?.end();
            Ok::<(), Error>(())
        },
    );
    assert!(
        matches!(
            escaped,
            Err(Error::CapExceeded {
                cap: Cap::Steps,
                ..
            })
        ),
        "{escaped:?}"
    );
    harness.flush()?;
    let spans = harness.spans()?;
    let run = named(&spans, "invoke_agent guarded")?;
    assert_eq!(attributes_of(run)?.get("error.type"), Some(&json!("Error")));
    assert_eq!(run.get("status"), Some(&json!({ "code": 2 })));
    Ok(())
}

#[test]
fn a_table_name_is_priced_by_the_table_the_release_carries() -> Outcome {
    let carried: Value = serde_json::from_str(include_str!("../src/model_prices.json"))?;
    let rows: Vec<Price> = serde_json::from_value(field(&carried, "prices")?.clone())?;
    let row = rows
        .into_iter()
        .find(|price| price.provider == "openai" && price.model == "gpt-4o-mini")
        .ok_or("the price file prices no gpt-4o-mini")?;
    let mut spent = Vec::new();
    for prices in [Vec::new(), vec![row]] {
        let harness = Harness::with(Config { prices, ..keyed() })?;
        let run = harness.telmoni.run("priced", RunOptions::default())?;
        let model_call = run.model_call(
            "gpt-4o-mini",
            ModelCallOptions {
                provider: Some("openai".to_owned()),
                ..ModelCallOptions::default()
            },
        )?;
        model_call.answered(Reply {
            usage: Some(Usage {
                input_tokens: Some(10_000),
                output_tokens: Some(2_000),
                cache_read_input_tokens: Some(4_000),
                ..Usage::default()
            }),
            ..Reply::default()
        });
        model_call.end();
        spent.push(run.spent());
        run.end();
    }
    assert!(spent.first().is_some_and(|spent| *spent > 0.0), "{spent:?}");
    assert_eq!(
        spent.first(),
        spent.get(1),
        "the table's row, as if it were handed"
    );
    Ok(())
}

#[test]
fn a_call_answered_again_counts_what_its_span_holds() -> Outcome {
    let harness = Harness::with(Config {
        prices: vec![Price {
            provider: "p".to_owned(),
            model: "m".to_owned(),
            input: 1e-6,
            output: 1e-5,
            ..Price::default()
        }],
        ..keyed()
    })?;
    let run = harness.telmoni.run("guarded", RunOptions::default())?;
    let model_call = run.model_call(
        "m",
        ModelCallOptions {
            provider: Some("p".to_owned()),
            ..ModelCallOptions::default()
        },
    )?;
    let answer = |usage| Reply {
        usage: Some(usage),
        ..Reply::default()
    };
    // Answers given at once, from threads of the agent's own.
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                model_call.answered(answer(Usage {
                    input_tokens: Some(1_000),
                    ..Usage::default()
                }));
            });
        }
    });
    // A stream's counts, answered as they arrive.
    model_call.answered(answer(Usage {
        input_tokens: Some(2_000),
        ..Usage::default()
    }));
    model_call.answered(answer(Usage {
        output_tokens: Some(100),
        ..Usage::default()
    }));
    model_call.end();
    let spent = run.spent();
    run.end();
    harness.flush()?;
    let held = attributes_of(named(&harness.spans()?, "chat m")?)?;
    let count = |key: &str| held.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN);
    assert!(
        near(
            spent,
            count("gen_ai.usage.input_tokens") * 1e-6 + count("gen_ai.usage.output_tokens") * 1e-5
        ),
        "spent {spent}, the span holding {held}"
    );
    assert!(near(spent, 0.003), "spent {spent}");
    Ok(())
}

/// Two threads answering one call at once leave its span holding what was counted.
#[test]
fn answers_raced_from_two_threads_count_what_the_span_holds() -> Outcome {
    const ROUNDS: usize = 2_000;
    let harness = Harness::with(Config {
        prices: vec![Price {
            provider: "p".to_owned(),
            model: "m".to_owned(),
            input: 1e-6,
            ..Price::default()
        }],
        ..keyed()
    })?;
    let mut spent = HashMap::new();
    for round in 0..ROUNDS {
        let run = harness.telmoni.run("raced", RunOptions::default())?;
        let model_call = run.model_call(
            "m",
            ModelCallOptions {
                provider: Some("p".to_owned()),
                ..ModelCallOptions::default()
            },
        )?;
        let start = Barrier::new(2);
        let (start, answering) = (&start, &model_call);
        thread::scope(|scope| {
            for input in [1_000, 2_000] {
                scope.spawn(move || {
                    start.wait();
                    answering.answered(Reply {
                        usage: Some(Usage {
                            input_tokens: Some(input),
                            ..Usage::default()
                        }),
                        output_messages: Some("the answer".into()),
                        ..Reply::default()
                    });
                });
            }
        });
        model_call.end();
        spent.insert(run.run_id().to_owned(), run.spent());
        run.end();
        // Within the queue's 2,048 spans.
        if round % 256 == 255 {
            harness.flush()?;
        }
    }
    harness.flush()?;
    let spans = harness.spans()?;
    let calls: Vec<&Value> = spans
        .iter()
        .filter(|span| span.get("name") == Some(&json!("chat m")))
        .collect();
    assert_eq!(calls.len(), ROUNDS);
    let mut astray = 0;
    for span in calls {
        let held = attributes_of(span)?;
        let input = held
            .get("gen_ai.usage.input_tokens")
            .and_then(Value::as_f64)
            .ok_or("an input count")?;
        let run_id = held
            .get("telmoni.run_id")
            .and_then(Value::as_str)
            .ok_or("a run id")?;
        let counted = spent.get(run_id).copied().ok_or("its run")?;
        if !near(counted, input * 1e-6) {
            astray += 1;
        }
    }
    assert_eq!(
        astray, 0,
        "{astray} of {ROUNDS} calls counted an answer their span did not hold"
    );
    Ok(())
}

#[test]
fn the_release_carries_the_price_file_whole() -> Outcome {
    let core = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/telemetry/model_prices.json"),
    )?;
    assert!(
        include_str!("../src/model_prices.json") == core,
        "src/model_prices.json is not the core's: make prices writes it"
    );
    Ok(())
}

#[test]
fn a_call_with_neither_under_a_spend_cap_is_warned_of_once() -> Outcome {
    let harness = Harness::new()?;
    let capped = harness.telmoni.run(
        "guarded",
        RunOptions {
            max_spend: Some(1.0),
            ..RunOptions::default()
        },
    )?;
    let unpriced = |run: &Run, model: &str| -> Outcome {
        let model_call = run.model_call(
            model,
            ModelCallOptions {
                provider: Some("vllm".to_owned()),
                ..ModelCallOptions::default()
            },
        )?;
        model_call.answered(Reply {
            usage: Some(Usage {
                input_tokens: Some(10),
                ..Usage::default()
            }),
            ..Reply::default()
        });
        model_call.end();
        Ok(())
    };
    for _ in 0..3 {
        unpriced(&capped, "my-model")?;
    }
    capped.end();
    let free = harness.telmoni.run("unguarded", RunOptions::default())?;
    unpriced(&free, "other-model")?;
    free.end();
    let warned: Vec<String> = harness
        .warnings
        .all()
        .into_iter()
        .filter(|warning| warning.contains("counts nothing"))
        .collect();
    assert_eq!(
        warned,
        [
            "telmoni: no price for my-model (vllm), which reported no cost; it counts nothing toward the run's spend cap"
        ]
    );
    Ok(())
}

/// The application's sink runs with the call's lock released.
#[test]
fn a_sink_answering_the_call_it_warns_of_is_never_held_up() -> Outcome {
    let transport = FakeTransport::new([])?;
    let answering: Arc<Mutex<Option<Arc<ModelCall>>>> = Arc::default();
    let stalled = Arc::new(AtomicBool::new(false));
    let warn = {
        let (answering, stalled) = (Arc::clone(&answering), Arc::clone(&stalled));
        move |_: &str| {
            let Some(call) = lock(&answering).clone() else {
                return;
            };
            let (done, answered) = mpsc::channel();
            thread::spawn(move || {
                call.answered(Reply::default());
                let _ = done.send(());
            });
            if answered.recv_timeout(Duration::from_secs(5)).is_err() {
                stalled.store(true, Ordering::Relaxed);
            }
        }
    };
    let telmoni = Telmoni::with_hooks(
        keyed(),
        Hooks {
            transport: Box::new(transport),
            warn: Box::new(warn),
            ..Hooks::default()
        },
    );
    let run = telmoni.run(
        "guarded",
        RunOptions {
            max_spend: Some(1.0),
            ..RunOptions::default()
        },
    )?;
    let call = Arc::new(run.model_call(
        "my-model",
        ModelCallOptions {
            provider: Some("vllm".to_owned()),
            ..ModelCallOptions::default()
        },
    )?);
    *lock(&answering) = Some(Arc::clone(&call));
    call.answered(Reply {
        usage: Some(Usage {
            input_tokens: Some(10),
            ..Usage::default()
        }),
        ..Reply::default()
    });
    lock(&answering).take();
    assert!(
        !stalled.load(Ordering::Relaxed),
        "the sink ran under the call's lock"
    );
    Ok(())
}

#[test]
fn a_cap_is_above_0() -> Outcome {
    let harness = Harness::new()?;
    let refused = |options: RunOptions| harness.telmoni.run("guarded", options).err();
    assert_eq!(
        refused(RunOptions {
            max_steps: Some(0),
            ..RunOptions::default()
        }),
        Some(Error::MaxSteps)
    );
    for spend in [0.0, -1.0, f64::INFINITY] {
        assert_eq!(
            refused(RunOptions {
                max_spend: Some(spend),
                ..RunOptions::default()
            }),
            Some(Error::MaxSpend(spend))
        );
    }
    assert!(matches!(
        refused(RunOptions {
            max_spend: Some(f64::NAN),
            ..RunOptions::default()
        }),
        Some(Error::MaxSpend(spend)) if spend.is_nan()
    ));
    Ok(())
}

#[test]
fn a_price_handed_that_is_no_price_is_left_out_with_one_warning() -> Outcome {
    let price = |provider: &str, model: &str, input: f64| Price {
        provider: provider.to_owned(),
        model: model.to_owned(),
        input,
        output: 1e-6,
        ..Price::default()
    };
    let harness = Harness::with(Config {
        prices: vec![
            price("", "m", 1e-6),
            price("p", "", 1e-6),
            price("p", "negative", -1e-6),
            price("p", "nan", f64::NAN),
            price("p", "kept", 1e-6),
        ],
        ..keyed()
    })?;
    let warnings = harness.warnings.all();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let run = harness.telmoni.run("priced", RunOptions::default())?;
    for model in ["negative", "kept"] {
        let model_call = run.model_call(
            model,
            ModelCallOptions {
                provider: Some("p".to_owned()),
                ..ModelCallOptions::default()
            },
        )?;
        model_call.answered(Reply {
            usage: Some(Usage {
                input_tokens: Some(1_000),
                ..Usage::default()
            }),
            ..Reply::default()
        });
        model_call.end();
    }
    assert!(
        near(run.spent(), 0.001),
        "the kept price alone: {}",
        run.spent()
    );
    run.end();
    Ok(())
}

#[test]
fn a_cost_goes_as_a_double_and_one_that_is_no_cost_is_left_off() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    for cost in [3.0, 0.0123, -0.5, f64::NAN, f64::INFINITY] {
        let model_call = run.model_call("m", ModelCallOptions::default())?;
        model_call.answered(Reply {
            usage: Some(Usage {
                input_tokens: Some(1),
                cost: Some(cost),
                ..Usage::default()
            }),
            ..Reply::default()
        });
        model_call.end();
    }
    run.end();
    harness.flush()?;
    let sent: Vec<Option<Value>> = harness
        .spans()?
        .iter()
        .filter(|span| span.get("name") == Some(&json!("chat m")))
        .map(|span| {
            span.get("attributes")
                .and_then(Value::as_array)
                .and_then(|attributes| {
                    attributes
                        .iter()
                        .find(|a| a.get("key") == Some(&json!("gen_ai.usage.cost")))
                })
                .and_then(|a| a.get("value").cloned())
        })
        .collect();
    assert_eq!(
        sent,
        [
            Some(json!({ "doubleValue": 3.0 })),
            Some(json!({ "doubleValue": 0.0123 })),
            None,
            None,
            None
        ]
    );
    assert_eq!(
        harness.warnings.all(),
        [
            "telmoni: gen_ai.usage.cost must be a finite number of US dollars from 0; the call is sent without it"
        ]
    );
    Ok(())
}
