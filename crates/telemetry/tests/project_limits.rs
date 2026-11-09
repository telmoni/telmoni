//! An organization dividing its pool among its projects: the organization's
//! owners and admins alone set a project's caps, each at most the plan's and
//! audited; every member reads them, and a project past one is refused alone.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, SECRET};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::Entitlements;
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    AlertCeilings, Grants, OrganizationId, OrganizationRole, ProjectId, Role, TelmoniError,
};
use telmoni_telemetry::router;

const LIMITS: &str = "/internal/v1/telemetry/limits";
const RULES: &str = "/internal/v1/telemetry/rules";
const MONITORS: &str = "/internal/v1/telemetry/monitors";

/// A plan of ten alerts and 500 deliveries a day.
struct TenAlerts;

#[async_trait]
impl Entitlements for TenAlerts {
    async fn grants(&self, _organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        Ok(Grants::unlimited())
    }

    async fn alert_ceilings(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<Option<AlertCeilings>, TelmoniError> {
        Ok(Some(AlertCeilings {
            alerts: 10,
            deliveries_per_day: 500,
            entitlement: "larger_plan".into(),
            detail: "This organization's plan holds ten alerts.".into(),
        }))
    }
}

/// A request from `user` on `project` of `organization`, as the console sends it.
fn request(
    method: &str,
    uri: &str,
    user: &str,
    organization: &OrganizationId,
    project: Option<&ProjectId>,
    body: Option<&Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(SERVICE_SECRET_HEADER, SECRET)
        .header("authorization", format!("Bearer tok_{user}"))
        .header("x-organization-id", organization.as_str())
        .header("content-type", "application/json");
    if let Some(project) = project {
        builder = builder.header("x-project-id", project.as_str());
    }
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
fn alert(name: &str) -> Value {
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

/// An objective: 99% of one agent's runs over 30 days.
fn objective(name: &str) -> Value {
    json!({
        "name": name,
        "kind": "run",
        "measure": "burn",
        "aggregation": "avg",
        "filters": { "agent": ["nightly-report"] },
        "window": "hour",
        "operator": "gte",
        "alert_threshold": 14.4,
        "objective": { "target": 0.99, "days": 30 }
    })
}

/// A monitor of the agent `agent`, every hour.
fn monitor(agent: &str) -> Value {
    json!({ "agent": agent, "every_seconds": 3600, "grace_seconds": 600 })
}

/// One organization, two of its projects, an admin of the organization and a
/// member seated on each.
fn seated() -> (Arc<AuthStub>, OrganizationId, ProjectId, ProjectId) {
    let auth = AuthStub::new();
    let organization = OrganizationId::new();
    let (first, second) = (ProjectId::new(), ProjectId::new());
    for project in [&first, &second] {
        auth.seat(
            "user_admin",
            &organization,
            project,
            OrganizationRole::Admin,
            Some(Role::Admin),
        );
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

/// ⚠ An organization admin sets a project's caps, at most the plan's; a
/// member sets none and reads them all; the change is audited; and the
/// project past its cap on alerts, objectives or monitors is refused alone,
/// in words naming the cap and who may raise it, never the plan.
#[sqlx::test]
async fn a_projects_caps_hold_it_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, first, second) = seated();
    let app = router(common::state_with_entitlements(
        &pool,
        auth,
        Arc::new(TenAlerts),
    ));
    let caps = json!({ "alerts": 1, "objectives": 1, "monitors": 1 });

    let (status, body) = answer(
        &app,
        request(
            "PUT",
            LIMITS,
            "user_member",
            &organization,
            Some(&first),
            Some(&caps),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = answer(
        &app,
        request(
            "PUT",
            LIMITS,
            "user_admin",
            &organization,
            Some(&first),
            Some(&json!({ "alerts": 11 })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "past the plan's: {body}");
    let (status, body) = answer(
        &app,
        request(
            "PUT",
            LIMITS,
            "user_admin",
            &organization,
            Some(&first),
            Some(&caps),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events
          WHERE organization_id = $1 AND metadata ? 'limits'",
    )
    .bind(organization.as_str())
    .fetch_one(&pool)
    .await
    .expect("the audit log");
    assert_eq!(audited, 1, "the change was audited");

    for (lane, first_one, second_one) in [
        (RULES, alert("Slow calls"), alert("Slower calls")),
        (RULES, objective("Nightly"), objective("Nightly again")),
        (
            MONITORS,
            monitor("nightly-report"),
            monitor("weekly-report"),
        ),
    ] {
        let (status, body) = answer(
            &app,
            request(
                "POST",
                lane,
                "user_member",
                &organization,
                Some(&first),
                Some(&first_one),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{lane}: {body}");
        let (status, refused) = answer(
            &app,
            request(
                "POST",
                lane,
                "user_member",
                &organization,
                Some(&first),
                Some(&second_one),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{lane}: {refused}");
        assert_eq!(refused["entitlement"], "project_limit", "{refused}");
        let detail = refused["detail"].as_str().unwrap_or_default();
        assert!(detail.contains("Limits page"), "{detail}");
        assert!(
            !detail.contains("plan holds"),
            "the plan is not named: {detail}"
        );
        // The organization's other project carries on.
        let (status, body) = answer(
            &app,
            request(
                "POST",
                lane,
                "user_member",
                &organization,
                Some(&second),
                Some(&second_one),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{lane}: {body}");
    }

    let (status, limits) = answer(
        &app,
        request("GET", LIMITS, "user_member", &organization, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{limits}");
    assert_eq!(limits["plan"]["alerts"], 10, "{limits}");
    let share = |project: &ProjectId| {
        limits["projects"]
            .as_array()
            .and_then(|projects| {
                projects
                    .iter()
                    .find(|p| p["id"] == project.as_str())
                    .cloned()
            })
            .unwrap_or_default()
    };
    let capped = share(&first);
    assert_eq!(capped["caps"]["alerts"], 1, "{limits}");
    assert_eq!(capped["used"]["alerts"], 1, "{limits}");
    assert_eq!(capped["used"]["objectives"], 1, "{limits}");
    assert_eq!(capped["used"]["monitors"], 1, "{limits}");
    let shared = share(&second);
    assert_eq!(shared["caps"]["alerts"], Value::Null, "{limits}");

    // Cleared, the project shares the pool whole again.
    let (status, _) = answer(
        &app,
        request(
            "PUT",
            LIMITS,
            "user_admin",
            &organization,
            Some(&first),
            Some(&json!({})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = answer(
        &app,
        request(
            "POST",
            RULES,
            "user_member",
            &organization,
            Some(&first),
            Some(&alert("Slower calls")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// ⚠ An edit that turns an alert into an objective, or an objective into an
/// alert, is held to the cap of the kind it becomes; an edit within its kind
/// is allowed at the cap, since it adds nothing.
#[sqlx::test]
async fn an_edit_that_changes_a_rules_kind_is_held_to_the_new_kinds_cap(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, first, second) = seated();
    let app = router(common::state_with_entitlements(
        &pool,
        auth,
        Arc::new(TenAlerts),
    ));

    for (project, caps, made, became, renamed) in [
        (
            &first,
            json!({ "alerts": 1, "objectives": 0 }),
            alert("Slow calls"),
            objective("Slow calls"),
            alert("Slower calls"),
        ),
        (
            &second,
            json!({ "alerts": 0, "objectives": 1 }),
            objective("Nightly"),
            alert("Nightly"),
            objective("Nightly again"),
        ),
    ] {
        let (status, body) = answer(
            &app,
            request(
                "PUT",
                LIMITS,
                "user_admin",
                &organization,
                Some(project),
                Some(&caps),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = answer(
            &app,
            request(
                "POST",
                RULES,
                "user_member",
                &organization,
                Some(project),
                Some(&made),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let one = format!("{RULES}/{}", body["id"].as_str().expect("an id"));

        let (status, refused) = answer(
            &app,
            request(
                "PUT",
                &one,
                "user_member",
                &organization,
                Some(project),
                Some(&became),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{refused}");
        assert_eq!(refused["entitlement"], "project_limit", "{refused}");

        let (status, body) = answer(
            &app,
            request(
                "PUT",
                &one,
                "user_member",
                &organization,
                Some(project),
                Some(&renamed),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "an edit within its kind, at the cap: {body}"
        );
    }
}

/// Caps set on a project that has sent nothing yet file its settings row, so
/// the purge sweep, which pages those rows, finds it, and its caps hold.
#[sqlx::test]
async fn caps_set_before_any_telemetry_file_the_projects_settings(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, first, _) = seated();
    let app = router(common::state_with_entitlements(
        &pool,
        auth,
        Arc::new(TenAlerts),
    ));
    let (status, body) = answer(
        &app,
        request(
            "PUT",
            LIMITS,
            "user_admin",
            &organization,
            Some(&first),
            Some(&json!({ "alerts": 0 })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let filed: Option<String> = sqlx::query_scalar(
        "SELECT organization_id FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(first.as_str())
    .fetch_optional(&pool)
    .await
    .expect("the settings row");
    assert_eq!(filed.as_deref(), Some(organization.as_str()));

    let (status, refused) = answer(
        &app,
        request(
            "POST",
            RULES,
            "user_member",
            &organization,
            Some(&first),
            Some(&alert("Slow calls")),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYMENT_REQUIRED,
        "its caps hold: {refused}"
    );
}

/// A plan of ten alerts and `0` deliveries a day.
struct Deliveries(u32);

#[async_trait]
impl Entitlements for Deliveries {
    async fn grants(&self, _organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        Ok(Grants::unlimited())
    }

    async fn alert_ceilings(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<Option<AlertCeilings>, TelmoniError> {
        Ok(Some(AlertCeilings {
            alerts: 10,
            deliveries_per_day: self.0,
            entitlement: "larger_plan".into(),
            detail: "This organization's plan holds its deliveries.".into(),
        }))
    }
}

/// A project filed under `organization`, capped at `cap` deliveries a day.
async fn deliveries_capped(
    pool: &PgPool,
    organization: &OrganizationId,
    project: &ProjectId,
    cap: i32,
) {
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id) VALUES ($1, $2)",
    )
    .bind(project.as_str())
    .bind(organization.as_str())
    .execute(pool)
    .await
    .expect("the project's settings");
    sqlx::query(
        "INSERT INTO telemetry.project_limits (project_id, organization_id, alert_deliveries_per_day)
         VALUES ($1, $2, $3)",
    )
    .bind(project.as_str())
    .bind(organization.as_str())
    .bind(cap)
    .execute(pool)
    .await
    .expect("the project's cap");
}

/// ⚠ A capped project's deliveries come out of its own day first: past it,
/// it is held alone and its owners told once that day; room it took and did
/// not use, a delivery failed or the organization's day spent, is given back.
#[sqlx::test]
async fn a_projects_deliveries_come_out_of_its_own_day_first(pool: PgPool) {
    use telmoni_telemetry::rules::deliveries::{settle, take};

    apply_audit_migrations(&pool).await;
    let notices = common::NoticeLog::new();
    let organization = OrganizationId::new();
    let state = common::state_with_redis_under(
        &pool,
        AuthStub::new(),
        Arc::new(Deliveries(500)),
        notices.clone(),
    );

    // Past its own day: held to it, the organization's room to spare.
    let full = ProjectId::new();
    deliveries_capped(&pool, &organization, &full, 2).await;
    let taken = take(&state, &organization, &full, 3).await;
    assert_eq!(taken.room(), 2, "the project's day, not the organization's");
    settle(&state, &organization, taken, 2).await;
    let taken = take(&state, &organization, &full, 1).await;
    assert_eq!(taken.room(), 0);
    settle(&state, &organization, taken, 0).await;
    let told = || {
        notices
            .notices()
            .into_iter()
            .filter(|n| n.title == "A project past its cap")
            .count()
    };
    common::eventually("the owners are told", || async { told() == 1 }).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(told(), 1, "told once a day");
    assert!(
        notices
            .notices()
            .iter()
            .all(|n| n.title != "Alert deliveries held until midnight UTC"),
        "the organization was told its day was spent when the project's was"
    );

    // A delivery that failed gives its room back.
    let failed = ProjectId::new();
    deliveries_capped(&pool, &organization, &failed, 2).await;
    let taken = take(&state, &organization, &failed, 2).await;
    assert_eq!(taken.room(), 2);
    settle(&state, &organization, taken, 0).await;
    let taken = take(&state, &organization, &failed, 2).await;
    assert_eq!(
        taken.room(),
        2,
        "the failed deliveries' room was not given back"
    );
    settle(&state, &organization, taken, 2).await;

    // The organization's day spent: the project gives back what it took of
    // its own and could not use.
    let spent = OrganizationId::new();
    let state = common::state_with_redis_under(
        &pool,
        AuthStub::new(),
        Arc::new(Deliveries(1)),
        common::NoticeLog::new(),
    );
    let blocked = ProjectId::new();
    deliveries_capped(&pool, &spent, &blocked, 5).await;
    let taken = take(&state, &spent, &blocked, 3).await;
    assert_eq!(taken.room(), 1, "the organization's day holds one");
    settle(&state, &spent, taken, 1).await;
    let mut connection = state.redis.connection().await.expect("Redis");
    let used: Option<u64> = redis::cmd("GET")
        .arg(format!(
            "telemetry:project-alert-deliveries:{blocked}:{}",
            chrono::Utc::now().date_naive()
        ))
        .query_async(&mut connection)
        .await
        .expect("the project's day");
    assert_eq!(
        used,
        Some(1),
        "the project kept room the organization never let it use"
    );
}
