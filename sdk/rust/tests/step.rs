//! A step: nested under its run or another step, carrying the run's ids, and
//! marked by the error a closure returns.

mod common;

use common::{Harness, Outcome, attributes_of, named, only};
use serde_json::json;
use telmoni_sdk::RunOptions;

#[test]
fn a_step_nests_under_its_run_and_carries_its_ids() -> Outcome {
    let harness = Harness::new()?;
    let run = harness.telmoni.run(
        "bot",
        RunOptions {
            user_id: Some("c".to_owned()),
            conversation_id: Some("v".to_owned()),
            ..RunOptions::default()
        },
    )?;
    {
        let plan = run.step("plan");
        let inner = plan.step("plan.inner");
        inner.end();
        plan.end();
    }
    let run_id = run.run_id().to_owned();
    run.end();
    harness.flush()?;

    let spans = harness.spans()?;
    let plan = named(&spans, "plan")?;
    let inner = named(&spans, "plan.inner")?;
    let agent = only(&spans, "invoke_agent ")?;
    assert_eq!(plan.get("parentSpanId"), agent.get("spanId"));
    assert_eq!(inner.get("parentSpanId"), plan.get("spanId"));
    assert_eq!(plan.get("traceId"), agent.get("traceId"));
    assert_eq!(inner.get("traceId"), agent.get("traceId"));
    assert_eq!(
        attributes_of(plan)?,
        json!({
            "telmoni.run_id": run_id,
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

    let harness = Harness::new()?;
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let result: Result<(), PlanningFailed> =
        run.try_step_with("plan", |_step| Err(PlanningFailed("secret details")));
    assert!(matches!(result, Err(PlanningFailed("secret details"))));
    run.end();
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
    let run = harness.telmoni.run("bot", RunOptions::default())?;
    let value = run.step_with("plan", |plan| plan.step_with("plan.inner", |_inner| 1) + 1);
    assert_eq!(value, 2);
    let ok: Result<u8, std::io::Error> = run.try_step_with("act", |_act| Ok(3));
    assert!(matches!(ok, Ok(3)));
    run.end();
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
