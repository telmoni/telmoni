//! A retrieval: the conventions' names, its query and documents as content
//! beside their count, the last list it is handed, and the children it opens.

mod common;

use std::sync::{Arc, Barrier};
use std::thread;

use common::{Failure, Harness, Outcome, attributes_of, keyed, mode_on, name_of, named, only};
use serde_json::{Value, json};
use telmoni::{Config, ModelCallOptions, RetrievalOptions, TraceOptions};

fn documents() -> Vec<Value> {
    vec![
        json!({ "id": "kb-112", "score": 0.91 }),
        json!({ "id": "kb-087", "score": 0.84 }),
    ]
}

/// A client whose content leaves: asked for, and the project's mode on.
fn keeping(config: Config) -> Result<Harness, Failure> {
    Harness::answering(
        Config {
            capture_content: true,
            ..config
        },
        std::iter::repeat_n(Ok(mode_on()), 16),
    )
}

fn searches(spans: &[Value]) -> Vec<&Value> {
    spans
        .iter()
        .filter(|span| name_of(span).starts_with("retrieval") || name_of(span) == "search_memory")
        .collect()
}

/// A content attribute's JSON, decoded.
fn decoded(attributes: &Value, key: &str) -> Result<Value, Failure> {
    let text = attributes
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} was not sent: {attributes}"))?;
    Ok(serde_json::from_str(text)?)
}

#[test]
fn a_retrieval_carries_the_conventions_names() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace(
        "bot",
        TraceOptions {
            user_id: Some("user".to_owned()),
            ..TraceOptions::default()
        },
    )?;
    let found = trace.retrieval(
        "kb-articles",
        RetrievalOptions {
            query: Some("refunds".into()),
            top_k: Some(5),
            ..RetrievalOptions::default()
        },
    )?;
    found.returned(&documents());
    found.end();
    let trace_id = trace.trace_id().to_owned();
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let span = only(&spans, "retrieval")?;
    assert_eq!(span.get("name"), Some(&json!("retrieval kb-articles")));
    assert_eq!(span.get("kind"), Some(&json!(3)));
    assert_eq!(span.get("traceId"), Some(&json!(trace_id)));
    assert_eq!(
        attributes_of(span)?,
        json!({
            "telmoni.agent": "bot",
            "user.id": "user",
            "gen_ai.operation.name": "retrieval",
            "gen_ai.data_source.id": "kb-articles",
            "gen_ai.retrieval.top_k": 5,
            "telmoni.retrieval.returned": 2,
        })
    );
    let text = harness.transport.text();
    assert!(!text.contains("refunds"));
    assert!(!text.contains("kb-112"));
    Ok(())
}

#[test]
fn a_memory_search_goes_under_the_memory_names() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let step = trace.step("recall")?;
    let found = step.retrieval(
        "preferences",
        RetrievalOptions {
            top_k: Some(0),
            memory: true,
            ..RetrievalOptions::default()
        },
    )?;
    found.returned(&[json!("mem-41")]);
    found.end();
    step.end();
    let trace_id = trace.trace_id().to_owned();
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let span = named(&spans, "search_memory")?;
    assert_eq!(span.get("kind"), Some(&json!(3)));
    assert_eq!(span.get("traceId"), Some(&json!(trace_id)));
    assert_eq!(
        attributes_of(span)?,
        json!({
            "telmoni.agent": "bot",
            "gen_ai.operation.name": "search_memory",
            "gen_ai.memory.store.id": "preferences",
            "gen_ai.retrieval.top_k": 0,
            "gen_ai.memory.record.count": 1,
        })
    );
    assert_eq!(
        span.get("parentSpanId"),
        named(&spans, "recall")?.get("spanId")
    );
    Ok(())
}

/// Unicode's White_Space alone is blank, as it is for an agent.
#[test]
fn a_blank_source_is_named_by_its_operation_alone_warned_once() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    for source in ["", " \u{3000}"] {
        trace.retrieval(source, RetrievalOptions::default())?.end();
    }
    trace
        .retrieval(
            "\u{85}",
            RetrievalOptions {
                memory: true,
                ..RetrievalOptions::default()
            },
        )?
        .end();
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let found = searches(&spans);
    let names: Vec<&str> = found.iter().map(|span| name_of(span)).collect();
    assert_eq!(names, ["retrieval", "retrieval", "search_memory"]);
    for span in found {
        let attributes = attributes_of(span)?;
        assert!(attributes.get("gen_ai.data_source.id").is_none());
        assert!(attributes.get("gen_ai.memory.store.id").is_none());
    }
    assert_eq!(
        harness.warnings.all(),
        ["telmoni: a retrieval needs its source; one without is sent named by its operation alone"]
    );
    Ok(())
}

/// Ingest names a retrieval by its source, and refuses a name past 145.
#[test]
fn a_source_past_ingests_cap_is_cut_warned_once() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    for source in ["😀".repeat(146), "😀".repeat(146), "😀".repeat(145)] {
        trace.retrieval(&source, RetrievalOptions::default())?.end();
    }
    trace
        .retrieval(
            &"😀".repeat(146),
            RetrievalOptions {
                memory: true,
                ..RetrievalOptions::default()
            },
        )?
        .end();
    trace.end();
    harness.flush()?;

    let cut = "😀".repeat(145);
    let spans = harness.spans()?;
    let found = searches(&spans);
    let names: Vec<&str> = found.iter().map(|span| name_of(span)).collect();
    let named_cut = format!("retrieval {cut}");
    let want = [
        named_cut.as_str(),
        named_cut.as_str(),
        named_cut.as_str(),
        "search_memory",
    ];
    assert_eq!(names, want);
    for span in found.iter().take(3) {
        assert_eq!(
            attributes_of(span)?.get("gen_ai.data_source.id"),
            Some(&json!(cut))
        );
    }
    let memory = found.get(3).ok_or("the memory search")?;
    assert_eq!(
        attributes_of(memory)?.get("gen_ai.memory.store.id"),
        Some(&json!(cut))
    );
    assert_eq!(
        harness.warnings.all(),
        [
            "telmoni: a retrieval's source must be up to 145 characters; it is sent cut to its first 145"
        ]
    );
    Ok(())
}

#[test]
fn the_last_list_wins_whole() -> Outcome {
    let harness = keeping(keyed())?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let mut more = documents();
    more.push(json!({ "id": "kb-203", "score": 0.77 }));
    let found = trace.retrieval("kb", RetrievalOptions::default())?;
    found.returned(&more);
    found.returned(&documents()[..1]);
    found.end();
    trace.end();
    harness.flush()?;

    let attributes = attributes_of(only(&harness.spans()?, "retrieval")?)?;
    assert_eq!(
        decoded(&attributes, "gen_ai.retrieval.documents")?,
        json!(&documents()[..1])
    );
    assert_eq!(
        attributes.get("telmoni.retrieval.returned"),
        Some(&json!(1))
    );
    Ok(())
}

#[test]
fn an_empty_list_is_counted_0_and_sent_as_one() -> Outcome {
    let harness = keeping(keyed())?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    trace
        .retrieval("kb", RetrievalOptions::default())?
        .returned(&[]);
    let recalled = trace.retrieval(
        "prefs",
        RetrievalOptions {
            memory: true,
            ..RetrievalOptions::default()
        },
    )?;
    recalled.returned(&[]);
    recalled.end();
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let retrieval = attributes_of(named(&spans, "retrieval kb")?)?;
    assert_eq!(retrieval.get("telmoni.retrieval.returned"), Some(&json!(0)));
    assert_eq!(
        retrieval.get("gen_ai.retrieval.documents"),
        Some(&json!("[]"))
    );
    let memory = attributes_of(named(&spans, "search_memory")?)?;
    assert_eq!(memory.get("gen_ai.memory.record.count"), Some(&json!(0)));
    assert_eq!(memory.get("gen_ai.memory.records"), Some(&json!("[]")));
    Ok(())
}

#[test]
fn the_query_and_documents_leave_only_under_both_locks() -> Outcome {
    let harness = keeping(keyed())?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let found = trace.retrieval(
        "kb",
        RetrievalOptions {
            query: Some("refunds".into()),
            ..RetrievalOptions::default()
        },
    )?;
    found.returned(&documents());
    found.end();
    let recalled = trace.retrieval(
        "prefs",
        RetrievalOptions {
            query: Some(json!({ "about": "tone" }).into()),
            memory: true,
            ..RetrievalOptions::default()
        },
    )?;
    recalled.returned(&documents()[..1]);
    recalled.end();
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let retrieval = attributes_of(named(&spans, "retrieval kb")?)?;
    assert_eq!(
        retrieval.get("gen_ai.retrieval.query.text"),
        Some(&json!("refunds"))
    );
    assert_eq!(
        decoded(&retrieval, "gen_ai.retrieval.documents")?,
        json!(documents())
    );
    let memory = attributes_of(named(&spans, "search_memory")?)?;
    assert_eq!(
        memory.get("gen_ai.memory.query.text"),
        Some(&json!(r#"{"about":"tone"}"#))
    );
    assert_eq!(
        decoded(&memory, "gen_ai.memory.records")?,
        json!(&documents()[..1])
    );
    assert!(memory.get("gen_ai.retrieval.query.text").is_none());
    Ok(())
}

/// The count is taken from the list handed, before the mask runs.
#[test]
fn a_mask_that_takes_documents_out_leaves_the_count() -> Outcome {
    let harness = keeping(Config {
        mask: Some(Arc::new(|key: &str, value: &str| {
            Some(if key == "gen_ai.retrieval.documents" {
                "[]".to_owned()
            } else {
                value.to_owned()
            })
        })),
        ..keyed()
    })?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let found = trace.retrieval("kb", RetrievalOptions::default())?;
    found.returned(&documents());
    found.end();
    trace.end();
    harness.flush()?;

    let attributes = attributes_of(only(&harness.spans()?, "retrieval")?)?;
    assert_eq!(
        attributes.get("gen_ai.retrieval.documents"),
        Some(&json!("[]"))
    );
    assert_eq!(
        attributes.get("telmoni.retrieval.returned"),
        Some(&json!(2))
    );
    Ok(())
}

/// Lists handed at once leave one call's documents beside that call's count.
#[test]
fn lists_returned_at_once_from_two_threads_leave_one_calls_pair() -> Outcome {
    const ROUNDS: usize = 1_000;
    let harness = keeping(keyed())?;
    let trace = harness.telmoni.trace("raced", TraceOptions::default())?;
    for round in 0..ROUNDS {
        let found = trace.retrieval("kb", RetrievalOptions::default())?;
        let start = Barrier::new(2);
        let (start, returning) = (&start, &found);
        thread::scope(|scope| {
            for size in [1, 3] {
                scope.spawn(move || {
                    let list = vec![json!("kb-112"); size];
                    start.wait();
                    // Handed again and again, so the two threads' calls overlap.
                    for _ in 0..8 {
                        returning.returned(&list);
                    }
                });
            }
        });
        found.end();
        // Within the queue's 2,048 spans.
        if round % 256 == 255 {
            harness.flush()?;
        }
    }
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let found = searches(&spans);
    assert_eq!(found.len(), ROUNDS);
    let mut astray = 0;
    for span in found {
        let attributes = attributes_of(span)?;
        let sent = decoded(&attributes, "gen_ai.retrieval.documents")?;
        let held = sent.as_array().map(Vec::len).ok_or("a list")?;
        if attributes.get("telmoni.retrieval.returned") != Some(&json!(held)) {
            astray += 1;
        }
    }
    assert_eq!(
        astray, 0,
        "{astray} of {ROUNDS} retrievals sent one call's documents beside another's count"
    );
    Ok(())
}

#[test]
fn a_retrieval_opens_children_of_its_own() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let found = trace.retrieval("kb", RetrievalOptions::default())?;
    found
        .model_call(
            "embed-small",
            ModelCallOptions {
                operation: Some("embeddings".to_owned()),
                ..ModelCallOptions::default()
            },
        )?
        .end();
    found.end();
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    assert_eq!(
        named(&spans, "embeddings embed-small")?.get("parentSpanId"),
        named(&spans, "retrieval kb")?.get("spanId")
    );
    Ok(())
}

#[test]
fn the_retrieval_closures_return_their_value_and_mark_an_err() -> Outcome {
    #[derive(Debug)]
    struct Timeout;

    impl std::fmt::Display for Timeout {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("timed out")
        }
    }

    impl From<telmoni::Error> for Timeout {
        fn from(_: telmoni::Error) -> Self {
            Self
        }
    }

    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let count = trace.retrieval_with("kb", RetrievalOptions::default(), |found| {
        found.returned(&documents());
        documents().len()
    })?;
    assert_eq!(count, 2);
    let failed: Result<(), Timeout> =
        trace.try_retrieval_with("web-index", RetrievalOptions::default(), |_found| {
            Err(Timeout)
        });
    assert!(failed.is_err());
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let web = named(&spans, "retrieval web-index")?;
    assert_eq!(
        attributes_of(web)?.get("error.type"),
        Some(&json!("Timeout"))
    );
    assert_eq!(web.get("status"), Some(&json!({ "code": 2 })));
    assert_eq!(
        attributes_of(named(&spans, "retrieval kb")?)?.get("telmoni.retrieval.returned"),
        Some(&json!(2))
    );
    Ok(())
}
