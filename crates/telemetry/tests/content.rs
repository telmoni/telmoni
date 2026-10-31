//! Content through the door: a project whose mode is `on` keeps each content
//! field, and each text no inventory names, whole and sealed under its own
//! key in `plain_content`, never in ClickHouse, its bytes on its
//! organization's row of `content_held`, counted once however often an export
//! is sent; a project left `off` keeps none and makes no key. A purge, a
//! transfer and the nightly purge each keep the count true.
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
use telmoni_shared::seam::Telemetry as _;
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{ApiKeyScope, ContentMode, OrganizationId, ProjectId, TelmoniError};
use telmoni_telemetry::ingest::batch::SpanContent;
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::Projects;
use telmoni_telemetry::{AppState, content, contract, otlp_router, retention, router};

const KEY: &str = "tm_content_key";
const RUN: &str = "0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01";
const SPAN: &str = "00f067aa0ba902b7";
const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const MESSAGES: &str = r#"[{"role":"user","parts":[{"type":"text","content":"hi"}]}]"#;

struct Door {
    app: Router,
    state: Arc<AppState>,
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
        common::state_with_stores(pool, auth)
    } else {
        common::state(pool, auth)
    };
    Door {
        app: router(state.clone()).nest("/otlp", otlp_router(state.clone())),
        state,
        organization,
        project,
    }
}

/// A model call carrying a message, a tool's result past an attribute's cap,
/// a map no inventory names, and its kept metadata.
fn export(long: &str) -> Vec<u8> {
    json!({
        "resourceSpans": [{
            "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [{
                "traceId": TRACE,
                "spanId": SPAN,
                "name": "chat claude-sonnet-5-5",
                "startTimeUnixNano": "1760000000000000000",
                "endTimeUnixNano": "1760000001500000000",
                "attributes": [
                    { "key": contract::RUN_ID, "value": { "stringValue": RUN } },
                    { "key": contract::AGENT, "value": { "stringValue": "nightly-report" } },
                    { "key": contract::OPERATION, "value": { "stringValue": "chat" } },
                    { "key": "gen_ai.response.id", "value": { "stringValue": "resp_01" } },
                    { "key": "gen_ai.input.messages", "value": { "stringValue": MESSAGES } },
                    { "key": "gen_ai.tool.call.result", "value": { "stringValue": long } },
                    { "key": "app.ticket", "value": { "kvlistValue": { "values": [
                        { "key": "id", "value": { "intValue": "42" } }
                    ]}}}
                ]
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
        &["0af7651916cd43dd8448eb211c80319c".to_owned()],
    )
    .await
    .expect("a trace without content opens to nothing");
    assert!(nothing.is_empty());
    assert!(
        door.viewed(&pool).await.is_empty(),
        "nothing read, nothing filed"
    );

    let traces = [
        TRACE.to_owned(),
        "0af7651916cd43dd8448eb211c80319c".to_owned(),
        TRACE.to_owned(),
    ];
    let opened = content::open(&door.state, Actor::User("user-1"), &door.project, &traces)
        .await
        .expect("the trace's content opens");
    assert_eq!(
        opened,
        [SpanContent {
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
    let traces = [TRACE.to_owned()];
    let read = || content::open(&door.state, Actor::User("user-1"), &door.project, &traces);

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

    seam.purge_project(&door.project).await.expect("the purge");
    assert!(door.kept(&pool).await.is_empty());
    assert_eq!(door.held(&pool, &taker).await, Some(0));
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

    let gone = retention::purge_content(&pool, Utc::now())
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
