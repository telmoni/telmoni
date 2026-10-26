//! Pausing an alert and muting it for a window, through the module's router
//! and its loop: a paused alert is not evaluated until a member resumes it,
//! and none is resumed past its plan's ceiling; a maintenance window, for
//! an alert or for every alert of the project, lets incidents open and
//! resolve in the feed while telling no connector, and stays as the record.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, NoticeLog, SECRET, clickhouse_or_skip};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::{
    AlertCeilings, Grants, NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role,
    SpanKind, TelmoniError, UserId,
};
use telmoni_telemetry::rules::{evaluate, mutes};
use telmoni_telemetry::{AppState, router};

const RULES: &str = "/internal/v1/telemetry/rules";
const MUTES: &str = "/internal/v1/telemetry/mutes";

/// A plan of as many alerts as the test sets.
struct Ceiling(AtomicU32);

#[async_trait]
impl Entitlements for Ceiling {
    async fn grants(&self, _organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        Ok(Grants::unlimited())
    }

    async fn alert_ceilings(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<Option<AlertCeilings>, TelmoniError> {
        Ok(Some(AlertCeilings {
            alerts: self.0.load(Ordering::SeqCst),
            deliveries_per_day: 1_000,
            entitlement: "teams".into(),
            detail: "The plan holds fewer alerts.".into(),
        }))
    }
}

struct Project {
    app: axum::Router,
    state: Arc<AppState>,
    notices: Arc<NoticeLog>,
    organization: OrganizationId,
    project: ProjectId,
}

impl Project {
    async fn new(pool: &PgPool, ch: &common::ClickHouse) -> Self {
        Self::held(pool, ch, Arc::new(NoLimits)).await
    }

    async fn held(
        pool: &PgPool,
        ch: &common::ClickHouse,
        entitlements: Arc<dyn Entitlements>,
    ) -> Self {
        let auth = AuthStub::new();
        let (organization, project) = (OrganizationId::new(), ProjectId::new());
        auth.seat(
            "user_member",
            &organization,
            &project,
            OrganizationRole::Member,
            Some(Role::Member),
        );
        let notices = NoticeLog::new();
        let state = common::state_with_stores_noticed(pool, auth, entitlements, notices.clone());
        ch.store
            .insert_spans(&[common::received(
                &project,
                SpanKind::ModelCall,
                "chat claude-sonnet-5-5",
                1_200,
                Utc::now() - TimeDelta::seconds(2),
            )])
            .await
            .expect("the span is kept");
        Self {
            app: router(state.clone()),
            state,
            notices,
            organization,
            project,
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<&Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", "Bearer tok_user_member")
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.project.as_str())
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .expect("a request");
        let response = self.app.clone().oneshot(request).await.expect("an answer");
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
            serde_json::from_slice(&bytes).expect("JSON")
        };
        (status, body)
    }

    async fn evaluate(&self, pool: &PgPool) -> u64 {
        sqlx::query("UPDATE telemetry.rules SET evaluated_at = NULL WHERE paused IS NULL")
            .execute(pool)
            .await
            .expect("due again");
        evaluate::pass(&self.state).await.expect("a pass")
    }

    /// An alert on the project's model calls against `alert` and a warning
    /// at `warning`, linking `connector`: its lane.
    async fn alert(&self, name: &str, alert: f64, warning: Option<f64>, connector: Uuid) -> String {
        let body = json!({
            "name": name, "kind": "model_call", "measure": "count", "aggregation": "count",
            "window": "hour", "operator": "gt", "alert_threshold": alert,
            "warning_threshold": warning, "connectors": [{ "connection_id": connector }],
        });
        let (status, made) = self.send("POST", RULES, Some(&body)).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        format!(
            "{RULES}/{}",
            made.get("id").and_then(Value::as_str).expect("an id")
        )
    }

    fn sent(&self) -> Vec<(NotificationKind, Vec<Uuid>)> {
        self.notices
            .notices()
            .into_iter()
            .map(|n| (n.kind, n.connections))
            .collect()
    }
}

/// ⚠ A paused alert is not evaluated until a member resumes it.
#[sqlx::test]
async fn a_paused_alert_is_not_evaluated_until_resumed(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let rule = project.alert("Calls", 0.0, None, Uuid::new_v4()).await;

    let (status, paused) = project.send("POST", &format!("{rule}/pause"), None).await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(
        (&paused["paused"], &paused["severity"]),
        (&json!("person"), &json!("paused"))
    );
    assert_eq!(
        project.evaluate(&pool).await,
        0,
        "a paused alert is not evaluated"
    );
    assert!(project.sent().is_empty());

    let (status, resumed) = project.send("POST", &format!("{rule}/resume"), None).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(
        (&resumed["paused"], &resumed["severity"]),
        (&Value::Null, &json!("unknown"))
    );
    assert_eq!(project.evaluate(&pool).await, 1);
    assert_eq!(project.sent().len(), 1, "its breach told once it runs");
}

/// ⚠ An alert its plan holds no room for is not resumed, paused by the plan
/// or then by a person, until the plan makes room.
#[sqlx::test]
async fn an_alert_past_its_plan_is_not_resumed(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let ceiling = Arc::new(Ceiling(AtomicU32::new(2)));
    let project = Project::held(&pool, &ch, ceiling.clone()).await;
    project.alert("Calls", 0.0, None, Uuid::new_v4()).await;
    let newer = project.alert("More calls", 0.0, None, Uuid::new_v4()).await;

    ceiling.0.store(1, Ordering::SeqCst);
    assert_eq!(project.evaluate(&pool).await, 1, "the older alone runs");
    let (_, held) = project.send("GET", &newer, None).await;
    assert_eq!(held["paused"], "plan", "{held}");
    let (status, refused) = project.send("POST", &format!("{newer}/resume"), None).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{refused}");

    let (status, paused) = project.send("POST", &format!("{newer}/pause"), None).await;
    assert_eq!(paused["paused"], "person", "{status} {paused}");
    let (status, refused) = project.send("POST", &format!("{newer}/resume"), None).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{refused}");

    ceiling.0.store(2, Ordering::SeqCst);
    let (status, resumed) = project.send("POST", &format!("{newer}/resume"), None).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["paused"], Value::Null);
}

/// ⚠ A window mutes its alert's connectors while its incidents open in the
/// feed, and one ended early lets the next notice through; every window
/// stays as the record of who muted what.
#[sqlx::test]
async fn a_window_mutes_an_alerts_connectors_and_stays_as_the_record(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let connector = Uuid::new_v4();
    let rule = project.alert("Calls", 5.0, Some(0.0), connector).await;
    let rule_id = rule.rsplit('/').next().expect("an id").to_owned();

    let (status, window) = project
        .send(
            "POST",
            MUTES,
            Some(&json!({ "rule_id": rule_id, "ends_at": Utc::now() + TimeDelta::hours(1) })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{window}");
    assert!(window["created_by"].is_string());

    project.evaluate(&pool).await;
    assert_eq!(
        project.sent(),
        [(NotificationKind::IncidentOpened, vec![])],
        "opened in the feed, told to no connector"
    );

    let id = window["id"].as_str().expect("an id");
    let (status, ended) = project
        .send("POST", &format!("{MUTES}/{id}/end"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{ended}");
    assert!(ended["ended_by"].is_string());

    let body = json!({
        "name": "Calls", "kind": "model_call", "measure": "count", "aggregation": "count",
        "window": "hour", "operator": "gt", "alert_threshold": 0.5, "warning_threshold": 0.1,
        "connectors": [{ "connection_id": connector }],
    });
    project.send("PUT", &rule, Some(&body)).await;
    project.evaluate(&pool).await;
    assert_eq!(
        project.sent().get(1),
        Some(&(NotificationKind::IncidentOpened, vec![connector])),
        "the rise after the window reaches its connector"
    );

    let (_, listed) = project.send("GET", MUTES, None).await;
    assert_eq!(
        listed["mutes"].as_array().map(Vec::len),
        Some(1),
        "kept as the record"
    );
}

/// ⚠ A window ended once keeps who ended it; one still to start ends just after its start.
#[sqlx::test]
async fn a_window_ended_once_keeps_who_ended_it(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let tomorrow = Utc::now() + TimeDelta::days(1);
    let (status, window) = project
        .send(
            "POST",
            MUTES,
            Some(&json!({ "starts_at": tomorrow, "ends_at": tomorrow + TimeDelta::hours(1) })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{window}");
    let id = window["id"].as_str().expect("an id");
    let (status, ended) = project
        .send("POST", &format!("{MUTES}/{id}/end"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{ended}");
    assert!(ended["ended_by"].is_string(), "{ended}");

    let mut tx = project_scope(&project.state.db, &project.project)
        .await
        .expect("the project's scope");
    let again = mutes::end(
        &mut tx,
        &project.project,
        id.parse().expect("a window's id"),
        &UserId::new(),
    )
    .await
    .expect("answered")
    .expect("the window");
    tx.commit().await.expect("commit");
    assert_eq!(
        Value::from(again.ended_by),
        ended["ended_by"],
        "the first to end it stays"
    );
}

/// A window for the whole project mutes every alert of it.
#[sqlx::test]
async fn a_projects_window_mutes_every_alert(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    project.alert("Calls", 0.0, None, Uuid::new_v4()).await;
    project.alert("More calls", 0.0, None, Uuid::new_v4()).await;
    let (status, _) = project
        .send(
            "POST",
            MUTES,
            Some(&json!({ "ends_at": Utc::now() + TimeDelta::minutes(30) })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    project.evaluate(&pool).await;
    let sent = project.sent();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|(_, to)| to.is_empty()), "{sent:?}");
}

/// A window that reads two ways is refused.
#[sqlx::test]
async fn a_window_that_reads_two_ways_is_refused(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let now = Utc::now();
    for (body, expected) in [
        (
            json!({ "ends_at": now - TimeDelta::minutes(1) }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "starts_at": now + TimeDelta::hours(2), "ends_at": now + TimeDelta::hours(1) }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "ends_at": now + TimeDelta::days(31) }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "rule_id": Uuid::new_v4(), "ends_at": now + TimeDelta::hours(1) }),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, refused) = project.send("POST", MUTES, Some(&body)).await;
        assert_eq!(status, expected, "{body}: {refused}");
    }
}
