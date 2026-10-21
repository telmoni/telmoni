//! A run across services: `inject` writes the W3C `traceparent` and our two
//! `baggage` members as `sdk/fixtures/headers.json` spells them, `extract`
//! reads them back as a run continued here, and nothing of a continued run
//! but its steps and calls is sent.

mod common;

use std::collections::HashMap;

use common::{
    FakeTransport, Harness, Outcome, Sleeps, Warnings, attributes_of, field, hooks, keyed,
    load_fixture, name_of, named, text,
};
use opentelemetry::trace::TraceContextExt as _;
use serde_json::Value;
use telmoni::{Config, Hooks, ModelCallOptions, RunOptions, Telmoni, ToolCallOptions};

type Headers = HashMap<String, String>;

/// The fixture's headers as a request's land in a `HashMap` carrier: their
/// names lowercased, as OpenTelemetry's own impl of the carrier reads them.
fn headers_of(given: &Value) -> Result<Headers, Box<dyn std::error::Error>> {
    given
        .as_object()
        .ok_or("headers")?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.to_ascii_lowercase(),
                value.as_str().ok_or("a header")?.to_owned(),
            ))
        })
        .collect()
}

#[test]
fn inject_writes_the_headers_the_fixture_spells_beside_the_carriers_own() -> Outcome {
    let fixture = load_fixture("headers.json")?;
    let spec = field(&fixture, "inject")?;
    let harness = Harness::new()?;
    let run = harness
        .telmoni
        .run(text(spec, "agent")?, RunOptions::default())?;
    let step = run.step("plan");
    let mut given: Headers = HashMap::new();
    harness.telmoni.inject_from(&step.context(), &mut given);
    let mut attached: Headers = HashMap::new();
    {
        let _current = step.attach();
        harness.telmoni.inject(&mut attached);
    }
    let mut beside: Headers = [(
        "baggage".to_owned(),
        text(field(spec, "beside")?, "given")?.to_owned(),
    )]
    .into_iter()
    .collect();
    harness.telmoni.inject_from(&step.context(), &mut beside);
    let run_id = run.run_id().to_owned();
    step.end();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let plan = named(&spans, "plan")?;
    let fill = |template: &str| {
        template
            .replace("<trace>", text(plan, "traceId").unwrap_or(""))
            .replace("<span>", text(plan, "spanId").unwrap_or(""))
            .replace("<uuid>", &run_id)
    };
    let expected: Headers = [
        ("traceparent".to_owned(), fill(text(spec, "traceparent")?)),
        ("baggage".to_owned(), fill(text(spec, "baggage")?)),
    ]
    .into_iter()
    .collect();
    assert_eq!(given, expected, "from the step's context");
    assert_eq!(attached, expected, "from the current context, attached");
    assert_eq!(
        beside.get("baggage").map(String::as_str),
        Some(fill(text(field(spec, "beside")?, "written")?).as_str()),
        "the carrier's members kept, ours replaced"
    );
    assert_eq!(beside.len(), 2);
    Ok(())
}

#[test]
fn inject_writes_nothing_outside_a_run_or_for_a_run_sampled_out() -> Outcome {
    let transport = FakeTransport::new([])?;
    let (warnings, sleeps) = (Warnings::default(), Sleeps::default());
    let telmoni = Telmoni::with_hooks(
        Config {
            sample_rate: 0.5,
            ..keyed()
        },
        Hooks {
            random: Box::new(|| 0.9),
            ..hooks(&transport, &warnings, &sleeps)
        },
    );
    let mut outside: Headers = [("baggage".to_owned(), "tenant=acme".to_owned())]
        .into_iter()
        .collect();
    telmoni.inject(&mut outside);
    assert_eq!(outside.len(), 1);
    assert_eq!(
        outside.get("baggage").map(String::as_str),
        Some("tenant=acme")
    );

    let run = telmoni.run("bot", RunOptions::default())?;
    let mut headers: Headers = HashMap::new();
    telmoni.inject_from(&run.context(), &mut headers);
    assert!(headers.is_empty(), "a run sampled out wrote {headers:?}");
    run.end();
    assert!(telmoni.flush());
    assert!(transport.spans()?.is_empty());
    telmoni.shutdown();
    Ok(())
}

#[test]
fn extract_reads_the_run_the_fixture_names_and_nothing_from_headers_naming_none() -> Outcome {
    let fixture = load_fixture("headers.json")?;
    let harness = Harness::new()?;
    for given in field(&fixture, "extract")?.as_array().ok_or("extract")? {
        let name = text(given, "name")?;
        let headers = headers_of(field(given, "headers")?)?;
        let run = harness.telmoni.extract(&headers);
        let expected = field(given, "run")?;
        if expected.is_null() {
            assert!(run.is_none(), "{name}: a run was read");
            continue;
        }
        let run = run.ok_or_else(|| format!("{name}: no run was read"))?;
        assert_eq!(run.run_id(), text(expected, "run_id")?, "{name}");
        assert_eq!(run.agent(), text(expected, "agent")?, "{name}");
        let cx = run.context();
        let span = cx.span();
        let parent = span.span_context();
        assert_eq!(
            parent.trace_id().to_string(),
            text(expected, "trace_id")?,
            "{name}"
        );
        assert_eq!(
            parent.span_id().to_string(),
            text(expected, "parent_span_id")?,
            "{name}"
        );
        assert!(parent.is_remote(), "{name}");
        assert_eq!(
            parent.is_sampled(),
            field(expected, "sampled")?.as_bool().ok_or("sampled")?,
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn a_continued_runs_steps_and_calls_carry_the_callers_run_under_its_span() -> Outcome {
    let caller = Harness::new()?;
    let run = caller.telmoni.run("orchestrator", RunOptions::default())?;
    let step = run.step("hand over");
    let mut headers: Headers = HashMap::new();
    caller.telmoni.inject_from(&step.context(), &mut headers);
    let run_id = run.run_id().to_owned();
    step.end();
    run.end();
    caller.flush()?;
    let caller_spans = caller.spans()?;
    let handed = named(&caller_spans, "hand over")?;
    let (caller_trace, caller_span) = (text(handed, "traceId")?, text(handed, "spanId")?);

    let worker = Harness::new()?;
    let continued = worker
        .telmoni
        .extract(&headers)
        .ok_or("the worker read no run")?;
    {
        let work = continued.step("work");
        let call = work.model_call(
            "gpt-x",
            ModelCallOptions {
                provider: Some("openai".to_owned()),
                ..ModelCallOptions::default()
            },
        );
        call.end();
        work.end();
    }
    let tool = continued.tool_call("search", ToolCallOptions::default());
    tool.end();
    continued.fail("Ignored");
    continued.end();
    worker.flush()?;

    let spans = worker.spans()?;
    let mut names: Vec<&str> = spans.iter().map(name_of).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["chat gpt-x", "execute_tool search", "work"],
        "nothing of the run itself: no invoke_agent, no telmoni.run.start"
    );
    for span in &spans {
        assert_eq!(text(span, "traceId")?, caller_trace, "{}", name_of(span));
        let attributes = attributes_of(span)?;
        assert_eq!(attributes["telmoni.run_id"], run_id, "{}", name_of(span));
        assert_eq!(
            attributes["telmoni.agent"],
            "orchestrator",
            "{}",
            name_of(span)
        );
    }
    assert_eq!(text(named(&spans, "work")?, "parentSpanId")?, caller_span);
    assert_eq!(
        text(named(&spans, "execute_tool search")?, "parentSpanId")?,
        caller_span
    );
    assert_eq!(
        text(named(&spans, "chat gpt-x")?, "parentSpanId")?,
        text(named(&spans, "work")?, "spanId")?
    );
    Ok(())
}

/// Uncut, ingest would refuse every span this side sends of the run.
#[test]
fn a_continued_runs_spans_carry_its_agent_cut_to_its_cap() -> Outcome {
    let fixture = load_fixture("headers.json")?;
    let given = field(&fixture, "extract")?
        .as_array()
        .ok_or("extract")?
        .iter()
        .find(|given| text(given, "name").is_ok_and(|name| name.starts_with("an agent past")))
        .ok_or("headers.json holds no agent past its cap")?;
    let harness = Harness::new()?;
    let continued = harness
        .telmoni
        .extract(&headers_of(field(given, "headers")?)?)
        .ok_or("no run was read")?;
    continued.step("work").end();
    harness.flush()?;
    let spans = harness.spans()?;
    let agent = text(field(given, "run")?, "agent")?;
    assert_eq!(
        attributes_of(named(&spans, "work")?)?.get("telmoni.agent"),
        Some(&Value::from(agent))
    );
    Ok(())
}

#[test]
fn a_continued_run_hands_itself_on_again_under_the_span_current_here() -> Outcome {
    let harness = Harness::new()?;
    let first: Headers = [
        (
            "traceparent".to_owned(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned(),
        ),
        (
            "baggage".to_owned(),
            "telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot".to_owned(),
        ),
    ]
    .into_iter()
    .collect();
    let continued = harness.telmoni.extract(&first).ok_or("a run")?;
    let relay = continued.step("relay");
    let mut onwards: Headers = HashMap::new();
    harness.telmoni.inject_from(&relay.context(), &mut onwards);
    relay.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let relay_span = text(named(&spans, "relay")?, "spanId")?;
    assert_eq!(
        onwards.get("traceparent").map(String::as_str),
        Some(format!("00-4bf92f3577b34da6a3ce929d0e0e4736-{relay_span}-01").as_str())
    );
    assert_eq!(
        onwards.get("baggage").map(String::as_str),
        Some("telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot")
    );
    Ok(())
}
