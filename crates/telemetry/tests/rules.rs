//! Alerts through the module's own router and its loop: any member of the
//! project makes, reads, edits and deletes one, each checked whole; one past
//! its organization's plan, paused ones counted, is refused as a 402 in the
//! plan's words; and the loop evaluates each over the spans ClickHouse holds,
//! the full hours of its window from the hourly totals and the rest from the
//! spans, holding the organization's alerts to its plan as it goes.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DurationRound as _, TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, SECRET, clickhouse_or_skip};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::{
    AlertCeilings, Grants, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind, SpanStatus,
    TelmoniError,
};
use telmoni_telemetry::router;
use telmoni_telemetry::rules::evaluate;

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
