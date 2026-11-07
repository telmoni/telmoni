//! An organization dividing its pool among its projects (roadmap 17.4): one
//! project's caps set by the organization's owners and admins alone, each at
//! most the plan's and audited, read by every member beside what each
//! project has used; and a project past its cap on alerts, objectives or
//! monitors refused alone, its organization's other projects untouched.
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
            entitlement: "pro".into(),
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
