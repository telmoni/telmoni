//! Scores (`sdk/fixtures/scores.json`): on a span still open, an evaluation
//! event on it; once ended, the client's, sent to `/v1/scores` and never as a
//! span; and each refused as the server would, with one warning.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{
    Failure, FakeTransport, Harness, Outcome, Sleeps, Warnings, hooks, is_uuid_v4, keyed,
    load_fixture, mode_on, text,
};
use serde_json::{Value, json};
use telmoni::transport::{Answer, TransportError};
use telmoni::{Config, Score, ScoreTarget, StepRecord, Telmoni, TraceOptions, TraceRecord};

const SCORES: &str = "https://telmoni.example.com/v1/scores";

/// A fixture's score, its non-finite numbers written as words.
fn score_of(case: &Value) -> Result<Score, Failure> {
    let number = |value: &Value| -> Result<f64, Failure> {
        match value {
            Value::String(word) => Ok(word.parse()?),
            value => value.as_f64().ok_or_else(|| "a number".into()),
        }
    };
    let name = text(case, "name")?;
    let mut score = match text(case, "type")? {
        "number" => Score::number(name, number(&case["value"])?),
        "boolean" => Score::boolean(name, case["value"] == true),
        _ => Score::category(name, text(case, "value")?),
    };
    if let Some([min, max]) = case["range"].as_array().map(Vec::as_slice) {
        score = score.range(number(min)?, number(max)?);
    }
    if let Some(version) = case["version"].as_str() {
        score = score.version(version);
    }
    Ok(score)
}

/// JSON compared with every number as a double: the SDK writes `1.0` where
/// the fixture says `1`.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w)))
        }
        (a, b) => a == b,
    }
}

/// A span's evaluation events, their attributes sorted and their time
/// checked within the span, then `<time>`.
fn events(span: &Value) -> Result<Vec<Value>, Failure> {
    let (start, end): (u64, u64) = (
        text(span, "startTimeUnixNano")?.parse()?,
        text(span, "endTimeUnixNano")?.parse()?,
    );
    let mut out = Vec::new();
    for event in span["events"].as_array().into_iter().flatten() {
        if event["name"] != "gen_ai.evaluation.result" {
            continue;
        }
        let at: u64 = text(event, "timeUnixNano")?.parse()?;
        assert!((start..=end).contains(&at), "{event}");
        let mut attributes = event["attributes"].as_array().cloned().unwrap_or_default();
        attributes.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
        out.push(
            json!({ "timeUnixNano": "<time>", "name": event["name"], "attributes": attributes }),
        );
    }
    Ok(out)
}

/// The bodies posted to `/v1/scores`.
fn posted(harness: &Harness) -> Result<Vec<Value>, Failure> {
    harness
        .transport
        .requests()
        .iter()
        .filter(|request| request.url == SCORES)
        .map(|request| Ok(serde_json::from_slice(&request.body)?))
        .collect()
}

/// Each `scores` case on an agent's span: kept as the event the fixture names,
/// or refused with one `input` warning and no event.
#[test]
fn a_live_score_is_kept_or_refused_as_the_fixture_says() -> Outcome {
    for case in load_fixture("scores.json")?["scores"]
        .as_array()
        .ok_or("scores")?
    {
        let label = text(case, "case")?;
        let harness = Harness::new()?;
        let trace = harness
            .telmoni
            .trace("support-bot", TraceOptions::default())?;
        trace.score(score_of(case)?);
        trace.end();
        harness.flush()?;
        let spans = harness.spans()?;
        let got = events(spans.first().ok_or("the agent's span")?)?;
        if case["refused"] == true {
            assert!(got.is_empty(), "{label}: {got:?}");
            assert_eq!(harness.warnings.kinds(), ["input"], "{label}");
            continue;
        }
        assert!(
            harness.warnings.all().is_empty(),
            "{label}: {:?}",
            harness.warnings.all()
        );
        let [event] = got.as_slice() else {
            return Err(format!("{label}: {got:?}").into());
        };
        let attribute = |key: &str| {
            event["attributes"]
                .as_array()
                .and_then(|all| all.iter().find(|a| a["key"] == key))
                .map(|a| a["value"].clone())
        };
        let kept = &case["kept"];
        assert_eq!(
            attribute("gen_ai.evaluation.name"),
            Some(json!({ "stringValue": kept["name"] })),
            "{label}"
        );
        match kept["label"].as_str() {
            Some(category) => assert_eq!(
                attribute("gen_ai.evaluation.score.label"),
                Some(json!({ "stringValue": category })),
                "{label}"
            ),
            None => assert!(
                attribute("gen_ai.evaluation.score.value")
                    .is_some_and(|v| same(&v["doubleValue"], &kept["value"])),
                "{label}: {event}"
            ),
        }
        let version = kept["version"].as_str().unwrap_or_default();
        assert_eq!(
            attribute("telmoni.score.version"),
            (!version.is_empty()).then(|| json!({ "stringValue": version })),
            "{label}"
        );
    }
    Ok(())
}

/// Each `targets` case through the client's `score`: one body naming the
/// kept ids, or no request and one `input` warning.
#[test]
fn a_late_scores_target_is_taken_or_refused_as_the_fixture_says() -> Outcome {
    for case in load_fixture("scores.json")?["targets"]
        .as_array()
        .ok_or("targets")?
    {
        let label = text(case, "case")?;
        let harness = Harness::new()?;
        let ids = |key: &str| case[key].as_str().map(str::to_owned);
        let target = match (ids("trace_id"), ids("span_id"), ids("session_id")) {
            (Some(trace), Some(span), None) => ScoreTarget::span(trace, span),
            (Some(trace), None, None) => ScoreTarget::trace(trace),
            (None, None, Some(session)) => ScoreTarget::session(session),
            // The shapes the builders cannot make, refused as the rest are.
            (None, Some(span), None) => ScoreTarget::span("", span),
            (Some(trace), _, Some(_)) => ScoreTarget::span(trace, "not-a-span"),
            _ => ScoreTarget::session(""),
        };
        harness
            .telmoni
            .score(&target, Score::number("relevance", 0.5));
        harness.flush()?;
        let bodies = posted(&harness)?;
        if case["refused"] == true {
            assert!(harness.transport.requests().is_empty(), "{label}");
            assert_eq!(harness.warnings.kinds(), ["input"], "{label}");
            continue;
        }
        let [body] = bodies.as_slice() else {
            return Err(format!("{label}: {bodies:?}").into());
        };
        let item = &body["scores"][0];
        for key in ["trace_id", "span_id", "session_id"] {
            assert_eq!(item.get(key), case["kept"].get(key), "{label}: {key}");
        }
    }
    Ok(())
}

/// `late`: three scores sent through the client go as one body to
/// `/v1/scores`, and no span of theirs is exported.
#[test]
fn late_scores_leave_as_one_body_and_never_as_spans() -> Outcome {
    let fixture = load_fixture("scores.json")?;
    let harness = Harness::new()?;
    for score in fixture["late"]["scores"].as_array().ok_or("late")? {
        let target = &score["target"];
        let id = |key: &str| target[key].as_str().map(str::to_owned);
        let target = match (id("trace_id"), id("span_id")) {
            (Some(trace), Some(span)) => ScoreTarget::span(trace, span),
            (Some(trace), None) => ScoreTarget::trace(trace),
            _ => ScoreTarget::session(id("session_id").unwrap_or_default()),
        };
        harness.telmoni.score(&target, score_of(score)?);
    }
    harness.flush()?;
    assert_eq!(
        harness.transport.requests().len(),
        1,
        "one request, and no export"
    );
    let mut body = posted(&harness)?.pop().ok_or("a body")?;
    for item in body["scores"].as_array_mut().ok_or("scores")? {
        assert!(is_uuid_v4(text(item, "id")?), "{item}");
        item["id"] = json!("<uuid>");
    }
    assert!(same(&body, &fixture["late"]["sent"]), "{body}");
    assert!(
        harness.warnings.all().is_empty(),
        "{:?}",
        harness.warnings.all()
    );
    let request = harness.transport.requests().pop().ok_or("a request")?;
    assert_eq!(
        request.header("Authorization"),
        Some("Bearer telmoni_test_key")
    );
    assert_eq!(request.header("Content-Type"), Some("application/json"));
    Ok(())
}

/// `live`: three scores on one trace, its span's events as the fixture sends them.
#[test]
fn live_scores_ride_on_their_span_as_evaluation_events() -> Outcome {
    let fixture = load_fixture("scores.json")?;
    let harness = Harness::new()?;
    let trace = harness
        .telmoni
        .trace("support-bot", TraceOptions::default())?;
    for score in fixture["live"]["scores"].as_array().ok_or("live")? {
        trace.score(score_of(score)?);
    }
    trace.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let got = Value::Array(events(spans.first().ok_or("the agent's span")?)?);
    assert!(same(&got, &fixture["live"]["events"]), "{got}");
    Ok(())
}

/// A body refused is dropped with one warning and never sent again; one the
/// platform was too busy for is sent again.
#[test]
fn a_refused_body_is_warned_of_once_and_a_busy_one_retried() -> Outcome {
    let answer = |status: u16| Answer::new(status, Vec::new(), Vec::new());
    let target = ScoreTarget::session("conv-1");
    let refused = Harness::answering(keyed(), [Ok::<_, TransportError>(answer(400))])?;
    refused
        .telmoni
        .score(&target, Score::boolean("thumbs", true));
    refused.flush()?;
    assert_eq!(refused.transport.requests().len(), 1);
    assert_eq!(refused.warnings.kinds(), ["refused"]);
    assert!(
        refused.warnings.all()[0].contains("scores refused"),
        "{:?}",
        refused.warnings.all()
    );

    let busy = Harness::answering(keyed(), [Ok(answer(503)), Ok(answer(204))])?;
    busy.telmoni.score(&target, Score::boolean("thumbs", true));
    busy.flush()?;
    assert_eq!(busy.transport.requests().len(), 2);
    assert!(busy.warnings.all().is_empty(), "{:?}", busy.warnings.all());
    Ok(())
}

/// A span holds the fixture's cap of scores, the next refused with one warning.
#[test]
fn a_span_holds_its_cap_of_scores() -> Outcome {
    let cap = load_fixture("scores.json")?["limits"]["per_span_max"]
        .as_u64()
        .ok_or("per_span_max")?;
    let harness = Harness::new()?;
    let trace = harness
        .telmoni
        .trace("support-bot", TraceOptions::default())?;
    for i in 0..=cap {
        trace.score(Score::number(format!("score-{i}"), 0.5));
    }
    trace.end();
    harness.flush()?;
    let spans = harness.spans()?;
    assert_eq!(
        events(spans.first().ok_or("the agent's span")?)?.len(),
        usize::try_from(cap)?
    );
    assert_eq!(harness.warnings.kinds(), ["input"]);
    Ok(())
}

/// A record's scores ride on their spans at each span's end, so the same
/// record sent again is the same bytes.
#[test]
fn a_records_scores_ride_on_its_spans_at_their_ends() -> Outcome {
    let harness = Harness::new()?;
    let start = UNIX_EPOCH + Duration::from_secs(1_760_000_000);
    let at = |seconds| start + Duration::from_secs(seconds);
    let trace_id = "5b7e1c2a9d4f4e8ba6c31f2e3d4c5b6a";
    harness.telmoni.record_trace(TraceRecord {
        scores: vec![Score::boolean("thumbs", true)],
        inside: vec![
            StepRecord {
                scores: vec![
                    Score::category("tone", "polite"),
                    Score::number("bad", f64::NAN),
                ],
                ..StepRecord::new("plan", at(1), at(2))
            }
            .into(),
        ],
        ..TraceRecord::new(trace_id, "support-bot", start, at(3))
    })?;
    harness.flush()?;
    let spans = harness.spans()?;
    let end_of = |name: &str| -> Result<(String, Vec<Value>), Failure> {
        let span = spans
            .iter()
            .find(|span| span["name"] == name)
            .ok_or(name.to_owned())?;
        let times = span["events"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|event| event["timeUnixNano"].clone())
            .collect();
        Ok((text(span, "endTimeUnixNano")?.to_owned(), times))
    };
    for name in ["invoke_agent support-bot", "plan"] {
        let (end, times) = end_of(name)?;
        assert_eq!(times, [Value::String(end)], "{name}");
    }
    assert_eq!(harness.warnings.kinds(), ["input"], "the NaN score alone");
    Ok(())
}

/// A score is metadata: sent where content is kept and masked as where it is
/// not, its event never handed to the mask.
#[test]
fn a_score_is_never_handed_to_the_mask() -> Outcome {
    let handed = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&handed);
    let config = Config {
        capture_content: true,
        mask: Some(Arc::new(move |key: &str, _: &str| {
            seen.lock().map(|mut keys| keys.push(key.to_owned())).ok();
            None
        })),
        ..keyed()
    };
    let harness = Harness::answering(config, std::iter::repeat_n(Ok(mode_on()), 4))?;
    // The mode learned first, so the trace's content would leave.
    harness
        .telmoni
        .trace("support-bot", TraceOptions::default())?
        .end();
    harness.flush()?;
    let trace = harness
        .telmoni
        .trace("support-bot", TraceOptions::default())?;
    trace.score(Score::category("tone", "polite"));
    trace.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let got = events(spans.last().ok_or("the agent's span")?)?;
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0]["attributes"].to_string().contains("polite"),
        "the mask, dropping everything, took nothing of the score: {got:?}"
    );
    assert!(
        handed
            .lock()
            .map_err(|_| "the mask's keys")?
            .iter()
            .all(|key| !key.starts_with("gen_ai.evaluation")),
        "{handed:?}"
    );
    Ok(())
}

/// An application's own evaluation event keeps its free text under the
/// content lock: with content off, it leaves with the score's keys alone.
#[test]
fn an_evaluation_events_explanation_stays_content() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness
        .telmoni
        .trace("support-bot", TraceOptions::default())?;
    opentelemetry::trace::TraceContextExt::span(&trace.context()).add_event(
        "gen_ai.evaluation.result",
        vec![
            opentelemetry::KeyValue::new("gen_ai.evaluation.name", "relevance"),
            opentelemetry::KeyValue::new("gen_ai.evaluation.score.value", 0.5),
            opentelemetry::KeyValue::new("gen_ai.evaluation.explanation", "4242 said the user"),
        ],
    );
    trace.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let got = events(spans.first().ok_or("the agent's span")?)?;
    let keys: Vec<&str> = got[0]["attributes"]
        .as_array()
        .ok_or("attributes")?
        .iter()
        .filter_map(|a| a["key"].as_str())
        .collect();
    assert_eq!(
        keys,
        ["gen_ai.evaluation.name", "gen_ai.evaluation.score.value"]
    );
    // With its space, so no id's or clock's digits match it.
    assert!(!harness.transport.text().contains("4242 "));
    Ok(())
}

/// Past a body's cap, scores go in bodies of the cap, in the order given.
#[test]
fn scores_past_a_bodys_cap_go_in_bodies_of_it() -> Outcome {
    let cap = load_fixture("scores.json")?["limits"]["sent_max"]
        .as_u64()
        .ok_or("sent_max")?;
    let harness = Harness::new()?;
    let target = ScoreTarget::session("conv-1");
    for i in 0..=cap {
        harness
            .telmoni
            .score(&target, Score::number(format!("score-{i}"), 0.5));
    }
    harness.flush()?;
    let names: Vec<Vec<String>> = posted(&harness)?
        .iter()
        .map(|body| {
            body["scores"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item["name"].as_str().map(str::to_owned))
                .collect()
        })
        .collect();
    let sizes: Vec<usize> = names.iter().map(Vec::len).collect();
    assert_eq!(sizes, [usize::try_from(cap)?, 1]);
    assert_eq!(
        names.concat().last().map(String::as_str),
        Some(format!("score-{cap}").as_str())
    );
    Ok(())
}

/// Only the client's own root span carries a late score: an agent's span an
/// application sets the attribute on still leaves as a span.
#[test]
fn an_applications_attribute_diverts_no_span() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness
        .telmoni
        .trace("support-bot", TraceOptions::default())?;
    opentelemetry::trace::TraceContextExt::span(&trace.context()).set_attribute(
        opentelemetry::KeyValue::new("telmoni.score.sent", r#"{"name":"x"}"#),
    );
    trace.end();
    harness.flush()?;
    assert_eq!(harness.spans()?.len(), 1, "the agent's span left as a span");
    assert!(posted(&harness)?.is_empty());
    Ok(())
}

/// A record's refused score is warned of before its hold, so a sink that
/// records a trace of its own, as a hook's may, is never held up by it.
#[test]
fn a_records_refused_score_is_warned_of_before_its_hold() -> Outcome {
    static CLIENT: OnceLock<Telmoni> = OnceLock::new();
    let transport = FakeTransport::new([])?;
    let (warnings, sleeps) = (Warnings::default(), Sleeps::default());
    let nested = AtomicBool::new(false);
    let mut hooks = hooks(&transport, &warnings, &sleeps);
    hooks.warn = Box::new(move |_, _| {
        if !nested.swap(true, Ordering::SeqCst)
            && let Some(telmoni) = CLIENT.get()
        {
            let now = SystemTime::now();
            let inner = TraceRecord::new("6b7e1c2a9d4f4e8ba6c31f2e3d4c5b6a", "inner", now, now);
            telmoni.record_trace(inner).ok();
        }
    });
    CLIENT.set(Telmoni::with_hooks(keyed(), hooks)).ok();
    let (sent, got) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let now = SystemTime::now();
        let record = TraceRecord {
            scores: vec![Score::number("", 0.5)],
            ..TraceRecord::new("5b7e1c2a9d4f4e8ba6c31f2e3d4c5b6a", "outer", now, now)
        };
        sent.send(
            CLIENT
                .get()
                .map(|telmoni| telmoni.record_trace(record).is_ok()),
        )
        .ok();
    });
    let recorded = got
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "record_trace held up")?;
    assert_eq!(recorded, Some(true));
    Ok(())
}
