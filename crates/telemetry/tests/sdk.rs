//! The door as a customer reaches it: a run that has ended, recorded with the
//! Rust SDK over TCP into this module's own ingest, lands with its calls under
//! it, its user hashed as the project's user ids are, and the trace lanes read
//! it back; a live run's retrievals land as retrievals. What the console agent's own questions travel as
//! (`crates/agent/src/report.rs`), and any customer's job.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::collections::BTreeMap;
use std::future::IntoFuture as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET, clickhouse_or_skip, state_with_stores};
use telmoni::{
    Config, ModelCallRecord, RetrievalOptions, RunOptions, RunRecord, Score, ScoreTarget, Telmoni,
    ToolCallOptions, ToolCallRecord, Usage,
};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{ApiKeyScope, OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::hashing::HASH_BYTES;
use telmoni_telemetry::{otlp_router, router, v1_router};

/// The project's ingest key, as auth would resolve it.
const INGEST_KEY: &str = "tm_sdk_ingest";

#[sqlx::test]
async fn a_run_recorded_with_the_sdk_lands_through_the_door_and_the_trace_lanes_read_it(
    pool: PgPool,
) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    // The project the run is of, with an ingest key on it and one of its
    // people seated to read the traces.
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.seat(
        "user_reader",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    )
    .key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let state = state_with_stores(&pool, auth);
    // Ingest nested as the binary mounts it.
    let app = router(Arc::clone(&state))
        .nest("/otlp", otlp_router(Arc::clone(&state)))
        .nest("/v1", v1_router(Arc::clone(&state)));

    // The door over TCP on this machine, as the SDK reaches it: the one host
    // it sends a key to over plain HTTP.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port of this machine's");
    let endpoint = format!("http://{}", listener.local_addr().expect("an address"));
    tokio::spawn(axum::serve(listener, app.clone()).into_future());
    let telmoni = Telmoni::new(Config {
        environment: Some("staging".to_owned()),
        release: Some("2026.10.18".to_owned()),
        ..Config::new(endpoint, None, Some(INGEST_KEY.to_owned()))
    });

    let run_id = Uuid::new_v4();
    let session = Uuid::new_v4().to_string();
    let start = SystemTime::now() - Duration::from_secs(5);
    let at = |millis| start + Duration::from_millis(millis);
    telmoni
        .record_run(RunRecord {
            user_id: Some("user_42".into()),
            session_id: Some(session.clone()),
            tags: BTreeMap::from([("asked_about".to_owned(), "project".to_owned())]),
            scores: vec![
                Score::number("relevance", 0.8)
                    .range(0.0, 1.0)
                    .version("judge-1"),
            ],
            inside: vec![
                ModelCallRecord {
                    provider: Some("anthropic".into()),
                    usage: Some(Usage {
                        input_tokens: Some(900),
                        output_tokens: Some(80),
                        cache_read_input_tokens: Some(600),
                        cache_write_input_tokens: Some(0),
                        cost: Some(0.0042),
                    }),
                    ..ModelCallRecord::new("claude-sonnet-5-5", at(0), at(2_000))
                }
                .into(),
                ToolCallRecord::new("list_members", at(2_000), at(2_300)).into(),
                ModelCallRecord {
                    provider: Some("anthropic".into()),
                    usage: Some(Usage {
                        input_tokens: Some(1_000),
                        output_tokens: Some(60),
                        ..Usage::default()
                    }),
                    ..ModelCallRecord::new("claude-sonnet-5-5", at(2_300), at(4_000))
                }
                .into(),
            ],
            ..RunRecord::new(run_id.to_string(), "nightly-triage", start, at(4_000))
        })
        .expect("a record the SDK takes");
    // Scored once the run ended, as a person's feedback comes: the run, and its session.
    telmoni.score(
        &ScoreTarget::run(run_id.to_string()),
        Score::boolean("thumbs", true),
    );
    telmoni.score(
        &ScoreTarget::session(session.clone()),
        Score::category("outcome", "resolved"),
    );
    // The flush waits on the SDK's thread, so off the runtime's.
    let client = telmoni.clone();
    let sent = tokio::task::spawn_blocking(move || client.flush())
        .await
        .expect("the flush ran");
    assert!(sent, "the export did not leave within the wait");

    let request = |path: String| {
        Request::builder()
            .method("GET")
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", "Bearer tok_user_reader")
            .header("x-organization-id", organization.as_str())
            .header("x-project-id", project.as_str())
            .body(Body::empty())
            .expect("a request")
    };
    let mut detail = Value::Null;
    for _ in 0..100 {
        let response = app
            .clone()
            .oneshot(request(format!("/internal/v1/telemetry/traces/{run_id}")))
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
    assert!(!detail.is_null(), "the run was never recorded");

    let spans = detail["spans"].as_array().expect("spans");
    let kinds: Vec<&str> = spans
        .iter()
        .map(|s| s["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(kinds, ["run", "model_call", "tool_call", "model_call"]);
    let run = &spans[0];
    assert_eq!(run["agent"], "nightly-triage");
    assert_eq!(run["name"], "nightly-triage");
    assert_eq!(run["status"], "ok");
    // ⚠ The user's and the session's ids are hashed at the door under the
    // project's own key, never stored as sent.
    let user = run["user_id"].as_str().expect("a user");
    assert_eq!(user.len(), HASH_BYTES * 2);
    assert!(user.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_ne!(user, "user_42");
    let stored_session = run["session_id"].as_str().expect("a session");
    assert_eq!(stored_session.len(), HASH_BYTES * 2);
    assert!(stored_session.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_ne!(stored_session, session);
    assert_eq!(run["tags"], serde_json::json!({ "asked_about": "project" }));
    let first = &spans[1];
    assert_eq!(first["model"], "claude-sonnet-5-5");
    assert_eq!(first["provider"], "anthropic");
    assert_eq!(first["input_tokens"], 900);
    assert_eq!(first["cache_read_input_tokens"], 600);
    assert_eq!(first["cost"], 0.0042, "the cost it reported, a column");
    assert_eq!(first["attributes"]["gen_ai.usage.cost"], Value::Null);
    assert_eq!(spans[3]["cost"], Value::Null, "a call that reported none");
    assert_eq!(first["attributes"]["telemetry.distro.name"], "telmoni");
    // The deployment the SDK's configuration named, read off its resource
    // into two columns of every span, and never an attribute.
    assert_eq!(first["environment"], "staging");
    assert_eq!(first["release"], "2026.10.18");
    assert_eq!(first["attributes"]["service.version"], Value::Null);
    assert_eq!(spans[2]["name"], "list_members");
    assert_eq!(spans[2]["status"], "ok");

    // The list sums it as any run.
    let response = app
        .clone()
        .oneshot(request(
            "/internal/v1/telemetry/traces?agent=nightly-triage".into(),
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
        .find(|t| t["run_id"] == run_id.to_string())
        .expect("the run is listed");
    assert_eq!(trace["model_calls"], 2);
    assert_eq!(trace["tool_calls"], 1);
    assert_eq!(trace["input_tokens"], 1_900);
    assert_eq!(trace["output_tokens"], 140);
    assert_eq!(trace["ended"], true);
    assert_eq!(trace["user_id"], user);
    assert_eq!(trace["session_id"], stored_session);
    assert_eq!(trace["environments"], serde_json::json!(["staging"]));
    assert_eq!(trace["releases"], serde_json::json!(["2026.10.18"]));

    // The record's score rode on the run's span; the two sent after it, to
    // `/v1/scores`, the session's by its id as sent, found by the id as stored.
    let response = app
        .clone()
        .oneshot(request(format!(
            "/internal/v1/telemetry/scores?run_id={run_id}&session_id={stored_session}"
        )))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let scores: Value = serde_json::from_slice(&bytes).expect("a JSON body");
    let mut got: Vec<(String, String, String)> = scores["scores"]
        .as_array()
        .expect("scores")
        .iter()
        .map(|s| {
            assert_eq!(s["source"], "sdk");
            let text = |key: &str| s[key].as_str().unwrap_or_default().to_owned();
            (text("name"), text("target"), text("scorer_version"))
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            ("outcome".into(), "session".into(), String::new()),
            ("relevance".into(), "run".into(), "judge-1".into()),
            ("thumbs".into(), "run".into(), String::new()),
        ],
        "{scores}"
    );
}

/// A live run's retrieval and memory search, opened with the SDK, land as
/// retrievals named by their sources, with what each asked for and how many
/// came back, and the list counts them beside the run's tool call.
#[sqlx::test]
async fn a_live_runs_retrievals_land_named_by_their_sources(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.seat(
        "user_reader",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    )
    .key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let state = state_with_stores(&pool, auth);
    let app = router(Arc::clone(&state))
        .nest("/otlp", otlp_router(Arc::clone(&state)))
        .nest("/v1", v1_router(Arc::clone(&state)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port of this machine's");
    let endpoint = format!("http://{}", listener.local_addr().expect("an address"));
    tokio::spawn(axum::serve(listener, app.clone()).into_future());
    let telmoni = Telmoni::new(Config::new(endpoint, None, Some(INGEST_KEY.to_owned())));

    // Opened, ended and flushed on the SDK's side of the runtime, as an agent's code is.
    let run_id = tokio::task::spawn_blocking(move || {
        let run = telmoni
            .run("support-bot", RunOptions::default())
            .expect("a run");
        let found = run
            .retrieval(
                "docs-index",
                RetrievalOptions {
                    top_k: Some(5),
                    ..RetrievalOptions::default()
                },
            )
            .expect("a retrieval");
        found.returned(&[
            serde_json::json!({ "id": "kb-1" }),
            serde_json::json!({ "id": "kb-2" }),
            serde_json::json!({ "id": "kb-3" }),
        ]);
        found.end();
        let remembered = run
            .retrieval(
                "preferences",
                RetrievalOptions {
                    top_k: Some(3),
                    memory: true,
                    ..RetrievalOptions::default()
                },
            )
            .expect("a memory search");
        remembered.returned(&[serde_json::json!({ "id": "mem-1" })]);
        remembered.end();
        run.tool_call("docs-index", ToolCallOptions::default())
            .expect("a tool call")
            .end();
        let run_id = run.run_id().to_owned();
        run.end();
        assert!(telmoni.flush(), "the export did not leave within the wait");
        run_id
    })
    .await
    .expect("the run ran");

    let request = |path: String| {
        Request::builder()
            .method("GET")
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", "Bearer tok_user_reader")
            .header("x-organization-id", organization.as_str())
            .header("x-project-id", project.as_str())
            .body(Body::empty())
            .expect("a request")
    };
    let read = |path: String| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(request(path))
                .await
                .expect("the router answers");
            if response.status() != StatusCode::OK {
                return Value::Null;
            }
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("a body")
                .to_bytes();
            serde_json::from_slice(&bytes).expect("a JSON body")
        }
    };
    let mut detail = Value::Null;
    for _ in 0..100 {
        detail = read(format!("/internal/v1/telemetry/traces/{run_id}")).await;
        let kinds = |kind: &str| {
            detail["spans"].as_array().map_or(0, |spans| {
                spans.iter().filter(|s| s["kind"] == kind).count()
            })
        };
        if kinds("run") == 1 && kinds("retrieval") == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let spans = detail["spans"].as_array().expect("the run was recorded");
    let span = |name: &str, kind: &str| {
        spans
            .iter()
            .find(|s| s["name"] == name && s["kind"] == kind)
            .expect("the span landed")
    };
    let found = span("docs-index", "retrieval");
    assert_eq!(found["attributes"]["gen_ai.data_source.id"], "docs-index");
    assert_eq!(found["attributes"]["gen_ai.retrieval.top_k"], "5");
    assert_eq!(found["attributes"]["telmoni.retrieval.returned"], "3");
    let remembered = span("preferences", "retrieval");
    assert_eq!(
        remembered["attributes"]["gen_ai.memory.store.id"],
        "preferences"
    );
    assert_eq!(remembered["attributes"]["gen_ai.retrieval.top_k"], "3");
    assert_eq!(remembered["attributes"]["gen_ai.memory.record.count"], "1");
    assert_eq!(span("docs-index", "tool_call")["status"], "ok");

    let list = read("/internal/v1/telemetry/traces?agent=support-bot".into()).await;
    let trace = list["traces"]
        .as_array()
        .expect("traces")
        .iter()
        .find(|t| t["run_id"] == run_id)
        .expect("the run is listed");
    assert_eq!(
        (&trace["retrievals"], &trace["tool_calls"]),
        (&Value::from(2), &Value::from(1))
    );
}
