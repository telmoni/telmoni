//! Dogfood against the real stores: a console question handed to the
//! observer lands as a run of the platform's own project with its calls under
//! it, the asking organization hashed as the project's user ids are, and the
//! trace lanes read it back as any user's.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{SubsecRound, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET, clickhouse_or_skip, state_with_stores};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{
    AgentGeneration, AgentObserver, AgentStep, AgentToolCall, AgentTurnEnding, AgentTurnRecord,
    TokenUsage,
};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, UserId};
use telmoni_telemetry::dogfood::{AGENT, Dogfood};
use telmoni_telemetry::hashing::HASH_BYTES;
use telmoni_telemetry::router;

#[sqlx::test]
async fn a_question_lands_as_a_run_of_our_project_and_the_trace_lanes_read_it(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    // Ours: the project the questions are recorded under, with one of us
    // seated to read the traces. The asker is somebody else's organization.
    let (ours, our_project) = (OrganizationId::new(), ProjectId::new());
    let asker = OrganizationId::new();
    let auth = AuthStub::new();
    auth.seat(
        "user_us",
        &ours,
        &our_project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    let state = state_with_stores(&pool, auth);
    let app = router(Arc::clone(&state));
    let dogfood = Dogfood::new(Arc::clone(&state), our_project.clone());

    let started = Utc::now().trunc_subsecs(6) - chrono::Duration::seconds(5);
    let question = Uuid::new_v4();
    let conversation = Uuid::new_v4();
    let record = AgentTurnRecord {
        trace_id: question,
        conversation_id: conversation,
        organization_id: asker.clone(),
        project_id: Some(ProjectId::new()),
        user_id: UserId::new(),
        started_at: started,
        ended_at: started + chrono::Duration::seconds(4),
        ending: AgentTurnEnding::Answered {
            stopped_short: None,
        },
        steps: vec![
            AgentStep::Generation(AgentGeneration {
                id: Uuid::new_v4(),
                round: 0,
                started_at: started,
                first_token_at: Some(started + chrono::Duration::milliseconds(400)),
                ended_at: started + chrono::Duration::seconds(2),
                provider: "anthropic",
                model: "claude-sonnet-5-5".into(),
                max_tokens: 3_000,
                usage: Some(TokenUsage {
                    input_tokens: 900,
                    output_tokens: 80,
                    cache_read_tokens: 600,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                }),
                stop_reason: Some("tool_use"),
                error: None,
                tool_calls: vec!["list_members"],
            }),
            AgentStep::Tool(AgentToolCall {
                id: Uuid::new_v4(),
                round: 0,
                started_at: started + chrono::Duration::seconds(2),
                ended_at: started + chrono::Duration::milliseconds(2_300),
                name: "list_members",
                is_error: false,
            }),
            AgentStep::Generation(AgentGeneration {
                id: Uuid::new_v4(),
                round: 1,
                started_at: started + chrono::Duration::milliseconds(2_300),
                first_token_at: Some(started + chrono::Duration::seconds(3)),
                ended_at: started + chrono::Duration::seconds(4),
                provider: "anthropic",
                model: "claude-sonnet-5-5".into(),
                max_tokens: 3_000,
                usage: Some(TokenUsage {
                    input_tokens: 1_000,
                    output_tokens: 60,
                    ..TokenUsage::default()
                }),
                stop_reason: Some("end_turn"),
                error: None,
                tool_calls: Vec::new(),
            }),
        ],
    };

    // As the agent hands it over: on a task of its own, so the write is
    // waited for here by reading until it is there.
    dogfood.observe(record);
    let request = |path: String| {
        Request::builder()
            .method("GET")
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", "Bearer tok_user_us")
            .header("x-organization-id", ours.as_str())
            .header("x-project-id", our_project.as_str())
            .body(Body::empty())
            .expect("a request")
    };
    let mut detail = Value::Null;
    for _ in 0..50 {
        let response = app
            .clone()
            .oneshot(request(format!("/internal/telemetry/traces/{question}")))
            .await
            .expect("the router answers");
        if response.status() == StatusCode::OK {
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("a body")
                .to_bytes();
            detail = serde_json::from_slice(&bytes).expect("a JSON body");
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!detail.is_null(), "the question was never recorded");

    let spans = detail["spans"].as_array().expect("spans");
    let kinds: Vec<&str> = spans
        .iter()
        .map(|s| s["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(kinds, ["run", "model_call", "tool_call", "model_call"]);
    let run = &spans[0];
    assert_eq!(run["agent"], AGENT);
    assert_eq!(run["name"], AGENT);
    assert_eq!(run["status"], "ok");
    assert_eq!(run["conversation_id"], conversation.to_string());
    // ⚠ The asking organization is a user id: hashed under our project's own
    // key, never its id.
    let user = run["user_id"].as_str().expect("a user");
    assert_eq!(user.len(), HASH_BYTES * 2);
    assert!(user.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_ne!(user, asker.as_str());
    assert_eq!(run["tags"], serde_json::json!({ "asked_about": "project" }));
    let first = &spans[1];
    assert_eq!(first["model"], "claude-sonnet-5-5");
    assert_eq!(first["input_tokens"], 900);
    assert_eq!(first["cache_read_input_tokens"], 600);
    assert_eq!(
        first["attributes"]["gen_ai.response.finish_reasons"],
        r#"["tool_use"]"#
    );
    assert_eq!(
        first["attributes"]["gen_ai.server.time_to_first_token"],
        "0.4"
    );
    assert_eq!(first["attributes"]["telemetry.distro.name"], "telmoni");
    assert_eq!(spans[2]["name"], "list_members");
    assert_eq!(spans[2]["status"], "ok");

    // The list sums it as any user's run.
    let response = app
        .clone()
        .oneshot(request(
            "/internal/telemetry/traces?agent=console-agent".into(),
        ))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let list: Value = serde_json::from_slice(&bytes).expect("a JSON body");
    let trace = list["traces"]
        .as_array()
        .expect("traces")
        .iter()
        .find(|t| t["run_id"] == question.to_string())
        .expect("the question is listed");
    assert_eq!(trace["model_calls"], 2);
    assert_eq!(trace["tool_calls"], 1);
    assert_eq!(trace["input_tokens"], 1_900);
    assert_eq!(trace["output_tokens"], 140);
    assert_eq!(trace["ended"], true);
    assert_eq!(trace["user_id"], user);
    assert_eq!(trace["conversation_id"], conversation.to_string());
}
