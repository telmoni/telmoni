//! Ingest through the module's own router: an export behind a key with the
//! ingest scope is read by the contract, its content dropped and named in
//! the answer, its user ids hashed under the project's own key, and its
//! rows put in ClickHouse; a key without the scope, a body past the cap, one
//! that is not OTLP and one with no span are each refused in OTLP's `Status`
//! in the request's encoding; a body sent again is answered as it first was.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::io::Write as _;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message as _;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, clickhouse_or_skip};
use telmoni_shared::{ApiKeyScope, OrganizationId, ProjectId};
use telmoni_telemetry::contract;
use telmoni_telemetry::router;
use telmoni_telemetry::store::Projects;

const LANE: &str = "/otlp/v1/traces";
const INGEST_KEY: &str = "telmoni_ingest_key";
const READ_KEY: &str = "telmoni_read_only_key";
const RUN: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01";

/// The module's router and the project its two keys open.
struct Lane {
    app: Router,
    project: ProjectId,
    state: Arc<telmoni_telemetry::AppState>,
}

/// A lane over the stores a suite can reach, or over none.
fn lane(pool: &PgPool, with_stores: bool) -> Lane {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(INGEST_KEY, &organization, &project, &[ApiKeyScope::Ingest])
        .key(READ_KEY, &organization, &project, &[ApiKeyScope::Read]);
    let state = if with_stores {
        common::state_with_stores(pool, auth)
    } else {
        common::state(pool, auth)
    };
    Lane {
        app: router(state.clone()),
        project,
        state,
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
}

fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("a JSON body")
}

/// An export as our SDK sends one: a model call and its run, one content
/// attribute among them, and a user id.
fn export_json(span_names: &[&str]) -> Value {
    let spans: Vec<Value> = span_names
        .iter()
        .enumerate()
        .map(|(n, name)| {
            json!({
                "traceId": "4bf92f3577b34da6a3ce929d0e0e4736",
                "spanId": format!("00f067aa0ba902b{n}"),
                "name": name,
                "kind": 1,
                "startTimeUnixNano": "1760000000000000000",
                "endTimeUnixNano": "1760000001500000000",
                "attributes": [
                    { "key": contract::RUN_ID, "value": { "stringValue": RUN } },
                    { "key": contract::AGENT, "value": { "stringValue": "nightly-report" } },
                    { "key": contract::OPERATION, "value": { "stringValue": "chat" } },
                    { "key": contract::RESPONSE_MODEL, "value": { "stringValue": "claude-sonnet-5-5" } },
                    { "key": contract::INPUT_TOKENS, "value": { "intValue": "1200" } },
                    { "key": contract::USER_ID, "value": { "stringValue": "acme-42" } },
                    { "key": "gen_ai.input.messages", "value": { "stringValue": "[{\"role\":\"user\",\"content\":\"hi\"}]" } }
                ]
            })
        })
        .collect();
    json!({
        "resourceSpans": [{
            "resource": { "attributes": [
                { "key": "service.name", "value": { "stringValue": "reporter" } },
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

    let rows: Vec<(String, String, String, u64)> = ch
        .store
        .read(
            &Projects::one(lane.project.clone()),
            "SELECT name, kind, user_id, input_tokens FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)} ORDER BY name",
        )
        .fetch_all()
        .await
        .expect("the rows");
    assert_eq!(rows.len(), 2, "{rows:?}");
    for (name, kind, user_id, input_tokens) in &rows {
        assert!(name.starts_with("chat "), "{name}");
        assert_eq!(kind, "model_call");
        assert_eq!(*input_tokens, 1_200);
        assert_eq!(user_id.len(), 32, "16 bytes of HMAC in hex: {user_id}");
        assert!(user_id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(user_id, "acme-42", "never stored as sent");
    }
    assert_eq!(rows[0].2, rows[1].2, "the same user hashes the same");

    let (sealed, hashed): (bool, bool) = sqlx::query_as(
        "SELECT hashing_key_ciphertext IS NOT NULL, user_id_hashed \
           FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(lane.project.as_str())
    .fetch_one(&pool)
    .await
    .expect("the settings row the first export files");
    assert!(
        sealed && hashed,
        "the project's key is made and sealed with its row"
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
        auth,
        Arc::new(common::PausedOrganization(organization)),
    );
    let lane = Lane {
        app: router(state.clone()),
        project,
        state,
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
    let cases: [(&str, Option<&str>, Vec<u8>, &str); 4] = [
        ("text/plain", None, b"hello".to_vec(), "Content-Type"),
        (
            "application/json",
            None,
            b"{not json".to_vec(),
            "not OTLP's JSON",
        ),
        (
            "application/json",
            None,
            json!({ "resourceSpans": [] }).to_string().into_bytes(),
            "holds no span",
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

/// ⚠ Until 6.2, a span with no run is another exporter's: refused one by
/// one, counted, and the export otherwise answered; nothing reaches the
/// store, so this needs none.
#[sqlx::test]
async fn spans_outside_a_run_are_refused_one_by_one(pool: PgPool) {
    let lane = lane(&pool, false);
    let foreign = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![
                    Span {
                        trace_id: vec![1; 16],
                        span_id: vec![2; 8],
                        name: "GET /".into(),
                        start_time_unix_nano: 1_760_000_000_000_000_000,
                        end_time_unix_nano: 1_760_000_000_100_000_000,
                        ..Span::default()
                    },
                    Span {
                        trace_id: vec![1; 16],
                        span_id: vec![3; 8],
                        name: "SELECT".into(),
                        start_time_unix_nano: 1_760_000_000_000_000_000,
                        end_time_unix_nano: 1_760_000_000_100_000_000,
                        ..Span::default()
                    },
                ],
                ..ScopeSpans::default()
            }],
            ..ResourceSpans::default()
        }],
    };
    let (status, headers, answer) = lane
        .send(
            Some(INGEST_KEY),
            "application/x-protobuf",
            None,
            foreign.encode_to_vec(),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&answer)
    );
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/x-protobuf"
    );
    let decoded = ExportTraceServiceResponse::decode(answer.as_slice()).expect("OTLP's response");
    let partial = decoded.partial_success.expect("something was refused");
    assert_eq!(partial.rejected_spans, 2);
    assert_eq!(
        partial.error_message.lines().count(),
        2,
        "{}",
        partial.error_message
    );
    assert!(
        partial
            .error_message
            .contains("span 0202020202020202 refused: no telmoni.run_id"),
        "{}",
        partial.error_message
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
