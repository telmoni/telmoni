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
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    ApiKeyScope, ContentMode, OrganizationId, OrganizationRole, ProjectId, Role, TelmoniError,
};
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
        app: router(state.clone()).nest("/otlp", otlp_router(state.clone())),
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
            run_id: RUN.parse().expect("a run's id"),
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

/// Whether the trace's lane still finds run `run`.
async fn found(door: &Door, run: &str) -> bool {
    door.lane("GET", &format!("/traces/{run}"), "admin-1", None)
        .await
        .0
        == StatusCode::OK
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
