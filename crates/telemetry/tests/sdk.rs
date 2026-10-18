//! The door as a customer reaches it: a run that has ended, recorded with the
//! Rust SDK over TCP into this module's own ingest, lands with its calls under
//! it, its user hashed as the project's user ids are, and the trace lanes read
//! it back. What the console agent's own questions travel as
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
use telmoni_sdk::{Config, ModelCallRecord, RunRecord, Telmoni, ToolCallRecord, Usage};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{ApiKeyScope, OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::hashing::HASH_BYTES;
use telmoni_telemetry::router;

/// The project's ingest key, as auth would resolve it.
const INGEST_KEY: &str = "telmoni_sdk_ingest";

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
    let app = router(Arc::clone(&state));

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
            inside: vec![
                ModelCallRecord {
                    provider: Some("anthropic".into()),
                    usage: Some(Usage {
                        input_tokens: Some(900),
                        output_tokens: Some(80),
                        cache_read_input_tokens: Some(600),
                        cache_write_input_tokens: Some(0),
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
            .oneshot(request(format!("/internal/telemetry/traces/{run_id}")))
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
    assert_eq!(run["session_id"], session);
    // ⚠ The user id is hashed at the door under the project's own key, never
    // stored as sent.
    let user = run["user_id"].as_str().expect("a user");
    assert_eq!(user.len(), HASH_BYTES * 2);
    assert!(user.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_ne!(user, "user_42");
    assert_eq!(run["tags"], serde_json::json!({ "asked_about": "project" }));
    let first = &spans[1];
    assert_eq!(first["model"], "claude-sonnet-5-5");
    assert_eq!(first["provider"], "anthropic");
    assert_eq!(first["input_tokens"], 900);
    assert_eq!(first["cache_read_input_tokens"], 600);
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
            "/internal/telemetry/traces?agent=nightly-triage".into(),
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
    assert_eq!(trace["session_id"], session);
    assert_eq!(trace["environments"], serde_json::json!(["staging"]));
    assert_eq!(trace["releases"], serde_json::json!(["2026.10.18"]));
}
