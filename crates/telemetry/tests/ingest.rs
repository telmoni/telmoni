//! Ingest through the module's own router: an export behind a key with the
//! ingest scope is read by the contract, its content dropped and named in
//! the answer, its user ids hashed under the project's own key, and its
//! rows put in ClickHouse; a key without the scope, a body past the cap, one
//! that is not OTLP and one with no span are each refused in OTLP's `Status`
//! in the request's encoding; a body sent again is answered as it first was;
//! and an export whose project was deleted as it landed is marked deleted
//! with the rest of the project's spans.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tower::ServiceExt;

use common::{AuthStub, clickhouse_or_skip};
use telmoni_shared::seam::{Entitlements, NoLimits, Telemetry as _};
use telmoni_shared::test_util::captured_logs;
use telmoni_shared::{ApiKeyScope, OrganizationId, ProjectId};
use telmoni_telemetry::contract;
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::Projects;
use telmoni_telemetry::{otlp_router, router};

const LANE: &str = "/otlp/v1/traces";
const INGEST_KEY: &str = "tm_ingest_key";
const READ_KEY: &str = "tm_read_only_key";
/// Another project's ingest key, in another organization.
const OTHER_KEY: &str = "tm_other_projects_key";
const RUN: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01";

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

/// An export as our SDK sends one: a model call and its run, one content
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

/// ⚠ For now a span with no run is another exporter's: refused one by
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
    assert_eq!(
        lane.auth.homes_asked(),
        1,
        "auth asked where the project is to key it, and not again with nothing written"
    );
}

/// A run's spans from two services — a worker continuing the orchestrator's
/// run under its span, from another deployment — land as one run, under
/// keys of one project; the same run id under another project's key is that
/// project's own run, so a forged header reaches no other tenant.
#[sqlx::test]
async fn a_runs_spans_from_two_services_land_as_one_run_within_their_project(pool: PgPool) {
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

    let runs: Vec<(String, u64, String, String)> = ch
        .store
        .read(
            &Projects::one(lane.project.clone()),
            "SELECT toString(run_id), count(), \
             arrayStringConcat(arraySort(groupUniqArray(environment)), ';'), \
             arrayStringConcat(arraySort(groupUniqArray(parent_span_id)), ';') \
             FROM telemetry.spans WHERE project_id IN {projects:Array(String)} \
             GROUP BY run_id",
        )
        .fetch_all()
        .await
        .expect("the runs");
    assert_eq!(runs.len(), 1, "one run, not one a service: {runs:?}");
    let (run_id, spans, environments, parents) = &runs[0];
    assert_eq!(run_id, RUN);
    assert_eq!(*spans, 2);
    assert_eq!(environments, "production;workers");
    assert_eq!(
        parents, ";00f067aa0ba902b0",
        "the worker's span sits under the orchestrator's, as the headers said"
    );
    assert_eq!(
        common::rows_of(&ch.store, &lane.other).await.0,
        1,
        "the other project's run of the same id is its own"
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
                .purge_project(&lane.project)
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
