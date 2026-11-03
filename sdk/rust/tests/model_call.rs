//! A model call: the conventions' names, content taken off unless asked and
//! allowed, and the error a closure returns.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use common::{
    Failure, Harness, Outcome, attributes_of, keyed, mode_on, name_of, only, starting_with,
};
use serde_json::{Map, Value, json};
use telmoni::transport::{Answer, TransportError};
use telmoni::{Config, Content, ContentMode, ModelCallOptions, Reply, RunOptions, Usage};

#[test]
fn a_model_call_carries_the_conventions_names() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run(
        "bot",
        RunOptions {
            session_id: Some("conv".to_owned()),
            ..RunOptions::default()
        },
    )?;
    let call = run.model_call(
        "gpt-4o-mini",
        ModelCallOptions {
            provider: Some("openai".to_owned()),
            ..ModelCallOptions::default()
        },
    )?;
    call.answered(Reply {
        response_model: Some("gpt-4o-mini-2024-07-18".to_owned()),
        usage: Some(Usage {
            input_tokens: Some(12),
            output_tokens: Some(7),
            cache_read_input_tokens: Some(3),
            cache_write_input_tokens: None,
            cost: None,
        }),
        output_messages: None,
        time_to_first_token: None,
    });
    call.end();
    let run_id = run.run_id().to_owned();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let span = only(&spans, "chat ")?;
    assert_eq!(span.get("name"), Some(&json!("chat gpt-4o-mini")));
    assert_eq!(span.get("kind"), Some(&json!(3)));
    assert_eq!(
        attributes_of(span)?,
        json!({
            "telmoni.run_id": run_id,
            "telmoni.agent": "bot",
            "telmoni.sample_rate": 1.0,
            "gen_ai.conversation.id": "conv",
            "gen_ai.operation.name": "chat",
            "gen_ai.request.model": "gpt-4o-mini",
            "gen_ai.provider.name": "openai",
            "gen_ai.response.model": "gpt-4o-mini-2024-07-18",
            "gen_ai.usage.input_tokens": 12,
            "gen_ai.usage.output_tokens": 7,
            "gen_ai.usage.cache_read.input_tokens": 3,
        })
    );
    Ok(())
}

/// A negative token count is left off its call, with one warning per count.
#[test]
fn a_negative_token_count_is_left_off_with_one_warning() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    for usage in [
        Usage {
            input_tokens: Some(-1),
            output_tokens: Some(0),
            cache_read_input_tokens: Some(i64::MIN),
            cache_write_input_tokens: Some(i64::MAX),
            cost: None,
        },
        Usage {
            input_tokens: Some(-5),
            output_tokens: Some(-1),
            cache_read_input_tokens: Some(0),
            cache_write_input_tokens: Some(-1),
            cost: None,
        },
    ] {
        let call = run.model_call("m", ModelCallOptions::default())?;
        call.answered(Reply {
            usage: Some(usage),
            ..Reply::default()
        });
        call.end();
    }
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let sent = starting_with(&spans, "chat m")
        .into_iter()
        .map(|call| {
            let counts: Map<String, Value> = attributes_of(call)?
                .as_object()
                .ok_or("attributes")?
                .iter()
                .filter(|(key, _)| key.starts_with("gen_ai.usage."))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            Ok(Value::Object(counts))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    assert_eq!(
        sent,
        [
            json!({
                "gen_ai.usage.output_tokens": 0,
                "gen_ai.usage.cache_write.input_tokens": i64::MAX,
            }),
            json!({ "gen_ai.usage.cache_read.input_tokens": 0 }),
        ]
    );
    assert_eq!(
        harness.warnings.all(),
        [
            "telmoni: gen_ai.usage.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
            "telmoni: gen_ai.usage.cache_read.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
            "telmoni: gen_ai.usage.output_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
            "telmoni: gen_ai.usage.cache_write.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
        ]
    );
    Ok(())
}

/// A blank operation is `chat`, as one left out is: sent blank, the span would
/// be named ` m` and ingest would read it as a step.
#[test]
fn a_blank_operation_is_chat() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    for blank in ["", "  "] {
        let call = run.model_call(
            "m",
            ModelCallOptions {
                operation: Some(blank.to_owned()),
                ..ModelCallOptions::default()
            },
        )?;
        call.end();
    }
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let calls = starting_with(&spans, "chat m");
    assert_eq!(calls.len(), 2, "{spans:?}");
    for call in calls {
        assert_eq!(
            attributes_of(call)?.get("gen_ai.operation.name"),
            Some(&json!("chat"))
        );
    }
    Ok(())
}

/// An empty response model or provider is left off: ingest refuses the call over either.
#[test]
fn an_empty_response_model_or_provider_is_left_off() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let call = run.model_call(
        "m",
        ModelCallOptions {
            provider: Some(String::new()),
            ..ModelCallOptions::default()
        },
    )?;
    call.answered(Reply {
        response_model: Some(String::new()),
        ..Reply::default()
    });
    call.end();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let calls = starting_with(&spans, "chat m");
    assert_eq!(calls.len(), 1, "{spans:?}");
    let attributes = attributes_of(calls[0])?;
    assert_eq!(attributes.get("gen_ai.response.model"), None);
    assert_eq!(attributes.get("gen_ai.provider.name"), None);
    Ok(())
}

/// Ingest refuses a call over an empty model, or text past its cap.
#[test]
fn a_model_calls_text_past_ingests_caps_keeps_its_call() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let m = |n: usize| "m".repeat(n);
    let generate = || ModelCallOptions {
        operation: Some("generate_content".to_owned()),
        ..ModelCallOptions::default()
    };
    for _ in 0..2 {
        run.model_call("", ModelCallOptions::default())?.end();
        let call = run.model_call(
            &m(129),
            ModelCallOptions {
                provider: Some("p".repeat(129)),
                ..generate()
            },
        )?;
        call.answered(Reply {
            response_model: Some("r".repeat(129)),
            ..Reply::default()
        });
        call.end();
        let custom = ModelCallOptions {
            operation: Some("o".repeat(20)),
            ..ModelCallOptions::default()
        };
        run.model_call(&m(128), custom)?.end();
    }
    // At their caps: the longest operation and a model at its cap fit.
    let smiles = "😀".repeat(128);
    let call = run.model_call(&smiles, generate())?;
    call.answered(Reply {
        response_model: Some(smiles.clone()),
        ..Reply::default()
    });
    call.end();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let sent: Map<String, Value> = spans
        .iter()
        .filter(|span| name_of(span) != "invoke_agent bot")
        .map(|span| Ok((name_of(span).to_owned(), attributes_of(span)?)))
        .collect::<Result<_, Failure>>()?;
    let names: BTreeSet<String> = sent.keys().cloned().collect();
    let want = BTreeSet::from([
        "chat".to_owned(),
        format!("generate_content {}", m(128)),
        format!("generate_content {smiles}"),
        format!("chat {}", m(128)),
    ]);
    assert_eq!(names, want);
    assert_eq!(sent["chat"].get("gen_ai.request.model"), None);
    let cut = &sent[&format!("generate_content {}", m(128))];
    assert_eq!(cut.get("gen_ai.request.model"), Some(&json!(m(128))));
    assert_eq!(
        cut.get("gen_ai.provider.name"),
        Some(&json!("p".repeat(128)))
    );
    assert_eq!(
        cut.get("gen_ai.response.model"),
        Some(&json!("r".repeat(128)))
    );
    assert_eq!(
        sent[&format!("generate_content {smiles}")].get("gen_ai.response.model"),
        Some(&json!(smiles))
    );
    let mut warnings = harness.warnings.all();
    warnings.sort();
    assert_eq!(
        warnings,
        [
            "telmoni: a model call needs its model; one without is sent named by its operation alone",
            "telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat",
            "telmoni: a model must be up to 128 characters; it is sent cut to its first 128",
            "telmoni: a provider must be up to 128 characters; it is sent cut to its first 128",
            "telmoni: a response model must be up to 128 characters; it is sent cut to its first 128",
        ]
    );
    Ok(())
}

#[test]
fn content_is_taken_off_before_it_leaves() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let call = run.model_call(
        "m",
        ModelCallOptions {
            input_messages: Some(json!([{ "role": "user", "content": "my secret prompt" }]).into()),
            system_instructions: Some("be brief".into()),
            ..ModelCallOptions::default()
        },
    )?;
    call.answered(Reply {
        output_messages: Some(json!([{ "role": "assistant", "content": "the answer" }]).into()),
        ..Reply::default()
    });
    call.end();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let attributes = attributes_of(only(&spans, "chat ")?)?;
    assert!(attributes.get("gen_ai.input.messages").is_none());
    assert!(attributes.get("gen_ai.output.messages").is_none());
    assert!(attributes.get("gen_ai.system_instructions").is_none());
    let text = harness.transport.text();
    assert!(!text.contains("my secret prompt"));
    assert!(!text.contains("the answer"));
    assert!(!text.contains("be brief"));
    Ok(())
}

/// An answer that says nothing of the mode, as a server without the content switch did.
fn mode_unknown() -> Answer {
    Answer::new(200, Vec::new(), br#"{"partialSuccess":null}"#.to_vec())
}

/// ⚠ Content stays home until an answer has said the mode. No answer here
/// names one, so wherever the worker's exports fall against the spans'
/// encoding, nothing has said `on` when the call leaves.
#[test]
fn content_stays_home_until_an_answer_has_said_the_mode() -> Outcome {
    let harness = Harness::answering(
        Config {
            capture_content: true,
            ..keyed()
        },
        std::iter::repeat_n(Ok(mode_unknown()), 4),
    )?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let call = run.model_call(
        "m",
        ModelCallOptions {
            input_messages: Some("first prompt".into()),
            ..ModelCallOptions::default()
        },
    )?;
    call.end();
    run.end();
    harness.flush()?;
    assert_eq!(harness.telmoni.content_mode(), None);
    let spans = harness.spans()?;
    assert!(
        attributes_of(only(&spans, "chat ")?)?
            .get("gen_ai.input.messages")
            .is_none()
    );
    assert!(!harness.transport.text().contains("first prompt"));
    Ok(())
}

/// Every answer says `on`. The first run's flush is what reads it for
/// certain; a run opened after that is encoded knowing the mode, so its
/// content is kept, whatever became of the first run's.
#[test]
fn content_leaves_once_the_server_has_said_on() -> Outcome {
    let harness = Harness::answering(
        Config {
            capture_content: true,
            ..keyed()
        },
        std::iter::repeat_n(Ok(mode_on()), 6),
    )?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    run.end();
    harness.flush()?;
    assert_eq!(harness.telmoni.content_mode(), Some(ContentMode::On));

    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let call = run.model_call(
        "m",
        ModelCallOptions {
            input_messages: Some("second".into()),
            ..ModelCallOptions::default()
        },
    )?;
    call.end();
    run.end();
    harness.flush()?;
    let spans = harness.spans()?;
    assert_eq!(
        attributes_of(only(&spans, "chat ")?)?.get("gen_ai.input.messages"),
        Some(&json!("second"))
    );
    Ok(())
}

/// A word ingest does not send reads as no mode, after an `on` too, so content stays home.
#[test]
fn a_mode_ingest_does_not_send_keeps_content_home() -> Outcome {
    let sealed = Answer::new(
        200,
        vec![("Telmoni-Content-Mode".to_owned(), "sealed".to_owned())],
        b"{}".to_vec(),
    );
    let harness = Harness::answering(
        Config {
            capture_content: true,
            ..keyed()
        },
        [Ok(mode_on()), Ok(sealed.clone()), Ok(sealed)],
    )?;
    for _ in 0..2 {
        harness.telmoni.run("bot", RunOptions::default())?.end();
        harness.flush()?;
    }
    assert_eq!(harness.telmoni.content_mode(), None);
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let call = run.model_call(
        "m",
        ModelCallOptions {
            input_messages: Some("kept home".into()),
            ..ModelCallOptions::default()
        },
    )?;
    call.end();
    run.end();
    harness.flush()?;
    assert!(!harness.transport.text().contains("kept home"));
    Ok(())
}

#[test]
fn content_stays_home_without_capture_content_even_when_allowed() -> Outcome {
    let harness = Harness::answering(keyed(), std::iter::repeat_n(Ok(mode_on()), 4))?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    run.end();
    harness.flush()?;
    assert_eq!(harness.telmoni.content_mode(), Some(ContentMode::On));

    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let call = run.model_call(
        "m",
        ModelCallOptions {
            input_messages: Some("never".into()),
            ..ModelCallOptions::default()
        },
    )?;
    call.end();
    run.end();
    harness.flush()?;
    assert!(!harness.transport.text().contains("never"));
    Ok(())
}

#[test]
fn a_failed_call_records_the_error_type() -> Outcome {
    #[derive(Debug)]
    struct ProviderDown;

    impl std::fmt::Display for ProviderDown {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("provider down")
        }
    }

    impl From<telmoni::Error> for ProviderDown {
        fn from(_: telmoni::Error) -> Self {
            Self
        }
    }

    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let result: Result<(), ProviderDown> = run.try_model_call_with(
        "m",
        ModelCallOptions {
            operation: Some("generate_content".to_owned()),
            ..ModelCallOptions::default()
        },
        |_call| Err(ProviderDown),
    );
    assert!(result.is_err());
    let answered = run.model_call_with("m", ModelCallOptions::default(), |call| {
        call.answered(Reply {
            response_model: Some("m-2".to_owned()),
            ..Reply::default()
        });
        "done"
    })?;
    assert_eq!(answered, "done");
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let failed = only(&spans, "generate_content m")?;
    assert_eq!(failed.get("status"), Some(&json!({ "code": 2 })));
    assert_eq!(
        attributes_of(failed)?.get("error.type"),
        Some(&json!("ProviderDown"))
    );
    let chat = only(&spans, "chat m")?;
    assert!(chat.get("status").is_none());
    assert_eq!(
        attributes_of(chat)?.get("gen_ai.response.model"),
        Some(&json!("m-2"))
    );
    Ok(())
}

#[test]
fn content_is_a_string_as_given_or_json() -> Outcome {
    assert_eq!(Content::from("text").as_str(), "text");
    assert_eq!(Content::from("text".to_owned()).as_str(), "text");
    assert_eq!(Content::from(json!("text")).as_str(), "text");
    assert_eq!(Content::from(json!({ "a": 1 })).as_str(), r#"{"a":1}"#);
    assert_eq!(Content::new(&[1, 2])?.as_str(), "[1,2]");
    assert_eq!(Content::new("text")?.as_str(), r#""text""#);
    Ok(())
}

/// The request's parameters go typed, 0 among them, and the first token in
/// seconds; none of them where not given.
#[test]
fn a_call_carries_its_parameters_and_its_first_token() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let asked = run.model_call(
        "asked",
        ModelCallOptions {
            temperature: Some(0.0),
            top_p: Some(0.9),
            top_k: Some(40.0),
            frequency_penalty: Some(0.5),
            presence_penalty: Some(-0.5),
            max_tokens: Some(256),
            seed: Some(7),
            ..ModelCallOptions::default()
        },
    )?;
    asked.answered(Reply {
        time_to_first_token: Some(Duration::from_millis(350)),
        ..Reply::default()
    });
    asked.end();
    let plain = run.model_call("plain", ModelCallOptions::default())?;
    plain.answered(Reply::default());
    plain.end();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let asked = attributes_of(only(&spans, "chat asked")?)?;
    for (key, value) in [
        ("gen_ai.request.temperature", json!(0.0)),
        ("gen_ai.request.top_p", json!(0.9)),
        ("gen_ai.request.top_k", json!(40.0)),
        ("gen_ai.request.frequency_penalty", json!(0.5)),
        ("gen_ai.request.presence_penalty", json!(-0.5)),
        ("gen_ai.request.max_tokens", json!(256)),
        ("gen_ai.request.seed", json!(7)),
        ("gen_ai.response.time_to_first_chunk", json!(0.35)),
    ] {
        assert_eq!(asked.get(key), Some(&value), "{key}");
    }
    let plain = attributes_of(only(&spans, "chat plain")?)?;
    let plain: BTreeSet<&str> = plain
        .as_object()
        .ok_or("attributes")?
        .keys()
        .map(String::as_str)
        .filter(|key| {
            key.starts_with("gen_ai.request.") || *key == "gen_ai.response.time_to_first_chunk"
        })
        .collect();
    assert_eq!(plain, BTreeSet::from(["gen_ai.request.model"]));
    Ok(())
}

/// The SHA-256 of `media.json`'s PNG signature, the one image below.
const PNG_SHA256: &str = "4c4b6a3be1314ab86138bef4314dde022e600960d8689a2c8f8631802d20dab6";

/// A user message holding one image, as the conventions carry it.
fn blob(content: &str) -> Content {
    json!([{ "role": "user", "parts": [
        { "type": "blob", "modality": "image", "mime_type": "image/png", "content": content },
    ] }])
    .into()
}

fn file_part(sha256: &str) -> Value {
    json!([{ "type": "file", "modality": "image", "mime_type": "image/png", "file_id": format!("telmoni:{sha256}") }])
}

fn sent_parts(call: &Value) -> Result<Value, Failure> {
    let messages = attributes_of(call)?;
    let text = messages
        .get("gen_ai.input.messages")
        .and_then(Value::as_str)
        .ok_or("the messages")?;
    let decoded: Value = serde_json::from_str(text)?;
    Ok(decoded.pointer("/0/parts").cloned().ok_or("the parts")?)
}

/// A client asked for content whose project keeps it, the mode learned.
fn media_harness() -> Result<Harness, Failure> {
    let harness = Harness::answering(
        Config {
            capture_content: true,
            api_key: Some("k".to_owned()),
            ..keyed()
        },
        std::iter::repeat_n(Ok(mode_on()), 4),
    )?;
    harness.telmoni.run("warm", RunOptions::default())?.end();
    harness.flush()?;
    Ok(harness)
}

fn call_with(run: &telmoni::Run, content: &str) -> Outcome {
    run.model_call(
        "m",
        ModelCallOptions {
            input_messages: Some(blob(content)),
            ..ModelCallOptions::default()
        },
    )?
    .end();
    Ok(())
}

#[test]
fn an_image_is_put_once_with_its_type_and_key() -> Outcome {
    let harness = media_harness()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    call_with(&run, "iVBORw0KGgo=")?;
    call_with(&run, "iVBORw0KGgo=")?;
    run.end();
    harness.flush()?;
    let uploads = harness.transport.uploads();
    assert_eq!(uploads.len(), 1);
    let upload = uploads.first().ok_or("the upload")?;
    assert_eq!(
        upload.url,
        format!("https://api.telmoni.com/v1/media/{PNG_SHA256}")
    );
    assert_eq!(upload.header("Authorization"), Some("Bearer k"));
    assert_eq!(upload.header("Content-Type"), Some("image/png"));
    assert_eq!(upload.body, b"\x89PNG\r\n\x1a\n");
    let spans = harness.spans()?;
    let calls = starting_with(&spans, "chat ");
    assert_eq!(calls.len(), 2);
    for call in calls {
        assert_eq!(sent_parts(call)?, file_part(PNG_SHA256));
    }
    Ok(())
}

#[test]
fn a_failed_upload_still_names_the_file_and_warns_once() -> Outcome {
    for (failure, status) in [
        (Ok(Answer::new(413, Vec::new(), b"{}".to_vec())), Some(413)),
        (Err(TransportError::new("Connect")), None),
    ] {
        let harness = media_harness()?;
        harness
            .transport
            .answer_uploads([failure.clone(), failure.clone()]);
        for _ in 0..2 {
            let run = harness.telmoni.run("bot", RunOptions::default())?;
            call_with(&run, "iVBORw0KGgo=")?;
            run.end();
            harness.flush()?;
        }
        assert_eq!(
            harness.transport.uploads().len(),
            2,
            "a hash is remembered only once kept"
        );
        let spans = harness.spans()?;
        for call in starting_with(&spans, "chat ") {
            assert_eq!(sent_parts(call)?, file_part(PNG_SHA256));
        }
        let warned: Vec<Value> = harness
            .warnings
            .fields()
            .into_iter()
            .filter(|fields| fields["kind"] == "upload_failed")
            .collect();
        let expected = match status {
            Some(status) => json!({ "kind": "upload_failed", "status": status }),
            None => json!({ "kind": "upload_failed" }),
        };
        assert_eq!(warned, vec![expected]);
    }
    Ok(())
}

#[test]
fn a_file_past_the_cap_is_named_but_never_put() -> Outcome {
    // 5 MiB and one byte of 0x89, three at a time.
    let content = "iYmJ".repeat(5 * 1024 * 1024 / 3 + 1);
    let harness = media_harness()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    call_with(&run, &content)?;
    run.end();
    harness.flush()?;
    assert!(harness.transport.uploads().is_empty());
    let spans = harness.spans()?;
    assert_eq!(
        sent_parts(only(&spans, "chat ")?)?,
        file_part("01e058c4b9333d565236aa78414d2c35b3cb8e77c35d7ac8fdb756d17eec58eb")
    );
    Ok(())
}
