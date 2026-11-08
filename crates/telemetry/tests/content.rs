//! Content through the door: a project whose mode is `on` keeps each content
//! field, and each text no inventory names, whole and sealed under its own
//! key in `plain_content`, never in ClickHouse, its bytes on its
//! organization's row of `content_held`, counted once however often an export
//! is sent; a project left `off` keeps none and makes no key. A purge, a
//! transfer, the nightly purge, a trace's deletion and an end user's erasure
//! each keep the count true.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, clickhouse_or_skip};
use telmoni_shared::audit::{Actor, lock_chain};
use telmoni_shared::envelope::SealedRef;
use telmoni_shared::middleware::api_key::{ApiKeyGate, refuse_as_problem, require_api_key};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::Telemetry as _;
use telmoni_shared::test_util::{apply_audit_migrations, captured_logs};
use telmoni_shared::{
    ApiKeyScope, ContentMode, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind,
    TelmoniError,
};
use telmoni_telemetry::ingest::batch::SpanContent;
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::Projects;
use telmoni_telemetry::{
    AppState, content, contract, otlp_router, retention, router, v1_reads, v1_router,
};

const KEY: &str = "tm_content_key";
/// A key holding `read` alone, on the same project.
const READ_KEY: &str = "tm_content_read_key";
const RUN: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01";
const SPAN: &str = "00f067aa0ba902b7";
const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const MESSAGES: &str = r#"[{"role":"user","parts":[{"type":"text","content":"hi"}]}]"#;

/// A run's id, as content is opened by.
fn run(id: &str) -> uuid::Uuid {
    id.parse().expect("a run's id")
}

struct Door {
    app: Router,
    state: Arc<AppState>,
    auth: Arc<AuthStub>,
    organization: OrganizationId,
    project: ProjectId,
}

/// The module's lanes over both stores, a key on a project whose settings row
/// says `mode`.
async fn door(pool: &PgPool, mode: ContentMode) -> Door {
    door_over(pool, mode, true).await
}

/// [`door`], over a ClickHouse never dialled where `stores` is false.
async fn door_over(pool: &PgPool, mode: ContentMode, stores: bool) -> Door {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.key(KEY, &organization, &project, &[ApiKeyScope::Ingest]);
    auth.key(READ_KEY, &organization, &project, &[ApiKeyScope::Read]);
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id, content_mode) \
         VALUES ($1, $2, $3)",
    )
    .bind(&project)
    .bind(&organization)
    .bind(mode)
    .execute(pool)
    .await
    .expect("seed the project's mode");
    let state = if stores {
        common::state_with_stores(pool, auth.clone())
    } else {
        common::state(pool, auth.clone())
    };
    // The read lanes behind the `read` gate auth's `/v1` router gives them.
    let gate = ApiKeyGate {
        auth: state.auth.clone(),
        scope: ApiKeyScope::Read,
        refuse: refuse_as_problem,
    };
    let reads = v1_reads(state.clone())
        .route_layer(axum::middleware::from_fn_with_state(gate, require_api_key));
    Door {
        app: router(state.clone())
            .nest("/otlp", otlp_router(state.clone()))
            .nest("/v1", v1_router(state.clone()).merge(reads)),
        state,
        auth,
        organization,
        project,
    }
}

/// A model call carrying a message, a tool's result past an attribute's cap,
/// a map no inventory names, and its kept metadata.
fn export(long: &str) -> Vec<u8> {
    export_of(RUN, TRACE, SPAN, &[], long)
}

/// [`export`], as run `run`'s span `span` of trace `trace`, with the text
/// attributes `named`.
fn export_of(run: &str, trace: &str, span: &str, named: &[(&str, &str)], long: &str) -> Vec<u8> {
    let mut attributes = vec![
        json!({ "key": contract::RUN_ID, "value": { "stringValue": run } }),
        json!({ "key": contract::AGENT, "value": { "stringValue": "nightly-report" } }),
        json!({ "key": contract::OPERATION, "value": { "stringValue": "chat" } }),
        json!({ "key": "gen_ai.response.id", "value": { "stringValue": "resp_01" } }),
        json!({ "key": "gen_ai.input.messages", "value": { "stringValue": MESSAGES } }),
        json!({ "key": "gen_ai.tool.call.result", "value": { "stringValue": long } }),
        json!({ "key": "app.ticket", "value": { "kvlistValue": { "values": [
            { "key": "id", "value": { "intValue": "42" } }
        ]}}}),
    ];
    attributes.extend(
        named
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": { "stringValue": value } })),
    );
    json!({
        "resourceSpans": [{
            "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [{
                "traceId": trace,
                "spanId": span,
                "name": "chat claude-sonnet-5-5",
                "startTimeUnixNano": "1760000000000000000",
                "endTimeUnixNano": "1760000001500000000",
                "attributes": attributes
            }]}]
        }]
    })
    .to_string()
    .into_bytes()
}

impl Door {
    async fn send(&self, body: Vec<u8>) -> (String, Value) {
        let (status, mode, answer) = self.post(body).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        (mode, answer)
    }

    async fn post(&self, body: Vec<u8>) -> (StatusCode, String, Value) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/otlp/v1/traces")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::from(body))
                    .expect("a request"),
            )
            .await
            .expect("an answer");
        let status = response.status();
        let mode = response
            .headers()
            .get("telmoni-content-mode")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let answer = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, mode, answer)
    }

    /// Each field the project keeps, by name, its ciphertext, nonce and bytes.
    async fn kept(&self, pool: &PgPool) -> Vec<(String, Vec<u8>, Vec<u8>, i32)> {
        sqlx::query_as(
            "SELECT field, ciphertext, nonce, bytes FROM telemetry.plain_content \
              WHERE project_id = $1 ORDER BY field",
        )
        .bind(&self.project)
        .fetch_all(pool)
        .await
        .expect("read the content as the owner")
    }

    /// The `Viewed` rows of the organization's chain: actor, kind, resource,
    /// project and details.
    async fn viewed(&self, pool: &PgPool) -> Vec<(String, String, String, String, Value)> {
        let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT actor_id, resource_kind, resource_id, in_project, metadata::text \
               FROM audit.events WHERE organization_id = $1 AND action = 'viewed' ORDER BY seq",
        )
        .bind(&self.organization)
        .fetch_all(pool)
        .await
        .expect("read the chain as the owner");
        rows.into_iter()
            .map(|(actor, kind, resource, project, details)| {
                let details = serde_json::from_str(&details).expect("details are JSON");
                (actor, kind, resource, project, details)
            })
            .collect()
    }

    /// One of the console's lanes, as `user` on the project: its status and
    /// answer.
    async fn lane(
        &self,
        method: &str,
        path: &str,
        user: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(format!("/internal/v1/telemetry{path}"))
            .header(SERVICE_SECRET_HEADER, common::SECRET)
            .header(header::AUTHORIZATION, format!("Bearer tok_{user}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.project.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .expect("a request");
        let response = self.app.clone().oneshot(request).await.expect("an answer");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// A `/v1` read with `key`.
    async fn read(&self, path: &str, key: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .uri(format!("/v1{path}"))
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .body(Body::empty())
            .expect("a request");
        let response = self.app.clone().oneshot(request).await.expect("an answer");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// The deletions in the organization's chain: actor, kind, resource and
    /// details.
    async fn deletions(&self, pool: &PgPool) -> Vec<(String, String, String, Value)> {
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT actor_id, resource_kind, resource_id, metadata::text \
               FROM audit.events WHERE organization_id = $1 AND action = 'deleted' ORDER BY seq",
        )
        .bind(&self.organization)
        .fetch_all(pool)
        .await
        .expect("read the chain as the owner");
        rows.into_iter()
            .map(|(actor, kind, resource, details)| {
                (
                    actor,
                    kind,
                    resource,
                    serde_json::from_str(&details).expect("details are JSON"),
                )
            })
            .collect()
    }

    /// The traces whose content the project keeps.
    async fn kept_traces(&self, pool: &PgPool) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT DISTINCT trace_id FROM telemetry.plain_content WHERE project_id = $1 ORDER BY 1",
        )
        .bind(&self.project)
        .fetch_all(pool)
        .await
        .expect("read the content as the owner")
    }

    async fn held(&self, pool: &PgPool, organization: &OrganizationId) -> Option<i64> {
        sqlx::query_scalar("SELECT bytes FROM telemetry.content_held WHERE organization_id = $1")
            .bind(organization)
            .fetch_optional(pool)
            .await
            .expect("read the count as the owner")
    }
}

/// An export of no spans, how an SDK asks the mode before its first content,
/// is answered under the mode as it is now, the same body sent again too.
#[sqlx::test]
async fn an_export_of_no_spans_is_answered_under_the_mode_as_it_is_now(pool: PgPool) {
    let door = door_over(&pool, ContentMode::On, false).await;
    let ask = json!({ "resourceSpans": [] }).to_string().into_bytes();
    let (status, mode, answer) = door.post(ask.clone()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(mode, "on");
    assert!(answer["partialSuccess"].is_null(), "nothing lost: {answer}");
    sqlx::query("UPDATE telemetry.project_settings SET content_mode = $2 WHERE project_id = $1")
        .bind(&door.project)
        .bind(ContentMode::Off)
        .execute(&pool)
        .await
        .expect("switch the mode off");
    let (status, mode, _) = door.post(ask).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mode, "off", "never an earlier answer's mode");
}

/// ⚠ On: every field kept whole and sealed, opened only by the project's own
/// key under its span and name; ClickHouse holds the span's metadata alone;
/// the bytes counted once, a copy of the export answered and counted no more.
#[sqlx::test]
async fn a_project_that_keeps_content_seals_each_field_and_counts_it_once(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    let long = "r".repeat(contract::ATTRIBUTE_VALUE_MAX * 8);
    let (mode, answer) = door.send(export(&long)).await;
    assert_eq!(mode, "on");
    assert!(
        answer["partialSuccess"].is_null(),
        "nothing dropped: {answer}"
    );

    let kept = door.kept(&pool).await;
    let names: Vec<&str> = kept.iter().map(|(field, ..)| field.as_str()).collect();
    assert_eq!(
        names,
        [
            "app.ticket",
            "gen_ai.input.messages",
            "gen_ai.tool.call.result"
        ]
    );
    let key = content::key_for(&door.state, &door.project)
        .await
        .expect("the project's key opens");
    let opened: Vec<String> = kept
        .iter()
        .map(|(field, ciphertext, nonce, _)| {
            assert!(
                !ciphertext
                    .windows(MESSAGES.len())
                    .any(|w| w == MESSAGES.as_bytes()),
                "{field} is sealed"
            );
            content::open_field(
                &door.state.vault,
                &key,
                SealedRef { ciphertext, nonce },
                &content::field_aad(SPAN, field),
            )
            .expect("a field opens under its own span and name")
        })
        .collect();
    assert_eq!(opened, [r#"{"id":42}"#, MESSAGES, long.as_str()]);
    let (_, ciphertext, nonce, _) = &kept[1];
    assert!(
        content::open_field(
            &door.state.vault,
            &key,
            SealedRef { ciphertext, nonce },
            &content::field_aad(SPAN, "gen_ai.output.messages"),
        )
        .is_err(),
        "a field moved to another name does not open"
    );

    // Each counted at its sealed size, compressed first.
    for (field, ciphertext, _, bytes) in &kept {
        assert_eq!(
            usize::try_from(*bytes).ok(),
            Some(ciphertext.len()),
            "{field}"
        );
    }
    assert!(
        kept[2].1.len() < long.len() / 10,
        "the long field compressed"
    );
    let bytes: i64 = kept.iter().map(|(.., bytes)| i64::from(*bytes)).sum();
    assert_eq!(door.held(&pool, &door.organization).await, Some(bytes));

    let attributes: Vec<String> = ch
        .store
        .read(
            &Projects::one(door.project.clone()),
            "SELECT toJSONString(attributes) FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_all()
        .await
        .expect("the span");
    assert_eq!(
        attributes,
        [r#"{"gen_ai.response.id":"resp_01"}"#],
        "no content in ClickHouse"
    );

    door.send(export(&long)).await;
    assert_eq!(
        door.kept(&pool).await.len(),
        3,
        "a copy keeps nothing twice"
    );
    assert_eq!(door.held(&pool, &door.organization).await, Some(bytes));
}

/// ⚠ Every read of content is a `Viewed` row in the organization's chain,
/// naming the traces and never their text; a read that finds none writes none.
#[sqlx::test]
async fn each_read_of_content_is_a_viewed_row_in_the_organizations_chain(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.send(export("result")).await;

    let nothing = content::open(
        &door.state,
        Actor::User("user-1"),
        &door.project,
        &[run(RUN_C)],
    )
    .await
    .expect("a run without content opens to nothing");
    assert!(nothing.is_empty());
    let unsettled = content::open(
        &door.state,
        Actor::User("user-1"),
        &ProjectId::new(),
        &[run(RUN)],
    )
    .await
    .expect("a project that never exported keeps no content, and says so");
    assert!(unsettled.is_empty());
    assert!(
        door.viewed(&pool).await.is_empty(),
        "nothing read, nothing filed"
    );

    let runs = [run(RUN), run(RUN_C), run(RUN)];
    let opened = content::open(&door.state, Actor::User("user-1"), &door.project, &runs)
        .await
        .expect("the trace's content opens");
    assert_eq!(
        opened,
        [SpanContent {
            run_id: run(RUN),
            trace_id: TRACE.to_owned(),
            span_id: SPAN.to_owned(),
            fields: vec![
                ("app.ticket".to_owned(), r#"{"id":42}"#.to_owned()),
                ("gen_ai.input.messages".to_owned(), MESSAGES.to_owned()),
                ("gen_ai.tool.call.result".to_owned(), "result".to_owned()),
            ],
        }]
    );
    let project = door.project.to_string();
    assert_eq!(
        door.viewed(&pool).await,
        [(
            "user-1".to_owned(),
            "project".to_owned(),
            project.clone(),
            project,
            json!({ "trace_ids": [TRACE], "fields": 3 }),
        )]
    );
}

/// A read that cannot be recorded opens nothing: not with no chain to file
/// it in, nor with the chain's lock held past a lane's wait, a conflict.
#[sqlx::test]
async fn a_read_that_cannot_be_recorded_opens_nothing(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    door.send(export("result")).await;
    let runs = [run(RUN)];
    let read = || content::open(&door.state, Actor::User("user-1"), &door.project, &runs);

    assert!(read().await.is_err(), "no chain, no content");

    apply_audit_migrations(&pool).await;
    let mut holder = pool.begin().await.expect("a holder's transaction");
    lock_chain(&mut holder, &door.organization)
        .await
        .expect("take the organization's chain lock");
    let waited = tokio::time::timeout(Duration::from_secs(10), read())
        .await
        .expect("the wait on the chain lock gave up on its own");
    drop(holder);
    assert!(
        waited.as_ref().is_err_and(TelmoniError::is_lock_timeout),
        "{waited:?}"
    );
    assert!(door.viewed(&pool).await.is_empty());
}

/// Off: the content is dropped and named, and no key is made for content the
/// project never keeps.
/// Past its organization's content ceiling, an export's spans keep their
/// metadata alone, each field named `content_full` in the answer, and
/// nothing is sealed or counted.
#[sqlx::test]
async fn content_past_the_organizations_ceiling_is_dropped_and_named(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let mut door = door(&pool, ContentMode::On).await;
    let state = common::state_with_stores_under(
        &pool,
        door.auth.clone(),
        Arc::new(common::PlanMonth {
            spans: 1_000_000,
            grace_days: 0,
            content_bytes: 100,
        }),
    );
    door.app = router(state.clone()).nest("/otlp", otlp_router(state.clone()));
    door.state = state;
    sqlx::query("INSERT INTO telemetry.content_held (organization_id, bytes) VALUES ($1, 100)")
        .bind(&door.organization)
        .execute(&pool)
        .await
        .expect("seed the bytes held");

    let (_, answer) = door.send(export("short")).await;
    let message = answer["partialSuccess"]["errorMessage"]
        .as_str()
        .unwrap_or_default();
    for field in [
        "gen_ai.input.messages",
        "gen_ai.tool.call.result",
        "app.ticket",
    ] {
        assert!(
            message.contains(&format!("span {SPAN} dropped {field}: content_full;")),
            "{field}: {message}"
        );
    }
    assert_eq!(
        answer["partialSuccess"]["rejectedSpans"], 0,
        "the span is kept: {answer}"
    );
    assert!(door.kept(&pool).await.is_empty(), "nothing sealed");
}

/// Past its project's own content cap, set by its organization's owners
/// and admins, an export's spans keep their metadata alone as they do past
/// the plan's, while the organization's ceiling has room to spare.
#[sqlx::test]
async fn content_past_the_projects_own_cap_is_dropped_and_named(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let mut door = door(&pool, ContentMode::On).await;
    let state = common::state_with_stores_under(
        &pool,
        door.auth.clone(),
        Arc::new(common::PlanMonth {
            spans: 1_000_000,
            grace_days: 0,
            content_bytes: 25_000_000,
        }),
    );
    door.app = router(state.clone()).nest("/otlp", otlp_router(state.clone()));
    door.state = state;
    sqlx::query(
        "INSERT INTO telemetry.project_limits (project_id, organization_id, content_bytes)
         VALUES ($1, $2, 0)",
    )
    .bind(&door.project)
    .bind(&door.organization)
    .execute(&pool)
    .await
    .expect("the project's cap");

    let (_, answer) = door.send(export("short")).await;
    let message = answer["partialSuccess"]["errorMessage"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains(&format!(
            "span {SPAN} dropped gen_ai.input.messages: content_full;"
        )),
        "{message}"
    );
    assert_eq!(
        answer["partialSuccess"]["rejectedSpans"], 0,
        "the span is kept: {answer}"
    );
    assert!(door.kept(&pool).await.is_empty(), "nothing sealed");
}

/// Each night an organization past its content ceiling loses its oldest
/// content, a day at a time, until a tenth of the ceiling is free, its bytes
/// taken off the count; what its retention keeps of its spans stays.
#[sqlx::test]
async fn the_nightly_pass_takes_the_oldest_content_past_the_ceiling(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let mut door = door(&pool, ContentMode::On).await;
    let state = common::state_with_stores_under(
        &pool,
        door.auth.clone(),
        Arc::new(common::PlanMonth {
            spans: 1_000_000,
            grace_days: 0,
            content_bytes: 10,
        }),
    );
    door.app = router(state.clone()).nest("/otlp", otlp_router(state.clone()));
    door.state = state;
    door.send(export("short")).await;
    assert!(!door.kept(&pool).await.is_empty(), "kept under the ceiling");

    let purged = telmoni_telemetry::capacity::purge(&door.state, chrono::Utc::now())
        .await
        .expect("the pass runs");
    assert!(purged.content > 0, "{purged:?}");
    assert!(door.kept(&pool).await.is_empty(), "the oldest content went");
    let held: i64 =
        sqlx::query_scalar("SELECT bytes FROM telemetry.content_held WHERE organization_id = $1")
            .bind(&door.organization)
            .fetch_one(&pool)
            .await
            .expect("the count");
    assert_eq!(held, 0, "its bytes came off the count");
    let spans: u64 = ch
        .store
        .read(
            &Projects::one(door.project.clone()),
            "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("the spans are read");
    assert_eq!(spans, 1, "the span within retention stays");
}

/// The month's usage names each project's span units a UTC day, from the hourly totals.
#[sqlx::test]
async fn the_months_usage_counts_each_projects_units_by_day(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let mut door = door(&pool, ContentMode::Off).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let month = common::PlanMonth {
        spans: 1_000_000,
        grace_days: 0,
        content_bytes: 1_000_000,
    };
    let state = common::state_with_stores_noticed(
        &pool,
        door.auth.clone(),
        Arc::new(month),
        common::NoticeLog::new(),
    );
    door.app = router(state.clone()).nest("/otlp", otlp_router(state.clone()));
    door.state = state;
    door.send(export("result")).await;

    let (status, usage) = door.lane("GET", "/usage", "admin-1", None).await;
    assert_eq!(status, StatusCode::OK, "{usage}");
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let days = usage["days"].as_array().expect("days");
    assert_eq!(days.len(), 1, "{usage}");
    assert_eq!(days[0]["day"], today.as_str());
    assert_eq!(days[0]["project_id"], door.project.as_str());
    assert!(
        days[0]["units"].as_u64().is_some_and(|units| units > 0),
        "{usage}"
    );
    assert_eq!(usage["projects"][0]["id"], door.project.as_str());
}

/// The usage pass counts the month's spans as the door did and tells the
/// owners once the content held reaches the plan's ceiling.
#[sqlx::test]
async fn the_usage_pass_counts_the_month_and_tells_of_the_content_held(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let mut door = door(&pool, ContentMode::On).await;
    let notices = common::NoticeLog::new();
    let month = common::PlanMonth {
        spans: 1_000_000,
        grace_days: 0,
        content_bytes: 100,
    };
    let state = common::state_with_stores_noticed(
        &pool,
        door.auth.clone(),
        Arc::new(month),
        notices.clone(),
    );
    door.app = router(state.clone()).nest("/otlp", otlp_router(state.clone()));
    door.state = state;
    door.send(export(&"r".repeat(400))).await;

    telmoni_telemetry::capacity::tell_usage(&door.state)
        .await
        .expect("the pass runs");
    let keys: Vec<String> = notices
        .notices()
        .into_iter()
        .filter_map(|n| n.dedup_key)
        .collect();
    assert!(
        keys.iter()
            .any(|key| key.contains(":content:") && key.ends_with(":100")),
        "{keys:?}"
    );
    let capacity = telmoni_shared::seam::capacity(
        &common::PlanMonth {
            spans: 1_000_000,
            grace_days: 0,
            content_bytes: 100,
        },
        &door.organization,
    )
    .await
    .expect("a month");
    let used =
        telmoni_telemetry::capacity::spans_used(&door.state, &door.organization, &capacity, true)
            .await
            .expect("the month is counted");
    assert!(used >= 1, "the export's span units: {used}");
}

/// A read reaches back no further than its organization's retention, so what
/// a move to a plan holding less keeps for its hold is hidden; a binary that
/// sells nothing reads every day asked.
#[sqlx::test]
async fn a_read_reaches_back_no_further_than_the_plans_retention(pool: PgPool) {
    let door = door_over(&pool, ContentMode::Off, false).await;
    assert_eq!(
        telmoni_telemetry::capacity::visible_days(&door.state, &door.project, 90).await,
        90,
        "no plan holds it"
    );
    let state = common::state_with_entitlements(
        &pool,
        door.auth.clone(),
        Arc::new(common::PlanMonth {
            spans: 1_000_000,
            grace_days: 0,
            content_bytes: 25_000_000,
        }),
    );
    assert_eq!(
        telmoni_telemetry::capacity::visible_days(&state, &door.project, 90).await,
        30
    );
    assert_eq!(
        telmoni_telemetry::capacity::visible_days(&state, &door.project, 7).await,
        7
    );
}

#[sqlx::test]
async fn a_project_left_off_keeps_no_content_and_makes_no_key(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::Off).await;
    let (mode, answer) = door.send(export("long")).await;
    assert_eq!(mode, "off");
    let message = answer["partialSuccess"]["errorMessage"]
        .as_str()
        .unwrap_or_default();
    for field in ["gen_ai.input.messages: content", "app.ticket: unlisted"] {
        assert!(message.contains(field), "{field}: {message}");
    }
    assert!(door.kept(&pool).await.is_empty());
    assert_eq!(door.held(&pool, &door.organization).await, None);
    let keyed: bool = sqlx::query_scalar(
        "SELECT content_key_wrapped_dek IS NOT NULL FROM telemetry.project_settings \
          WHERE project_id = $1",
    )
    .bind(&door.project)
    .fetch_one(&pool)
    .await
    .expect("the settings row");
    assert!(!keyed);
}

/// A retrieval's query and documents are content, dropped and named on an
/// `off` project and sealed on an `on` one; what it asked for and how many
/// came back are metadata, kept in either mode.
#[sqlx::test]
async fn a_retrievals_documents_are_content_and_its_counts_metadata(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let int = |key: &str, value: i64| json!({ "key": key, "value": { "intValue": value } });
    let body = json!({
        "resourceSpans": [{
            "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [{
                "traceId": TRACE,
                "spanId": SPAN,
                "name": "retrieval docs-index",
                "startTimeUnixNano": "1760000000000000000",
                "endTimeUnixNano": "1760000000200000000",
                "attributes": [
                    text(contract::RUN_ID, RUN),
                    text(contract::AGENT, "nightly-report"),
                    text(contract::OPERATION, "retrieval"),
                    text(contract::DATA_SOURCE_ID, "docs-index"),
                    int(contract::RETRIEVAL_TOP_K, 5),
                    int(contract::RETRIEVAL_RETURNED, 3),
                    text("gen_ai.retrieval.query.text", "refund policy"),
                    text("gen_ai.retrieval.documents", r#"[{"id":"a"},{"id":"b"},{"id":"c"}]"#),
                ],
            }]}]
        }]
    })
    .to_string()
    .into_bytes();
    for mode in [ContentMode::Off, ContentMode::On] {
        let door = door(&pool, mode).await;
        let (_, answer) = door.send(body.clone()).await;
        let kept = door.kept(&pool).await;
        let sealed: Vec<&str> = kept.iter().map(|(field, ..)| field.as_str()).collect();
        if mode == ContentMode::Off {
            let message = answer["partialSuccess"]["errorMessage"]
                .as_str()
                .unwrap_or_default();
            for field in [
                "gen_ai.retrieval.documents: content",
                "gen_ai.retrieval.query.text: content",
            ] {
                assert!(message.contains(field), "{field}: {message}");
            }
            assert!(sealed.is_empty(), "{sealed:?}");
        } else {
            assert!(
                answer["partialSuccess"].is_null(),
                "nothing dropped: {answer}"
            );
            assert_eq!(
                sealed,
                ["gen_ai.retrieval.documents", "gen_ai.retrieval.query.text"]
            );
        }
        let rows: Vec<(String, String, String, String)> = ch
            .store
            .read(
                &Projects::one(door.project.clone()),
                "SELECT kind, name, attributes['gen_ai.retrieval.top_k'], \
                 attributes['telmoni.retrieval.returned'] FROM telemetry.spans \
                 WHERE project_id IN {projects:Array(String)}",
            )
            .fetch_all()
            .await
            .expect("the row");
        let row = (
            "retrieval".to_owned(),
            "docs-index".to_owned(),
            "5".to_owned(),
            "3".to_owned(),
        );
        assert_eq!(rows, [row], "{mode}");
    }
}

/// The posture counts what spans arrived with by the inventory's rows, the
/// content an `off` project dropped among them, to any role on it.
#[sqlx::test]
async fn the_posture_counts_what_arrived_by_inventory_row_dropped_content_too(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::Off).await;
    door.auth.seat(
        "member-1",
        &door.organization,
        &door.project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    door.send(export_of(
        RUN,
        TRACE,
        SPAN,
        &[("telmoni.tag.team", "ops")],
        "long",
    ))
    .await;
    door.send(export_of(RUN_B, TRACE_B, SPAN_B, &[], "other"))
        .await;
    common::eventually("both traces are read", || async {
        door.lane("GET", "/privacy", "member-1", None).await.1["spans"] == 2
    })
    .await;

    let (status, posture) = door.lane("GET", "/privacy", "member-1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(posture["days"], 30);
    let since = chrono::DateTime::parse_from_rfc3339(posture["since"].as_str().expect("since"))
        .expect("an instant");
    assert_eq!(
        (Utc::now() - since.to_utc()).num_days(),
        30,
        "the window starts thirty days back"
    );
    assert_eq!(
        posture["retention"]["span_days"],
        retention::SPAN_RETENTION_DAYS
    );
    assert_eq!(
        posture["retention"]["rollup_months"],
        retention::ROLLUP_RETENTION_MONTHS
    );
    let sent: Vec<(&str, u64)> = posture["sent"]
        .as_array()
        .expect("sent")
        .iter()
        .map(|row| {
            (
                row["attribute"].as_str().expect("an attribute"),
                row["spans"].as_u64().expect("a count"),
            )
        })
        .collect();
    for (attribute, spans) in [
        (contract::RUN_ID, 2),
        ("gen_ai.input.messages", 2),
        ("gen_ai.tool.call.result", 2),
        (contract::OTHER_TEXT, 2),
        (contract::TAG_FAMILY, 1),
    ] {
        assert!(sent.contains(&(attribute, spans)), "{attribute}: {sent:?}");
    }
    assert!(door.kept(&pool).await.is_empty(), "the content was dropped");
    assert_eq!(sent[0].1, 2, "most spans first");

    let (refused, _) = door.lane("GET", "/privacy", "stranger", None).await;
    assert_eq!(refused, StatusCode::FORBIDDEN);
}

/// A transfer moves the project's bytes to the organization that now holds
/// it; its purge then deletes its content and takes the bytes off that one.
#[sqlx::test]
async fn a_transfer_moves_the_bytes_and_a_purge_takes_them_off(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    door.send(export("result")).await;
    let bytes = door.held(&pool, &door.organization).await.expect("counted");
    assert!(bytes > 0);

    let taker = OrganizationId::new();
    let seam = TelemetrySeam(door.state.clone());
    seam.move_project(&door.project, &taker)
        .await
        .expect("the move");
    assert_eq!(door.held(&pool, &door.organization).await, Some(0));
    assert_eq!(door.held(&pool, &taker).await, Some(bytes));

    seam.purge_project(&door.organization, &door.project)
        .await
        .expect("the purge");
    assert!(door.kept(&pool).await.is_empty());
    assert_eq!(door.held(&pool, &taker).await, Some(0));
}

/// A project's content key is wrapped under `CONTENT_KEK`, never the hashing
/// key's `TELEMETRY_KEK`, and a purge deletes it with the content, so no
/// copy of either outlives the other.
#[sqlx::test]
async fn the_content_key_is_wrapped_apart_and_a_purge_deletes_it_with_the_content(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    door.send(export("result")).await;
    let (wrapped, version): (Vec<u8>, i16) = sqlx::query_as(
        "SELECT content_key_wrapped_dek, content_key_version \
           FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(&door.project)
    .fetch_one(&pool)
    .await
    .expect("the project's content key");
    let aad = format!("{}/content", door.project);
    let http = &door.state.http;
    door.state
        .content_kek
        .unwrap_dek(http, &wrapped, aad.as_bytes(), version)
        .await
        .expect("CONTENT_KEK opens it");
    assert!(
        door.state
            .kek
            .unwrap_dek(http, &wrapped, aad.as_bytes(), version)
            .await
            .is_err(),
        "TELEMETRY_KEK must not open a content key"
    );

    TelemetrySeam(door.state.clone())
        .purge_project(&door.organization, &door.project)
        .await
        .expect("the purge");
    let keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.project_settings \
          WHERE project_id = $1 AND content_key_wrapped_dek IS NOT NULL",
    )
    .bind(&door.project)
    .fetch_one(&pool)
    .await
    .expect("count the keys left");
    assert_eq!(keys, 0, "the key went");
    assert!(door.kept(&pool).await.is_empty(), "and the content with it");
}

/// ⚠ A row sealed under a key other than the project's, one a switch deleted
/// and a purge left, is read as none, never opened against the wrong key:
/// back under the project's key, it opens again.
#[sqlx::test]
async fn a_row_sealed_under_another_key_is_read_as_none(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "owner-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    door.send(export("result")).await;
    let runs = [run(RUN)];
    let open = || content::open(&door.state, Actor::User("owner-1"), &door.project, &runs);
    assert!(!open().await.expect("the content opens").is_empty());
    sqlx::query(
        "UPDATE telemetry.plain_content SET key_id = gen_random_uuid() WHERE project_id = $1",
    )
    .bind(&door.project)
    .execute(&pool)
    .await
    .expect("rows under another key");
    assert!(
        open().await.expect("a read answers").is_empty(),
        "nothing under another key opens"
    );
    sqlx::query(
        "UPDATE telemetry.plain_content
            SET key_id = (SELECT content_key_id FROM telemetry.project_settings
                           WHERE project_id = $1)
          WHERE project_id = $1",
    )
    .bind(&door.project)
    .execute(&pool)
    .await
    .expect("rows under the project's key");
    assert!(!open().await.expect("the content opens again").is_empty());
}

/// ⚠ The switch's purge deletes by the key it deleted: rows sealed under the
/// project's key since, `on` again, are left alone.
#[sqlx::test]
async fn the_purge_of_a_deleted_key_leaves_content_under_a_later_key(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    door.send(export("result")).await;
    // The same fields again, as a deleted key would have left them.
    let old = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO telemetry.plain_content
             (project_id, run_id, trace_id, span_id, field, received_at, ciphertext, nonce,
              bytes, key_id)
         SELECT project_id, run_id, trace_id, span_id || 'old', field, received_at, ciphertext,
                nonce, bytes, $2
           FROM telemetry.plain_content WHERE project_id = $1",
    )
    .bind(&door.project)
    .bind(old)
    .execute(&pool)
    .await
    .expect("rows under a deleted key");
    // A file under each key, their bytes counted as the copies' are.
    sqlx::query(
        "INSERT INTO telemetry.media (project_id, sha256, mime_type, bytes, ciphertext, nonce, key_id)
         VALUES ($1, repeat('a', 64), 'image/png', 10, '\\x00', '\\x00', $2),
                ($1, repeat('b', 64), 'image/png', 10, '\\x00', '\\x00',
                 (SELECT content_key_id FROM telemetry.project_settings WHERE project_id = $1))",
    )
    .bind(&door.project)
    .bind(old)
    .execute(&pool)
    .await
    .expect("a file under each key");
    sqlx::query(
        "UPDATE telemetry.content_held SET bytes = bytes + 20
            + (SELECT sum(bytes) FROM telemetry.plain_content WHERE project_id = $1 AND key_id = $2)
          WHERE organization_id = $3",
    )
    .bind(&door.project)
    .bind(old)
    .bind(&door.organization)
    .execute(&pool)
    .await
    .expect("the copies and the files counted");
    let before = door.kept(&pool).await.len();
    let mut conn = pool.acquire().await.expect("a connection");
    let gone = telmoni_telemetry::db::delete_key_content(
        &mut conn,
        &door.project,
        old,
        Utc::now(),
        i64::MAX,
    )
    .await
    .expect("the purge");
    assert_eq!(
        usize::try_from(gone).ok(),
        Some(before / 2),
        "the deleted key's rows alone"
    );
    let files: Vec<String> =
        sqlx::query_scalar("SELECT sha256 FROM telemetry.media WHERE project_id = $1")
            .bind(&door.project)
            .fetch_all(&pool)
            .await
            .expect("the files left");
    assert_eq!(
        files,
        ["b".repeat(64)],
        "the deleted key's file went, the project's stays"
    );
    let left_bytes: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(bytes), 0)::bigint + 10 FROM telemetry.plain_content WHERE project_id = $1",
    )
    .bind(&door.project)
    .fetch_one(&pool)
    .await
    .expect("the bytes left");
    assert_eq!(
        door.held(&pool, &door.organization).await,
        Some(left_bytes),
        "the count is what is left"
    );
    let left: Vec<bool> = sqlx::query_scalar(
        "SELECT key_id = (SELECT content_key_id FROM telemetry.project_settings
                           WHERE project_id = $1)
           FROM telemetry.plain_content WHERE project_id = $1",
    )
    .bind(&door.project)
    .fetch_all(&pool)
    .await
    .expect("the rows left");
    assert!(
        left.len() == before / 2 && left.iter().all(|under| *under),
        "the project's key's rows stand"
    );
}

/// ⚠ Content an export carried while its project was `on` is refused once
/// the project is `off`, however the switch and the keep interleave: no key
/// is minted for an `off` project, and the exporter is told to try again.
#[sqlx::test]
async fn content_kept_after_the_switch_to_off_is_refused(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "owner-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    door.send(export("result")).await;
    let response = door
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/internal/v1/telemetry/content-mode")
                .header(SERVICE_SECRET_HEADER, common::SECRET)
                .header(header::AUTHORIZATION, "Bearer tok_owner-1")
                .header("x-organization-id", door.organization.as_str())
                .header("x-project-id", door.project.as_str())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "content_mode": "off", "delete_content": true }).to_string(),
                ))
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(response.status(), StatusCode::OK);

    // A keep that read `on` before the switch and runs after it.
    let late = content::keep(
        &door.state,
        &door.project,
        Utc::now(),
        vec![SpanContent {
            run_id: run(RUN_B),
            trace_id: TRACE_B.to_owned(),
            span_id: SPAN_B.to_owned(),
            fields: vec![("app.note".to_owned(), "kept late".to_owned())],
        }],
    )
    .await;
    assert!(late.is_err(), "refused, for the exporter to retry");
    let keyed: bool = sqlx::query_scalar(
        "SELECT content_key_wrapped_dek IS NOT NULL FROM telemetry.project_settings \
          WHERE project_id = $1",
    )
    .bind(&door.project)
    .fetch_one(&pool)
    .await
    .expect("the settings row");
    assert!(!keyed, "no key minted for an off project");
    let late_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.plain_content WHERE project_id = $1 AND span_id = $2",
    )
    .bind(&door.project)
    .bind(SPAN_B)
    .fetch_one(&pool)
    .await
    .expect("count the late rows");
    assert_eq!(late_rows, 0, "nothing kept on an off project");
}

/// ⚠ Back to `off` with its content deleted, nothing the project kept opens
/// from the switch's commit, though its rows wait for the purge after; the
/// purge takes them, and the owner is mailed the receipt.
#[sqlx::test]
async fn going_off_with_content_deleted_leaves_none_that_opens(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "owner-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    door.send(export("result")).await;
    let runs = [run(RUN)];
    let open = || content::open(&door.state, Actor::User("owner-1"), &door.project, &runs);
    assert!(!open().await.expect("the content opens").is_empty());

    let response = door
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/internal/v1/telemetry/content-mode")
                .header(SERVICE_SECRET_HEADER, common::SECRET)
                .header(header::AUTHORIZATION, "Bearer tok_owner-1")
                .header("x-organization-id", door.organization.as_str())
                .header("x-project-id", door.project.as_str())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "content_mode": "off", "delete_content": true }).to_string(),
                ))
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(response.status(), StatusCode::OK);

    let keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.project_settings \
          WHERE project_id = $1 AND content_key_wrapped_dek IS NOT NULL",
    )
    .bind(&door.project)
    .fetch_one(&pool)
    .await
    .expect("count the keys left");
    assert_eq!(keys, 0, "the key went in the switch's commit");
    assert!(
        open().await.expect("a read answers").is_empty(),
        "nothing opens"
    );
    common::eventually("the purge takes the rows", || async {
        door.kept(&pool).await.is_empty()
    })
    .await;
    common::eventually("the receipt is mailed", || async {
        door.auth.receipts() == [("owner-1".to_owned(), format!("content {}", door.project))]
    })
    .await;
    let held: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(bytes), 0)::bigint FROM telemetry.content_held WHERE organization_id = $1",
    )
    .bind(&door.organization)
    .fetch_one(&pool)
    .await
    .expect("the organization's count");
    assert_eq!(held, 0, "its bytes came off the count");
}

/// What every content field below carries, so a copy of it anywhere is found.
const SENTINEL: &str = "s3nt1nel-written-by-a-person";

/// Every content field the inventory names, and a text it names nowhere, each
/// carrying [`SENTINEL`], but the two [`export_of`] sets itself: its tool's
/// result is given the sentinel, its messages are its own.
fn every_content_field() -> Vec<(String, String)> {
    contract::CONTENT
        .iter()
        .copied()
        .chain(["app.note"])
        .filter(|key| !matches!(*key, "gen_ai.input.messages" | "gen_ai.tool.call.result"))
        .map(|key| (key.to_owned(), format!("{SENTINEL} as {key}")))
        .collect()
}

/// ⚠ Minimization: what a person or a model wrote reaches no log line at any
/// level, no answer of ingest's nor an error's detail, no ClickHouse column
/// and nothing the console agent reads — the audit chain it indexes; its role
/// reaches no telemetry table (`crates/shared/tests/db_roles.rs`) — whether
/// its project keeps it sealed (`on`) or drops it (`off`).
#[sqlx::test]
async fn no_content_reaches_a_log_an_answer_clickhouse_or_what_the_agent_reads(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let (_logging, logs) = captured_logs();
    let fields = every_content_field();
    let named: Vec<(&str, &str)> = fields
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let long = format!("{SENTINEL} as the tool's result");
    let kept = door(&pool, ContentMode::On).await;
    let dropped = door(&pool, ContentMode::Off).await;
    kept.auth.seat(
        "admin-1",
        &kept.organization,
        &kept.project,
        OrganizationRole::Owner,
        None,
    );
    let mut answers = Vec::new();
    for door in [&kept, &dropped] {
        let (_, answer) = door.send(export_of(RUN, TRACE, SPAN, &named, &long)).await;
        answers.push(answer.to_string());
    }
    common::eventually("the kept trace is read", || found(&kept, RUN)).await;
    assert!(
        !kept.kept(&pool).await.is_empty(),
        "the on project kept it, sealed"
    );

    // Every column of every row as text, searched where it is kept: a row
    // holds bytes, the hashed user id among them, that are no UTF-8 here.
    let (mut rows, mut carrying) = (0_u64, 0_u64);
    for door in [&kept, &dropped] {
        let only = Projects::one(door.project.clone());
        for every_column in [
            "SELECT count(), countIf(position(formatRow('JSONEachRow', *), {sentinel:String}) > 0) \
               FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
            "SELECT count(), countIf(position(formatRow('JSONEachRow', *), {sentinel:String}) > 0) \
               FROM telemetry.spans_hourly WHERE project_id IN {projects:Array(String)}",
        ] {
            let (all, with): (u64, u64) = ch
                .store
                .read(&only, every_column)
                .param("sentinel", SENTINEL)
                .fetch_one()
                .await
                .expect("search every column of the project's rows");
            rows += all;
            carrying += with;
        }
    }
    assert!(rows >= 4, "both projects' spans and totals: {rows}");
    assert_eq!(carrying, 0, "content in ClickHouse");

    content::open(
        &kept.state,
        Actor::User("admin-1"),
        &kept.project,
        &[run(RUN)],
    )
    .await
    .expect("the content opens, its read filed");
    let refused = door_over(&pool, ContentMode::On, false).await;
    let (status, _, answer) = refused
        .post(export_of(RUN_B, TRACE_B, SPAN_B, &named, &long))
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
    answers.push(answer.to_string());
    let (status, answer) = kept
        .lane("DELETE", &format!("/traces/{RUN}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answers.push(answer.to_string());

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_schema || '.' || table_name FROM information_schema.tables \
          WHERE table_schema IN ('telemetry', 'audit') AND table_type = 'BASE TABLE'",
    )
    .fetch_all(&pool)
    .await
    .expect("list the tables");
    assert!(tables.iter().any(|table| table == "audit.events"));
    // The search finds what is there: the project's id, in its settings row.
    let mut found_the_project = false;
    for table in tables {
        let text: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT coalesce(string_agg(t::text, ' '), '') FROM {table} t"
        )))
        .fetch_one(&pool)
        .await
        .expect("read the table whole");
        assert!(!text.contains(SENTINEL), "content in {table} in plain text");
        found_the_project |= text.contains(kept.project.as_str());
    }
    assert!(found_the_project, "the search reads the tables' text");
    assert!(
        !answers.iter().any(|answer| answer.contains(SENTINEL)),
        "{answers:?}"
    );
    assert!(!logs.text().contains(SENTINEL), "content in a log line");
}

/// ⚠ Records that hold nothing personal: a tag's value and an end user's id
/// reach no log line at any level, no error's detail and no audit row's
/// metadata, through ingest, the lists and downloads filtered by both, a
/// filter refused, and the end user's erasure.
#[sqlx::test]
async fn no_tag_value_or_end_user_id_reaches_a_log_an_error_or_the_audit_chain(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let (_logging, logs) = captured_logs();
    const TAG: &str = "t4g-value-a-customer-chose";
    const USER: &str = "end-user-4u1-who-the-agent-served";
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let named = [("telmoni.tag.app", TAG), ("user.id", USER)];
    door.send(export_of(RUN, TRACE, SPAN, &named, "result"))
        .await;
    common::eventually("the trace is read", || found(&door, RUN)).await;

    let tag = format!("app:{TAG}");
    let mut errors = Vec::new();
    for (path, expected) in [
        (format!("/traces?tag={tag}"), StatusCode::OK),
        (format!("/traces?user={USER}"), StatusCode::OK),
        (
            format!("/traces/export?tag={tag}&format=json"),
            StatusCode::OK,
        ),
        // Refused: neither the value nor the id may come back in the detail.
        (format!("/traces?tag={TAG}"), StatusCode::BAD_REQUEST),
        (
            format!("/traces?user={}", USER.repeat(8)),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, answer) = door.lane("GET", &path, "admin-1", None).await;
        assert_eq!(status, expected, "{path}: {answer}");
        if !status.is_success() {
            errors.push(answer.to_string());
        }
    }
    let (status, answer) = door
        .lane(
            "POST",
            "/users/erase",
            "admin-1",
            Some(json!({ "user_id": USER })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    let metadata: Vec<String> =
        sqlx::query_scalar("SELECT coalesce(metadata::text, '') FROM audit.events")
            .fetch_all(&pool)
            .await
            .expect("read the chain's metadata");
    assert!(!metadata.is_empty(), "the erasure was filed");
    for (what, text) in [
        ("an error's detail", errors.join(" ")),
        ("an audit row's metadata", metadata.join(" ")),
        ("a log line", logs.text()),
    ] {
        assert!(!text.contains(TAG), "a tag's value in {what}: {text}");
        assert!(!text.contains(USER), "an end user's id in {what}: {text}");
    }
    assert_eq!(errors.len(), 2, "both refusals were read");
}

/// ⚠ A field held small: compressed before it is sealed.
#[sqlx::test]
async fn a_kept_field_is_compressed_before_it_is_sealed(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    // Ten kilobytes of prose: two hundred words, drawn as a writer repeats them.
    let vocabulary: Vec<String> = (0..200)
        .map(|n| format!("w{}", n * 7919 % 100_003))
        .collect();
    let (mut text, mut seed) = (String::new(), 42_u64);
    while text.len() < 10 * 1024 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let at = usize::try_from(seed >> 33).unwrap_or_default() % vocabulary.len();
        text.push_str(&vocabulary[at]);
        text.push_str(if seed % 13 == 0 { ". " } else { " " });
    }
    door.send(export_of(
        RUN,
        TRACE,
        SPAN,
        &[("app.note", text.as_str())],
        "result",
    ))
    .await;
    let sealed: i32 = sqlx::query_scalar(
        "SELECT bytes FROM telemetry.plain_content WHERE project_id = $1 AND field = 'app.note'",
    )
    .bind(&door.project)
    .fetch_one(&pool)
    .await
    .expect("the field, kept");
    let written = i64::try_from(text.len()).expect("small");
    assert!(
        i64::from(sealed) < written / 2,
        "compressed: {sealed} for {written}"
    );
}

/// ⚠ A file a message carried, kept once by the hash of its bytes: sealed
/// for an `on` project and nothing for an `off` one, its bytes counted; named
/// by the runs whose content cites it; read with its read filed, served so a
/// browser runs nothing it holds; and gone with the one trace that named it.
#[sqlx::test]
async fn a_file_is_kept_once_named_by_its_runs_read_filed_and_erased_with_them(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let png: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";
    let hash = telmoni_shared::digest::sha256_hex(png);
    let upload = |door: &Door, path: String, body: &'static [u8]| {
        door.app.clone().oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                .header(header::CONTENT_TYPE, "image/png")
                .body(Body::from(body))
                .expect("a request"),
        )
    };
    let media = |pool: &PgPool, project: &ProjectId| {
        let (pool, project) = (pool.clone(), project.clone());
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM telemetry.media WHERE project_id = $1",
            )
            .bind(&project)
            .fetch_one(&pool)
            .await
            .expect("count the files")
        }
    };

    for _ in 0..2 {
        let answer = upload(&door, format!("/v1/media/{hash}"), png)
            .await
            .expect("an answer");
        let status = answer.status();
        let text = answer
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&text)
        );
    }
    assert_eq!(media(&pool, &door.project).await, 1, "kept once");
    let held = door.held(&pool, &door.organization).await.expect("counted");
    assert_eq!(held, i64::try_from(png.len()).expect("small"));
    let wrong = upload(&door, format!("/v1/media/{}", "0".repeat(64)), png)
        .await
        .expect("an answer");
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    let refused = |method: &str, body: Vec<u8>| {
        door.app.clone().oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/v1/media/{hash}"))
                .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                .body(Body::from(body))
                .expect("a request"),
        )
    };
    for (answer, status) in [
        (
            refused("PUT", vec![0; content::MEDIA_MAX + 1]).await,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            refused("GET", Vec::new()).await,
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let answer = answer.expect("an answer");
        assert_eq!(answer.status(), status);
        assert_eq!(
            answer
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(&b"application/problem+json"[..]),
            "{status} is a problem document, as every /v1 refusal is"
        );
    }
    let off = door_over(&pool, ContentMode::Off, true).await;
    let answer = upload(&off, format!("/v1/media/{hash}"), png)
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        media(&pool, &off.project).await,
        0,
        "an off project keeps nothing"
    );

    let message = format!(
        r#"[{{"role":"user","parts":[{{"type":"file","modality":"image","mime_type":"image/png","file_id":"telmoni:{hash}"}}]}}]"#
    );
    door.send(export_of(
        RUN,
        TRACE,
        SPAN,
        &[("gen_ai.input.messages", &message)],
        "result",
    ))
    .await;
    let names: Vec<(String, uuid::Uuid)> =
        sqlx::query_as("SELECT sha256, run_id FROM telemetry.media_names WHERE project_id = $1")
            .bind(&door.project)
            .fetch_all(&pool)
            .await
            .expect("read the names");
    assert_eq!(names, [(hash.clone(), run(RUN))]);

    let read = door
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/internal/v1/telemetry/media/{hash}"))
                .header(SERVICE_SECRET_HEADER, common::SECRET)
                .header(header::AUTHORIZATION, "Bearer tok_admin-1")
                .header("x-organization-id", door.organization.as_str())
                .header("x-project-id", door.project.as_str())
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(read.status(), StatusCode::OK);
    let headers = read.headers().clone();
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(headers[header::CONTENT_SECURITY_POLICY], "sandbox");
    let body = read.into_body().collect().await.expect("a body").to_bytes();
    assert_eq!(&body[..], png);
    assert!(
        door.viewed(&pool)
            .await
            .iter()
            .any(|(.., details)| details["media"] == hash.as_str()),
        "the read is filed"
    );

    common::eventually("the trace is read", || found(&door, RUN)).await;
    let (status, answer) = door
        .lane("DELETE", &format!("/traces/{RUN}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        media(&pool, &door.project).await,
        0,
        "gone with the one trace that named it"
    );
    assert_eq!(door.held(&pool, &door.organization).await, Some(0));
}

/// A run's turn of a conversation as an SDK sends it: the run's own span
/// with the person's message and the agent's answer, and a model call under
/// it with the messages it was sent, both in session `session`.
fn turn_of(session: &str) -> Vec<u8> {
    let span = |id: &str, parent: &str, operation: &str, fields: &[(&str, &str)]| {
        let mut attributes = vec![
            json!({ "key": contract::RUN_ID, "value": { "stringValue": RUN } }),
            json!({ "key": contract::AGENT, "value": { "stringValue": "nightly-report" } }),
            json!({ "key": contract::OPERATION, "value": { "stringValue": operation } }),
            json!({ "key": contract::SESSION_ID, "value": { "stringValue": session } }),
        ];
        attributes.extend(
            fields
                .iter()
                .map(|(key, value)| json!({ "key": key, "value": { "stringValue": value } })),
        );
        json!({
            "traceId": TRACE,
            "spanId": id,
            "parentSpanId": parent,
            "name": operation,
            "startTimeUnixNano": "1760000000000000000",
            "endTimeUnixNano": "1760000001500000000",
            "attributes": attributes
        })
    };
    json!({
        "resourceSpans": [{
            "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [
                span(SPAN, "", "invoke_agent", &[
                    ("gen_ai.input.messages", MESSAGES),
                    ("gen_ai.output.messages", ANSWER),
                ]),
                span(SPAN_B, SPAN, "chat", &[("gen_ai.input.messages", CONTEXT)]),
            ]}]
        }]
    })
    .to_string()
    .into_bytes()
}

const ANSWER: &str = r#"[{"role":"assistant","parts":[{"type":"text","content":"hello"}]}]"#;
const CONTEXT: &str = r#"[{"role":"system","parts":[{"type":"text","content":"be brief"}]}]"#;

/// The trace's page carries each span's content and files the read, and the
/// list's download carries it too, one read; a project that keeps none
/// answers an empty list and files nothing.
#[sqlx::test]
async fn the_trace_page_and_the_download_carry_content_each_read_filed(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let kept = door(&pool, ContentMode::On).await;
    kept.auth.seat(
        "admin-1",
        &kept.organization,
        &kept.project,
        OrganizationRole::Owner,
        None,
    );
    kept.send(turn_of("conv-1")).await;
    common::eventually("the trace is read", || found(&kept, RUN)).await;

    let (status, plain) = kept
        .lane("GET", &format!("/traces/{RUN}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{plain}");
    assert_eq!(
        plain["content"],
        Value::Null,
        "content opened only where asked"
    );
    assert!(
        kept.viewed(&pool).await.is_empty(),
        "nothing opened, nothing filed"
    );

    let (status, trace) = kept
        .lane(
            "GET",
            &format!("/traces/{RUN}?content=true"),
            "admin-1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{trace}");
    assert_eq!(
        trace["content"],
        json!([
            { "run_id": RUN, "span_id": SPAN, "fields": {
                "gen_ai.input.messages": MESSAGES,
                "gen_ai.output.messages": ANSWER,
            }},
            { "run_id": RUN, "span_id": SPAN_B, "fields": {
                "gen_ai.input.messages": CONTEXT,
            }},
        ])
    );
    assert_eq!(kept.viewed(&pool).await.len(), 1, "the trace's read, filed");

    let (status, file) = kept
        .lane("GET", "/traces/export?format=json", "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file[0]["run_id"], RUN);
    assert_eq!(
        file[0]["content"], trace["content"],
        "the page's content, in the file"
    );
    assert_eq!(kept.viewed(&pool).await.len(), 2, "the download, one read");

    let dropped = door(&pool, ContentMode::Off).await;
    dropped.auth.seat(
        "admin-1",
        &dropped.organization,
        &dropped.project,
        OrganizationRole::Owner,
        None,
    );
    dropped.send(turn_of("conv-2")).await;
    common::eventually("the other trace is read", || found(&dropped, RUN)).await;
    let (status, trace) = dropped
        .lane(
            "GET",
            &format!("/traces/{RUN}?content=true"),
            "admin-1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{trace}");
    assert_eq!(trace["content"], json!([]), "none kept, none shown");
    assert!(
        dropped.viewed(&pool).await.is_empty(),
        "nothing read, nothing filed"
    );
}

/// Another exporter's content waits sealed, never as text, and is kept under
/// its run once its trace lands, readable as any run's.
#[sqlx::test]
async fn another_exporters_content_waits_sealed_and_lands_with_its_run(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    let attribute =
        |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let body = json!({ "resourceSpans": [{
        "resource": { "attributes": [attribute("service.name", "checkout")] },
        "scopeSpans": [{ "scope": { "name": "openllmetry" }, "spans": [{
            "traceId": TRACE, "spanId": SPAN, "name": "chat gpt-5",
            "startTimeUnixNano": "1760000000000000000",
            "endTimeUnixNano": "1760000001000000000",
            "attributes": [
                attribute(contract::OPERATION, "chat"),
                attribute("gen_ai.input.messages", MESSAGES),
            ],
        }]}],
    }]});
    let (mode, _) = door.send(body.to_string().into_bytes()).await;
    assert_eq!(mode, "on");
    let waiting: Vec<(Vec<String>, Vec<Vec<u8>>)> =
        sqlx::query_as("SELECT fields, ciphertexts FROM telemetry.held_spans")
            .fetch_all(&pool)
            .await
            .expect("held");
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].0, ["gen_ai.input.messages"]);
    assert!(
        !waiting[0].1[0].windows(2).any(|w| w == b"hi"),
        "held as ciphertext, never as its text"
    );
    assert!(door.kept(&pool).await.is_empty(), "kept only once it lands");

    sqlx::query("UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("quiet");
    telmoni_telemetry::foreign::land_quiet(&door.state)
        .await
        .expect("a pass");
    let kept = door.kept(&pool).await;
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].0, "gen_ai.input.messages");
    let run = telmoni_telemetry::foreign::run_of_trace(&door.project, TRACE);
    let opened = content::open(&door.state, Actor::Service("test"), &door.project, &[run])
        .await
        .expect("opened");
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened[0].fields,
        [("gen_ai.input.messages".to_owned(), MESSAGES.to_owned())]
    );
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.held_spans WHERE cardinality(ciphertexts) > 0",
    )
    .fetch_one(&pool)
    .await
    .expect("held");
    assert_eq!(left, 0, "landed, it keeps no content of its own");
}

/// Content switched off while another exporter's spans wait: they land with
/// their metadata and none of it, rather than wait on a key for good.
#[sqlx::test]
async fn held_content_switched_off_lands_without_it(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    let attribute =
        |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let body = json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": "x" }, "spans": [{
        "traceId": TRACE, "spanId": SPAN, "name": "chat gpt-5",
        "startTimeUnixNano": "1760000000000000000",
        "endTimeUnixNano": "1760000001000000000",
        "attributes": [
            attribute(contract::OPERATION, "chat"),
            attribute("gen_ai.input.messages", MESSAGES),
        ],
    }]}]}]});
    door.send(body.to_string().into_bytes()).await;
    for sql in [
        "UPDATE telemetry.project_settings SET content_mode = 'off'",
        "UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes'",
    ] {
        sqlx::query(sql)
            .execute(&pool)
            .await
            .expect("off, and quiet");
    }
    telmoni_telemetry::foreign::land_quiet(&door.state)
        .await
        .expect("a pass");
    assert!(door.kept(&pool).await.is_empty(), "no content kept");
    common::eventually("the span lands", || async {
        ch.store
            .read(
                &Projects::one(door.project.clone()),
                "SELECT count() FROM telemetry.spans WHERE project_id IN {projects:Array(String)}",
            )
            .fetch_one::<u64>()
            .await
            .is_ok_and(|spans| spans == 2)
    })
    .await;
}

/// An end user's erasure takes every held span of their trace, so its children never land later.
#[sqlx::test]
async fn an_end_users_held_trace_goes_whole_with_them(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let attribute =
        |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let body = json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": "x" }, "spans": [
        {
            "traceId": TRACE, "spanId": SPAN, "name": "planner",
            "startTimeUnixNano": "1760000000000000000",
            "endTimeUnixNano": "1760000001000000000",
            "attributes": [
                attribute(contract::OPERATION, "invoke_agent"),
                attribute(contract::USER_ID, "acme-42"),
            ],
        },
        {
            "traceId": TRACE, "spanId": SPAN_B, "parentSpanId": SPAN, "name": "chat gpt-5",
            "startTimeUnixNano": "1760000000000000000",
            "endTimeUnixNano": "1760000001000000000",
            "attributes": [
                attribute(contract::OPERATION, "chat"),
                attribute("gen_ai.input.messages", MESSAGES),
            ],
        },
    ]}]}]});
    door.send(body.to_string().into_bytes()).await;
    let (status, answer) = door
        .lane(
            "POST",
            "/users/erase",
            "admin-1",
            Some(json!({ "user_id": "acme-42" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    common::eventually("the held trace goes whole", || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM telemetry.held_spans")
            .fetch_one(&pool)
            .await
            .is_ok_and(|held| held == 0)
    })
    .await;
}

/// The hourly totals price their calls as the spans do — as reported, else
/// at the project's own price — and count runs and failed runs.
#[sqlx::test]
async fn the_hourly_totals_price_their_calls_and_count_runs(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door_over(&pool, ContentMode::Off, true).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    sqlx::query(
        "INSERT INTO telemetry.project_prices
             (project_id, position, provider, models, input, output, cache_read, cache_write)
         VALUES ($1, 0, 'self', ARRAY['ours-1'], 1e-6, 0, 0, 0)",
    )
    .bind(&door.project)
    .execute(&pool)
    .await
    .expect("our price");
    let at = Utc::now() - chrono::Duration::seconds(30);
    let reported = telmoni_telemetry::store::SpanRow {
        provider: "openai".into(),
        model: "gpt-5".into(),
        cost: Some(1.5),
        ..common::received(&door.project, SpanKind::ModelCall, "chat", 500, at)
    };
    let priced = telmoni_telemetry::store::SpanRow {
        provider: "self".into(),
        model: "ours-1".into(),
        input_tokens: 2_000_000,
        ..common::received(&door.project, SpanKind::ModelCall, "chat", 500, at)
    };
    let failed = telmoni_telemetry::store::SpanRow {
        status: telmoni_shared::SpanStatus::Error,
        ..common::received(&door.project, SpanKind::Run, "nightly-report", 900, at)
    };
    let rows = [
        reported,
        priced.clone(),
        priced,
        common::received(&door.project, SpanKind::Run, "nightly-report", 900, at),
        failed,
    ];
    ch.store.insert_spans(&rows).await.expect("the spans");

    let (status, totals) = door
        .lane("GET", "/rollups?days=1&group=model", "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{totals}");
    assert_eq!(totals["step"], "hour", "a day's window is read by the hour");
    let points = totals["points"].as_array().expect("points");
    let order: Vec<(String, String)> = points
        .iter()
        .map(|p| (p["at"].to_string(), p["value"].to_string()))
        .collect();
    assert!(
        order.is_sorted(),
        "oldest first, each hour's values in order: {order:?}"
    );
    let spend = |model: &str| {
        points
            .iter()
            .filter(|p| p["value"] == model)
            .map(|p| p["spend"].as_f64().unwrap_or(0.0))
            .sum::<f64>()
    };
    assert!((spend("gpt-5") - 1.5).abs() < 1e-9, "{totals}");
    assert!(
        (spend("ours-1") - 4.0).abs() < 1e-9,
        "two calls at $1 a million: {totals}"
    );
    let runs: u64 = points.iter().map(|p| p["runs"].as_u64().unwrap_or(0)).sum();
    let failed: u64 = points
        .iter()
        .map(|p| p["failed_runs"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!((runs, failed), (2, 1));
}

/// The totals keep a retrieval's source where they keep a tool's name, and a
/// split by tool names tools alone: a source of a tool's name never joins it.
#[sqlx::test]
async fn a_split_by_tool_names_tools_alone(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door_over(&pool, ContentMode::Off, true).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let at = Utc::now() - chrono::Duration::seconds(30);
    let rows = [
        common::received(&door.project, SpanKind::ToolCall, "docs-index", 100, at),
        common::received(&door.project, SpanKind::Retrieval, "docs-index", 100, at),
        common::received(&door.project, SpanKind::Retrieval, "faq-index", 100, at),
        common::received(&door.project, SpanKind::ModelCall, "chat", 100, at),
    ];
    ch.store.insert_spans(&rows).await.expect("the spans");

    let (status, totals) = door
        .lane("GET", "/rollups?days=1&group=tool", "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{totals}");
    let spans = |key: &str| -> u64 {
        totals["points"]
            .as_array()
            .expect("points")
            .iter()
            .filter(|p| p["value"] == key)
            .map(|p| p["spans"].as_u64().unwrap_or(0))
            .sum()
    };
    assert_eq!(spans("docs-index"), 1, "the tool alone: {totals}");
    assert_eq!(spans("faq-index"), 0, "a source is no tool: {totals}");
    assert_eq!(
        spans(""),
        3,
        "the retrievals sit with every other kind: {totals}"
    );
    let kept: Vec<(String, String)> = ch
        .store
        .read(
            &Projects::one(door.project.clone()),
            "SELECT kind, tool FROM telemetry.spans_hourly \
             WHERE project_id IN {projects:Array(String)} AND tool != '' ORDER BY kind, tool",
        )
        .fetch_all()
        .await
        .expect("the totals");
    let keyed = |kind: &str, tool: &str| (kind.to_owned(), tool.to_owned());
    assert_eq!(
        kept,
        [
            keyed("retrieval", "docs-index"),
            keyed("retrieval", "faq-index"),
            keyed("tool_call", "docs-index"),
        ]
    );
}

/// A split by a tag, an end user or a session reads the spans: each call by
/// its run's value — the run's own span's tag first, and a run begun before
/// the window still its own — the ten that spent most named and the rest
/// folded as one, a run carrying none apart, the window no longer than the
/// plan keeps the spans.
#[sqlx::test]
async fn a_split_by_tag_user_or_session_reads_the_spans_within_retention(pool: PgPool) {
    use telmoni_telemetry::store::SpanRow;
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let mut door = door_over(&pool, ContentMode::Off, true).await;
    // A plan that keeps the spans 30 days.
    let state = common::state_with_stores_under(
        &pool,
        door.auth.clone(),
        Arc::new(common::PlanMonth {
            spans: 1_000_000,
            grace_days: 0,
            content_bytes: 1_000_000,
        }),
    );
    door.app = router(state.clone());
    door.state = state;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let now = Utc::now() - chrono::Duration::seconds(30);
    let run = |user: &str, session: &str, tags: &[(&str, &str)], at| SpanRow {
        user_id: user.into(),
        session_id: session.into(),
        tags: tags
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        ..common::received(&door.project, SpanKind::Run, "nightly-report", 900, at)
    };
    let call = |of: &SpanRow, cost: f64, tags: &[(&str, &str)]| SpanRow {
        run_id: of.run_id,
        provider: "openai".into(),
        model: "gpt-5".into(),
        cost: Some(cost),
        tags: tags
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        ..common::received(&door.project, SpanKind::ModelCall, "chat", 300, now)
    };
    // Sampled at a half, so its calls stand for twice what they cost.
    let a = SpanRow {
        sample_rate: Some(0.5),
        ..run("u-a", "s-1", &[("team", "billing")], now)
    };
    let b = run("u-b", "", &[("team", "search")], now);
    let c = run("", "", &[], now);
    // Begun before a day's window, its call inside it.
    let d = run(
        "u-a",
        "",
        &[("team", "billing")],
        now - chrono::Duration::hours(25),
    );
    let mut rows = vec![
        call(&a, 1.0, &[]),
        SpanRow {
            sample_rate: Some(0.5),
            ..call(&a, 2.0, &[])
        },
        call(&b, 1.0, &[("team", "billing")]),
        call(&c, 0.25, &[]),
        call(&d, 0.5, &[]),
    ];
    // Begun past what the plan keeps: its value is none it shows.
    let e = run(
        "",
        "",
        &[("team", "billing")],
        now - chrono::Duration::hours(30 * 24 + 12),
    );
    rows.push(SpanRow {
        received_at: now - chrono::Duration::days(2),
        ..call(&e, 0.07, &[])
    });
    rows.extend([a.clone(), b.clone(), c.clone(), d.clone(), e.clone()]);
    for i in 0..11_u32 {
        let small = run(&format!("u-{i}"), "", &[], now);
        rows.push(call(&small, 0.01 * f64::from(i + 1), &[]));
        rows.push(small);
    }
    ch.store.insert_spans(&rows).await.expect("the spans");

    let read = async |path: &str| {
        let (status, answer) = door.lane("GET", path, "admin-1", None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {answer}");
        answer
    };
    let by = |answer: &Value| {
        let mut spent = std::collections::BTreeMap::<String, f64>::new();
        for point in answer["points"].as_array().expect("points") {
            let value = if point["others"] == true {
                "(others)".to_owned()
            } else {
                point["value"].as_str().unwrap_or_default().to_owned()
            };
            *spent.entry(value).or_default() += point["spend"].as_f64().unwrap_or(0.0);
        }
        spent
    };
    let near = |spent: &std::collections::BTreeMap<String, f64>, value: &str, want: f64| {
        let got = spent.get(value).copied().unwrap_or(f64::NAN);
        assert!(
            (got - want).abs() < 1e-9,
            "{value}: {got} for {want} in {spent:?}"
        );
    };

    let tags = read("/rollups?days=1&group=tag&tag=%20team%20").await;
    assert_eq!(
        (tags["group"].as_str(), tags["tag"].as_str()),
        (Some("tag"), Some("team"))
    );
    let spent = by(&tags);
    near(&spent, "billing", 1.0 + 4.0 + 0.5);
    near(&spent, "search", 1.0);
    near(&spent, "", 0.25 + 0.66);
    assert!(!spent.contains_key("(others)"), "{spent:?}");

    let users = by(&read("/rollups?days=1&group=user").await);
    near(&users, "u-a", 5.5);
    near(&users, "u-b", 1.0);
    near(&users, "u-10", 0.11);
    near(&users, "u-3", 0.04);
    near(&users, "(others)", 0.03 + 0.02 + 0.01);
    near(&users, "", 0.25);
    assert_eq!(
        users.len(),
        12,
        "ten named, the rest and nobody's: {users:?}"
    );
    let total: f64 = users.values().sum();
    let whole: f64 = by(&read("/rollups?days=1").await).values().sum();
    assert!(
        (total - whole).abs() < 1e-9,
        "the split sums to the totals: {total} {whole}"
    );

    let sessions = by(&read("/rollups?days=1&group=session").await);
    near(&sessions, "s-1", 5.0);

    // By the hour, the window's day before reaches past the plan's 30 days.
    let month = by(&read("/rollups?days=30&step=hour&group=tag&tag=team").await);
    near(&month, "billing", 5.5);
    near(&month, "", 0.91 + 0.07);

    // The spans are kept 30 days; the totals thirteen months.
    assert_eq!(read("/rollups?days=90&group=user").await["days"], 30);
    assert_eq!(read("/rollups?days=90&group=model").await["days"], 90);

    for path in [
        "/rollups?group=tag",
        "/rollups?group=tag&tag=%20",
        "/rollups?tag=team",
        "/rollups?group=model&tag=team",
        "/rollups?group=name",
    ] {
        let (status, answer) = door.lane("GET", path, "admin-1", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {answer}");
    }
    let long = format!(
        "/rollups?group=tag&tag={}",
        "k".repeat(contract::TAG_KEY_MAX + 1)
    );
    let (status, _) = door.lane("GET", &long, "admin-1", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The trace list shows and sorts by each run's own spend, never scaled.
#[sqlx::test]
async fn the_list_shows_spend_each_run_its_own(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door_over(&pool, ContentMode::Off, true).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let at = Utc::now() - chrono::Duration::seconds(30);
    let run = |rate: Option<f64>, cost: f64, span: &str| {
        let run_id = uuid::Uuid::new_v4();
        let own = telmoni_telemetry::store::SpanRow {
            run_id,
            span_id: format!("{span:0<16}"),
            session_id: "s1".into(),
            sample_rate: rate,
            ..common::received(&door.project, SpanKind::Run, "nightly-report", 900, at)
        };
        let call = telmoni_telemetry::store::SpanRow {
            run_id,
            span_id: format!("{span}1{:0<14}", ""),
            parent_span_id: own.span_id.clone(),
            session_id: "s1".into(),
            cost: Some(cost),
            ..common::received(&door.project, SpanKind::ModelCall, "chat", 500, at)
        };
        (run_id, [own, call])
    };
    let (sampled, sampled_rows) = run(Some(0.5), 1.0, "a");
    let (whole, whole_rows) = run(None, 3.0, "b");
    // A call that reported no cost, at the project's own price: $2.
    let (priced, mut priced_rows) = run(None, 0.0, "c");
    priced_rows[1].cost = None;
    priced_rows[1].provider = "self".into();
    priced_rows[1].model = "ours-1".into();
    priced_rows[1].input_tokens = 1_000_000;
    sqlx::query(
        "INSERT INTO telemetry.project_prices
             (project_id, position, provider, models, input, output, cache_read, cache_write)
         VALUES ($1, 0, 'self', ARRAY['ours-1'], 2e-6, 0, 0, 0)",
    )
    .bind(&door.project)
    .execute(&pool)
    .await
    .expect("our price");
    let rows: Vec<_> = sampled_rows
        .into_iter()
        .chain(whole_rows)
        .chain(priced_rows)
        .collect();
    ch.store.insert_spans(&rows).await.expect("the runs");

    let (status, list) = door
        .lane("GET", "/traces?sort=spend&dir=desc", "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let spends: Vec<(String, f64)> = list["traces"]
        .as_array()
        .expect("traces")
        .iter()
        .map(|t| {
            (
                t["run_id"].as_str().unwrap_or_default().to_owned(),
                t["spend"].as_f64().unwrap_or(-1.0),
            )
        })
        .collect();
    assert_eq!(
        spends,
        [
            (whole.to_string(), 3.0),
            (priced.to_string(), 2.0),
            (sampled.to_string(), 1.0)
        ],
        "each run its own, the costliest first"
    );
}

/// A project's own prices: every role reads them, its owners and admins set
/// them whole, per million tokens, held to their rules, each change audited.
#[sqlx::test]
async fn a_projects_prices_are_read_by_all_and_set_by_its_admins(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let door = door_over(&pool, ContentMode::Off, false).await;
    door.auth
        .seat(
            "admin-1",
            &door.organization,
            &door.project,
            OrganizationRole::Owner,
            None,
        )
        .seat(
            "member-1",
            &door.organization,
            &door.project,
            OrganizationRole::Member,
            Some(Role::Member),
        );
    let (status, empty) = door.lane("GET", "/prices", "member-1", None).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty, json!({ "prices": [] }));

    let prices = json!({ "prices": [{
        "provider": "openai", "models": ["ft:gpt-5:acme", " gpt-5 "],
        "input": 1.25, "output": 10.0, "cache_read": 0.125, "cache_write": 0.0,
    }]});
    let (status, _) = door
        .lane("PUT", "/prices", "member-1", Some(prices.clone()))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member reads, never sets");
    let (status, set) = door
        .lane("PUT", "/prices", "admin-1", Some(prices.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(
        set["prices"][0]["models"],
        json!(["ft:gpt-5:acme", "gpt-5"])
    );
    let stored: f64 = sqlx::query_scalar("SELECT input FROM telemetry.project_prices")
        .fetch_one(&pool)
        .await
        .expect("stored");
    assert!(
        (stored - 1.25e-6).abs() < 1e-15,
        "a token's, as the table keeps it"
    );
    let (_, read) = door.lane("GET", "/prices", "member-1", None).await;
    assert_eq!(read["prices"][0]["input"], 1.25);

    let changes: Vec<String> = sqlx::query_scalar(
        "SELECT metadata::text FROM audit.events WHERE action = 'updated' ORDER BY seq",
    )
    .fetch_all(&pool)
    .await
    .expect("the chain");
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(
        changes[0].contains(r#""prices": {"to": 1, "from": 0}"#),
        "{}",
        changes[0]
    );
    let (status, _) = door.lane("PUT", "/prices", "admin-1", Some(prices)).await;
    assert_eq!(status, StatusCode::OK);
    let again: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit.events WHERE action = 'updated'")
            .fetch_one(&pool)
            .await
            .expect("the chain");
    assert_eq!(again, 1, "the same list again records nothing");

    let (status, _) = door
        .lane(
            "PUT",
            "/prices",
            "admin-1",
            Some(json!({ "prices": [{ "provider": "openai", "models": ["a"],
                "input": -1.0, "output": 0.0, "cache_read": 0.0, "cache_write": 0.0 }] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A `read` key reads the traces, a trace's content only when it asks, each
/// such read filed as the key's, the rollup and the incidents; an ingest key
/// reads none.
#[sqlx::test]
async fn a_read_key_reads_traces_content_metrics_and_incidents(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    door.send(turn_of("conv-1")).await;
    common::eventually("the trace is read", || found(&door, RUN)).await;

    let (status, list) = door.read("/traces", READ_KEY).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["traces"][0]["run_id"], RUN);
    assert_eq!(list["total"], 1);

    let (status, trace) = door.read(&format!("/traces/{RUN}"), READ_KEY).await;
    assert_eq!(status, StatusCode::OK, "{trace}");
    assert_eq!(trace["spans"].as_array().map(Vec::len), Some(2));
    let head = Request::builder()
        .method("HEAD")
        .uri(format!("/v1/traces/{RUN}?content=true"))
        .header(header::AUTHORIZATION, format!("Bearer {READ_KEY}"))
        .body(Body::empty())
        .expect("a request");
    let response = door.app.clone().oneshot(head).await.expect("an answer");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(trace["content"], Value::Null, "content only when asked");
    assert!(
        door.viewed(&pool).await.is_empty(),
        "nothing read, nothing filed"
    );

    let (status, trace) = door
        .read(&format!("/traces/{RUN}?content=true"), READ_KEY)
        .await;
    assert_eq!(status, StatusCode::OK, "{trace}");
    assert_eq!(
        trace["content"][0]["fields"]["gen_ai.input.messages"],
        MESSAGES
    );
    let viewed = door.viewed(&pool).await;
    assert_eq!(viewed.len(), 1, "the read, filed");
    assert!(viewed[0].0.starts_with("key:"), "{}", viewed[0].0);

    let (status, metrics) = door.read("/metrics?group=agent&step=hour", READ_KEY).await;
    assert_eq!(status, StatusCode::OK, "{metrics}");
    assert_eq!(metrics["group"], "agent");
    assert_eq!(metrics["points"][0]["value"], "nightly-report");
    assert_eq!(metrics["points"][0]["spans"], 2);
    let (status, daily) = door.read("/metrics", READ_KEY).await;
    assert_eq!(status, StatusCode::OK, "{daily}");
    assert_eq!(
        (daily["step"].as_str(), daily["group"].as_str()),
        (Some("day"), None)
    );
    let day = Utc::now().format("%Y-%m-%dT00:00:00Z").to_string();
    assert_eq!(
        daily["points"],
        json!([daily["points"][0].clone()]),
        "one day, ungrouped"
    );
    assert_eq!(daily["points"][0]["at"], day);
    assert_eq!(daily["points"][0].get("value"), None);
    let (status, _) = door.read("/metrics?step=week", READ_KEY).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = door.read("/metrics?group=name", READ_KEY).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, incidents) = door.read("/incidents", READ_KEY).await;
    assert_eq!(status, StatusCode::OK, "{incidents}");
    assert_eq!(incidents["incidents"], json!([]));
    let (status, _) = door
        .read(&format!("/incidents/{}", uuid::Uuid::now_v7()), READ_KEY)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    door.auth.key(
        "tm_other_project",
        &door.organization,
        &ProjectId::new(),
        &[ApiKeyScope::Read],
    );
    let (status, _) = door
        .read(&format!("/traces/{RUN}"), "tm_other_project")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another project's run");

    for path in ["/traces", "/metrics", "/incidents"] {
        let (status, _) = door.read(path, KEY).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{path}: an ingest key reads nothing"
        );
    }
}

/// A moved project's content still opens, filed in the new organization's
/// chain, and an erasure by `user_id` still finds it: both keys moved with
/// its settings row.
#[sqlx::test]
async fn a_moved_project_still_opens_its_content_and_erases_by_user_id(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    door.send(export_of(
        RUN,
        TRACE,
        SPAN,
        &[(contract::USER_ID, "acme-42")],
        "result",
    ))
    .await;
    common::eventually("the trace is read", || found(&door, RUN)).await;

    let taker = OrganizationId::new();
    TelemetrySeam(door.state.clone())
        .move_project(&door.project, &taker)
        .await
        .expect("the move");
    let left = door.organization.clone();
    let moved = Door {
        organization: taker,
        ..door
    };
    moved.auth.seat(
        "admin-1",
        &moved.organization,
        &moved.project,
        OrganizationRole::Owner,
        None,
    );

    let opened = content::open(
        &moved.state,
        Actor::User("admin-1"),
        &moved.project,
        &[run(RUN)],
    )
    .await
    .expect("the moved content opens");
    assert_eq!(opened.len(), 1, "its key moved with the settings row");
    assert_eq!(moved.viewed(&pool).await.len(), 1, "filed where it now is");
    let stayed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events WHERE organization_id = $1 AND action = 'viewed'",
    )
    .bind(&left)
    .fetch_one(&pool)
    .await
    .expect("read the old chain as the owner");
    assert_eq!(stayed, 0, "nothing filed in the organization it left");

    let (status, answer) = moved
        .lane(
            "POST",
            "/users/erase",
            "admin-1",
            Some(json!({ "user_id": "acme-42" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["traces"], 1, "its hashing key moved too: {answer}");
    assert!(
        moved.kept(&pool).await.is_empty(),
        "the content went with it"
    );
}

/// The nightly purge deletes content whose span's day has gone, and the
/// bytes with it; content inside retention stays.
#[sqlx::test]
async fn content_past_its_spans_retention_goes_with_its_bytes(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    door.send(export("result")).await;
    let before = door.held(&pool, &door.organization).await.expect("counted");
    sqlx::query(
        "UPDATE telemetry.plain_content SET received_at = now() - interval '400 days' \
          WHERE project_id = $1 AND field = 'gen_ai.tool.call.result'",
    )
    .bind(&door.project)
    .execute(&pool)
    .await
    .expect("age one field past retention");

    let gone = retention::purge_content(&pool, Utc::now(), &[])
        .await
        .expect("the purge");
    assert_eq!(gone, 1);
    let names: Vec<String> = door
        .kept(&pool)
        .await
        .into_iter()
        .map(|row| row.0)
        .collect();
    assert_eq!(names, ["app.ticket", "gen_ai.input.messages"]);
    let left: i64 = door
        .kept(&pool)
        .await
        .iter()
        .map(|(.., bytes)| i64::from(*bytes))
        .sum();
    assert!(left < before, "the aged field's bytes go with it");
    assert_eq!(door.held(&pool, &door.organization).await, Some(left));
}

/// A project that keeps fewer days loses the content past them with its
/// spans; content inside them stays.
#[sqlx::test]
async fn content_past_a_projects_own_retention_goes(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    door.send(export("result")).await;
    sqlx::query(
        "UPDATE telemetry.plain_content SET received_at = now() - interval '40 days' \
          WHERE project_id = $1 AND field = 'gen_ai.tool.call.result'",
    )
    .bind(&door.project)
    .execute(&pool)
    .await
    .expect("age one field past the project's thirty days");

    let gone = retention::purge_content(&pool, Utc::now(), &[(door.project.clone(), 30)])
        .await
        .expect("the purge");
    assert_eq!(gone, 1);
    let names: Vec<String> = door
        .kept(&pool)
        .await
        .into_iter()
        .map(|row| row.0)
        .collect();
    assert_eq!(names, ["app.ticket", "gen_ai.input.messages"]);
}

/// ⚠ An owner or admin chooses the project's retention, one day up to the
/// longest sold, each change filed with its days from and to; a member reads
/// it and is refused the change.
#[sqlx::test]
async fn an_owner_or_admin_chooses_the_retention_and_a_member_reads_it(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::Off).await;
    for (user, organization_role, seat) in [
        ("admin-1", OrganizationRole::Admin, None),
        ("member-1", OrganizationRole::Member, Some(Role::Member)),
    ] {
        door.auth.seat(
            user,
            &door.organization,
            &door.project,
            organization_role,
            seat,
        );
    }
    let put = |user: &'static str, days: Value| {
        door.lane(
            "PUT",
            "/retention",
            user,
            Some(json!({ "retention_days": days })),
        )
    };

    let (status, answer) = put("member-1", json!(30)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    for days in [json!(0), json!(361), json!(-1)] {
        let (status, answer) = put("admin-1", days.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{days}: {answer}");
    }
    let (status, answer) = put("admin-1", json!(30)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer,
        json!({ "span_days": 30, "max_span_days": 360, "rollup_months": 13 })
    );
    let (_, posture) = door.lane("GET", "/privacy", "member-1", None).await;
    assert_eq!(posture["retention"]["span_days"], 30, "{posture}");
    // The same days again files nothing; the longest, named, is none again.
    put("admin-1", json!(30)).await;
    let (status, answer) = put("admin-1", json!(360)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["span_days"], 360);

    let filed: Vec<String> = sqlx::query_scalar(
        "SELECT metadata::text FROM audit.events \
          WHERE organization_id = $1 AND resource_kind = 'project' ORDER BY seq",
    )
    .bind(&door.organization)
    .fetch_all(&pool)
    .await
    .expect("the chain");
    assert_eq!(
        filed,
        [
            r#"{"retention_days": {"to": 30, "from": 360}}"#,
            r#"{"retention_days": {"to": 360, "from": 30}}"#,
        ]
    );
}

/// ⚠ Content waits for its spans: an export whose spans ClickHouse does not
/// take keeps no content, so none outlives or precedes a span; the retry,
/// once the spans land, keeps it.
#[sqlx::test]
async fn content_is_kept_only_once_its_spans_are(pool: PgPool) {
    let door = door_over(&pool, ContentMode::On, false).await;
    let (status, _, _) = door.post(export("result")).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "ClickHouse is out of reach"
    );
    assert!(
        door.kept(&pool).await.is_empty(),
        "no content without its spans"
    );
    assert_eq!(door.held(&pool, &door.organization).await, None);
}

/// ⚠ The nightly purge and a transfer each lock the project's settings row
/// first: a transfer that meets a purge in flight waits for it, and moves only
/// the bytes the purge left, so neither organization counts what went.
#[sqlx::test]
async fn a_transfer_waits_for_a_purge_in_flight_and_moves_what_it_left(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::On).await;
    door.send(export("result")).await;
    assert!(
        door.held(&pool, &door.organization)
            .await
            .unwrap_or_default()
            > 0
    );

    let mut purging = pool.begin().await.expect("the purge's transaction");
    let gone = telmoni_telemetry::db::delete_content(
        &mut purging,
        std::slice::from_ref(&door.project),
        Some(Utc::now() + chrono::Duration::days(1)),
        i64::MAX,
    )
    .await
    .expect("the purge deletes every field");
    assert_eq!(gone, 3);

    let taker = OrganizationId::new();
    let seam = TelemetrySeam(door.state.clone());
    let moving = tokio::spawn({
        let (project, taker) = (door.project.clone(), taker.clone());
        async move { seam.move_project(&project, &taker).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!moving.is_finished(), "the transfer waits for the purge");
    purging.commit().await.expect("the purge commits");
    moving.await.expect("the move ran").expect("the move");

    assert_eq!(door.held(&pool, &door.organization).await, Some(0));
    assert_eq!(
        door.held(&pool, &taker).await.unwrap_or_default(),
        0,
        "no expired bytes followed the project"
    );
}

const RUN_B: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f02";
const TRACE_B: &str = "5bf92f3577b34da6a3ce929d0e0e4737";
const SPAN_B: &str = "01f067aa0ba902b8";
const RUN_C: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f03";
const TRACE_C: &str = "6bf92f3577b34da6a3ce929d0e0e4738";
const SPAN_C: &str = "02f067aa0ba902b9";
const RUN_D: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f04";
const TRACE_D: &str = "7bf92f3577b34da6a3ce929d0e0e4739";
const SPAN_D: &str = "04f067aa0ba902bb";
const SPAN_E: &str = "05f067aa0ba902bc";

/// Whether the run is among the project's traces: read from the list, which
/// opens no content, so waiting for a trace files no read.
async fn found(door: &Door, run: &str) -> bool {
    let (status, list) = door.lane("GET", "/traces", "admin-1", None).await;
    status == StatusCode::OK
        && list
            .get("traces")
            .and_then(Value::as_array)
            .is_some_and(|traces| {
                traces
                    .iter()
                    .any(|trace| trace.get("run_id") == Some(&json!(run)))
            })
}

/// ⚠ A trace is deleted by an admin alone: the request filed before anything
/// goes, then its spans gone from every read and its content and bytes gone,
/// a span it sends after dropped, and the receipt mailed; another trace
/// stands, and a run not the project's files nothing.
#[sqlx::test]
async fn a_deleted_trace_is_filed_then_gone_from_both_stores(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth
        .seat(
            "admin-1",
            &door.organization,
            &door.project,
            OrganizationRole::Member,
            Some(Role::Admin),
        )
        .seat(
            "member-1",
            &door.organization,
            &door.project,
            OrganizationRole::Member,
            Some(Role::Member),
        );
    door.send(export("result")).await;
    door.send(export_of(RUN_B, TRACE_B, SPAN_B, &[], "other"))
        .await;
    common::eventually("both traces are read", || async {
        found(&door, RUN).await && found(&door, RUN_B).await
    })
    .await;

    let (status, _) = door
        .lane("DELETE", &format!("/traces/{RUN}"), "member-1", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member deletes nothing");
    let stranger = "0191a3b4-7c2e-7d3f-8e4a-000000000000";
    let (status, _) = door
        .lane("DELETE", &format!("/traces/{stranger}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        door.deletions(&pool).await.is_empty(),
        "nothing found, nothing filed"
    );

    let (status, answer) = door
        .lane("DELETE", &format!("/traces/{RUN}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer, json!({ "run_id": RUN, "spans": 1, "fields": 3 }));
    assert_eq!(
        door.deletions(&pool).await,
        [(
            "admin-1".to_owned(),
            "trace".to_owned(),
            RUN.to_owned(),
            json!({ "spans": 1 })
        )]
    );
    assert_eq!(door.kept_traces(&pool).await, [TRACE_B]);
    // The other trace's fields alone stay counted, each at its sealed size.
    let other: i64 = door
        .kept(&pool)
        .await
        .iter()
        .map(|(.., bytes)| i64::from(*bytes))
        .sum();
    assert_eq!(door.held(&pool, &door.organization).await, Some(other));
    assert!(
        !found(&door, RUN).await,
        "gone from every read once answered"
    );
    door.send(export_of(RUN, TRACE, "03f067aa0ba902ba", &[], "late"))
        .await;
    assert!(
        !found(&door, RUN).await,
        "a span the run sends after is dropped"
    );
    assert_eq!(door.kept_traces(&pool).await, [TRACE_B], "and its content");
    assert!(found(&door, RUN_B).await, "another trace stands");
    assert_eq!(
        door.auth.receipts(),
        [("admin-1".to_owned(), "trace".to_owned())]
    );
}

/// ⚠ An end user is erased by the id their spans carry, hashed as ingest
/// hashed it: their traces and their sessions' unnamed ones go with their
/// content, another user's stand in the same session, and the chain names
/// the hash, never the id. An id on no trace is filed too.
#[sqlx::test]
async fn an_end_user_is_erased_by_the_id_their_spans_carry(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let (user, session) = (contract::USER_ID, contract::SESSION_ID);
    door.send(export_of(
        RUN,
        TRACE,
        SPAN,
        &[(user, "acme-42"), (session, "conv-1")],
        "result",
    ))
    .await;
    door.send(export_of(
        RUN_C,
        TRACE_C,
        SPAN_C,
        &[(session, "conv-1")],
        "unnamed",
    ))
    .await;
    door.send(export_of(
        RUN_B,
        TRACE_B,
        SPAN_B,
        &[(user, "acme-43"), (session, "conv-1")],
        "other",
    ))
    .await;
    // A run of the session, unnamed there, that names another user elsewhere.
    door.send(export_of(
        RUN_D,
        TRACE_D,
        SPAN_D,
        &[(session, "conv-1")],
        "turn",
    ))
    .await;
    door.send(export_of(
        RUN_D,
        TRACE_D,
        SPAN_E,
        &[(user, "acme-44")],
        "tool",
    ))
    .await;
    common::eventually("every trace is read", || async {
        found(&door, RUN).await
            && found(&door, RUN_B).await
            && found(&door, RUN_C).await
            && found(&door, RUN_D).await
    })
    .await;

    let long = "u".repeat(contract::USER_ID_MAX + 1);
    let (status, _) = door
        .lane(
            "POST",
            "/users/erase",
            "admin-1",
            Some(json!({ "user_id": long })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no span carries an id that long"
    );

    let (status, answer) = door
        .lane(
            "POST",
            "/users/erase",
            "admin-1",
            Some(json!({ "user_id": "acme-42" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer, json!({ "traces": 2, "spans": 2, "fields": 6 }));
    assert_eq!(door.kept_traces(&pool).await, [TRACE_B, TRACE_D]);
    assert!(
        !found(&door, RUN).await,
        "their trace is gone once answered"
    );
    assert!(
        !found(&door, RUN_C).await,
        "and their session's unnamed one"
    );
    assert!(
        found(&door, RUN_B).await,
        "another user's trace stands, though it shares the session"
    );
    assert!(
        found(&door, RUN_D).await,
        "as does one naming another user on a span outside it"
    );

    let (status, answer) = door
        .lane(
            "POST",
            "/users/erase",
            "admin-1",
            Some(json!({ "user_id": "nobody" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer, json!({ "traces": 0, "spans": 0, "fields": 0 }));

    let hashed = |raw: &str| {
        let door = &door;
        let raw = raw.to_owned();
        async move {
            telmoni_telemetry::hashing::storage_for_project(&door.state, &door.project)
                .await
                .expect("the project's key")
                .store(&raw)
        }
    };
    assert_eq!(
        door.deletions(&pool).await,
        [
            (
                "admin-1".to_owned(),
                "end_user".to_owned(),
                hashed("acme-42").await,
                json!({ "traces": 2 })
            ),
            (
                "admin-1".to_owned(),
                "end_user".to_owned(),
                hashed("nobody").await,
                json!({ "traces": 0 })
            ),
        ]
    );
    let named: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events WHERE organization_id = $1 \
            AND (resource_id = 'acme-42' OR metadata::text LIKE '%acme-42%')",
    )
    .bind(&door.organization)
    .fetch_one(&pool)
    .await
    .expect("read the chain as the owner");
    assert_eq!(named, 0, "the chain never names the id");
    assert_eq!(
        door.auth.receipts(),
        [
            ("admin-1".to_owned(), "end_user".to_owned()),
            ("admin-1".to_owned(), "end_user".to_owned())
        ]
    );
}

/// An export whose spans each carry OpenTelemetry's evaluation events.
fn scored_export(run: &str) -> Vec<u8> {
    scored_export_in(run, TRACE)
}

/// [`scored_export`] under its own trace.
fn scored_export_in(run: &str, trace: &str) -> Vec<u8> {
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let event = |attributes: Value| {
        json!({
            "name": "gen_ai.evaluation.result", "timeUnixNano": "1760000001000000000",
            "attributes": attributes,
        })
    };
    json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [
        {
            "traceId": trace, "spanId": SPAN, "name": "nightly-report",
            "startTimeUnixNano": "1760000000000000000", "endTimeUnixNano": "1760000002000000000",
            "attributes": [
                text(contract::RUN_ID, run), text(contract::AGENT, "nightly-report"),
                text(contract::OPERATION, "invoke_agent"), text(contract::SESSION_ID, "conv-1"),
            ],
            "events": [event(json!([
                text("gen_ai.evaluation.name", "relevance"),
                { "key": "gen_ai.evaluation.score.value", "value": { "doubleValue": 0.75 } },
                { "key": "telmoni.score.range_min", "value": { "doubleValue": 0.0 } },
                { "key": "telmoni.score.range_max", "value": { "intValue": "1" } },
                text("telmoni.score.version", "judge-1"),
            ]))],
        },
        {
            "traceId": trace, "spanId": "01f067aa0ba902b8", "parentSpanId": SPAN, "name": "chat",
            "startTimeUnixNano": "1760000000000000000", "endTimeUnixNano": "1760000001000000000",
            "attributes": [
                text(contract::RUN_ID, run), text(contract::AGENT, "nightly-report"),
                text(contract::OPERATION, "chat"),
            ],
            "events": [
                event(json!([
                    text("gen_ai.evaluation.name", "grounded"), text("telmoni.score.type", "boolean"),
                    { "key": "gen_ai.evaluation.score.value", "value": { "intValue": "1" } },
                ])),
                event(json!([text("gen_ai.evaluation.name", "tone"), text("gen_ai.evaluation.score.label", "polite")])),
                // No name: no score.
                event(json!([{ "key": "gen_ai.evaluation.score.value", "value": { "doubleValue": 3.0 } }])),
            ],
        },
    ]}]}]})
    .to_string()
    .into_bytes()
}

/// The application's evaluation events land as its spans' scores, the run's
/// own span's the run's, each once however often its export is sent.
#[sqlx::test]
async fn the_applications_evaluation_events_land_once_as_its_spans_scores(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::Off).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    door.send(scored_export(RUN)).await;
    door.send(scored_export(RUN)).await;
    let (status, scores) = door
        .lane("GET", &format!("/scores?run_id={RUN}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{scores}");
    let relevance = scores["scores"]
        .as_array()
        .and_then(|all| all.iter().find(|s| s["name"] == "relevance"))
        .expect("relevance");
    assert_eq!(
        (
            &relevance["range_min"],
            &relevance["range_max"],
            &relevance["scorer_version"]
        ),
        (&json!(0.0), &json!(1.0), &json!("judge-1")),
        "an event's range and version ride on its score"
    );
    let mut got: Vec<(String, String, String, Value, Value)> = scores["scores"]
        .as_array()
        .expect("scores")
        .iter()
        .map(|s| {
            assert_eq!(s["source"], "sdk");
            (
                s["name"].as_str().unwrap_or_default().to_owned(),
                s["target"].as_str().unwrap_or_default().to_owned(),
                s["data_type"].as_str().unwrap_or_default().to_owned(),
                s["value"].clone(),
                s["categories"].clone(),
            )
        })
        .collect();
    got.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        got,
        [
            (
                "grounded".into(),
                "span".into(),
                "boolean".into(),
                json!(1.0),
                json!([])
            ),
            (
                "relevance".into(),
                "run".into(),
                "number".into(),
                json!(0.75),
                json!([])
            ),
            (
                "tone".into(),
                "span".into(),
                "category".into(),
                Value::Null,
                json!(["polite"])
            ),
        ],
        "{scores}"
    );
}

/// Two scores of one name at one time on one span, as a record times them at
/// the span's end, stay two, and an export sent again keeps them two.
#[sqlx::test]
async fn two_scores_of_one_name_at_one_time_stay_two(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::Off).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let event = |version: &str| {
        json!({ "name": "gen_ai.evaluation.result", "timeUnixNano": "1760000002000000000",
            "attributes": [
                text("gen_ai.evaluation.name", "relevance"),
                { "key": "gen_ai.evaluation.score.value", "value": { "doubleValue": 0.5 } },
                text("telmoni.score.version", version),
            ] })
    };
    let export = json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [{
        "traceId": TRACE, "spanId": SPAN, "name": "nightly-report",
        "startTimeUnixNano": "1760000000000000000", "endTimeUnixNano": "1760000002000000000",
        "attributes": [
            text(contract::RUN_ID, RUN), text(contract::AGENT, "nightly-report"),
            text(contract::OPERATION, "invoke_agent"),
        ],
        "events": [event("judge-1"), event("judge-2")],
    }]}]}]})
    .to_string()
    .into_bytes();
    door.send(export.clone()).await;
    door.send(export).await;
    let (status, scores) = door
        .lane("GET", &format!("/scores?run_id={RUN}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{scores}");
    let mut versions: Vec<&str> = scores["scores"]
        .as_array()
        .expect("scores")
        .iter()
        .filter_map(|s| s["scorer_version"].as_str())
        .collect();
    versions.sort_unstable();
    assert_eq!(versions, ["judge-1", "judge-2"], "{scores}");
}

/// The application's scores sent after their run ended land once under
/// their own ids, a run's spans needing no lookup; one score breaking a
/// score's rules refuses the body whole, and a GET is told to POST.
#[sqlx::test]
async fn the_applications_late_scores_land_once_and_a_bad_one_keeps_none(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::Off).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let post = |method: &str, body: Value| {
        door.app.clone().oneshot(
            Request::builder()
                .method(method)
                .uri("/v1/scores")
                .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("a request"),
        )
    };
    let scores = json!({ "scores": [
        { "id": "0b9a3f0e-6c1d-4f7a-9e2b-5d8c7a6b4e31", "run_id": RUN, "name": "thumbs",
          "data_type": "boolean", "value": 1 },
        { "id": "7c2e5a1b-3d4f-4a6b-8c9d-0e1f2a3b4c5d", "run_id": RUN, "span_id": SPAN.to_uppercase(),
          "name": "relevance", "data_type": "number", "value": 0.5, "range_min": 0, "range_max": 1,
          "version": " judge-2 " },
        { "id": "1d2c3b4a-5e6f-4a7b-8c9d-0a1b2c3d4e5f", "session_id": "conv-1", "name": "outcome",
          "data_type": "category", "categories": ["resolved"] },
    ]});
    for _ in 0..2 {
        let answer = post("POST", scores.clone()).await.expect("an answer");
        assert_eq!(answer.status(), StatusCode::NO_CONTENT);
    }
    // The session is named as sent and kept as its spans keep it, hashed, so
    // it is read by the hash and by the id as sent finds nothing.
    let session = telmoni_telemetry::hashing::storage_for_project(&door.state, &door.project)
        .await
        .expect("the project's key")
        .store("conv-1");
    let (status, read) = door
        .lane(
            "GET",
            &format!("/scores?run_id={RUN}&session_id={session}"),
            "admin-1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    let (status, as_sent) = door
        .lane(
            "GET",
            // Another run: a session's score names the nil run, which would match it.
            &format!("/scores?run_id={}&session_id=conv-1", uuid::Uuid::new_v4()),
            "admin-1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{as_sent}");
    assert_eq!(
        as_sent["scores"],
        json!([]),
        "the session as sent is kept nowhere"
    );
    let mut got: Vec<(String, String, String, String)> = read["scores"]
        .as_array()
        .expect("scores")
        .iter()
        .map(|s| {
            assert_eq!(s["source"], "sdk");
            let text = |key: &str| s[key].as_str().unwrap_or_default().to_owned();
            (
                text("name"),
                text("target"),
                text("span_id"),
                text("scorer_version"),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            (
                "outcome".into(),
                "session".into(),
                String::new(),
                String::new()
            ),
            (
                "relevance".into(),
                "span".into(),
                SPAN.into(),
                "judge-2".into()
            ),
            ("thumbs".into(), "run".into(), String::new(), String::new()),
        ],
        "{read}"
    );

    let bad = json!({ "scores": [
        { "run_id": RUN, "name": "kept-not", "data_type": "boolean", "value": 1 },
        { "run_id": RUN, "name": "relevance", "data_type": "number", "value": 2, "range_max": 1 },
    ]});
    let answer = post("POST", bad).await.expect("an answer");
    assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
    let body = answer
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    assert!(
        String::from_utf8_lossy(&body).contains("scores[1]"),
        "names its place"
    );
    let none = post("POST", json!({ "scores": [] }))
        .await
        .expect("an answer");
    assert_eq!(none.status(), StatusCode::BAD_REQUEST);
    let (_, read) = door
        .lane("GET", &format!("/scores?run_id={RUN}"), "admin-1", None)
        .await;
    assert!(
        !read.to_string().contains("kept-not"),
        "a refused body keeps nothing: {read}"
    );
    let get = post("GET", json!({})).await.expect("an answer");
    assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        get.headers().get(header::ALLOW).map(|v| v.as_bytes()),
        Some(&b"POST"[..])
    );
}

/// A member's comment on a score is content: kept sealed under the score's
/// run where the project keeps content, opened with the run's content as its
/// read, replaced by the next, taken back with the score, its bytes given
/// back; refused on a session's score and where the project keeps none.
#[sqlx::test]
async fn a_scores_comment_is_kept_as_its_runs_content(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::On).await;
    door.auth.seat(
        "member-1",
        &door.organization,
        &door.project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    door.send(scored_export(RUN)).await;
    let before = door.held(&pool, &door.organization).await.expect("counted");
    let put = |body: Value| {
        let door = &door;
        async move { door.lane("PUT", "/scores", "member-1", Some(body)).await }
    };
    let scored = |comment: &str| json!({ "run_id": RUN, "name": "useful", "data_type": "boolean", "value": 1, "comment": comment });
    let comments = || async {
        let (status, trace) = door
            .lane(
                "GET",
                &format!("/traces/{RUN}?content=true"),
                "member-1",
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{trace}");
        trace["content"]
            .as_array()
            .expect("content")
            .iter()
            .filter(|span| {
                span["span_id"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("score/"))
            })
            .map(|span| span["fields"]["telmoni.score.comment"].clone())
            .collect::<Vec<Value>>()
    };
    let (status, first) = put(scored("  The refund was right.  ")).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert!(
        first.get("comment").is_none(),
        "a score's answer carries no content"
    );
    assert_eq!(comments().await, [json!("The refund was right.")]);
    let (status, _) = put(scored("Said again, better.")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        comments().await,
        [json!("Said again, better.")],
        "the last replaces it"
    );
    let (status, _) =
        put(json!({ "run_id": RUN, "name": "useful", "data_type": "boolean", "value": 0 })).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        comments().await.is_empty(),
        "a score given again without one takes it back"
    );
    let (_, with) = put(scored("Kept a while.")).await;
    let id = with["score_id"].as_str().expect("an id").to_owned();
    let (status, _) = door
        .lane("DELETE", &format!("/scores/{id}"), "member-1", None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(comments().await.is_empty(), "taken back with its score");
    assert_eq!(
        door.held(&pool, &door.organization).await.expect("counted"),
        before,
        "its bytes given back"
    );

    for (body, why) in [
        (
            json!({ "session_id": "conv-1", "name": "resolved", "data_type": "boolean", "value": 1, "comment": "x" }),
            "a session's",
        ),
        (
            scored(&"x".repeat(telmoni_telemetry::scores::COMMENT_MAX + 1)),
            "too long",
        ),
    ] {
        let (status, answer) = put(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {answer}");
    }
    let off = door_over(&pool, ContentMode::Off, true).await;
    off.auth.seat(
        "member-1",
        &off.organization,
        &off.project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    off.send(scored_export(RUN)).await;
    let (status, answer) = off
        .lane("PUT", "/scores", "member-1", Some(scored("x")))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
}

/// A member scores a run, a span and a session, a second score of one name
/// replacing their first, takes one back, and may not take another's; each
/// filed in the chain by its name, never its value.
#[sqlx::test]
async fn a_member_scores_a_run_a_span_and_a_session_each_filed(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::Off).await;
    door.auth
        .seat(
            "member-1",
            &door.organization,
            &door.project,
            OrganizationRole::Member,
            Some(Role::Member),
        )
        .seat(
            "member-2",
            &door.organization,
            &door.project,
            OrganizationRole::Member,
            Some(Role::Member),
        );
    door.send(scored_export(RUN)).await;
    // The session as the console names it: as its spans keep it, hashed.
    let conv = telmoni_telemetry::hashing::storage_for_project(&door.state, &door.project)
        .await
        .expect("the project's key")
        .store("conv-1");
    let put = |user: &'static str, body: Value| {
        let door = &door;
        async move { door.lane("PUT", "/scores", user, Some(body)).await }
    };
    let (status, first) = put("member-1", json!({ "run_id": RUN, "name": "useful", "data_type": "number", "value": 2, "range_min": 0, "range_max": 5 })).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (status, again) = put("member-1", json!({ "run_id": RUN, "name": "useful", "data_type": "number", "value": 4, "range_min": 0, "range_max": 5 })).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(
        first["score_id"], again["score_id"],
        "the same name again replaces it"
    );
    let (status, span) = put("member-1", json!({ "run_id": RUN, "span_id": SPAN, "name": "on topic", "data_type": "category", "categories": ["yes"] })).await;
    assert_eq!(status, StatusCode::OK, "{span}");
    let (status, session) = put(
        "member-2",
        json!({ "session_id": conv, "name": "resolved", "data_type": "boolean", "value": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{session}");

    for (body, want) in [
        (
            json!({ "run_id": "00000000-0000-4000-8000-000000000000", "name": "x", "data_type": "boolean", "value": 1 }),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({ "session_id": "nobody's", "name": "x", "data_type": "boolean", "value": 1 }),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({ "run_id": RUN, "name": "x", "data_type": "number", "value": 9, "range_max": 5 }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "run_id": RUN, "session_id": "conv-1", "name": "x", "data_type": "boolean", "value": 1 }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, answer) = put("member-1", body).await;
        assert_eq!(status, want, "{answer}");
    }

    let read = || async {
        let (status, scores) = door
            .lane(
                "GET",
                &format!("/scores?run_id={RUN}&session_id={conv}"),
                "member-1",
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{scores}");
        let mut names: Vec<(String, Value)> = scores["scores"]
            .as_array()
            .expect("scores")
            .iter()
            .filter(|s| s["source"] == "member")
            .map(|s| {
                (
                    s["name"].as_str().unwrap_or_default().to_owned(),
                    s["value"].clone(),
                )
            })
            .collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        names
    };
    assert_eq!(
        read().await,
        [
            ("on topic".to_owned(), Value::Null),
            ("resolved".to_owned(), json!(1.0)),
            ("useful".to_owned(), json!(4.0)),
        ]
    );

    let take = |user: &'static str, id: &Value| {
        let door = &door;
        let id = id.as_str().unwrap_or_default().to_owned();
        async move {
            door.lane("DELETE", &format!("/scores/{id}"), user, None)
                .await
                .0
        }
    };
    assert_eq!(
        take("member-1", &session["score_id"]).await,
        StatusCode::FORBIDDEN,
        "another's score"
    );
    assert_eq!(
        take("member-1", &span["score_id"]).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        take("member-1", &span["score_id"]).await,
        StatusCode::NOT_FOUND,
        "taken back once"
    );
    assert_eq!(
        read()
            .await
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
        ["resolved", "useful"]
    );

    let filed: Vec<Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit.events WHERE metadata ? 'score' ORDER BY seq",
    )
    .fetch_all(&pool)
    .await
    .expect("the chain");
    assert_eq!(filed.len(), 5, "{filed:?}");
    assert!(
        filed.iter().all(|d| {
            let d = d.to_string();
            d.find("4.0").is_none() && !d.contains("conv-1") && !d.contains(conv.as_str())
        }),
        "names a score, never its value or a session: {filed:?}"
    );
}

/// A run's scores go with it, a project's with its purge, and every score
/// past retention with the spans, as the night's purge and a shorter one take them.
#[sqlx::test]
async fn scores_leave_with_their_runs_their_project_and_their_days(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::Off).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    door.send(scored_export(RUN)).await;
    let count = |sql: &'static str| {
        let ch = &ch;
        let door = &door;
        async move {
            ch.store
                .read(&Projects::one(door.project.clone()), sql)
                .fetch_one::<u64>()
                .await
                .expect("counted")
        }
    };
    const SCORES: &str =
        "SELECT count() FROM telemetry.scores FINAL WHERE project_id IN {projects:Array(String)}";
    assert_eq!(count(SCORES).await, 3);
    ch.store
        .delete_runs(&door.project, &[RUN.parse().expect("a run")])
        .await
        .expect("deleted");
    assert_eq!(count(SCORES).await, 0, "the run's scores went with it");

    door.send(scored_export("0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f02"))
        .await;
    assert_eq!(count(SCORES).await, 3);
    ch.store
        .delete_before(&door.project, Utc::now() + chrono::Duration::seconds(5))
        .await
        .expect("deleted");
    assert_eq!(count(SCORES).await, 0, "past the project's retention");

    // A score given past the longest retention, beside today's: the night's
    // purge, at today's date so no other suite's rows go, takes it alone.
    door.send(scored_export("0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f03"))
        .await;
    let old = telmoni_telemetry::scores::ScoreRow {
        project_id: door.project.clone(),
        received_at: Utc::now() - chrono::Duration::days(400),
        score_id: uuid::Uuid::new_v4(),
        target: telmoni_shared::ScoreTarget::Run,
        run_id: uuid::Uuid::new_v4(),
        trace_id: String::new(),
        span_id: String::new(),
        session_id: String::new(),
        name: "old".into(),
        data_type: telmoni_shared::ScoreType::Boolean,
        value: 1.0,
        categories: Vec::new(),
        range_min: None,
        range_max: None,
        source: telmoni_shared::ScoreSource::Sdk,
        scorer: String::new(),
        scorer_version: String::new(),
        is_deleted: 0,
    };
    ch.store
        .insert_scores_once(&[old], &uuid::Uuid::new_v4().to_string())
        .await
        .expect("an old score");
    assert_eq!(count(SCORES).await, 4);
    retention::purge(&ch.migrator, Utc::now(), &[])
        .await
        .expect("the night's purge");
    common::eventually("the night's purge took the old score alone", || async {
        count(SCORES).await == 3
    })
    .await;
    ch.store
        .delete_runs(&door.project, &[RUN_C.parse().expect("a run")])
        .await
        .expect("deleted");

    door.send(scored_export("0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f04"))
        .await;
    ch.store
        .delete_project(&door.project)
        .await
        .expect("purged");
    common::eventually("the purge took the scores", || async {
        count(SCORES).await == 0
    })
    .await;
}

/// Another exporter's span's score waits with its span, and lands under its
/// run once its trace is placed.
#[sqlx::test]
async fn another_exporters_score_lands_with_its_run(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let door = door(&pool, ContentMode::Off).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let body = json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": "x" }, "spans": [{
        "traceId": TRACE, "spanId": SPAN, "name": "chat gpt-5",
        "startTimeUnixNano": "1760000000000000000", "endTimeUnixNano": "1760000001000000000",
        "attributes": [text(contract::OPERATION, "chat")],
        "events": [{ "name": "gen_ai.evaluation.result", "timeUnixNano": "1760000001000000000",
            "attributes": [text("gen_ai.evaluation.name", "relevance"),
                { "key": "gen_ai.evaluation.score.value", "value": { "doubleValue": 0.5 } }] }],
    }]}]}]});
    door.send(body.to_string().into_bytes()).await;
    let waiting: u64 = ch
        .store
        .read(
            &Projects::one(door.project.clone()),
            "SELECT count() FROM telemetry.scores FINAL WHERE project_id IN {projects:Array(String)} \
             AND run_id = toUUID('00000000-0000-0000-0000-000000000000')",
        )
        .fetch_one()
        .await
        .expect("counted");
    assert_eq!(waiting, 1, "the score waits with its span");

    sqlx::query("UPDATE telemetry.held_spans SET received_at = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("quiet");
    telmoni_telemetry::foreign::land_quiet(&door.state)
        .await
        .expect("a pass");
    let run = telmoni_telemetry::foreign::run_of_trace(&door.project, TRACE);
    let (status, scores) = door
        .lane("GET", &format!("/scores?run_id={run}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{scores}");
    let scores = scores["scores"].as_array().expect("scores");
    assert_eq!(scores.len(), 1, "{scores:?}");
    assert_eq!(scores[0]["name"], "relevance");
    assert_eq!(scores[0]["target"], "span");
    assert_eq!(scores[0]["run_id"], run.to_string());
}

/// The trace list answers each run's own scores by name and keeps only the
/// runs a score's bounds admit.
#[sqlx::test]
async fn the_list_carries_its_scores_and_filters_by_one(pool: PgPool) {
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let door = door(&pool, ContentMode::Off).await;
    door.auth.seat(
        "admin-1",
        &door.organization,
        &door.project,
        OrganizationRole::Owner,
        None,
    );
    let other = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f09";
    door.send(scored_export(RUN)).await;
    door.send(scored_export_in(other, TRACE_B)).await;
    let (status, _) = door
        .lane("PUT", "/scores", "admin-1", Some(json!({ "run_id": other, "name": "relevance", "data_type": "number", "value": 0.25 })))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, list) = door.lane("GET", "/traces", "admin-1", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let relevance = |run: &str| {
        list["scores"][run]
            .as_array()
            .and_then(|names| names.iter().find(|s| s["name"] == "relevance").cloned())
    };
    assert_eq!(
        relevance(RUN).map(|s| (s["mean"].clone(), s["count"].clone())),
        Some((json!(0.75), json!(1)))
    );
    assert_eq!(
        relevance(other).map(|s| (s["mean"].clone(), s["count"].clone())),
        Some((json!(0.5), json!(2))),
        "the application's and a member's, together"
    );

    let (status, high) = door
        .lane(
            "GET",
            "/traces?score=relevance&score_max=0.5",
            "admin-1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{high}");
    let runs: Vec<&str> = high["traces"]
        .as_array()
        .expect("traces")
        .iter()
        .filter_map(|t| t["run_id"].as_str())
        .collect();
    assert_eq!(runs, [other], "a run any score of the name admits: {high}");
    let (status, _) = door
        .lane("GET", "/traces?score_min=0.6", "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "bounds name their score");

    // Charted over the window: the score's mean on the runs the list admits.
    let means = |histogram: &Value| -> Vec<f64> {
        histogram["bars"]
            .as_array()
            .expect("bars")
            .iter()
            .filter_map(|bar| bar["score_mean"].as_f64())
            .collect()
    };
    let (status, charted) = door
        .lane(
            "GET",
            "/traces/histogram?score=relevance&score_max=0.5",
            "admin-1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{charted}");
    assert_eq!(charted["score"], "relevance");
    assert_eq!(
        means(&charted),
        [0.5],
        "the admitted run's two scores, by when given: {charted}"
    );
    let (_, plain) = door.lane("GET", "/traces/histogram", "admin-1", None).await;
    assert!(
        means(&plain).is_empty() && plain["score"].is_null(),
        "{plain}"
    );
}
