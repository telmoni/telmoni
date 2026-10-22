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

use common::{AuthStub, Ceiling, clickhouse_or_skip};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::seam::{Entitlements, NoLimits, Telemetry as _};
use telmoni_shared::test_util::{apply_audit_migrations, captured_logs};
use telmoni_shared::{ApiKeyScope, MonitorState, OrganizationId, ProjectId};
use telmoni_telemetry::contract;
use telmoni_telemetry::db::{self as telemetry_db, MonitorWrite};
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::Projects;
use telmoni_telemetry::{otlp_router, router};

const LANE: &str = "/otlp/v1/traces";
const INGEST_KEY: &str = "telmoni_ingest_key";
const READ_KEY: &str = "telmoni_read_only_key";
/// Another project's ingest key, in another organization.
const OTHER_KEY: &str = "telmoni_other_projects_key";
const RUN: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01";

/// The module's lanes as the binary mounts them, ingest nested at `/otlp`.
fn lanes(state: Arc<telmoni_telemetry::AppState>) -> Router {
    router(state.clone()).nest("/otlp", otlp_router(state))
}

/// The module's router, the project its two keys open, another project its
/// own key opens, and auth as the module holds it.
struct Lane {
    app: Router,
    organization: OrganizationId,
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
        organization,
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
        organization,
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

/// A span of `RUN` as our SDK sends one: `name`, under the agent
/// `nightly-report`, `extra` attributes beside its run and agent, begun
/// `ago_s` seconds before now and ended one second later.
fn run_span(n: u32, name: &str, extra: &[Value], ago_s: u64) -> Value {
    span_of(RUN, "nightly-report", n, name, extra, ago_s)
}

/// [`run_span`] of another run, or another agent's.
fn span_of(run: &str, agent: &str, n: u32, name: &str, extra: &[Value], ago_s: u64) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_nanos();
    let start = now - u128::from(ago_s) * 1_000_000_000;
    let mut attributes = vec![
        json!({ "key": contract::RUN_ID, "value": { "stringValue": run } }),
        json!({ "key": contract::AGENT, "value": { "stringValue": agent } }),
    ];
    attributes.extend(extra.iter().cloned());
    json!({
        "traceId": "4bf92f3577b34da6a3ce929d0e0e4736",
        "spanId": format!("00f067aa0ba9{n:04x}"),
        "name": name,
        "kind": 1,
        "startTimeUnixNano": start.to_string(),
        "endTimeUnixNano": (start + 1_000_000_000).to_string(),
        "attributes": attributes,
    })
}

/// `spans` in one export, under our SDK's scope.
fn spans_export(spans: Vec<Value>) -> Vec<u8> {
    json!({
        "resourceSpans": [{
            "resource": { "attributes": [
                { "key": "service.name", "value": { "stringValue": "reporter" } }
            ]},
            "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": spans }]
        }]
    })
    .to_string()
    .into_bytes()
}

impl Lane {
    /// `spans` exported under the ingest key, answered `200`: the answer.
    async fn export_spans(&self, spans: Vec<Value>) -> Value {
        let (status, _, answer) = self
            .send(
                Some(INGEST_KEY),
                "application/json",
                None,
                spans_export(spans),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&answer)
        );
        json_body(&answer)
    }

    /// The project's monitor of `agent` made, healthy, held to `max_steps`.
    async fn monitor_of(&self, agent: &str, max_steps: Option<i32>) {
        let mut tx = project_scope(&self.state.db, &self.project)
            .await
            .expect("the project's scope")
            .bind_organization(&self.organization)
            .await
            .expect("bound to its organization");
        telemetry_db::ensure_settings(&mut tx, &self.project, &self.organization)
            .await
            .expect("settings");
        telemetry_db::insert_monitor(
            &mut tx,
            &self.project,
            &self.organization,
            uuid::Uuid::new_v4(),
            MonitorState::Healthy,
            MonitorWrite {
                slug: agent,
                name: agent,
                agent,
                schedule: None,
                interval_s: None,
                timezone: "UTC",
                grace_s: None,
                expected_duration_s: None,
                stall_after_s: None,
                max_steps,
                max_cost_usd: None,
                next_due_at: None,
            },
        )
        .await
        .expect("a monitor");
        tx.commit().await.expect("committed");
    }
}

/// The project's open run of `RUN`: its steps, its step cap and its source.
async fn open_run_of(pool: &PgPool, project: &ProjectId) -> Option<(i32, Option<i32>, String)> {
    sqlx::query_as(
        "SELECT steps, max_steps, source FROM telemetry.open_runs \
          WHERE project_id = $1 AND id = $2::uuid",
    )
    .bind(project)
    .bind(RUN)
    .fetch_optional(pool)
    .await
    .expect("the open run")
}

/// The project's monitor of `agent`: its state, its last run and its steps.
async fn monitor_state(
    pool: &PgPool,
    project: &ProjectId,
    agent: &str,
) -> (String, Option<uuid::Uuid>, Option<i32>) {
    sqlx::query_as(
        "SELECT state::text, last_run_id, last_run_steps FROM telemetry.monitors \
          WHERE project_id = $1 AND agent = $2",
    )
    .bind(project)
    .bind(agent)
    .fetch_one(pool)
    .await
    .expect("the monitor")
}

/// An SDK's run is kept open in Postgres while it goes: opened by its
/// `telmoni.run.start` with the tighter of its own and its monitor's bounds,
/// its steps added export by export and held to the cap, and ended by its
/// own span, its totals left on the monitor; a span of it that comes later
/// opens nothing again.
#[sqlx::test]
async fn an_sdks_run_is_kept_open_held_to_its_bounds_and_ended_by_its_own_span(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    lane.monitor_of("nightly-report", Some(2)).await;
    let max_steps = json!({ "key": contract::BOUND_MAX_STEPS, "value": { "intValue": "5" } });

    lane.export_spans(vec![run_span(0, contract::RUN_START, &[max_steps], 30)])
        .await;
    assert_eq!(
        open_run_of(&pool, &lane.project).await,
        Some((0, Some(2), "ingest".to_owned())),
        "the tighter cap, the monitor's"
    );
    assert_eq!(
        monitor_state(&pool, &lane.project, "nightly-report")
            .await
            .0,
        "running"
    );

    lane.export_spans(vec![
        run_span(1, "plan", &[], 20),
        run_span(2, "fetch", &[], 19),
    ])
    .await;
    assert_eq!(
        open_run_of(&pool, &lane.project).await.map(|run| run.0),
        Some(2)
    );
    assert_eq!(
        monitor_state(&pool, &lane.project, "nightly-report")
            .await
            .0,
        "running",
        "at the cap is within it"
    );
    lane.export_spans(vec![run_span(3, "write", &[], 10)]).await;
    assert_eq!(
        monitor_state(&pool, &lane.project, "nightly-report")
            .await
            .0,
        "over_bound"
    );

    let invoke = json!({ "key": contract::OPERATION, "value": { "stringValue": "invoke_agent" } });
    lane.export_spans(vec![run_span(4, "nightly-report", &[invoke], 31)])
        .await;
    assert_eq!(open_run_of(&pool, &lane.project).await, None, "ended");
    let (state, last_run, steps) = monitor_state(&pool, &lane.project, "nightly-report").await;
    assert_eq!(state, "healthy");
    assert_eq!(last_run.map(|id| id.to_string()).as_deref(), Some(RUN));
    assert_eq!(steps, Some(3));

    lane.export_spans(vec![run_span(5, "late", &[], 5)]).await;
    assert_eq!(
        open_run_of(&pool, &lane.project).await,
        None,
        "a span that comes after the run's end opens nothing"
    );
}

/// A run naming an agent no monitor has makes one, `new`, as a ping's
/// unknown slug does; past the plan's monitors nothing is made, the spans
/// are kept, and the export's answer says why.
#[sqlx::test]
async fn a_run_of_an_unknown_agent_makes_its_monitor_or_says_why_not(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let lane = lane(&pool, true);
    let answer = lane.export_spans(vec![run_span(0, "plan", &[], 10)]).await;
    assert!(
        answer["partialSuccess"].is_null(),
        "nothing to say: {answer}"
    );
    let (state, _, _) = monitor_state(&pool, &lane.project, "nightly-report").await;
    assert_eq!(state, "new");
    assert_eq!(
        open_run_of(&pool, &lane.project).await.map(|run| run.0),
        Some(1)
    );

    let full = lane_under(&pool, Some(Ceiling::of(&pool, 0)));
    let answer = full.export_spans(vec![run_span(0, "plan", &[], 10)]).await;
    let message = answer["partialSuccess"]["errorMessage"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("nightly-report") && message.contains("Move to Pro for more."),
        "{answer}"
    );
    assert_eq!(
        answer["partialSuccess"]["rejectedSpans"],
        json!(0),
        "nothing refused: {answer}"
    );
    let made: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry.monitors WHERE project_id = $1")
            .bind(&full.project)
            .fetch_one(&pool)
            .await
            .expect("a count");
    assert_eq!(made, 0);
}

/// A run first heard long after it began — a late first export, a sender's
/// slow clock — is heard now: not quiet at once, nor past its ceiling.
#[sqlx::test]
async fn a_run_first_heard_late_is_heard_now(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    lane.monitor_of("nightly-report", None).await;
    sqlx::query("UPDATE telemetry.monitors SET stall_after_s = 60 WHERE project_id = $1")
        .bind(&lane.project)
        .execute(&pool)
        .await
        .expect("a stall timeout");

    lane.export_spans(vec![run_span(0, "plan", &[], 25 * 60 * 60)])
        .await;
    let (heard_now, ends_ahead): (bool, bool) = sqlx::query_as(
        "SELECT last_heard_at > now() - interval '1 minute', ends_by > now() \
           FROM telemetry.open_runs WHERE project_id = $1 AND id = $2::uuid",
    )
    .bind(&lane.project)
    .bind(RUN)
    .fetch_one(&pool)
    .await
    .expect("the open run");
    assert!(heard_now && ends_ahead);
    assert_eq!(
        monitor_state(&pool, &lane.project, "nightly-report")
            .await
            .0,
        "running"
    );
}

/// Two runs ending in one export leave their monitor as the later stands,
/// whatever their ids.
#[sqlx::test]
async fn runs_ending_in_one_export_settle_in_the_order_they_ended(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    lane.monitor_of("nightly-report", None).await;
    let (failed, later) = (
        "ff91a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f03",
        "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f02",
    );
    let invoke = json!({ "key": contract::OPERATION, "value": { "stringValue": "invoke_agent" } });
    let mut first = span_of(
        failed,
        "nightly-report",
        0,
        "nightly-report",
        std::slice::from_ref(&invoke),
        60,
    );
    first["status"] = json!({ "code": 2 });
    let second = span_of(later, "nightly-report", 1, "nightly-report", &[invoke], 30);

    lane.export_spans(vec![second, first]).await;
    let (state, last_run, _) = monitor_state(&pool, &lane.project, "nightly-report").await;
    assert_eq!(state, "healthy");
    assert_eq!(last_run.map(|id| id.to_string()).as_deref(), Some(later));
}

/// A run whose agent's slug another agent's monitor holds makes nothing, and
/// its export is told whose it is once, however many of its runs it carries.
#[sqlx::test]
async fn a_run_whose_slug_another_agent_holds_is_told_so_once(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    lane.monitor_of("nightly-report", None).await;
    let answer = lane
        .export_spans(vec![
            span_of(RUN, "Nightly Report", 0, "plan", &[], 20),
            span_of(
                "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f04",
                "Nightly Report",
                1,
                "plan",
                &[],
                10,
            ),
        ])
        .await;
    let message = answer["partialSuccess"]["errorMessage"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        message
            .matches("the monitor nightly-report watches agent \"nightly-report\"")
            .count(),
        1,
        "{answer}"
    );
    let monitors: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry.monitors WHERE project_id = $1")
            .bind(&lane.project)
            .fetch_one(&pool)
            .await
            .expect("a count");
    assert_eq!(monitors, 1);
}

/// A run of a project moved since its export's key was resolved is left for
/// its next export, never filed under the organization the project left.
#[sqlx::test]
async fn a_run_of_a_project_moved_meanwhile_is_left(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let lane = lane(&pool, true);
    lane.monitor_of("nightly-report", None).await;
    sqlx::query("UPDATE telemetry.project_settings SET organization_id = $2 WHERE project_id = $1")
        .bind(&lane.project)
        .bind(OrganizationId::new())
        .execute(&pool)
        .await
        .expect("moved");

    lane.export_spans(vec![run_span(0, contract::RUN_START, &[], 5)])
        .await;
    assert_eq!(open_run_of(&pool, &lane.project).await, None);
}
