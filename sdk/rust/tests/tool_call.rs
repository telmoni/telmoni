//! A tool call: the conventions' names, its arguments and result as content,
//! the error a closure returns, and the children it may open.

mod common;

use common::{Harness, Outcome, attributes_of, named, only, starting_with};
use serde_json::json;
use telmoni_sdk::{ModelCallOptions, RunOptions, ToolCallOptions};

#[test]
fn a_tool_call_carries_the_conventions_names() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run(
        "bot",
        RunOptions {
            user_id: Some("user".to_owned()),
            ..RunOptions::default()
        },
    )?;
    let tool = run.tool_call(
        "search",
        ToolCallOptions {
            arguments: Some(json!({ "q": "weather" }).into()),
        },
    );
    tool.returned(json!({ "temp": 21 }));
    tool.end();
    let run_id = run.run_id().to_owned();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let span = only(&spans, "execute_tool ")?;
    assert_eq!(span.get("name"), Some(&json!("execute_tool search")));
    assert_eq!(span.get("kind"), Some(&json!(1)));
    assert_eq!(
        attributes_of(span)?,
        json!({
            "telmoni.run_id": run_id,
            "telmoni.agent": "bot",
            "user.id": "user",
            "gen_ai.operation.name": "execute_tool",
            "gen_ai.tool.name": "search",
        })
    );
    let text = harness.transport.text();
    assert!(!text.contains("weather"));
    assert!(!text.contains("temp"));
    Ok(())
}

#[test]
fn a_failing_tool_records_its_error_type() -> Outcome {
    #[derive(Debug)]
    struct Timeout;

    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let result: Result<(), Timeout> =
        run.try_tool_call_with("search", ToolCallOptions::default(), |_tool| Err(Timeout));
    assert!(result.is_err());
    let lookup = run.tool_call("lookup", ToolCallOptions::default());
    lookup.fail("NotFound");
    lookup.end();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let search = named(&spans, "execute_tool search")?;
    assert_eq!(
        attributes_of(search)?.get("error.type"),
        Some(&json!("Timeout"))
    );
    assert_eq!(search.get("status"), Some(&json!({ "code": 2 })));
    let lookup = named(&spans, "execute_tool lookup")?;
    assert_eq!(
        attributes_of(lookup)?.get("error.type"),
        Some(&json!("NotFound"))
    );
    assert_eq!(lookup.get("status"), Some(&json!({ "code": 2 })));
    Ok(())
}

#[test]
fn the_tool_call_closure_returns_its_value() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let reversed = run.tool_call_with("search", ToolCallOptions::default(), |tool| {
        let reversed: String = "abc".chars().rev().collect();
        tool.returned(reversed.as_str());
        reversed
    });
    assert_eq!(reversed, "cba");
    run.end();
    harness.flush()?;
    assert_eq!(starting_with(&harness.spans()?, "execute_tool ").len(), 1);
    Ok(())
}

#[test]
fn a_tool_call_opens_children_of_its_own() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let tool = run.tool_call("summarize", ToolCallOptions::default());
    let call = tool.model_call("m", ModelCallOptions::default());
    call.end();
    let step = tool.step("rank");
    step.end();
    tool.end();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let tool = named(&spans, "execute_tool summarize")?;
    assert_eq!(
        named(&spans, "chat m")?.get("parentSpanId"),
        tool.get("spanId")
    );
    assert_eq!(
        named(&spans, "rank")?.get("parentSpanId"),
        tool.get("spanId")
    );
    Ok(())
}

/// Ingest refuses a tool call named past 145 characters.
#[test]
fn a_tool_name_past_ingests_cap_is_cut_warned_once() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    for name in ["t".repeat(146), "t".repeat(146), "t".repeat(145)] {
        run.tool_call(&name, ToolCallOptions::default()).end();
    }
    run.end();
    harness.flush()?;
    let spans = harness.spans()?;
    let tools = starting_with(&spans, "execute_tool ");
    assert_eq!(tools.len(), 3);
    for tool in tools {
        assert_eq!(
            attributes_of(tool)?.get("gen_ai.tool.name"),
            Some(&json!("t".repeat(145)))
        );
    }
    assert_eq!(
        harness.warnings.all(),
        ["telmoni: a tool's name must be up to 145 characters; it is sent cut to its first 145"]
    );
    Ok(())
}
