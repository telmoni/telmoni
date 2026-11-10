//! Monitors through the module's own router and its loop, on a clock set by
//! hand: any member makes, edits, pauses and deletes one, each change filed;
//! and the loop judges each over the run starts and ends ClickHouse holds —
//! a run on time, one late, one down and recovered, a failed run, a clock
//! change in London, and a paused monitor told nothing.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, ClickHouse, NoticeLog, SECRET, clickhouse_or_skip};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind, SpanStatus,
};
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::{AppState, monitors, router};

const LANE: &str = "/internal/v1/telemetry/monitors";
const AGENT: &str = "nightly-triage";

fn at(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .expect("a time")
        .with_timezone(&Utc)
}

/// A request from `user` on `project` of `organization`, as the console sends it.
fn request(
    method: &str,
    uri: &str,
    user: &str,
    (organization, project): (&OrganizationId, &ProjectId),
    body: Option<&Value>,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(SERVICE_SECRET_HEADER, SECRET)
        .header("authorization", format!("Bearer tok_{user}"))
        .header("x-organization-id", organization.as_str())
        .header("x-project-id", project.as_str())
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .expect("a request")
}

async fn answer(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON body")
    };
    (status, body)
}

/// One organization, one project, and a member seated on it.
fn seated() -> (Arc<AuthStub>, OrganizationId, ProjectId) {
    let auth = AuthStub::new();
    let organization = OrganizationId::new();
    let project = ProjectId::new();
    auth.seat(
        "user_member",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    (auth, organization, project)
}

/// A span of the agent's run `run`, of `kind`, started and received at
/// `started`, ending at `ended`.
fn of_run(
    project: &ProjectId,
    run: Uuid,
    kind: SpanKind,
    started: DateTime<Utc>,
    ended: DateTime<Utc>,
    status: SpanStatus,
) -> SpanRow {
    SpanRow {
        trace_id: run.simple().to_string(),
        agent: AGENT.into(),
        span_id: format!("{:016x}", started.timestamp_micros()),
        started_at: started,
        ended_at: ended,
        status,
        units: 0,
        ..common::received(project, kind, AGENT, 0, ended)
    }
}

/// A trace's start, sent as it opens.
fn opening(project: &ProjectId, started: DateTime<Utc>) -> SpanRow {
    of_run(
        project,
        Uuid::new_v4(),
        SpanKind::TraceStart,
        started,
        started,
        SpanStatus::Ok,
    )
}

/// A monitor of the agent made through the lane: its id.
async fn made(app: &axum::Router, of: (&OrganizationId, &ProjectId), body: &Value) -> String {
    let (status, made) = answer(app, request("POST", LANE, "user_member", of, Some(body))).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    made.get("id")
        .and_then(Value::as_str)
        .expect("an id")
        .to_owned()
}

/// The monitor as the lane answers it.
async fn read(app: &axum::Router, of: (&OrganizationId, &ProjectId), id: &str) -> Value {
    let (status, monitor) = answer(
        app,
        request("GET", &format!("{LANE}/{id}"), "user_member", of, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{monitor}");
    monitor
}

/// The monitor's incidents: severity, status and cause, oldest first.
async fn incidents(pool: &PgPool, id: &str) -> Vec<(String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT severity, status, cause FROM telemetry.incidents
          WHERE monitor_id = $1::uuid ORDER BY opened_at",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .expect("its incidents")
}

/// A state whose ticks read the suite's ClickHouse, and the notices it told.
async fn watched(
    pool: &PgPool,
    test: &str,
) -> Option<(
    ClickHouse,
    Arc<AppState>,
    Arc<NoticeLog>,
    OrganizationId,
    ProjectId,
)> {
    apply_audit_migrations(pool).await;
    let ch = clickhouse_or_skip(test).await?;
    let (auth, organization, project) = seated();
    let notices = NoticeLog::new();
    let state = common::state_with_console(pool, auth, notices.clone());
    Some((ch, state, notices, organization, project))
}

async fn tick(state: &AppState, now: &str) {
    monitors::tick_at(state, at(now)).await.expect("a tick");
}

/// ⚠ Any member makes, reads, edits, pauses and deletes a monitor, each
/// change filed; what cannot be one is refused naming its field, a second
/// of one agent is a conflict, and someone with no seat reaches none of it.
#[sqlx::test]
async fn a_member_makes_edits_pauses_and_deletes_a_monitor(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project) = seated();
    let app = router(common::state(&pool, auth));
    let of = (&organization, &project);
    let nightly = json!({ "agent": AGENT, "cron": "0 2 * * *", "time_zone": "Europe/London",
        "grace_seconds": 1800 });
    let id = made(&app, of, &nightly).await;
    let monitor = read(&app, of, &id).await;
    assert_eq!(monitor["state"], "new");
    assert_eq!(monitor["next_due_at"], Value::Null);

    let (status, _) = answer(
        &app,
        request("POST", LANE, "user_member", of, Some(&nightly)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "one monitor an agent");
    for (bad, field) in [
        (
            json!({ "agent": "a", "every_seconds": 30, "grace_seconds": 60 }),
            "every_seconds",
        ),
        (
            json!({ "agent": "a", "cron": "0 0 30 2 *", "time_zone": "UTC", "grace_seconds": 60 }),
            "cron",
        ),
        (
            json!({ "agent": "a", "cron": "0 2 * * *", "time_zone": "london", "grace_seconds": 60 }),
            "time_zone",
        ),
        (
            json!({ "agent": "a", "every_seconds": 60, "grace_seconds": 59 }),
            "grace_seconds",
        ),
        (
            json!({ "agent": "a", "every_seconds": 60, "cron": "0 2 * * *", "time_zone": "UTC",
            "grace_seconds": 60 }),
            "schedule",
        ),
    ] {
        let (status, refused) =
            answer(&app, request("POST", LANE, "user_member", of, Some(&bad))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert!(
            refused.to_string().contains(field),
            "{refused} names {field}"
        );
    }

    let one = format!("{LANE}/{id}");
    let edited = json!({ "agent": AGENT, "every_seconds": 3600, "grace_seconds": 600 });
    let (status, saved) =
        answer(&app, request("PUT", &one, "user_member", of, Some(&edited))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["every_seconds"], 3600);
    let elsewhere = json!({ "agent": "another", "every_seconds": 3600, "grace_seconds": 600 });
    let (status, _) = answer(
        &app,
        request("PUT", &one, "user_member", of, Some(&elsewhere)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a monitor stays its agent's"
    );

    let (_, paused) = answer(
        &app,
        request("POST", &format!("{one}/pause"), "user_member", of, None),
    )
    .await;
    assert_eq!(paused["state"], "paused");
    let (_, resumed) = answer(
        &app,
        request("POST", &format!("{one}/resume"), "user_member", of, None),
    )
    .await;
    assert_eq!(resumed["state"], "new");

    let stranger = (&organization, &ProjectId::new());
    let (status, _) = answer(&app, request("GET", LANE, "user_member", stranger, None)).await;
    assert!(status.is_client_error(), "no seat, no monitors: {status}");

    let (status, _) = answer(&app, request("DELETE", &one, "user_member", of, None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = answer(&app, request("GET", &one, "user_member", of, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let filed: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'monitor' ORDER BY seq",
    )
    .bind(organization.as_str())
    .fetch_all(&pool)
    .await
    .expect("the chain");
    assert_eq!(
        filed,
        ["created", "updated", "updated", "updated", "deleted"]
    );
}

/// ⚠ A nightly run on time keeps it up; none a minute past due is late, past
/// its grace down, and the next start brings it up: one incident, opened at
/// warning, raised to alert and resolved, each told once.
#[sqlx::test]
async fn a_run_on_time_one_late_and_one_down_and_recovered(pool: PgPool) {
    let Some((ch, state, notices, organization, project)) = watched(&pool, module_path!()).await
    else {
        return;
    };
    let app = router(state.clone());
    let of = (&organization, &project);
    let id = made(
        &app,
        of,
        &json!({ "agent": AGENT, "cron": "0 2 * * *", "time_zone": "UTC",
        "grace_seconds": 1800 }),
    )
    .await;

    ch.store
        .insert_spans(&[opening(&project, at("2026-11-10T02:00:02Z"))])
        .await
        .expect("kept");
    tick(&state, "2026-11-10T02:00:10Z").await;
    let monitor = read(&app, of, &id).await;
    assert_eq!(monitor["state"], "up", "{monitor}");
    assert_eq!(monitor["next_due_at"], "2026-11-11T02:00:00Z");

    tick(&state, "2026-11-11T02:00:30Z").await;
    assert_eq!(
        read(&app, of, &id).await["state"],
        "up",
        "a start may be on its way"
    );
    tick(&state, "2026-11-11T02:01:00Z").await;
    assert_eq!(read(&app, of, &id).await["state"], "late");
    tick(&state, "2026-11-11T02:30:00Z").await;
    let down = read(&app, of, &id).await;
    assert_eq!(
        (down["state"].clone(), down["cause"].clone()),
        (json!("down"), json!("missed"))
    );

    ch.store
        .insert_spans(&[opening(&project, at("2026-11-11T03:00:00Z"))])
        .await
        .expect("kept");
    tick(&state, "2026-11-11T03:00:30Z").await;
    let up = read(&app, of, &id).await;
    assert_eq!(up["state"], "up");
    assert_eq!(up["next_due_at"], "2026-11-12T02:00:00Z");

    assert_eq!(
        incidents(&pool, &id).await,
        [("alert".into(), "resolved".into(), Some("missed".into()))]
    );
    let kinds: Vec<(NotificationKind, bool)> = notices
        .notices()
        .iter()
        .map(|notice| (notice.kind, notice.reminded))
        .collect();
    assert_eq!(
        kinds,
        [
            (NotificationKind::IncidentOpened, false),
            (NotificationKind::IncidentOpened, true),
            (NotificationKind::IncidentResolved, false),
        ],
        "late opens it, down is said again, a start resolves it"
    );
}

/// ⚠ A run that ends in error makes it down, naming the run; a start alone
/// leaves it down, and a run that ends well brings it up.
#[sqlx::test]
async fn a_failed_run_is_down_until_one_ends_well(pool: PgPool) {
    let Some((ch, state, notices, organization, project)) = watched(&pool, module_path!()).await
    else {
        return;
    };
    let app = router(state.clone());
    let of = (&organization, &project);
    let id = made(
        &app,
        of,
        &json!({ "agent": AGENT, "every_seconds": 3600,
        "grace_seconds": 600 }),
    )
    .await;
    let failed = Uuid::new_v4();
    ch.store
        .insert_spans(&[of_run(
            &project,
            failed,
            SpanKind::Agent,
            at("2026-11-10T09:00:00Z"),
            at("2026-11-10T09:00:40Z"),
            SpanStatus::Error,
        )])
        .await
        .expect("kept");
    tick(&state, "2026-11-10T09:01:00Z").await;
    let down = read(&app, of, &id).await;
    assert_eq!(
        (down["state"].clone(), down["cause"].clone()),
        (json!("down"), json!("failed"))
    );
    let opened = notices.notices().into_iter().next().expect("told");
    assert_eq!(
        opened.metadata["trace"],
        json!(failed.simple().to_string()),
        "the notice names the trace"
    );

    ch.store
        .insert_spans(&[opening(&project, at("2026-11-10T10:00:00Z"))])
        .await
        .expect("kept");
    tick(&state, "2026-11-10T10:00:20Z").await;
    assert_eq!(
        read(&app, of, &id).await["state"],
        "down",
        "a start ends no failure"
    );

    ch.store
        .insert_spans(&[of_run(
            &project,
            Uuid::new_v4(),
            SpanKind::Agent,
            at("2026-11-10T10:00:00Z"),
            at("2026-11-10T10:00:50Z"),
            SpanStatus::Ok,
        )])
        .await
        .expect("kept");
    tick(&state, "2026-11-10T10:01:00Z").await;
    assert_eq!(read(&app, of, &id).await["state"], "up");
    assert_eq!(
        incidents(&pool, &id).await,
        [("alert".into(), "resolved".into(), Some("failed".into()))]
    );
}

/// ⚠ At 01:30 in London across the autumn change, the repeated 01:30 is not
/// due again: the run of the first is the night's.
#[sqlx::test]
async fn a_clock_change_in_london_is_due_once(pool: PgPool) {
    let Some((ch, state, _, organization, project)) = watched(&pool, module_path!()).await else {
        return;
    };
    let app = router(state.clone());
    let of = (&organization, &project);
    let id = made(
        &app,
        of,
        &json!({ "agent": AGENT, "cron": "30 1 * * *",
        "time_zone": "Europe/London", "grace_seconds": 600 }),
    )
    .await;
    ch.store
        .insert_spans(&[opening(&project, at("2026-10-25T00:30:05Z"))])
        .await
        .expect("kept");
    tick(&state, "2026-10-25T00:30:30Z").await;
    assert_eq!(
        read(&app, of, &id).await["next_due_at"],
        "2026-10-26T01:30:00Z"
    );
    tick(&state, "2026-10-25T01:31:00Z").await;
    assert_eq!(
        read(&app, of, &id).await["state"],
        "up",
        "01:30 GMT is not due again"
    );
}

/// ⚠ A paused monitor is judged no more and told nothing, however long its
/// agent stays away.
#[sqlx::test]
async fn a_paused_monitor_is_told_nothing(pool: PgPool) {
    let Some((ch, state, notices, organization, project)) = watched(&pool, module_path!()).await
    else {
        return;
    };
    let app = router(state.clone());
    let of = (&organization, &project);
    let id = made(
        &app,
        of,
        &json!({ "agent": AGENT, "every_seconds": 600,
        "grace_seconds": 60 }),
    )
    .await;
    ch.store
        .insert_spans(&[opening(&project, at("2026-11-10T09:00:00Z"))])
        .await
        .expect("kept");
    tick(&state, "2026-11-10T09:00:30Z").await;
    let (_, paused) = answer(
        &app,
        request(
            "POST",
            &format!("{LANE}/{id}/pause"),
            "user_member",
            of,
            None,
        ),
    )
    .await;
    assert_eq!(paused["state"], "paused");
    tick(&state, "2026-11-10T12:00:00Z").await;
    assert_eq!(read(&app, of, &id).await["state"], "paused");
    assert!(incidents(&pool, &id).await.is_empty());
    assert!(notices.notices().is_empty());
}
