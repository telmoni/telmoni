//! Ingest through the module's own router: an export behind a key with the
//! ingest scope is read by the contract, its content dropped and named in
//! the answer, its user ids hashed under the project's own key, and its
//! rows put in ClickHouse; a key without the scope, a body past the cap and
//! one that is not OTLP are each refused in OTLP's `Status` in the request's
//! encoding; a body sent again is answered as it first was;
//! and an export whose project was deleted as it landed is marked deleted
//! with the rest of the project's spans.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use prost::Message as _;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tower::ServiceExt;

use chrono::Utc;
use common::{AuthStub, SECRET, clickhouse_or_skip};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits, Telemetry as _};
use telmoni_shared::test_util::captured_logs;
use telmoni_shared::{ApiKeyScope, OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::contract;
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::{Projects, Store};
use telmoni_telemetry::{otlp_router, router};

const LANE: &str = "/otlp/v1/traces";
const INGEST_KEY: &str = "tm_ingest_key";
const READ_KEY: &str = "tm_read_only_key";
/// Another project's ingest key, in another organization.
const OTHER_KEY: &str = "tm_other_projects_key";
/// The trace every [`export_json`] is of.
const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

/// The module's lanes as the binary mounts them, ingest nested at `/otlp`.
fn lanes(state: Arc<telmoni_telemetry::AppState>) -> Router {
    router(state.clone()).nest("/otlp", otlp_router(state))
}

/// The module's router, the project its two keys open, another project its
/// own key opens, and auth as the module holds it.
struct Lane {
    app: Router,
    project: ProjectId,
    other: ProjectId,
    state: Arc<telmoni_telemetry::AppState>,
    auth: Arc<AuthStub>,
}

/// A lane over the stores a suite can reach, or over none.
fn lane(pool: &PgPool, with_stores: bool) -> Lane {
    lane_under(pool, with_stores.then_some(Arc::new(NoLimits)))
}

/// [`lane`], over the stores and under `entitlements` where given.
fn lane_under(pool: &PgPool, stores_under: Option<Arc<dyn Entitlements>>) -> Lane {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let other = ProjectId::new();
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest])
        .key(READ_KEY, &organization, &project, &[ApiKeyScope::Read])
        .key(
            OTHER_KEY,
            &OrganizationId::new(),
            &other,
            &[ApiKeyScope::Ingest],
        );
    let state = match stores_under {
        Some(entitlements) => common::state_with_stores_under(pool, auth.clone(), entitlements),
        None => common::state(pool, auth.clone()),
    };
    Lane {
        app: lanes(state.clone()),
        project,
        other,
        state,
        auth,
    }
}

impl Lane {
    async fn send(
        &self,
        bearer: Option<&str>,
        content_type: &str,
        content_encoding: Option<&str>,
        body: Vec<u8>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut builder = Request::builder()
            .method("POST")
            .uri(LANE)
            .header(header::CONTENT_TYPE, content_type);
        if let Some(bearer) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
        }
        if let Some(encoding) = content_encoding {
            builder = builder.header(header::CONTENT_ENCODING, encoding);
        }
        let response = self
            .app
            .clone()
            .oneshot(builder.body(Body::from(body)).expect("a request"))
            .await
            .expect("an answer");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    /// An export of one span named `name` under the ingest key, answered
    /// `200`: its headers.
    async fn export(&self, name: &str) -> HeaderMap {
        let (status, headers, answer) = self
            .send(
                Some(INGEST_KEY),
                "application/json",
                None,
                export_json(&[name]).to_string().into_bytes(),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{name}: {}",
            String::from_utf8_lossy(&answer)
        );
        headers
    }
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("a JSON body")
}

/// An export as our SDK sends one: a model call and its trace, one content
/// attribute among them, and a user id, from `reporter` deployed to staging.
fn export_json(span_names: &[&str]) -> Value {
    export_from("reporter", "staging", span_names)
}

/// [`export_json`] from the service `service` deployed to `environment`,
/// each span's id from its place; a span carries `parentSpanId` where its
/// name says `under <id>`.
fn export_from(service: &str, environment: &str, span_names: &[&str]) -> Value {
    let spans: Vec<Value> = span_names
        .iter()
        .enumerate()
        .map(|(n, name)| {
            let mut span = json!({
                "traceId": TRACE,
                "spanId": format!("00f067aa0ba902b{n}"),
                "name": name,
                "kind": 1,
                "startTimeUnixNano": "1760000000000000000",
                "endTimeUnixNano": "1760000001500000000",
                "attributes": [
                    { "key": contract::AGENT, "value": { "stringValue": "nightly-report" } },
                    { "key": contract::OPERATION, "value": { "stringValue": "chat" } },
                    { "key": contract::RESPONSE_MODEL, "value": { "stringValue": "claude-sonnet-5-5" } },
                    { "key": contract::INPUT_TOKENS, "value": { "intValue": "1200" } },
                    { "key": contract::USER_ID, "value": { "stringValue": "acme-42" } },
                    { "key": "gen_ai.input.messages", "value": { "stringValue": "[{\"role\":\"user\",\"content\":\"hi\"}]" } }
                ]
            });
            if let (Some((_, parent)), Some(object)) =
                (name.split_once(" under "), span.as_object_mut())
            {
                object.insert("parentSpanId".to_owned(), json!(parent));
            }
            span
        })
        .collect();
    json!({
        "resourceSpans": [{
            "resource": { "attributes": [
                { "key": "service.name", "value": { "stringValue": service } },
                { "key": contract::ENVIRONMENT, "value": { "stringValue": environment } },
                { "key": contract::RELEASE, "value": { "stringValue": "2026.10.18" } },
                { "key": contract::DISTRO_NAME, "value": { "stringValue": contract::DISTRO } }
            ]},
            "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": spans }]
        }]
    })
}

#[sqlx::test]
async fn an_export_is_kept_its_content_dropped_and_its_user_hashed(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    let body = export_json(&["chat one", "chat two"])
        .to_string()
        .into_bytes();
    let (status, headers, answer) = lane
        .send(Some(INGEST_KEY), "application/json", None, body)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&answer)
    );
    assert_eq!(
        headers.get("telmoni-content-mode").expect("the mode"),
        "off",
        "the SDK learns the mode from its first export"
    );
    let answer = json_body(&answer);
    assert_eq!(answer["partialSuccess"]["rejectedSpans"], 0, "{answer}");
    let message = answer["partialSuccess"]["errorMessage"]
        .as_str()
        .expect("content was dropped, and the answer says so");
    assert!(
        message
            .contains("dropped gen_ai.input.messages: content; the project's content mode is off"),
        "{message}"
    );
    assert!(
        !message.contains("hi"),
        "the content itself is never echoed: {message}"
    );

    let rows: Vec<(String, String, String, u64, String, String, String)> = ch
        .store
        .read(
            &Projects::one(lane.project.clone()),
            "SELECT name, kind, user_id, input_tokens, environment, release, \
             toJSONString(attributes) FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)} ORDER BY name",
        )
        .fetch_all()
        .await
        .expect("the rows");
    assert_eq!(rows.len(), 2, "{rows:?}");
    for (name, kind, user_id, input_tokens, environment, release, attributes) in &rows {
        assert!(name.starts_with("chat "), "{name}");
        assert_eq!(kind, "model_call");
        assert_eq!(*input_tokens, 1_200);
        assert_eq!(user_id.len(), 32, "16 bytes of HMAC in hex: {user_id}");
        assert!(user_id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(user_id, "acme-42", "never stored as sent");
        // The deployment is two columns, read off the resource, and no
        // attribute beside the service's name.
        assert_eq!(
            (environment.as_str(), release.as_str()),
            ("staging", "2026.10.18")
        );
        assert_eq!(
            attributes,
            r#"{"service.name":"reporter","telemetry.distro.name":"telmoni"}"#
        );
    }
    assert_eq!(rows[0].2, rows[1].2, "the same user hashes the same");

    let sealed: bool = sqlx::query_scalar(
        "SELECT hashing_key_ciphertext IS NOT NULL \
           FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(lane.project.as_str())
    .fetch_one(&pool)
    .await
    .expect("the settings row the first export files");
    assert!(sealed, "the project's key is made and sealed with its row");
}

/// A session's id is kept as a user's is, hashed under the project's key: an
/// email sent as one is stored nowhere, and its traces still group and filter
/// by the hash, which the id as sent never matches.
#[sqlx::test]
async fn an_email_sent_as_a_session_id_is_stored_nowhere_and_its_traces_still_group(pool: PgPool) {
    const EMAIL: &str = "dana.whitfield@example.com";
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    auth.seat(
        "user_member",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    let app = lanes(common::state_with_stores_under(
        &pool,
        auth,
        Arc::new(NoLimits),
    ));
    let started = Utc::now() - chrono::Duration::minutes(5);
    let nanos = |at: chrono::DateTime<Utc>| at.timestamp_nanos_opt().expect("in range").to_string();
    let export = |trace: &str, session: &str| {
        json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [{
            "traceId": trace, "spanId": "00f067aa0ba902b7", "name": "triage", "kind": 1,
            "startTimeUnixNano": nanos(started),
            "endTimeUnixNano": nanos(started + chrono::Duration::seconds(2)),
            "attributes": [
                { "key": contract::AGENT, "value": { "stringValue": "triage" } },
                { "key": contract::OPERATION, "value": { "stringValue": "invoke_agent" } },
                { "key": contract::SESSION_ID, "value": { "stringValue": session } }
            ]
        }]}]}]})
        .to_string()
    };
    let runs = [
        ("4bf92f3577b34da6a3ce929d0e0e4711", EMAIL),
        ("4bf92f3577b34da6a3ce929d0e0e4712", EMAIL),
        ("4bf92f3577b34da6a3ce929d0e0e4713", "another-session"),
    ];
    for (trace, session) in runs {
        let answer = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(LANE)
                    .header(header::AUTHORIZATION, format!("Bearer {INGEST_KEY}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(export(trace, session)))
                    .expect("a request"),
            )
            .await
            .expect("an answer");
        assert_eq!(answer.status(), StatusCode::OK);
        let body = answer
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        assert!(
            !String::from_utf8_lossy(&body).contains(EMAIL),
            "the answer never echoes it"
        );
    }

    let only = Projects::one(project.clone());
    let rows: Vec<(String, String)> = ch
        .store
        .read(
            &only,
            "SELECT trace_id, session_id FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)} ORDER BY trace_id",
        )
        .fetch_all()
        .await
        .expect("the rows");
    assert_eq!(rows.len(), 3, "{rows:?}");
    for (_, session) in &rows {
        assert_eq!(session.len(), 32, "16 bytes of HMAC in hex: {session}");
        assert!(session.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert_eq!(rows[0].1, rows[1].1, "the same session hashes the same");
    assert_ne!(rows[0].1, rows[2].1, "another session hashes apart");
    let anywhere: u64 = ch
        .store
        .read(
            &only,
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)} \
             AND (position(session_id, {email:String}) > 0 OR position(name, {email:String}) > 0 \
             OR position(user_id, {email:String}) > 0 \
             OR position(toJSONString(attributes), {email:String}) > 0 \
             OR position(toJSONString(tags), {email:String}) > 0)",
        )
        .param("email", EMAIL)
        .fetch_one()
        .await
        .expect("the count");
    assert_eq!(anywhere, 0, "the email is kept nowhere a span holds text");

    // Traces, as the console reads them: the hash groups the session's two,
    // and the id as sent finds none.
    let list = |session: &str| {
        let app = app.clone();
        let (organization, project) = (organization.clone(), project.clone());
        let session = session.to_owned();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/internal/v1/telemetry/traces?session={session}"))
                        .header(SERVICE_SECRET_HEADER, SECRET)
                        .header(header::AUTHORIZATION, "Bearer tok_user_member")
                        .header("x-organization-id", organization.as_str())
                        .header("x-project-id", project.as_str())
                        .body(Body::empty())
                        .expect("a request"),
                )
                .await
                .expect("an answer");
            assert_eq!(response.status(), StatusCode::OK);
            json_body(
                &response
                    .into_body()
                    .collect()
                    .await
                    .expect("a body")
                    .to_bytes(),
            )
        }
    };
    let grouped = list(&rows[0].1).await;
    let mut listed: Vec<&str> = grouped["traces"]
        .as_array()
        .expect("traces")
        .iter()
        .map(|trace| trace["trace_id"].as_str().expect("a trace"))
        .collect();
    listed.sort_unstable();
    assert_eq!(listed, [runs[0].0, runs[1].0], "{grouped}");
    assert_eq!(list(EMAIL).await["total"], 0);
}

/// A trace's opening, sent alone, lists the trace open with nothing counted
/// and costs no unit; the agent's own span closes it, and an opening sent
/// again, or after the trace ended, changes neither.
#[sqlx::test]
async fn a_traces_opening_lists_it_open_until_its_own_span_closes_it(pool: PgPool) {
    const OPENED: &str = "4bf92f3577b34da6a3ce929d0e0e9f21";
    const LATE: &str = "4bf92f3577b34da6a3ce929d0e0e9f22";
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    auth.seat(
        "user_member",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    let app = lanes(common::state_with_stores_under(
        &pool,
        auth,
        Arc::new(NoLimits),
    ));
    let started = Utc::now() - chrono::Duration::minutes(5);
    let nanos = |at: chrono::DateTime<Utc>| at.timestamp_nanos_opt().expect("in range").to_string();
    let opening = |trace: &str| {
        json!({
            "traceId": trace, "spanId": "00f067aa0ba902b8",
            "parentSpanId": "00f067aa0ba902b7", "name": contract::TRACE_START, "kind": 1,
            "startTimeUnixNano": nanos(started), "endTimeUnixNano": nanos(started),
            "attributes": [
                { "key": contract::AGENT, "value": { "stringValue": "nightly-triage" } }
            ]
        })
    };
    let own = |trace: &str| {
        json!({
            "traceId": trace, "spanId": "00f067aa0ba902b7",
            "name": "invoke_agent nightly-triage", "kind": 1,
            "startTimeUnixNano": nanos(started),
            "endTimeUnixNano": nanos(started + chrono::Duration::seconds(2)),
            "attributes": [
                { "key": contract::AGENT, "value": { "stringValue": "nightly-triage" } },
                { "key": contract::OPERATION, "value": { "stringValue": "invoke_agent" } }
            ]
        })
    };
    let send = |spans: Vec<Value>| {
        let app = app.clone();
        async move {
            let body = json!({ "resourceSpans": [{ "scopeSpans": [{
                "scope": { "name": contract::SCOPE }, "spans": spans
            }]}]});
            let answer = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(LANE)
                        .header(header::AUTHORIZATION, format!("Bearer {INGEST_KEY}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .expect("a request"),
                )
                .await
                .expect("an answer");
            assert_eq!(answer.status(), StatusCode::OK);
        }
    };
    let listed = |trace_id: &'static str| {
        let app = app.clone();
        let (organization, project) = (organization.clone(), project.clone());
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/internal/v1/telemetry/traces")
                        .header(SERVICE_SECRET_HEADER, SECRET)
                        .header(header::AUTHORIZATION, "Bearer tok_user_member")
                        .header("x-organization-id", organization.as_str())
                        .header("x-project-id", project.as_str())
                        .body(Body::empty())
                        .expect("a request"),
                )
                .await
                .expect("an answer");
            assert_eq!(response.status(), StatusCode::OK);
            let all = json_body(
                &response
                    .into_body()
                    .collect()
                    .await
                    .expect("a body")
                    .to_bytes(),
            );
            all["traces"]
                .as_array()
                .expect("traces")
                .iter()
                .find(|trace| trace["trace_id"] == trace_id)
                .cloned()
                .unwrap_or_else(|| panic!("{trace_id} listed: {all}"))
        }
    };

    send(vec![opening(OPENED)]).await;
    let open = listed(OPENED).await;
    assert_eq!(open["ended"], false, "{open}");
    assert_eq!(
        (open["spans"].as_u64(), open["steps"].as_u64()),
        (Some(0), Some(0))
    );
    let only = Projects::one(project.clone());
    let (kind, units): (String, u32) = ch
        .store
        .read(
            &only,
            "SELECT kind, units FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)} AND name = {name:String}",
        )
        .param("name", contract::TRACE_START)
        .fetch_one()
        .await
        .expect("the opening's row");
    assert_eq!((kind.as_str(), units), ("trace_start", 0));

    // The agent's own span with its opening again, in a body of their own.
    send(vec![opening(OPENED), own(OPENED)]).await;
    let closed = listed(OPENED).await;
    assert_eq!(closed["ended"], true, "{closed}");
    assert_eq!(closed["status"], "ok");
    assert_eq!(
        (closed["spans"].as_u64(), closed["steps"].as_u64()),
        (Some(1), Some(0))
    );

    // An opening that lands after its trace ended leaves it ended.
    send(vec![own(LATE)]).await;
    send(vec![opening(LATE)]).await;
    let late = listed(LATE).await;
    assert_eq!(late["ended"], true, "{late}");

    // The dashboards count the two traces' own spans and none of their three
    // openings, from the hourly totals and from the spans alike.
    for uri in ["/rollups?days=1", "/rollups?days=1&group=user"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/internal/v1/telemetry{uri}"))
                    .header(SERVICE_SECRET_HEADER, SECRET)
                    .header(header::AUTHORIZATION, "Bearer tok_user_member")
                    .header("x-organization-id", organization.as_str())
                    .header("x-project-id", project.as_str())
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("an answer");
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let points = json_body(
            &response
                .into_body()
                .collect()
                .await
                .expect("a body")
                .to_bytes(),
        );
        let sum = |field: &str| -> u64 {
            points["points"]
                .as_array()
                .expect("points")
                .iter()
                .map(|point| point[field].as_u64().expect("a count"))
                .sum()
        };
        assert_eq!((sum("spans"), sum("traces")), (2, 2), "{uri}: {points}");
    }
    assert_eq!(
        (late["spans"].as_u64(), late["steps"].as_u64()),
        (Some(1), Some(0))
    );
}

#[sqlx::test]
async fn a_key_without_the_ingest_scope_or_no_key_is_refused_in_otlp_status(pool: PgPool) {
    let lane = lane(&pool, false);
    let body = export_json(&["chat"]).to_string().into_bytes();

    let (status, headers, answer) = lane
        .send(Some(READ_KEY), "application/json", None, body.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let answer = json_body(&answer);
    assert_eq!(answer["code"], 7, "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .unwrap()
            .starts_with("/errors/authz/forbidden"),
        "{answer}"
    );

    let (status, _, answer) = lane
        .send(None, "application/json", None, body.clone())
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(&answer)["code"], 16);

    // A protobuf request is refused in protobuf.
    let (status, headers, answer) = lane
        .send(Some(READ_KEY), "application/x-protobuf", None, body)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/x-protobuf"
    );
    #[derive(Clone, PartialEq, prost::Message)]
    struct RpcStatus {
        #[prost(int32, tag = "1")]
        code: i32,
        #[prost(string, tag = "2")]
        message: String,
    }
    let decoded = RpcStatus::decode(answer.as_slice()).expect("a Status");
    assert_eq!(decoded.code, 7);
    assert!(
        decoded.message.starts_with("/errors/authz/forbidden"),
        "{}",
        decoded.message
    );
}

/// A paused organization's scores are refused as its exports are, a 402.
#[sqlx::test]
async fn a_paused_organizations_scores_are_refused(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let state = common::state_with_entitlements(
        &pool,
        auth,
        Arc::new(common::PausedOrganization(organization)),
    );
    let body = json!({ "scores": [{ "session_id": "conv-1", "name": "thumbs",
        "data_type": "boolean", "value": 1 }] });
    let answer = telmoni_telemetry::v1_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/scores")
                .header(header::AUTHORIZATION, format!("Bearer {INGEST_KEY}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::PAYMENT_REQUIRED);
}

/// A paused organization keeps what it holds and takes in nothing new: its
/// export is refused whole, before its body is read, in OTLP's `Status` with
/// the plan's own words, as a 402 an exporter drops rather than retries.
#[sqlx::test]
async fn a_paused_organizations_export_is_refused_whole(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let state = common::state_with_entitlements(
        &pool,
        auth.clone(),
        Arc::new(common::PausedOrganization(organization.clone())),
    );
    let lane = Lane {
        app: lanes(state.clone()),
        project,
        other: ProjectId::new(),
        state,
        auth,
    };

    let (status, headers, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            export_json(&["chat"]).to_string().into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert!(
        headers.get(header::RETRY_AFTER).is_none(),
        "a paused organization is not asked to retry"
    );
    let answer = json_body(&answer);
    assert_eq!(answer["code"], 9, "{answer}");
    assert_eq!(
        answer["message"], "/errors/plan/limit-reached: This organization is paused.",
        "{answer}"
    );
}

#[sqlx::test]
async fn a_body_past_the_cap_is_too_large_however_it_is_sent(pool: PgPool) {
    let lane = lane(&pool, false);
    let huge = vec![b' '; contract::BATCH_MAX_BYTES + 1];
    let (status, headers, answer) = lane
        .send(Some(INGEST_KEY), "application/json", None, huge.clone())
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let answer = json_body(&answer);
    assert_eq!(answer["code"], 8, "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .unwrap()
            .starts_with("/errors/telemetry/batch-too-large"),
        "{answer}"
    );
    assert!(
        headers.get(header::RETRY_AFTER).is_none(),
        "a 413 is not retried"
    );
    let past_the_read = vec![b' '; contract::BATCH_MAX_BYTES + 2];
    let (status, _, _) = lane
        .send(Some(INGEST_KEY), "application/json", None, past_the_read)
        .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "a body the read itself stops"
    );

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&huge).expect("compress");
    let gzipped = encoder.finish().expect("compress");
    let (status, _, _) = lane
        .send(Some(INGEST_KEY), "application/json", Some("gzip"), gzipped)
        .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "a small body that unfolds past the cap"
    );
}

#[sqlx::test]
async fn a_body_that_is_not_an_export_is_invalid(pool: PgPool) {
    let lane = lane(&pool, false);
    let cases: [(&str, Option<&str>, Vec<u8>, &str); 3] = [
        ("text/plain", None, b"hello".to_vec(), "Content-Type"),
        (
            "application/json",
            None,
            b"{not json".to_vec(),
            "not OTLP's JSON",
        ),
        (
            "application/json",
            Some("br"),
            b"{}".to_vec(),
            "Content-Encoding",
        ),
    ];
    for (content_type, encoding, body, expected) in cases {
        let (status, _, answer) = lane
            .send(Some(INGEST_KEY), content_type, encoding, body)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{expected}");
        let answer = json_body(&answer);
        assert_eq!(answer["code"], 3, "{answer}");
        let message = answer["message"].as_str().unwrap();
        assert!(
            message.starts_with("/errors/telemetry/invalid-batch: "),
            "{message}"
        );
        assert!(message.contains(expected), "{message}");
    }
}

/// Another exporter's spans are answered as kept and held until their trace
/// is quiet; then an agent with none above it is the trace's, its calls and plain
/// spans its own, the calls of a trace with no agent above them a trace named by
/// the service, and a span in no trace dropped.
#[sqlx::test]
async fn another_exporters_spans_are_held_then_placed_in_their_traces(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    let span = |trace: &str, id: &str, parent: &str, name: &str, attributes: Value| {
        json!({
            "traceId": trace, "spanId": id, "parentSpanId": parent, "name": name,
            "startTimeUnixNano": "1760000000000000000",
            "endTimeUnixNano": "1760000000100000000",
            "attributes": attributes,
        })
    };
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let (agent_trace, calls_trace) = ("aa".repeat(16), "bb".repeat(16));
    let body = json!({ "resourceSpans": [{
        "resource": { "attributes": [text("service.name", "checkout")] },
        "scopeSpans": [{ "scope": { "name": "openinference" }, "spans": [
            span(&agent_trace, &"11".repeat(8), "", "GET /chat", json!([])),
            span(&agent_trace, &"22".repeat(8), &"11".repeat(8), "planner", json!([
                text(contract::OPERATION, "invoke_agent"), text(contract::AGENT_NAME, "planner"),
            ])),
            span(&agent_trace, &"33".repeat(8), &"22".repeat(8), "chat gpt-5", json!([
                text(contract::OPERATION, "chat"), text(contract::REQUEST_MODEL, "gpt-5"),
            ])),
            span(&calls_trace, &"44".repeat(8), "", "POST /ask", json!([])),
            span(&calls_trace, &"55".repeat(8), &"44".repeat(8), "chat gpt-5", json!([
                text(contract::OPERATION, "chat"), text(contract::REQUEST_MODEL, "gpt-5"),
            ])),
        ]}],
    }]});
    let (status, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            body.to_string().into_bytes(),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&answer)
    );
    assert_eq!(
        json_body(&answer)["partialSuccess"],
        Value::Null,
        "nothing refused"
    );
    let held: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry.held_spans WHERE landed_at IS NULL")
            .fetch_one(&pool)
            .await
            .expect("held");
    assert_eq!(held, 5);

    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    let rows = |sql: &'static str| {
        ch.store
            .read(&Projects::one(lane.project.clone()), sql)
            .fetch_all::<(String, String, String, String)>()
    };
    let none = rows(
        "SELECT span_id, kind, agent, trace_id FROM telemetry.spans \
         WHERE project_id IN {projects:Array(String)} ORDER BY span_id",
    )
    .await
    .expect("the rows");
    assert!(none.is_empty(), "a trace still sending waits: {none:?}");

    sqlx::query("UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("quiet");
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    common::eventually("the placed spans are read", || async {
        rows(
            "SELECT span_id, kind, agent, trace_id FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)} ORDER BY span_id",
        )
        .await
        .is_ok_and(|rows| rows.len() == 6)
    })
    .await;
    let landed = rows(
        "SELECT span_id, kind, agent, trace_id FROM telemetry.spans \
         WHERE project_id IN {projects:Array(String)} ORDER BY span_id",
    )
    .await
    .expect("the rows");
    let of = |id: &str| landed.iter().find(|(span, ..)| span == id).cloned();
    let row = |id: &str, kind: &str, agent: &str, trace: &str| {
        Some((
            id.repeat(8),
            kind.to_owned(),
            agent.to_owned(),
            trace.to_owned(),
        ))
    };
    assert_eq!(
        of(&"11".repeat(8)),
        row("11", "span", "planner", &agent_trace),
        "a span above the agent is the trace's, named for it"
    );
    assert_eq!(
        of(&"22".repeat(8)),
        row("22", "agent", "planner", &agent_trace)
    );
    assert_eq!(
        of(&"33".repeat(8)),
        row("33", "model_call", "planner", &agent_trace)
    );
    assert_eq!(
        of(&"44".repeat(8)),
        row("44", "span", "checkout", &calls_trace),
        "a trace with no agent is the service's"
    );
    assert_eq!(
        of(&"55".repeat(8)),
        row("55", "model_call", "checkout", &calls_trace)
    );
    let written = landed
        .iter()
        .find(|(_, kind, _, trace)| kind == "agent" && *trace == calls_trace)
        .expect("the calls' trace has an agent's span, written for it");
    assert_eq!(written.2, "checkout");
    assert_eq!(landed.len(), 6, "{landed:?}");

    // Landed, each keeps its trace; a second pass lands nothing again.
    let waiting: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry.held_spans WHERE landed_at IS NULL")
            .fetch_one(&pool)
            .await
            .expect("held");
    assert_eq!(waiting, 0);
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    let again = rows(
        "SELECT span_id, kind, agent, trace_id FROM telemetry.spans \
         WHERE project_id IN {projects:Array(String)} ORDER BY span_id",
    )
    .await
    .expect("the rows");
    assert_eq!(again.len(), 6);
}

/// Another exporter's texts holding a NUL, which Postgres cannot store, are
/// held and land with the NUL removed.
#[sqlx::test]
async fn another_exporters_texts_with_a_nul_are_held_and_land_without_it(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let trace = "cc".repeat(16);
    let span = |id: &str, parent: &str, name: &str, attributes: Value| {
        json!({
            "traceId": trace, "spanId": id, "parentSpanId": parent, "name": name,
            "startTimeUnixNano": "1760000000000000000",
            "endTimeUnixNano": "1760000000100000000",
            "attributes": attributes,
        })
    };
    let body = json!({ "resourceSpans": [{
        "resource": { "attributes": [
            text("service.name", "checkout"), text("service.version", "v\u{0}2"),
        ] },
        "scopeSpans": [{ "scope": { "name": "openinference" }, "spans": [
            span(&"11".repeat(8), "", "plan\u{0}ner", json!([
                text(contract::OPERATION, "invoke_agent"), text(contract::AGENT_NAME, "plan\u{0}ner"),
            ])),
            span(&"22".repeat(8), &"11".repeat(8), "chat\u{0} gpt-5", json!([
                text(contract::OPERATION, "chat"), text(contract::REQUEST_MODEL, "gpt\u{0}-5"),
                text("gen_ai.prompt.name", "tri\u{0}age"), text("telmoni.tag.team", "sup\u{0}port"),
            ])),
        ]}],
    }]});
    let (status, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            body.to_string().into_bytes(),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&answer)
    );
    assert_eq!(
        json_body(&answer)["partialSuccess"],
        Value::Null,
        "nothing refused"
    );

    sqlx::query("UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("quiet");
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    type Landed = (String, String, String, String, String, String, String);
    let read = || {
        ch.store
            .read(
                &Projects::one(lane.project.clone()),
                "SELECT span_id, name, agent, model, release, \
                        attributes['gen_ai.prompt.name'], tags['team'] \
                   FROM telemetry.spans \
                  WHERE project_id IN {projects:Array(String)} ORDER BY span_id",
            )
            .fetch_all::<Landed>()
    };
    common::eventually("the placed spans are read", || async {
        read().await.is_ok_and(|rows| rows.len() == 2)
    })
    .await;
    let landed = read().await.expect("the rows");
    let s = String::from;
    assert_eq!(
        landed,
        vec![
            (
                "11".repeat(8),
                s("planner"),
                s("planner"),
                s(""),
                s("v2"),
                s(""),
                s("")
            ),
            (
                "22".repeat(8),
                s("chat gpt-5"),
                s("planner"),
                s("gpt-5"),
                s("v2"),
                s("triage"),
                s("support")
            ),
        ]
    );
}

/// Another exporter's retrievals and memory searches land as retrievals,
/// named by what they searched: under an agent, as its own, and with none,
/// as the service's; an agent's data source stays an attribute of its span.
#[sqlx::test]
async fn another_exporters_retrievals_land_as_retrievals_named_by_their_source(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    let span = |trace: &str, id: &str, parent: &str, name: &str, attributes: Value| {
        json!({
            "traceId": trace, "spanId": id, "parentSpanId": parent, "name": name,
            "startTimeUnixNano": "1760000000000000000",
            "endTimeUnixNano": "1760000000100000000",
            "attributes": attributes,
        })
    };
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let int = |key: &str, value: i64| json!({ "key": key, "value": { "intValue": value } });
    let (agent_trace, calls_trace) = ("aa".repeat(16), "bb".repeat(16));
    let body = json!({ "resourceSpans": [{
        "resource": { "attributes": [text("service.name", "checkout")] },
        "scopeSpans": [{ "scope": { "name": "openinference" }, "spans": [
            span(&agent_trace, &"22".repeat(8), "", "invoke_agent planner", json!([
                text(contract::OPERATION, "invoke_agent"), text(contract::AGENT_NAME, "planner"),
                text(contract::DATA_SOURCE_ID, "docs-index"),
            ])),
            span(&agent_trace, &"33".repeat(8), &"22".repeat(8), "retrieval docs-index", json!([
                text(contract::OPERATION, "retrieval"), text(contract::DATA_SOURCE_ID, "docs-index"),
                int(contract::RETRIEVAL_TOP_K, 5),
            ])),
            span(&calls_trace, &"44".repeat(8), "", "POST /ask", json!([])),
            span(&calls_trace, &"55".repeat(8), &"44".repeat(8), "retrieval kb", json!([
                text(contract::OPERATION, "retrieval"), text(contract::DATA_SOURCE_ID, "kb"),
            ])),
            span(&calls_trace, &"66".repeat(8), &"44".repeat(8), "search_memory", json!([
                text(contract::OPERATION, contract::SEARCH_MEMORY),
                text(contract::MEMORY_STORE_ID, "preferences"),
                int(contract::MEMORY_RECORD_COUNT, 2),
            ])),
        ]}],
    }]});
    let (status, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            body.to_string().into_bytes(),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&answer)
    );
    sqlx::query("UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("quiet");
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    let read = || {
        ch.store
            .read(
                &Projects::one(lane.project.clone()),
                "SELECT span_id, kind, name, agent, trace_id, \
                 attributes['gen_ai.data_source.id'], attributes['gen_ai.retrieval.top_k'], \
                 attributes['gen_ai.memory.record.count'] FROM telemetry.spans \
                 WHERE project_id IN {projects:Array(String)} AND kind != 'trace_start' \
                 ORDER BY span_id",
            )
            .fetch_all::<(
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                String,
            )>()
    };
    common::eventually("the placed spans are read", || async {
        read().await.is_ok_and(|rows| rows.len() == 6)
    })
    .await;
    let landed = read().await.expect("the rows");
    let of = |id: &str| {
        landed
            .iter()
            .find(|row| row.0 == id)
            .cloned()
            .expect("the span landed")
    };
    let agent = of(&"22".repeat(8));
    assert_eq!((agent.1.as_str(), agent.2.as_str()), ("agent", "planner"));
    assert_eq!(
        agent.5, "docs-index",
        "an agent's data source is its own span's attribute"
    );
    let found = of(&"33".repeat(8));
    assert_eq!(
        (
            found.1.as_str(),
            found.2.as_str(),
            found.3.as_str(),
            found.4.as_str()
        ),
        ("retrieval", "docs-index", "planner", agent_trace.as_str())
    );
    assert_eq!((found.5.as_str(), found.6.as_str()), ("docs-index", "5"));
    let alone = of(&"55".repeat(8));
    assert_eq!(
        (
            alone.1.as_str(),
            alone.2.as_str(),
            alone.3.as_str(),
            alone.4.as_str()
        ),
        ("retrieval", "kb", "checkout", calls_trace.as_str()),
        "a retrieval with no agent above it is the service's"
    );
    let memory = of(&"66".repeat(8));
    assert_eq!(
        (memory.1.as_str(), memory.2.as_str(), memory.4.as_str()),
        ("retrieval", "preferences", calls_trace.as_str())
    );
    assert_eq!(memory.7, "2");
    let root = of(&"44".repeat(8));
    assert_eq!(
        (root.1.as_str(), root.3.as_str(), root.4.as_str()),
        ("span", "checkout", calls_trace.as_str()),
        "a root with no agent above it is kept, the service's"
    );
}

/// An agent's children, sent before the agent ends, wait for it however
/// quiet the trace goes, then land under it once it arrives.
#[sqlx::test]
async fn a_long_agents_children_wait_for_its_own_span(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    let trace = "cc".repeat(16);
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let export = |spans: Value| {
        json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": "x" }, "spans": spans }] }] })
            .to_string()
            .into_bytes()
    };
    let span = |id: &str, parent: &str, attributes: Value| {
        json!({
            "traceId": trace, "spanId": id, "parentSpanId": parent, "name": "work",
            "startTimeUnixNano": "1760000000000000000",
            "endTimeUnixNano": "1760000000100000000",
            "attributes": attributes,
        })
    };
    // Back in time as one, so a try stays before what arrived after it.
    let quiet = || async {
        sqlx::query(
            "UPDATE telemetry.held_spans SET received_at = received_at - interval '2 minutes', \
             tried_at = tried_at - interval '2 minutes'",
        )
        .execute(&pool)
        .await
        .expect("quiet");
    };
    let count = || async {
        ch.store
            .read(
                &Projects::one(lane.project.clone()),
                "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
            )
            .fetch_one::<u64>()
            .await
            .expect("counted")
    };
    let (status, _, _) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            export(json!([
                span(
                    &"02".repeat(8),
                    &"01".repeat(8),
                    json!([text(contract::OPERATION, "chat")])
                ),
                span(&"03".repeat(8), &"01".repeat(8), json!([])),
            ])),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    quiet().await;
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    assert_eq!(count().await, 0, "the children wait for their agent");
    let tried: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry.held_spans WHERE tried_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .expect("tried");
    assert_eq!(tried, 2);

    let (status, _, _) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            export(json!([span(
                &"01".repeat(8),
                "",
                json!([
                    text(contract::OPERATION, "invoke_agent"),
                    text(contract::AGENT_NAME, "coder")
                ]),
            )])),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    quiet().await;
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    common::eventually("the trace lands whole", || async { count().await == 3 }).await;
    let agents: Vec<(String, String)> = ch
        .store
        .read(
            &Projects::one(lane.project.clone()),
            "SELECT DISTINCT trace_id, agent FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_all()
        .await
        .expect("traces");
    assert_eq!(
        agents,
        [(trace.clone(), "coder".to_owned())],
        "one trace, the agent's, its plain span kept"
    );
}

/// A landing another replica began is left to it, then retried under its token once overdue.
#[sqlx::test]
async fn a_landing_begun_elsewhere_is_retried_only_once_overdue(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let body = json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": "x" }, "spans": [{
        "traceId": "dd".repeat(16), "spanId": "0d".repeat(8), "parentSpanId": "", "name": "chat",
        "startTimeUnixNano": "1760000000000000000", "endTimeUnixNano": "1760000000100000000",
        "attributes": [text(contract::OPERATION, "chat")],
    }]}]}]});
    let (status, _, _) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            body.to_string().into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // Named for the project, as a landing's token is: a store reused across
    // runs keeps one insert a token, and would drop this run's under a fixed one.
    sqlx::query(
        "UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes', \
         landing = $1, tried_at = now()",
    )
    .bind(format!("held/elsewhere/{}", lane.project.as_str()))
    .execute(&pool)
    .await
    .expect("begun elsewhere");
    let count = || async {
        ch.store
            .read(
                &Projects::one(lane.project.clone()),
                "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
            )
            .fetch_one::<u64>()
            .await
            .expect("counted")
    };
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    assert_eq!(count().await, 0, "its replica may still be writing");

    sqlx::query("UPDATE telemetry.held_spans SET tried_at = now() - interval '10 minutes'")
        .execute(&pool)
        .await
        .expect("overdue");
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    common::eventually("the overdue landing lands", || async { count().await == 2 }).await;
    let waiting: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry.held_spans WHERE landed_at IS NULL")
            .fetch_one(&pool)
            .await
            .expect("held");
    assert_eq!(waiting, 0);
}

/// Spans held as their project's purge ran are deleted, not tried forever.
#[sqlx::test]
async fn spans_held_for_a_project_gone_are_deleted(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    let body = json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": "x" }, "spans": [{
        "traceId": "ee".repeat(16), "spanId": "0e".repeat(8), "parentSpanId": "", "name": "work",
        "startTimeUnixNano": "1760000000000000000", "endTimeUnixNano": "1760000000100000000",
    }]}]}]});
    let (status, _, _) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            body.to_string().into_bytes(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("quiet");
    sqlx::query("DELETE FROM telemetry.project_settings WHERE project_id = $1")
        .bind(lane.project.as_str())
        .execute(&pool)
        .await
        .expect("purged");
    telmoni_telemetry::foreign::land_quiet(&lane.state)
        .await
        .expect("a pass");
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.held_spans")
        .fetch_one(&pool)
        .await
        .expect("held");
    assert_eq!(held, 0);
}

/// A trace's spans from two services — a worker continuing the orchestrator's
/// trace under its span, from another deployment — land as one trace, under
/// keys of one project; the same trace id under another project's key is that
/// project's own trace, so a forged header reaches no other tenant.
#[sqlx::test]
async fn a_traces_spans_from_two_services_land_as_one_trace_within_their_project(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    for (key, body) in [
        (
            INGEST_KEY,
            export_from("orchestrator", "production", &["chat plan"]),
        ),
        (
            INGEST_KEY,
            export_from("worker", "workers", &["chat under 00f067aa0ba902b0"]),
        ),
        (
            OTHER_KEY,
            export_from("elsewhere", "production", &["chat theirs"]),
        ),
    ] {
        let (status, _, answer) = lane
            .send(
                Some(key),
                "application/json",
                None,
                body.to_string().into_bytes(),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&answer)
        );
    }

    let traces: Vec<(String, u64, String, String)> = ch
        .store
        .read(
            &Projects::one(lane.project.clone()),
            "SELECT trace_id, count(), \
             arrayStringConcat(arraySort(groupUniqArray(environment)), ';'), \
             arrayStringConcat(arraySort(groupUniqArray(parent_span_id)), ';') \
             FROM telemetry.spans WHERE project_id IN {projects:Array(String)} \
             GROUP BY trace_id",
        )
        .fetch_all()
        .await
        .expect("the traces");
    assert_eq!(traces.len(), 1, "one trace, not one a service: {traces:?}");
    let (trace_id, spans, environments, parents) = &traces[0];
    assert_eq!(trace_id, TRACE);
    assert_eq!(*spans, 2);
    assert_eq!(environments, "production;workers");
    assert_eq!(
        parents, ";00f067aa0ba902b0",
        "the worker's span sits under the orchestrator's, as the headers said"
    );
    assert_eq!(
        common::rows_of(&ch.store, &lane.other).await.0,
        1,
        "the other project's trace of the same id is its own"
    );
}

/// An exporter retries with the same bytes when an answer was lost: the
/// retry is answered as the first was, and the rows are written once.
#[sqlx::test]
async fn an_export_sent_again_is_answered_as_it_was_and_written_once(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    if lane.state.redis.connection().await.is_none() {
        eprintln!(
            "skipping {}: Redis is not reachable (REDIS_URL, or the compose file's)",
            module_path!()
        );
        return;
    }
    let body = export_json(&["chat once"]).to_string().into_bytes();
    let (first_status, _, first) = lane
        .send(Some(INGEST_KEY), "application/json", None, body.clone())
        .await;
    assert_eq!(
        first_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&first)
    );
    assert!(
        json_body(&first)["partialSuccess"]["errorMessage"].is_string(),
        "an answer naming what was dropped, which the retry's is rebuilt to"
    );
    let (again_status, headers, again) = lane
        .send(Some(INGEST_KEY), "application/json", None, body)
        .await;
    assert_eq!(again_status, StatusCode::OK);
    assert_eq!(again, first, "the same answer again");
    assert_eq!(headers.get("telmoni-content-mode").unwrap(), "off");

    let count: u64 = ch
        .store
        .read(
            &Projects::one(lane.project.clone()),
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("a count");
    assert_eq!(count, 1, "written once");
}

/// A way to the ClickHouse at `url` that passes every request on, and none
/// of its answers while `withheld` is set: `url`, through it.
async fn withholding(url: &str, withheld: Arc<AtomicBool>) -> String {
    let mut through = reqwest::Url::parse(url).expect("a ClickHouse URL");
    let server = format!(
        "{}:{}",
        through.host_str().expect("its host"),
        through.port_or_known_default().expect("its port")
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let port = listener.local_addr().expect("its address").port();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let (server, withheld) = (server.clone(), withheld.clone());
            tokio::spawn(async move {
                let Ok(upstream) = tokio::net::TcpStream::connect(&server).await else {
                    return;
                };
                let (mut from_client, mut to_client) = client.into_split();
                let (mut from_server, mut to_server) = upstream.into_split();
                let answers = async {
                    let mut buffer = [0_u8; 8192];
                    while let Ok(read @ 1..) = from_server.read(&mut buffer).await {
                        let Some(answer) = buffer.get(..read) else {
                            return;
                        };
                        if !withheld.load(Ordering::SeqCst)
                            && to_client.write_all(answer).await.is_err()
                        {
                            return;
                        }
                    }
                };
                tokio::select! {
                    _ = tokio::io::copy(&mut from_client, &mut to_server) => {}
                    () = answers => {}
                }
            });
        }
    });
    through.set_port(Some(port)).expect("a port");
    through.set_host(Some("127.0.0.1")).expect("a host");
    through.into()
}

/// ⚠ A retry of an export whose insert outlasted its wait, which ClickHouse
/// still wrote, is answered, and neither its spans nor their totals count twice.
#[sqlx::test]
async fn a_retry_of_an_insert_that_outlasted_its_wait_is_written_once(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    if lane.state.redis.connection().await.is_none() {
        eprintln!(
            "skipping {}: Redis is not reachable (REDIS_URL, or the compose file's)",
            module_path!()
        );
        return;
    }
    let withheld = Arc::new(AtomicBool::new(false));
    let url = std::env::var("TELEMETRY_CLICKHOUSE_URL").expect("checked by clickhouse_or_skip");
    let mut store = Store::connect(&withholding(&url, withheld.clone()).await)
        .expect("ClickHouse through the way");
    store.insert_wait = Duration::from_secs(1);
    let state = common::state_with_stores_under(&pool, lane.auth.clone(), Arc::new(NoLimits));
    let mut state = Arc::try_unwrap(state).ok().expect("a state of its own");
    state.store = store;
    let state = Arc::new(state);
    let lane = Lane {
        app: lanes(state.clone()),
        state,
        ..lane
    };
    // Answered, so the client holds the table's columns before the answers stop.
    lane.export("chat before").await;

    withheld.store(true, Ordering::SeqCst);
    let body = export_json(&["chat unanswered"]).to_string().into_bytes();
    let (status, _, _) = lane
        .send(Some(INGEST_KEY), "application/json", None, body.clone())
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "told to retry");
    withheld.store(false, Ordering::SeqCst);
    common::eventually("ClickHouse writes the unanswered copy", || async {
        common::rows_of(&ch.store, &lane.project).await.0 == 2
    })
    .await;

    let (again, _, answer) = lane
        .send(Some(INGEST_KEY), "application/json", None, body)
        .await;
    assert_eq!(
        again,
        StatusCode::OK,
        "the retry is answered: {}",
        String::from_utf8_lossy(&answer)
    );
    let only = Projects::one(lane.project.clone());
    let spans: u64 = ch
        .store
        .read(
            &only,
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("the spans counted");
    let totalled: u64 = ch
        .store
        .read(
            &only,
            "SELECT sum(spans) FROM telemetry.spans_hourly \
             WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("their totals summed");
    assert_eq!(
        (spans, totalled),
        (2, 2),
        "the first copy written, the retry's not"
    );
}

/// An export of spans of one trace, the `n`th named `<name> <n>` and carrying
/// `tags[n]`, each span's id from its place.
fn tagged_export(name: &str, tags: &[Vec<(String, String)>]) -> Vec<u8> {
    let spans: Vec<Value> = tags
        .iter()
        .enumerate()
        .map(|(n, tags)| {
            let mut attributes = vec![json!({
                "key": contract::AGENT, "value": { "stringValue": "nightly-report" }
            })];
            attributes.extend(tags.iter().map(|(key, value)| {
                json!({ "key": format!("{}{key}", contract::TAG_PREFIX), "value": { "stringValue": value } })
            }));
            json!({
                "traceId": "4bf92f3577b34da6a3ce929d0e0e4736",
                "spanId": format!("{:016x}", n + 1),
                "name": format!("{name} {n}"),
                "kind": 1,
                "startTimeUnixNano": "1760000000000000000",
                "endTimeUnixNano": "1760000001500000000",
                "attributes": attributes,
            })
        })
        .collect();
    json!({
        "resourceSpans": [{
            "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": spans }]
        }]
    })
    .to_string()
    .into_bytes()
}

/// The lines of an answer's partial success that drop a tag as `cardinality`.
fn cardinality_lines(answer: &[u8]) -> Vec<String> {
    json_body(answer)
        .pointer("/partialSuccess/errorMessage")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(": cardinality; "))
        .map(str::to_owned)
        .collect()
}

/// ⚠ A tag's key takes so many values a day and a project so many keys; a tag
/// past either is dropped as `cardinality`, the span kept and counted again,
/// while a value or key already taken that day is always kept.
#[sqlx::test]
async fn tags_past_the_cardinality_cap_are_dropped_and_their_spans_kept(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    if lane.state.redis.connection().await.is_none() {
        eprintln!(
            "skipping {}: Redis is not reachable (REDIS_URL, or the compose file's)",
            module_path!()
        );
        return;
    }
    let tag = |key: &str, value: String| vec![(key.to_owned(), value)];
    let past = contract::TAG_VALUES_PER_DAY + 2;
    let first: Vec<_> = (0..past).map(|n| tag("request", format!("r{n}"))).collect();
    let (status, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            tagged_export("values", &first),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&answer)
    );
    let lines = cardinality_lines(&answer);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines.iter().all(|line| line.ends_with(
            "dropped telmoni.tag.request: cardinality; a project's tag takes up to 500 values a \
             UTC day"
        )),
        "{lines:?}"
    );
    assert_eq!(json_body(&answer)["partialSuccess"]["rejectedSpans"], 0);
    let (_, _, retried) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            tagged_export("values", &first),
        )
        .await;
    assert_eq!(retried, answer, "a retry is answered as its first copy was");

    // A value taken earlier in the day is kept; a new one is not.
    let again = vec![
        tag("request", "r0".to_owned()),
        tag("request", "new".to_owned()),
    ];
    let (_, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            tagged_export("again", &again),
        )
        .await;
    let lines = cardinality_lines(&answer);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("span 0000000000000002 dropped"),
        "{lines:?}"
    );

    // Keys past the project's cap: the export's first ones are taken, and a
    // span that lost its tags counts what it kept.
    let long = "v".repeat(contract::TAG_VALUE_MAX);
    let keys: Vec<Vec<(String, String)>> = (0..3)
        .map(|span| {
            (0..contract::TAGS_MAX)
                .map(|n| (format!("k{span}-{n}"), long.clone()))
                .collect()
        })
        .collect();
    let (_, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            tagged_export("keys", &keys),
        )
        .await;
    let lines = cardinality_lines(&answer);
    let taken = contract::TAG_KEYS_PER_DAY - 1;
    assert_eq!(lines.len(), 3 * contract::TAGS_MAX - taken, "{lines:?}");
    assert!(
        lines
            .iter()
            .all(|line| line.ends_with("cardinality; a project takes up to 64 tag keys a UTC day")),
        "{lines:?}"
    );

    let rows: Vec<(String, u64, u32)> = ch
        .store
        .read(
            &Projects::one(lane.project.clone()),
            "SELECT name, length(mapKeys(tags)), units FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)} ORDER BY name",
        )
        .fetch_all()
        .await
        .expect("the rows");
    assert_eq!(rows.len(), past + 2 + 3, "every span kept");
    let untagged = rows.iter().filter(|(_, tags, _)| *tags == 0).count();
    assert_eq!(untagged, 2 + 1 + 1, "{rows:?}");
    let keys: Vec<(u64, u32)> = rows
        .iter()
        .filter(|(name, _, _)| name.starts_with("keys "))
        .map(|(_, tags, units)| (*tags, *units))
        .collect();
    assert_eq!(keys.len(), 3);
    assert_eq!(keys[0].0, 32);
    assert_eq!(keys[1].0, 31);
    assert_eq!(
        keys[2],
        (0, 1),
        "a span that lost its tags counts what it kept"
    );
    assert!(keys[0].1 > 8, "{keys:?}");
}

/// An export landing after its project's purge marked the project's spans is
/// marked deleted with them, whether or not the purge has deleted the
/// settings row yet.
#[sqlx::test]
async fn an_export_landing_after_its_projects_purge_is_marked_with_it(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    for purged in ["the mark alone", "the whole purge"] {
        let lane = lane(&pool, true);
        lane.export("chat before").await;
        assert_eq!(
            common::rows_of(&ch.store, &lane.project).await,
            (1, 1),
            "{purged}"
        );

        // Deleted, auth's answer to change once the next export it read the
        // key of is in; and the purge run, to its mark or to its end.
        lane.auth.forget_after(&lane.project, 0);
        if purged == "the mark alone" {
            ch.store
                .delete_project(&lane.project)
                .await
                .expect("the purge's mark");
        } else {
            TelemetrySeam(lane.state.clone())
                .purge_project(&OrganizationId::new(), &lane.project)
                .await
                .expect("the project's purge");
        }
        common::eventually("the purge marks the project's spans deleted", || async {
            common::rows_of(&ch.store, &lane.project).await == (0, 0)
        })
        .await;
        let settings: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM telemetry.project_settings WHERE project_id = $1",
        )
        .bind(lane.project.as_str())
        .fetch_one(&pool)
        .await
        .expect("count the settings row");
        assert_eq!(
            settings,
            i64::from(purged == "the mark alone"),
            "{purged}: the settings row"
        );

        let (logging, logs) = captured_logs();
        let headers = lane.export("chat after").await;
        drop(logging);
        assert_eq!(
            headers.get("telmoni-content-mode").expect("the mode"),
            "off",
            "{purged}"
        );
        common::eventually(
            "the export's spans are marked deleted with the project's",
            || async { common::rows_of(&ch.store, &lane.project).await == (0, 0) },
        )
        .await;
        assert!(
            logs.text()
                .contains("deleted as it landed; its spans are marked deleted again"),
            "{purged}: {}",
            logs.text()
        );
        assert_eq!(
            lane.auth.homes_asked(),
            3,
            "{purged}: asked where the project is to key it, then once after each write"
        );
    }
}

/// Auth unable to say whether an export's project still exists keeps the
/// rows as written, and is not waited out: silence is never "gone".
#[sqlx::test]
async fn an_export_auth_cannot_settle_keeps_its_rows(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    lane.export("chat first").await;

    lane.auth.failing_homes(true);
    let (logging, logs) = captured_logs();
    lane.export("chat unanswered").await;
    drop(logging);
    let logged = logs.text();
    assert!(
        logged.contains("auth could not say whether an export's project outlived its write"),
        "{logged}"
    );
    assert!(!logged.contains("deleted as it landed"), "{logged}");

    lane.auth
        .failing_homes(false)
        .delaying_homes(Duration::from_secs(5));
    let (logging, logs) = captured_logs();
    let started = Instant::now();
    lane.export("chat late").await;
    let took = started.elapsed();
    drop(logging);
    assert!(
        took < Duration::from_secs(4),
        "auth's late answer is not waited for: {took:?}"
    );
    let logged = logs.text();
    assert!(
        logged.contains("auth did not say in time whether an export's project outlived its write"),
        "{logged}"
    );
    assert!(!logged.contains("deleted as it landed"), "{logged}");

    // A mark records at once and hides the rows within the second.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        common::rows_of(&ch.store, &lane.project).await.0,
        3,
        "every export's span kept"
    );
}

/// An export whose settling auth could not answer is settled later: auth
/// asked again on a task of its own, and a project deleted meanwhile has its
/// spans marked deleted once auth says so.
#[sqlx::test]
async fn an_export_left_unsettled_is_settled_once_auth_answers(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    lane.export("chat first").await;
    lane.auth.failing_homes(true);
    ch.store
        .delete_project(&lane.project)
        .await
        .expect("the purge's mark");
    lane.export("chat unanswered").await;
    common::eventually("the export's span lands past the mark", || async {
        common::rows_of(&ch.store, &lane.project).await.0 == 1
    })
    .await;

    lane.auth
        .forget_after(&lane.project, 0)
        .failing_homes(false);
    common::eventually("the later settle marks it deleted", || async {
        common::rows_of(&ch.store, &lane.project).await.0 == 0
    })
    .await;
}

/// An exporter that hangs up once its rows are in does not cut the write off
/// before its settling: a project deleted meanwhile has them marked deleted.
#[sqlx::test]
async fn an_export_nobody_waits_for_is_settled_all_the_same(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    lane.export("chat before").await;
    lane.auth.forget_after(&lane.project, 0);
    ch.store
        .delete_project(&lane.project)
        .await
        .expect("the purge's mark");
    common::eventually("the purge marks the project's spans deleted", || async {
        common::rows_of(&ch.store, &lane.project).await == (0, 0)
    })
    .await;

    // Auth's answer to come after the exporter has gone, within its second.
    lane.auth.delaying_homes(Duration::from_millis(500));
    let asked = lane.auth.homes_asked();
    let request = lane.app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri(LANE)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {INGEST_KEY}"))
            .body(Body::from(
                export_json(&["chat after"]).to_string().into_bytes(),
            ))
            .expect("a request"),
    );
    tokio::select! {
        answered = request => panic!(
            "answered before auth was asked: {:?}",
            answered.map(|response| response.status())
        ),
        () = async {
            while lane.auth.homes_asked() == asked {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        } => {}
    }
    common::eventually(
        "the export nobody waited for is settled, its spans marked deleted",
        || async { common::rows_of(&ch.store, &lane.project).await == (0, 0) },
    )
    .await;
}

/// What a server over `state` answers `request`, sent raw on a connection of
/// its own, so hyper reads the body as it reads a client's.
async fn answer_raw(state: Arc<telmoni_telemetry::AppState>, request: &str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("its address");
    tokio::spawn(async move { axum::serve(listener, lanes(state)).await });

    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("a connection");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request");
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut buffer = [0u8; 4096];
        loop {
            let read = stream.read(&mut buffer).await.expect("the answer");
            answer.extend_from_slice(buffer.get(..read).expect("within the buffer"));
            if read == 0 || String::from_utf8_lossy(&answer).contains("invalid-batch") {
                break;
            }
        }
    })
    .await
    .expect("answered within ten seconds");
    String::from_utf8_lossy(&answer).into_owned()
}

/// What a server over `state` answers an export of `length` bytes whose head
/// and first half come at once and whose second half comes once a fifth of a
/// second has passed with nothing answered.
async fn answered_after_its_body(
    state: Arc<telmoni_telemetry::AppState>,
    head: &str,
    length: usize,
) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("its address");
    tokio::spawn(async move { axum::serve(listener, lanes(state)).await });

    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("a connection");
    let half = vec![b' '; length / 2];
    stream
        .write_all(
            format!("{head}connection: close\r\ncontent-length: {length}\r\n\r\n").as_bytes(),
        )
        .await
        .expect("the head");
    stream.write_all(&half).await.expect("the first half");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), stream.peek(&mut [0u8; 1]))
            .await
            .is_err(),
        "answered with half the body still to come"
    );
    stream.write_all(&half).await.expect("the second half");
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut answer))
        .await
        .expect("answered within ten seconds")
        .expect("the answer");
    String::from_utf8_lossy(&answer).into_owned()
}

/// A refusal before the body is read waits for the body: hyper's close over
/// one still coming breaks the upload, which an exporter takes for no answer.
#[sqlx::test]
async fn a_refusal_before_the_body_is_read_waits_for_the_body(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest])
        .key(READ_KEY, &organization, &project, &[ApiKeyScope::Read]);
    let open = common::state(&pool, auth.clone());
    let paused = common::state_with_entitlements(
        &pool,
        auth.clone(),
        Arc::new(common::PausedOrganization(organization)),
    );
    let head = |bearer: &str, content_type: &str| {
        format!(
            "POST {LANE} HTTP/1.1\r\nhost: localhost\r\ncontent-type: {content_type}\r\n\
             authorization: Bearer {bearer}\r\n"
        )
    };
    for (state, head, status) in [
        (open.clone(), head("not_a_key", "application/json"), "401"),
        (open.clone(), head(READ_KEY, "application/json"), "403"),
        (open, head(INGEST_KEY, "text/plain"), "400"),
        (paused, head(INGEST_KEY, "application/json"), "402"),
    ] {
        let answer = answered_after_its_body(state, &head, 600_000).await;
        assert!(
            answer.starts_with(&format!("HTTP/1.1 {status} ")),
            "{answer}"
        );
    }
}

/// A path or a method the lane does not serve is answered in OTLP's shape
/// once the body is in, and before any key is asked for.
#[sqlx::test]
async fn a_stray_path_or_method_is_answered_after_its_body(pool: PgPool) {
    let state = common::state(&pool, AuthStub::new());
    for (request, status, shape) in [
        (
            "POST /otlp/v1/logs HTTP/1.1\r\ncontent-type: application/json\r\n",
            "404",
            r#"{"code":5,"message":"/errors/not-found: no such lane under /otlp; traces go to /otlp/v1/traces"}"#,
        ),
        (
            "POST /otlp/v1/metrics HTTP/1.1\r\ncontent-type: application/x-protobuf\r\n",
            "404",
            "content-type: application/x-protobuf",
        ),
        (
            "PUT /otlp/v1/traces HTTP/1.1\r\ncontent-type: application/json\r\n",
            "405",
            r#"{"code":12,"message":"/errors/method-not-allowed: this lane takes POST"}"#,
        ),
    ] {
        let answer = answered_after_its_body(
            state.clone(),
            &format!("{request}host: localhost\r\n"),
            600_000,
        )
        .await;
        assert!(
            answer.starts_with(&format!("HTTP/1.1 {status} ")),
            "{answer}"
        );
        assert!(answer.contains(shape), "{answer}");
    }
}

/// A body that has not arrived within the wait (a fifth of a second here, a
/// minute on every binary) is refused `invalid-batch`, and nothing written.
#[sqlx::test]
async fn a_body_that_does_not_arrive_in_time_is_refused(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let state = common::state_waiting_bodies(&pool, auth.clone(), Duration::from_millis(200));

    // Its headers whole, and the first byte of the thousand its body says.
    let started = Instant::now();
    let answer = answer_raw(
        state,
        &format!(
            "POST {LANE} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {INGEST_KEY}\r\n\
             content-type: application/json\r\ncontent-length: 1000\r\n\r\n{{"
        ),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
    assert!(
        answer.contains(
            "/errors/telemetry/invalid-batch: the body did not arrive within 0.2 seconds"
        ),
        "{answer}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(auth.homes_asked(), 0, "nothing keyed");
}

/// An export past the process's in-flight bound is refused `503`, which
/// exporters retry.
#[sqlx::test]
async fn an_export_past_the_in_flight_bound_is_refused_unavailable(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let Ok(mut state) = Arc::try_unwrap(common::state(&pool, auth.clone())) else {
        panic!("the suite's state is shared");
    };
    state.ingest_slots = Arc::new(tokio::sync::Semaphore::new(0));

    let answer = answer_raw(
        Arc::new(state),
        &format!(
            "POST {LANE} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {INGEST_KEY}\r\n\
             content-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
        ),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
    assert!(
        answer.contains("/errors/telemetry/store-unavailable: ingest is at its in-flight limit"),
        "{answer}"
    );
}

/// A chunked body hyper cannot read is refused `invalid-batch`, never as too large.
#[sqlx::test]
async fn a_body_that_cannot_be_read_is_invalid_not_too_large(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);

    // A chunk whose size line is no hex.
    let answer = answer_raw(
        common::state(&pool, auth.clone()),
        &format!(
            "POST {LANE} HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer {INGEST_KEY}\r\n\
             content-type: application/json\r\ntransfer-encoding: chunked\r\n\r\nzz\r\n"
        ),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
    assert!(
        answer.contains("/errors/telemetry/invalid-batch: the body could not be read"),
        "{answer}"
    );
    assert_eq!(auth.homes_asked(), 0, "nothing keyed");
}

/// The `n`th export of two traces, a span each, started `n` nanoseconds
/// apart, so none is answered as one sent again: past a plan's month, the
/// first is kept and the second sampled out, where `traces` were picked so
/// ([`picked`]).
fn two_traces(n: u32, traces: &[String; 2]) -> Vec<u8> {
    let mut export = export_json(&["kept", "sampled"]);
    for (span, trace) in traces.iter().enumerate() {
        let at = |field: &str| format!("/resourceSpans/0/scopeSpans/0/spans/{span}/{field}");
        *export
            .pointer_mut(&at("traceId"))
            .expect("the span's trace") = json!(trace);
        *export
            .pointer_mut(&at("startTimeUnixNano"))
            .expect("the span's start") =
            json!((1_760_000_000_000_000_000_u64 + u64::from(n)).to_string());
    }
    export.to_string().into_bytes()
}

/// Two traces, the first kept past a plan's month and the second sampled out
/// under the project's key (`capacity::kept_past_the_grant`), once the key
/// exists.
async fn picked(lane: &Lane) -> [String; 2] {
    let storage = telmoni_telemetry::hashing::storage_for_project(&lane.state, &lane.project)
        .await
        .expect("the project's key");
    let pick = |kept: bool| {
        (1..u128::MAX)
            .map(|n| uuid::Uuid::from_u128(n).simple().to_string())
            .find(|trace| telmoni_telemetry::capacity::kept_past_the_grant(&storage, trace) == kept)
            .expect("a trace")
    };
    [pick(true), pick(false)]
}

/// A lane over the stores, under a plan's month, and the notices it hands on.
fn lane_for_a_month(pool: &PgPool, month: common::PlanMonth) -> (Lane, Arc<common::NoticeLog>) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let notices = common::NoticeLog::new();
    let state =
        common::state_with_stores_noticed(pool, auth.clone(), Arc::new(month), notices.clone());
    let lane = Lane {
        app: lanes(state.clone()),
        project,
        other: ProjectId::new(),
        state,
        auth,
    };
    (lane, notices)
}

/// Past a month with no grace, one trace in ten is kept, flagged with its
/// rate, and the rest are counted in the answer in the plan's words; the
/// owners are told once.
#[sqlx::test]
async fn spans_past_the_month_are_sampled_and_counted_in_the_answer(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (lane, notices) = lane_for_a_month(
        &pool,
        common::PlanMonth {
            spans: 1,
            grace_days: 0,
            content_bytes: 25_000_000,
        },
    );
    let any = [TRACE.to_owned(), TRACE.to_owned()];
    let (status, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(1, &any),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let answer = json_body(&answer);
    assert_eq!(
        answer["partialSuccess"]["rejectedSpans"], 0,
        "the month had room: {answer}"
    );

    let runs = picked(&lane).await;
    let send = |n| {
        lane.send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(n, &runs),
        )
    };
    let (status, _, first) = send(2).await;
    assert_eq!(status, StatusCode::OK);
    let answer = json_body(&first);
    assert_eq!(answer["partialSuccess"]["rejectedSpans"], 1, "{answer}");
    let message = answer["partialSuccess"]["errorMessage"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.starts_with("1 spans sampled out: A larger plan holds more.\n"),
        "{message}"
    );
    assert!(
        message.contains("span 00f067aa0ba902b0 dropped"),
        "the kept span: {message}"
    );
    assert!(
        !message.contains("00f067aa0ba902b1"),
        "the sampled one went whole: {message}"
    );
    let told = || {
        notices
            .notices()
            .into_iter()
            .filter(|n| n.title == "Spans past the plan's month")
            .count()
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while told() == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The same export sent again is answered as it was: its sampled trace
    // still counted, not told it was kept.
    let (_, _, again) = send(2).await;
    assert_eq!(
        json_body(&again),
        json_body(&first),
        "answered as it first was"
    );
    send(3).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(told(), 1, "told once a month");
}

/// ⚠ A project capped mid-period is held to what it sent before its cap: the
/// hourly pass raises its count to its hours wholly past, so the next export
/// past the cap is sampled at once; and a count Redis lost is raised again.
#[sqlx::test]
async fn a_cap_set_mid_period_counts_the_hours_before_it(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let month = common::PlanMonth {
        spans: 1_000_000,
        grace_days: 7,
        content_bytes: 25_000_000,
    };
    let state = common::state_with_stores_noticed(
        &pool,
        auth.clone(),
        Arc::new(month),
        common::NoticeLog::new(),
    );
    let lane = Lane {
        app: lanes(state.clone()),
        project: project.clone(),
        other: ProjectId::new(),
        state: state.clone(),
        auth,
    };
    // Five units two hours ago, while nothing counted the project.
    let earlier = (Utc::now() - chrono::Duration::hours(2)).timestamp();
    ch.migrator
        .query(
            "INSERT INTO telemetry.spans_hourly \
               (project_id, hour, sample_rate, cost_reported, spans, units, duration_ms) \
             SELECT ?, toStartOfHour(toDateTime(?, 'UTC')), 1, false, 5, 5, \
                    quantilesTDigestState(0.5, 0.9, 0.99)(toFloat64(0))",
        )
        .bind(project.as_str())
        .bind(earlier)
        .execute()
        .await
        .expect("the earlier hour");
    sqlx::query(
        "INSERT INTO telemetry.project_limits (project_id, organization_id, spans)
         VALUES ($1, $2, 5)",
    )
    .bind(project.as_str())
    .bind(organization.as_str())
    .execute(&pool)
    .await
    .expect("the project's cap");

    // Its first export files it; its count knows nothing of the earlier hour.
    let any = [TRACE.to_owned(), TRACE.to_owned()];
    let (status, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(1, &any),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&answer)["partialSuccess"]["rejectedSpans"], 0);

    let sampled = |answer: &[u8]| {
        let answer = json_body(answer);
        assert_eq!(answer["partialSuccess"]["rejectedSpans"], 1, "{answer}");
        let message = answer["partialSuccess"]["errorMessage"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            message.starts_with("1 spans sampled out: This project's cap is 5 spans a month"),
            "{message}"
        );
    };
    telmoni_telemetry::capacity::tell_usage(&state)
        .await
        .expect("the hourly pass");
    let runs = picked(&lane).await;
    let (status, _, past) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(2, &runs),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    sampled(&past);

    // The count lost, as after a Redis restart, and raised again.
    let period =
        telmoni_telemetry::capacity::project_period(&state, &organization, Utc::now()).await;
    let mut connection = state.redis.connection().await.expect("Redis");
    let _: () = redis::cmd("DEL")
        .arg(format!("telemetry:project-spans:{project}:{period}"))
        .query_async(&mut connection)
        .await
        .expect("the count dropped");
    telmoni_telemetry::capacity::raise_project_spans(&state, &organization, &project)
        .await
        .expect("the count raised");
    let (status, _, past) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(3, &runs),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    sampled(&past);
}

/// ⚠ Where no plan sells capacity, as on a self-hosted installation, a cap
/// saved through the console counts the hours before it at once, again once
/// cleared and set again, and the hourly pass rebuilds a count Redis lost.
#[sqlx::test]
async fn a_cap_with_no_plan_counts_the_hours_before_it_and_is_rebuilt(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    // Saving caps files an audit row.
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    // The calendar month's first hour has no hour wholly past in it.
    use chrono::{Datelike as _, DurationRound as _};
    let this_hour = Utc::now()
        .duration_trunc(chrono::Duration::hours(1))
        .expect("an hour");
    let month_start = this_hour
        .date_naive()
        .with_day(1)
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .expect("the month")
        .and_utc();
    if this_hour == month_start {
        return;
    }
    let earlier = (this_hour - chrono::Duration::hours(1)).max(month_start);

    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    auth.seat(
        "user_admin",
        &organization,
        &project,
        OrganizationRole::Admin,
        Some(Role::Admin),
    );
    let state = common::state_with_stores(&pool, auth.clone());
    let lane = Lane {
        app: lanes(state.clone()),
        project: project.clone(),
        other: ProjectId::new(),
        state: state.clone(),
        auth,
    };
    ch.migrator
        .query(
            "INSERT INTO telemetry.spans_hourly \
               (project_id, hour, sample_rate, cost_reported, spans, units, duration_ms) \
             SELECT ?, toDateTime(?, 'UTC'), 1, false, 5, 5, \
                    quantilesTDigestState(0.5, 0.9, 0.99)(toFloat64(0))",
        )
        .bind(project.as_str())
        .bind(earlier.timestamp())
        .execute()
        .await
        .expect("the earlier hour");

    // Filed by its first export, with no cap and nothing counted.
    let any = [TRACE.to_owned(), TRACE.to_owned()];
    let (status, _, _) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(1, &any),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let console = router(state.clone());
    let set = |caps: Value| {
        let console = console.clone();
        let organization = organization.clone();
        let project = project.clone();
        async move {
            let answer = console
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri("/internal/v1/telemetry/limits")
                        .header(SERVICE_SECRET_HEADER, SECRET)
                        .header("authorization", "Bearer tok_user_admin")
                        .header("x-organization-id", organization.as_str())
                        .header("x-project-id", project.as_str())
                        .header("content-type", "application/json")
                        .body(Body::from(caps.to_string()))
                        .expect("a request"),
                )
                .await
                .expect("an answer");
            assert_eq!(answer.status(), StatusCode::OK, "the caps saved");
        }
    };
    let runs = picked(&lane).await;
    let sampled = |answer: &[u8]| {
        let answer = json_body(answer);
        assert_eq!(answer["partialSuccess"]["rejectedSpans"], 1, "{answer}");
    };

    set(json!({ "spans": 5 })).await;
    let (_, _, past) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(2, &runs),
        )
        .await;
    sampled(&past);

    // Cleared, its count lost meanwhile, and set again: counted at once.
    let period =
        telmoni_telemetry::capacity::project_period(&state, &organization, Utc::now()).await;
    let count = format!("telemetry:project-spans:{project}:{period}");
    let mut connection = state.redis.connection().await.expect("Redis");
    set(json!({})).await;
    let _: () = redis::cmd("DEL")
        .arg(&count)
        .query_async(&mut connection)
        .await
        .expect("the count dropped");
    set(json!({ "spans": 5 })).await;
    let (_, _, past) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(3, &runs),
        )
        .await;
    sampled(&past);

    // Lost again, and rebuilt by the hourly pass with no plan to read.
    let _: () = redis::cmd("DEL")
        .arg(&count)
        .query_async(&mut connection)
        .await
        .expect("the count dropped");
    telmoni_telemetry::capacity::tell_usage(&state)
        .await
        .expect("the hourly pass");
    let (_, _, past) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(4, &runs),
        )
        .await;
    sampled(&past);
}

/// A project its organization capped at fewer spans a month than the plan
/// holds is sampled past its cap with no grace, in words naming the cap and
/// who may raise it, its owners told once; the organization's month, with
/// room to spare and a grace besides, holds back nothing.
#[sqlx::test]
async fn a_project_past_its_own_cap_is_sampled_with_no_grace(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    let notices = common::NoticeLog::new();
    let month = common::PlanMonth {
        spans: 1_000_000,
        grace_days: 7,
        content_bytes: 25_000_000,
    };
    let state =
        common::state_with_stores_noticed(&pool, auth.clone(), Arc::new(month), notices.clone());
    let lane = Lane {
        app: lanes(state.clone()),
        project: project.clone(),
        other: ProjectId::new(),
        state,
        auth,
    };
    sqlx::query(
        "INSERT INTO telemetry.project_limits (project_id, organization_id, spans)
         VALUES ($1, $2, 1)",
    )
    .bind(project.as_str())
    .bind(organization.as_str())
    .execute(&pool)
    .await
    .expect("the project's cap");

    let any = [TRACE.to_owned(), TRACE.to_owned()];
    let (status, _, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(1, &any),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let answer = json_body(&answer);
    assert_eq!(
        answer["partialSuccess"]["rejectedSpans"], 0,
        "under the cap: {answer}"
    );

    let runs = picked(&lane).await;
    let (status, _, past) = lane
        .send(
            Some(INGEST_KEY),
            "application/json",
            None,
            two_traces(2, &runs),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let answer = json_body(&past);
    assert_eq!(answer["partialSuccess"]["rejectedSpans"], 1, "{answer}");
    let message = answer["partialSuccess"]["errorMessage"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.starts_with("1 spans sampled out: This project's cap is 1 spans a month"),
        "{message}"
    );
    let told = || {
        notices
            .notices()
            .into_iter()
            .filter(|n| n.title == "A project past its cap")
            .count()
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while told() == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    lane.send(
        Some(INGEST_KEY),
        "application/json",
        None,
        two_traces(3, &runs),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(told(), 1, "told once a month");
}

/// Past a month with a grace, every span is kept while it lasts.
#[sqlx::test]
async fn spans_past_the_month_are_kept_whole_through_the_grace(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (lane, _) = lane_for_a_month(
        &pool,
        common::PlanMonth {
            spans: 1,
            grace_days: 7,
            content_bytes: 25_000_000,
        },
    );
    for n in 0..2 {
        let (status, _, answer) = lane
            .send(
                Some(INGEST_KEY),
                "application/json",
                None,
                two_traces(n, &[TRACE.to_owned(), TRACE.to_owned()]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let answer = json_body(&answer);
        assert_eq!(
            answer["partialSuccess"]["rejectedSpans"], 0,
            "kept through the grace: {answer}"
        );
    }
}
