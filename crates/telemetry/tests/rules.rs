//! Alerts through the module's own router and its loop: any member of the
//! project makes, reads, edits and deletes one, each checked whole; one past
//! its organization's plan, paused ones counted, is refused as a 402 in the
//! plan's words; and the loop evaluates each over the spans ClickHouse holds,
//! the full hours of its window from the hourly totals and the rest from the
//! spans, holding the organization's alerts to its plan as it goes.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, DurationRound as _, TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET, clickhouse_or_skip};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    AlertCeilings, Grants, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind, SpanStatus,
    TelmoniError,
};
use telmoni_telemetry::db::{self as module_db, TelemetryLane};
use telmoni_telemetry::rules::db::{self as rules_db, RuleRow};
use telmoni_telemetry::rules::{evaluate, incidents};
use telmoni_telemetry::{AppState, router};

const LANE: &str = "/internal/v1/telemetry/rules";

/// A plan of `alerts` alerts for every organization.
struct Holds(u32);

#[async_trait]
impl Entitlements for Holds {
    async fn grants(&self, _organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        Ok(Grants::unlimited())
    }

    async fn alert_ceilings(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<Option<AlertCeilings>, TelmoniError> {
        Ok(Some(AlertCeilings {
            alerts: self.0,
            deliveries_per_day: 500,
            entitlement: "pro".into(),
            detail: "This organization's plan holds two alerts.".into(),
        }))
    }
}

/// A request from `user` on `project` of `organization`, as the console sends it.
fn request(
    method: &str,
    uri: &str,
    user: &str,
    organization: &OrganizationId,
    project: &ProjectId,
    body: Option<&Value>,
) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(SERVICE_SECRET_HEADER, SECRET)
        .header("authorization", format!("Bearer tok_{user}"))
        .header("x-organization-id", organization.as_str())
        .header("x-project-id", project.as_str())
        .header("content-type", "application/json");
    builder
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

/// An alert on the p95 of model calls' durations over the last hour.
fn slow_calls(name: &str) -> Value {
    json!({
        "name": name,
        "kind": "model_call",
        "measure": "duration",
        "aggregation": "p95",
        "filters": { "model": ["claude-sonnet-5-5"] },
        "window": "hour",
        "operator": "gt",
        "alert_threshold": 5000,
        "warning_threshold": 3000
    })
}

/// One organization, two of its projects, and a member seated on each.
fn seated() -> (Arc<AuthStub>, OrganizationId, ProjectId, ProjectId) {
    let auth = AuthStub::new();
    let organization = OrganizationId::new();
    let (first, second) = (ProjectId::new(), ProjectId::new());
    for project in [&first, &second] {
        auth.seat(
            "user_member",
            &organization,
            project,
            OrganizationRole::Member,
            Some(Role::Member),
        );
    }
    (auth, organization, first, second)
}

/// ⚠ Any member makes, reads, edits and deletes an alert; someone with no
/// seat on the project reaches none of it.
#[sqlx::test]
async fn a_member_makes_reads_edits_and_deletes_an_alert(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let app = router(common::state(&pool, auth.clone()));
    let member = |method: &str, uri: &str, body: Option<&Value>| {
        request(method, uri, "user_member", &organization, &project, body)
    };

    let (status, made) = answer(&app, member("POST", LANE, Some(&slow_calls("Slow calls")))).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["window"], "hour");
    assert_eq!(made["paused"], Value::Null);
    assert_eq!(made["value"], Value::Null, "evaluated by the loop alone");
    let id = made["id"].as_str().expect("an id").to_owned();

    let (status, again) = answer(&app, member("POST", LANE, Some(&slow_calls("slow CALLS")))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");

    let (status, listed) = answer(&app, member("GET", LANE, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["rules"].as_array().map(Vec::len), Some(1));

    let mut edited = slow_calls("Slower calls");
    edited["alert_threshold"] = json!(8000);
    let (status, saved) = answer(&app, member("PUT", &format!("{LANE}/{id}"), Some(&edited))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["name"], "Slower calls");
    assert_eq!(saved["alert_threshold"], 8000.0);

    let stranger = request("GET", LANE, "user_nobody", &organization, &project, None);
    let (status, _) = answer(&app, stranger).await;
    assert!(status.is_client_error(), "{status}");

    let (status, _) = answer(&app, member("DELETE", &format!("{LANE}/{id}"), None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = answer(&app, member("GET", &format!("{LANE}/{id}"), None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// What cannot be an alert is refused before anything is written.
#[sqlx::test]
async fn an_alert_that_reads_two_ways_is_refused(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let app = router(common::state(&pool, auth));
    for (field, value) in [
        ("aggregation", json!("count")),
        ("warning_threshold", json!(6000)),
        ("kind", json!("span")),
        ("window", json!("month")),
        ("renotify_minutes", json!(0)),
    ] {
        let mut body = slow_calls("Refused");
        body[field] = value;
        let (status, refused) = answer(
            &app,
            request(
                "POST",
                LANE,
                "user_member",
                &organization,
                &project,
                Some(&body),
            ),
        )
        .await;
        assert!(status.is_client_error(), "{field}: {status} {refused}");
    }
    let cap = json!({
        "name": "Cost cap", "kind": "run", "measure": "cost", "aggregation": "max",
        "window": "day", "operator": "gt", "alert_threshold": 5
    });
    for (field, value) in [
        ("kind", json!("model_call")),
        ("aggregation", json!("p95")),
        ("filters", json!({ "status": ["error"] })),
    ] {
        let mut body = cap.clone();
        body[field] = value;
        let (status, refused) = answer(
            &app,
            request(
                "POST",
                LANE,
                "user_member",
                &organization,
                &project,
                Some(&body),
            ),
        )
        .await;
        assert!(status.is_client_error(), "{field}: {status} {refused}");
    }
    let (status, made) = answer(
        &app,
        request(
            "POST",
            LANE,
            "user_member",
            &organization,
            &project,
            Some(&cap),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the cap itself is an alert: {made}"
    );
}

/// ⚠ The plan's number holds across the organization's projects: the
/// third alert of two is refused in the plan's words, wherever it is made.
#[sqlx::test]
async fn an_alert_past_the_plans_number_is_refused(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, first, second) = seated();
    let app = router(common::state_with_entitlements(
        &pool,
        auth,
        Arc::new(Holds(2)),
    ));
    for (n, project) in [&first, &second].into_iter().enumerate() {
        let (status, made) = answer(
            &app,
            request(
                "POST",
                LANE,
                "user_member",
                &organization,
                project,
                Some(&slow_calls(&format!("Alert {n}"))),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
    }
    let (status, refused) = answer(
        &app,
        request(
            "POST",
            LANE,
            "user_member",
            &organization,
            &first,
            Some(&slow_calls("Alert 3")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{refused}");
    assert_eq!(refused["type"], "/errors/plan/limit-reached");
    assert_eq!(
        refused["detail"],
        "This organization's plan holds two alerts."
    );
}

/// ⚠ The loop evaluates each alert over the spans ClickHouse holds: the full
/// hours of a day from the hourly totals and the rest from the spans, both
/// read once each; and holds the organization to its plan, the oldest
/// running and the newest paused by the plan until there is room again.
#[sqlx::test]
async fn the_loop_evaluates_each_alert_and_holds_its_organization_to_its_plan(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (auth, organization, project, _) = seated();
    let state = common::state_with_stores_under(&pool, auth, Arc::new(NoLimits));
    let app = router(state.clone());

    // Four model calls in the day: three in an hour the totals hold whole,
    // one in this hour, which the spans alone answer.
    let now = Utc::now();
    let earlier = now.duration_trunc(TimeDelta::hours(1)).expect("an hour") - TimeDelta::hours(3);
    let mut rows = Vec::new();
    for (n, millis, status) in [
        (1u64, 1_000, SpanStatus::Ok),
        (2, 2_000, SpanStatus::Ok),
        (3, 3_000, SpanStatus::Error),
    ] {
        rows.push(telmoni_telemetry::store::SpanRow {
            span_id: format!("{n:016x}"),
            model: "claude-sonnet-5-5".into(),
            input_tokens: 100,
            output_tokens: 50,
            status,
            ..common::received(
                &project,
                SpanKind::ModelCall,
                "chat claude-sonnet-5-5",
                millis,
                earlier + TimeDelta::minutes(5),
            )
        });
    }
    rows.push(telmoni_telemetry::store::SpanRow {
        span_id: format!("{:016x}", 4),
        model: "claude-sonnet-5-5".into(),
        input_tokens: 100,
        output_tokens: 50,
        ..common::received(
            &project,
            SpanKind::ModelCall,
            "chat claude-sonnet-5-5",
            4_000,
            now - TimeDelta::seconds(2),
        )
    });
    ch.store
        .insert_spans(&rows)
        .await
        .expect("the spans are kept");

    let make = |name: &str, body: Value| {
        let mut body = body;
        body["name"] = json!(name);
        request(
            "POST",
            LANE,
            "user_member",
            &organization,
            &project,
            Some(&body),
        )
    };
    let day = json!({
        "kind": "model_call", "measure": "duration", "aggregation": "avg",
        "window": "day", "operator": "gt", "alert_threshold": 1
    });
    let mut failures = day.clone();
    failures["measure"] = json!("failures");
    let mut tokens = day.clone();
    tokens["measure"] = json!("tokens");
    tokens["aggregation"] = json!("sum");
    let mut slowest = day.clone();
    slowest["aggregation"] = json!("p95");
    let mut hour = day.clone();
    hour["window"] = json!("hour");
    hour["measure"] = json!("count");
    hour["aggregation"] = json!("count");
    for (name, body) in [
        ("A day's durations", day),
        ("A day's failure rate", failures),
        ("A day's tokens", tokens),
        ("A day's slowest calls", slowest),
        ("An hour's calls", hour),
    ] {
        let (status, made) = answer(&app, make(name, body)).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
    }

    let settled = evaluate::pass(&state).await.expect("a pass");
    assert_eq!(settled, 5);
    let (_, listed) = answer(
        &app,
        request("GET", LANE, "user_member", &organization, &project, None),
    )
    .await;
    let value = |name: &str| {
        listed["rules"]
            .as_array()
            .and_then(|rules| rules.iter().find(|r| r["name"] == name))
            .map(|r| r["value"].clone())
            .expect("the alert")
    };
    assert_eq!(value("A day's durations"), json!(2500.0), "{listed}");
    assert_eq!(value("A day's failure rate"), json!(0.25));
    assert_eq!(value("A day's tokens"), json!(600.0));
    assert_eq!(
        value("An hour's calls"),
        json!(1.0),
        "this hour's span alone"
    );
    let p95 = value("A day's slowest calls")
        .as_f64()
        .expect("a percentile");
    assert!(
        (3_000.0..=4_000.0).contains(&p95),
        "the t-digests of both sources merged: {p95}"
    );
    assert_eq!(
        evaluate::pass(&state).await.expect("a pass"),
        0,
        "none is due again within its minute"
    );

    // Its plan falls to two: the two oldest run, the rest paused by it.
    let held = common::state_with_stores_under(&pool, AuthStub::new(), Arc::new(Holds(2)));
    sqlx::query("UPDATE telemetry.rules SET evaluated_at = NULL")
        .execute(&pool)
        .await
        .expect("due again");
    evaluate::pass(&held).await.expect("a pass");
    let paused: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT name, paused FROM telemetry.rules ORDER BY created_at, id")
            .fetch_all(&pool)
            .await
            .expect("the alerts");
    assert_eq!(
        paused.iter().map(|(_, p)| p.as_deref()).collect::<Vec<_>>(),
        [None, None, Some("plan"), Some("plan"), Some("plan")],
        "{paused:?}"
    );

    // Room again: what the plan paused runs again.
    let freed = common::state_with_stores_under(&pool, AuthStub::new(), Arc::new(NoLimits));
    evaluate::pass(&freed).await.expect("a pass");
    let still: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry.rules WHERE paused IS NOT NULL")
            .fetch_one(&pool)
            .await
            .expect("a count");
    assert_eq!(still, 0);
}

/// A plan that cannot answer its alerts' number, as billing during an outage.
struct Unanswered;

#[async_trait]
impl Entitlements for Unanswered {
    async fn grants(&self, _organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        Ok(Grants::unlimited())
    }

    async fn alert_ceilings(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<Option<AlertCeilings>, TelmoniError> {
        Err(TelmoniError::Internal("billing cannot be reached".into()))
    }
}

/// An alert made through the lane, by its id.
async fn made(app: &axum::Router, organization: &OrganizationId, project: &ProjectId) -> Uuid {
    let body = slow_calls("Slow calls");
    let made = request(
        "POST",
        LANE,
        "user_member",
        organization,
        project,
        Some(&body),
    );
    let (status, made) = answer(app, made).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    made.get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse().ok())
        .expect("an id")
}

/// The alert's lease as its row holds it.
async fn lease_of(pool: &PgPool, id: Uuid) -> (Option<Uuid>, Option<DateTime<Utc>>) {
    sqlx::query_as("SELECT lease_id, leased_until FROM telemetry.rules WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the alert")
}

/// The alert leased as a pass leases it.
async fn leased(state: &AppState, id: Uuid) -> (Uuid, RuleRow) {
    let lease_id = Uuid::new_v4();
    let mut tx = maintenance_scope(&state.db, TelemetryLane)
        .await
        .expect("the lane");
    let rows = rules_db::lease_due(&mut tx, 60, lease_id, 120, 100)
        .await
        .expect("leased");
    tx.commit().await.expect("commit");
    let rule = rows
        .into_iter()
        .find(|rule| rule.id == id)
        .expect("the alert leased");
    (lease_id, rule)
}

async fn incidents_kept(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM telemetry.incidents")
        .fetch_one(pool)
        .await
        .expect("a count")
}

/// ⚠ An alert whose window cannot be read waits for the next pass, not this one.
#[sqlx::test]
async fn an_alert_whose_window_is_unread_waits_for_the_next_pass(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let state = common::state(&pool, auth);
    let id = made(&router(state.clone()), &organization, &project).await;

    assert_eq!(evaluate::pass(&state).await.expect("a pass"), 0);
    let (lease, until) = lease_of(&pool, id).await;
    assert!(lease.is_some());
    let until = until.expect("held until the next pass");
    assert!(until > Utc::now() + TimeDelta::seconds(30));
    evaluate::pass(&state).await.expect("a second pass");
    assert_eq!(lease_of(&pool, id).await.1, Some(until), "not leased again");
}

/// ⚠ A pass lets go of the alerts it cannot reach within its lease, unread.
#[sqlx::test]
async fn a_pass_lets_go_of_what_it_cannot_reach_within_its_lease(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let state = common::state(&pool, auth);
    let id = made(&router(state.clone()), &organization, &project).await;

    let settled = evaluate::pass_within(&state, Duration::ZERO)
        .await
        .expect("a pass");
    assert_eq!(settled, 0);
    assert_eq!(lease_of(&pool, id).await, (None, None), "free for another");
}

/// ⚠ A replica asked to stop starts no alert.
#[sqlx::test]
async fn a_stopping_replica_starts_no_alert(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let state = common::state(&pool, auth);
    let id = made(&router(state.clone()), &organization, &project).await;

    state.alerts_stopping.ask();
    assert_eq!(evaluate::pass(&state).await.expect("a pass"), 0);
    assert_eq!(lease_of(&pool, id).await, (None, None));
}

/// ⚠ A pause by the plan ends an evaluation in flight on another replica.
#[sqlx::test]
async fn a_pause_by_the_plan_ends_an_evaluation_in_flight(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let state = common::state(&pool, auth);
    let id = made(&router(state.clone()), &organization, &project).await;
    let (lease_id, rule) = leased(&state, id).await;

    let mut tx = maintenance_scope(&state.db, TelemetryLane)
        .await
        .expect("the lane");
    let held = rules_db::hold_to(&mut tx, &organization, Some(0))
        .await
        .expect("held");
    tx.commit().await.expect("commit");
    assert_eq!(held, 1);
    let (settled, told) = incidents::settle(&state, &rule, lease_id, Some(6000.0), Utc::now())
        .await
        .expect("a settle");
    assert!(!settled && told.is_empty(), "the plan's pause stands");
    assert_eq!(incidents_kept(&pool).await, 0);
}

/// ⚠ A move of the project ends an evaluation in flight, so its old organization files nothing.
#[sqlx::test]
async fn a_project_moved_mid_evaluation_files_nothing_under_the_old_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let state = common::state(&pool, auth);
    let id = made(&router(state.clone()), &organization, &project).await;
    let (lease_id, rule) = leased(&state, id).await;

    let mut tx = maintenance_scope(&state.db, TelemetryLane)
        .await
        .expect("the lane");
    module_db::move_project(&mut tx, &project, &OrganizationId::new())
        .await
        .expect("moved");
    tx.commit().await.expect("commit");
    let (settled, _) = incidents::settle(&state, &rule, lease_id, Some(6000.0), Utc::now())
        .await
        .expect("a settle");
    assert!(!settled, "the move ended the lease");
    assert_eq!(incidents_kept(&pool).await, 0);
}

/// ⚠ An alert saved over while a replica holds it in a batch is let go, so the
/// next pass reads it as saved.
#[sqlx::test]
async fn an_alert_saved_over_mid_batch_is_let_go(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let state = common::state(&pool, auth);
    let app = router(state.clone());
    let id = made(&app, &organization, &project).await;
    leased(&state, id).await;

    let body = slow_calls("Slower calls");
    let uri = format!("{LANE}/{id}");
    let saved = request(
        "PUT",
        &uri,
        "user_member",
        &organization,
        &project,
        Some(&body),
    );
    let (status, saved) = answer(&app, saved).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        lease_of(&pool, id).await,
        (None, None),
        "still leased to the batch"
    );
}

/// ⚠ A plan that cannot be read holds its organization's alerts as they stand.
#[sqlx::test]
async fn an_unread_plan_holds_its_alerts_as_they_stand(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let state = common::state_with_entitlements(&pool, auth, Arc::new(Unanswered));
    let id = made(&router(state.clone()), &organization, &project).await;
    sqlx::query(
        "UPDATE telemetry.rules SET paused = 'plan', paused_at = now(), severity = 'paused'
          WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("paused by the plan");

    evaluate::pass(&state).await.expect("a pass");
    let paused: Option<String> =
        sqlx::query_scalar("SELECT paused FROM telemetry.rules WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("the alert");
    assert_eq!(paused.as_deref(), Some("plan"));
}

/// ⚠ A percentile merges the totals' digests with the spans', and is neither's alone.
#[sqlx::test]
async fn a_percentile_reads_both_sources_merged(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (auth, organization, project, _) = seated();
    let state = common::state_with_stores_under(&pool, auth, Arc::new(NoLimits));
    let app = router(state.clone());
    // The median is 2,000 of the totals alone, 8,000 of the spans alone and
    // 6,000 of the two.
    let now = Utc::now();
    let earlier = now.duration_trunc(TimeDelta::hours(1)).expect("an hour") - TimeDelta::hours(3);
    let calls = [
        (1_000, earlier),
        (2_000, earlier),
        (6_000, earlier),
        (7_000, earlier),
        (3_000, now),
        (8_000, now),
        (9_000, now),
    ];
    let rows: Vec<_> = calls
        .iter()
        .zip(1u64..)
        .map(|((millis, at), n)| telmoni_telemetry::store::SpanRow {
            span_id: format!("{n:016x}"),
            model: "claude-sonnet-5-5".into(),
            ..common::received(
                &project,
                SpanKind::ModelCall,
                "chat claude-sonnet-5-5",
                *millis,
                if *at == now {
                    now - TimeDelta::seconds(2)
                } else {
                    *at + TimeDelta::minutes(5)
                },
            )
        })
        .collect();
    ch.store
        .insert_spans(&rows)
        .await
        .expect("the spans are kept");
    let body = json!({
        "name": "A day's middle call", "kind": "model_call", "measure": "duration",
        "aggregation": "p50", "window": "day", "operator": "gt", "alert_threshold": 1
    });
    let made = request(
        "POST",
        LANE,
        "user_member",
        &organization,
        &project,
        Some(&body),
    );
    let (status, made) = answer(&app, made).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");

    assert_eq!(evaluate::pass(&state).await.expect("a pass"), 1);
    let (_, listed) = answer(
        &app,
        request("GET", LANE, "user_member", &organization, &project, None),
    )
    .await;
    assert_eq!(listed["rules"][0]["value"], json!(6000.0), "{listed}");
}

/// ⚠ An alert the plan lets run again reads as never evaluated, as a resume leaves it.
#[sqlx::test]
async fn an_alert_the_plan_lets_run_again_reads_as_never_evaluated(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, project, _) = seated();
    let state = common::state_with_entitlements(&pool, auth, Arc::new(Holds(50)));
    let id = made(&router(state.clone()), &organization, &project).await;
    sqlx::query(
        "UPDATE telemetry.rules SET paused = 'plan', paused_at = now(), severity = 'paused',
                value = 3, evaluated_at = now() - interval '5 minutes'
          WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("paused by the plan");

    evaluate::pass(&state).await.expect("a pass");
    let (paused, value, evaluated): (Option<String>, Option<f64>, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT paused, value, evaluated_at FROM telemetry.rules WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("the alert");
    assert_eq!((paused, value, evaluated), (None, None, None));
}
