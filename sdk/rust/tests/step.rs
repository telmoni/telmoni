//! A step: nested under its agent's span or another step, carrying the
//! trace's ids, and marked by the error a closure returns.

mod common;

use common::{Harness, Outcome, attributes_of, name_of, named, only};
use serde_json::json;
use telmoni::TraceOptions;

#[test]
fn a_step_nests_under_its_agent_and_carries_its_ids() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace(
        "bot",
        TraceOptions {
            user_id: Some("c".to_owned()),
            session_id: Some("v".to_owned()),
            ..TraceOptions::default()
        },
    )?;
    {
        let plan = trace.step("plan")?;
        let inner = plan.step("plan.inner")?;
        inner.end();
        plan.end();
    }
    let trace_id = trace.trace_id().to_owned();
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let plan = named(&spans, "plan")?;
    let inner = named(&spans, "plan.inner")?;
    let agent = only(&spans, "invoke_agent ")?;
    assert_eq!(plan.get("parentSpanId"), agent.get("spanId"));
    assert_eq!(inner.get("parentSpanId"), plan.get("spanId"));
    assert_eq!(agent.get("traceId"), Some(&json!(trace_id)));
    assert_eq!(plan.get("traceId"), agent.get("traceId"));
    assert_eq!(inner.get("traceId"), agent.get("traceId"));
    assert_eq!(
        attributes_of(plan)?,
        json!({
            "telmoni.agent": "bot",
            "user.id": "c",
            "gen_ai.conversation.id": "v",
        })
    );
    assert_eq!(plan.get("kind"), Some(&json!(1)));
    assert!(plan.get("status").is_none());
    Ok(())
}

#[test]
fn a_failing_step_records_the_error_type_and_returns_the_err() -> Outcome {
    #[derive(Debug)]
    struct PlanningFailed(&'static str);

    impl std::fmt::Display for PlanningFailed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl From<telmoni::Error> for PlanningFailed {
        fn from(_: telmoni::Error) -> Self {
            Self("capped")
        }
    }

    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let result: Result<(), PlanningFailed> =
        trace.try_step_with("plan", |_step| Err(PlanningFailed("secret details")));
    assert!(matches!(result, Err(PlanningFailed("secret details"))));
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let plan = named(&spans, "plan")?;
    assert_eq!(plan.get("status"), Some(&json!({ "code": 2 })));
    assert_eq!(
        attributes_of(plan)?.get("error.type"),
        Some(&json!("PlanningFailed"))
    );
    assert!(
        only(&spans, "invoke_agent ")?.get("status").is_none(),
        "the run itself went on"
    );
    assert!(!harness.transport.text().contains("secret details"));
    Ok(())
}

#[test]
fn the_step_closure_returns_its_value_and_nests() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let value = trace.step_with("plan", |plan| plan.step_with("plan.inner", |_inner| 1))??;
    assert_eq!(value, 1);
    let ok: Result<u8, common::Failure> = trace.try_step_with("act", |_act| Ok(3));
    assert!(matches!(ok, Ok(3)));
    trace.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let plan = named(&spans, "plan")?;
    assert_eq!(
        named(&spans, "plan.inner")?.get("parentSpanId"),
        plan.get("spanId")
    );
    assert!(named(&spans, "act")?.get("status").is_none());
    Ok(())
}

/// Ingest refuses a span named nothing, or past 145 characters.
#[test]
fn a_step_named_nothing_or_past_ingests_cap_keeps_its_span() -> Outcome {
    let harness = Harness::new()?;
    let trace = harness.telmoni.trace("bot", TraceOptions::default())?;
    let n = |count: usize| "n".repeat(count);
    for name in ["", "", &n(146), &n(146), &n(145)] {
        trace.step(name)?.end();
    }
    trace.end();
    harness.flush()?;
    let mut steps: Vec<String> = harness
        .spans()?
        .iter()
        .map(|span| name_of(span).to_owned())
        .filter(|name| name != "invoke_agent bot")
        .collect();
    steps.sort();
    assert_eq!(
        steps,
        [n(145), n(145), n(145), "step".to_owned(), "step".to_owned()]
    );
    assert_eq!(
        harness.warnings.all(),
        [
            "telmoni: a step needs its name; one without is sent named step",
            "telmoni: a step's name must be up to 145 characters; it is sent cut to its first 145",
        ]
    );
    assert_eq!(harness.warnings.kinds(), ["input", "input"]);
    Ok(())
}
