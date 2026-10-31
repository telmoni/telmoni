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
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::Telemetry as _;
use telmoni_shared::test_util::{apply_audit_migrations, captured_logs};
use telmoni_shared::{
    ApiKeyScope, ContentMode, OrganizationId, OrganizationRole, ProjectId, Role, TelmoniError,
};
use telmoni_telemetry::ingest::batch::SpanContent;
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::Projects;
use telmoni_telemetry::{AppState, content, contract, otlp_router, retention, router, v1_router};

const KEY: &str = "tm_content_key";
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
    Door {
        app: router(state.clone())
            .nest("/otlp", otlp_router(state.clone()))
            .nest("/v1", v1_router(state.clone())),
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
            door.state
                .vault
                .open(
                    &key,
                    SealedRef { ciphertext, nonce },
                    content::field_aad(SPAN, field).as_bytes(),
                )
                .expect("a field opens under its own span and name")
                .expose()
                .to_owned()
        })
        .collect();
    assert_eq!(opened, [r#"{"id":42}"#, MESSAGES, long.as_str()]);
    let (_, ciphertext, nonce, _) = &kept[1];
    assert!(
        door.state
            .vault
            .open(
                &key,
                SealedRef { ciphertext, nonce },
                content::field_aad(SPAN, "gen_ai.output.messages").as_bytes(),
            )
            .is_err(),
        "a field moved to another name does not open"
    );

    let bytes: i64 = kept.iter().map(|(.., bytes)| i64::from(*bytes)).sum();
    assert_eq!(
        bytes,
        i64::try_from(MESSAGES.len() + long.len() + 9).expect("small")
    );
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
        None,
    )
    .await
    .expect("a run without content opens to nothing");
    assert!(nothing.is_empty());
    let unsettled = content::open(
        &door.state,
        Actor::User("user-1"),
        &ProjectId::new(),
        &[run(RUN)],
        None,
    )
    .await
    .expect("a project that never exported keeps no content, and says so");
    assert!(unsettled.is_empty());
    assert!(
        door.viewed(&pool).await.is_empty(),
        "nothing read, nothing filed"
    );

    let runs = [run(RUN), run(RUN_C), run(RUN)];
    let opened = content::open(
        &door.state,
        Actor::User("user-1"),
        &door.project,
        &runs,
        None,
    )
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

    let elsewhere = ["00f067aa0ba902ff".to_owned()];
    let none = content::open(
        &door.state,
        Actor::User("user-1"),
        &door.project,
        &runs,
        Some(&elsewhere),
    )
    .await
    .expect("a span the run does not hold opens to nothing");
    assert!(none.is_empty());
    let named = [SPAN.to_owned()];
    let only = content::open(
        &door.state,
        Actor::User("user-1"),
        &door.project,
        &runs,
        Some(&named),
    )
    .await
    .expect("the span named opens");
    assert_eq!(only.len(), 1, "the span named, alone");
    assert_eq!(
        door.viewed(&pool).await.len(),
        2,
        "each read filed once, none for nothing"
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
    let read = || {
        content::open(
            &door.state,
            Actor::User("user-1"),
            &door.project,
            &runs,
            None,
        )
    };

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

/// The Privacy page counts what spans arrived with by the inventory's rows,
/// the content an `off` project dropped among them, to any role on it.
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
    let open = || {
        content::open(
            &door.state,
            Actor::User("owner-1"),
            &door.project,
            &runs,
            None,
        )
    };
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
        None,
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
        (format!("/users?user={USER}"), StatusCode::OK),
        (
            format!("/traces/export?tag={tag}&format=json"),
            StatusCode::OK,
        ),
        (
            format!("/users/export?user={USER}&format=json"),
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

/// ⚠ Content searched: the trace list narrows to the runs whose kept content
/// holds every word, a phrase in order; a search files no read, finds nothing
/// on a project that keeps none, and nothing once the content key is deleted.
#[sqlx::test]
async fn the_trace_list_finds_runs_by_their_kept_content(pool: PgPool) {
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
        &[("app.note", "Where is my refund?")],
        "result",
    ))
    .await;
    door.send(export_of(
        RUN_B,
        TRACE_B,
        SPAN_B,
        &[("app.note", "My refund: where is it")],
        "result",
    ))
    .await;
    common::eventually("both traces are read", || async {
        found(&door, RUN).await && found(&door, RUN_B).await
    })
    .await;
    let runs = |q: &'static str| {
        let door = &door;
        async move {
            let path = format!("/traces?q={}", q.replace(' ', "%20"));
            let (status, list) = door.lane("GET", &path, "admin-1", None).await;
            assert_eq!(status, StatusCode::OK, "{q}: {list}");
            let mut runs: Vec<String> = list["traces"]
                .as_array()
                .expect("a list")
                .iter()
                .map(|t| t["run_id"].as_str().expect("a run").to_owned())
                .collect();
            runs.sort();
            runs
        }
    };

    assert_eq!(
        runs("REFUND").await,
        [RUN, RUN_B],
        "a word, whatever its case"
    );
    assert_eq!(runs("where is my").await, [RUN], "a phrase, in order");
    assert!(runs("refunds").await.is_empty(), "a whole word, not a part");
    assert!(runs("refund missing").await.is_empty(), "every word");
    assert!(
        door.viewed(&pool).await.is_empty(),
        "a search shows no content, and files none"
    );

    let other = door_over(&pool, ContentMode::Off, true).await;
    other.auth.seat(
        "admin-1",
        &other.organization,
        &other.project,
        OrganizationRole::Owner,
        None,
    );
    let (status, list) = other.lane("GET", "/traces?q=refund", "admin-1", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        list["traces"],
        json!([]),
        "a project that keeps none finds none"
    );

    sqlx::query(
        "UPDATE telemetry.project_settings \
            SET content_key_wrapped_dek = NULL, content_key_version = NULL \
          WHERE project_id = $1",
    )
    .bind(&door.project)
    .execute(&pool)
    .await
    .expect("delete the content key");
    assert!(
        runs("refund").await.is_empty(),
        "nothing matches once the key goes"
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

/// The trace's page carries each span's content and files the read; the
/// session's carries each run's own turn alone, filed as its own read; a
/// project that keeps none answers an empty list and files nothing.
#[sqlx::test]
async fn the_trace_and_session_pages_carry_content_each_read_filed(pool: PgPool) {
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

    let (status, trace) = kept
        .lane("GET", &format!("/traces/{RUN}"), "admin-1", None)
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

    let (status, session) = kept.lane("GET", "/sessions/conv-1", "admin-1", None).await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(
        session["content"],
        json!([{ "run_id": RUN, "span_id": SPAN, "fields": {
            "gen_ai.input.messages": MESSAGES,
            "gen_ai.output.messages": ANSWER,
        }}]),
        "the run's own turn, not its model call's context"
    );
    let viewed = kept.viewed(&pool).await;
    assert_eq!(viewed.len(), 2, "the session's read, filed");
    assert_eq!(viewed[1].4, json!({ "trace_ids": [TRACE], "fields": 2 }));

    let (status, file) = kept
        .lane(
            "GET",
            &format!("/traces/export?format=json&run={RUN}"),
            "admin-1",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file[0]["run_id"], RUN);
    assert_eq!(
        file[0]["content"], trace["content"],
        "the page's content, in the file"
    );
    assert_eq!(kept.viewed(&pool).await.len(), 3, "the download, one read");

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
        .lane("GET", &format!("/traces/{RUN}"), "admin-1", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{trace}");
    assert_eq!(trace["content"], json!([]), "none kept, none shown");
    assert!(
        dropped.viewed(&pool).await.is_empty(),
        "nothing read, nothing filed"
    );
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
        None,
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
    assert_eq!(
        door.held(&pool, &door.organization).await,
        Some(before - i64::try_from("result".len()).expect("small"))
    );
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
    let other =
        i64::try_from("other".len() + r#"{"id":42}"#.len() + MESSAGES.len()).expect("small");
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
