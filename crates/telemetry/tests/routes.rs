//! Where an alert's notices go, through the module's router and its loop:
//! the connectors it links, each hearing its warnings and alerts or its
//! alerts alone, in the hours it is set to; and an organization's deliveries
//! a day held to its plan, the rest left to the feed and the owner told once.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Datelike as _, TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, NoticeLog, SECRET, clickhouse_or_skip};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::{
    AlertCeilings, Grants, NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role,
    SpanKind, TelmoniError,
};
use telmoni_telemetry::rules::{deliveries, evaluate};
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::{AppState, router};

const RULES: &str = "/internal/v1/telemetry/rules";

/// A plan of fifty alerts and `deliveries` a day for every organization.
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
            alerts: 50,
            deliveries_per_day: self.0,
            entitlement: "teams".into(),
            detail: "The plan holds 50 alerts.".into(),
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
    async fn new(
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
            .insert_spans(&[SpanRow {
                ..common::received(
                    &project,
                    SpanKind::ModelCall,
                    "chat claude-sonnet-5-5",
                    1_200,
                    Utc::now() - TimeDelta::seconds(2),
                )
            }])
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

    async fn evaluate(&self, pool: &PgPool) {
        sqlx::query("UPDATE telemetry.rules SET evaluated_at = NULL")
            .execute(pool)
            .await
            .expect("due again");
        evaluate::pass(&self.state).await.expect("a pass");
    }

    /// The incident notices told so far: their kind and where they went.
    fn sent(&self) -> Vec<(NotificationKind, Vec<Uuid>)> {
        self.notices
            .notices()
            .into_iter()
            .filter(|n| n.kind != NotificationKind::OrganizationAlert)
            .map(|n| (n.kind, n.connections))
            .collect()
    }
}

/// The count of the project's model calls over the hour, against `alert`
/// and a warning at `warning`, linking `connectors`.
fn calls(alert: f64, warning: Option<f64>, connectors: &Value) -> Value {
    json!({
        "name": "Model calls", "kind": "model_call", "measure": "count",
        "aggregation": "count", "window": "hour", "operator": "gt",
        "alert_threshold": alert, "warning_threshold": warning, "connectors": connectors,
    })
}

/// ⚠ Each linked connector hears what its level asks, in its hours: one at
/// `warning` hears the warning, one at `alert` the rise to alert, and one out
/// of its hours neither; a recovery reaches those that heard its incident.
#[sqlx::test]
async fn an_alerts_notices_go_to_the_connectors_that_hear_them(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, Arc::new(NoLimits)).await;
    let (warnings, alerts, closed) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let today = u8::try_from(Utc::now().weekday().number_from_monday()).expect("a weekday");
    let another_day = if today == 7 { 1 } else { today + 1 };
    let links = json!([
        { "connection_id": warnings, "level": "warning" },
        { "connection_id": alerts, "level": "alert" },
        { "connection_id": closed, "hours": { "days": [another_day], "from": "00:00", "to": "00:00" } },
    ]);
    let (status, made) = project
        .send("POST", RULES, Some(&calls(5.0, Some(0.0), &links)))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let rule = format!("{RULES}/{}", made["id"].as_str().expect("an id"));
    let (_, read) = project.send("GET", &rule, None).await;
    let listed = read["connectors"].as_array().cloned().unwrap_or_default();
    assert_eq!(listed.len(), 3, "{read}");
    assert!(listed.iter().all(|link| link["muted"] == false));

    project.evaluate(&pool).await;
    assert_eq!(
        project.sent(),
        [(NotificationKind::IncidentOpened, vec![warnings])],
        "the warning to the connector at warning alone"
    );

    project
        .send("PUT", &rule, Some(&calls(0.5, Some(0.1), &links)))
        .await;
    project.evaluate(&pool).await;
    let sent = project.sent();
    let (kind, mut to) = sent.get(1).cloned().expect("the rise told");
    to.sort_unstable();
    let mut both = vec![warnings, alerts];
    both.sort_unstable();
    assert_eq!((kind, to), (NotificationKind::IncidentOpened, both));

    project
        .send("PUT", &rule, Some(&calls(100.0, None, &links)))
        .await;
    project.evaluate(&pool).await;
    let (kind, to) = project.sent().get(2).cloned().expect("the recovery told");
    assert_eq!(kind, NotificationKind::IncidentResolved);
    assert_eq!(to.len(), 2, "those that heard its alert hear its recovery");
}

/// ⚠ An organization's deliveries a day are held to its plan: the notice past
/// it reaches the feed alone, and the owner is told once that day.
#[sqlx::test]
async fn an_organizations_deliveries_a_day_are_held_to_its_plan(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, Arc::new(Deliveries(1))).await;
    if project.state.redis.connection().await.is_none() {
        eprintln!("skipping {}: Redis is not reachable", module_path!());
        return;
    }
    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    let links = json!([{ "connection_id": first }, { "connection_id": second }]);
    let (status, made) = project
        .send("POST", RULES, Some(&calls(5.0, Some(0.0), &links)))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let rule = format!("{RULES}/{}", made["id"].as_str().expect("an id"));

    project.evaluate(&pool).await;
    let sent = project.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.len(), 1, "one delivery of the day's one");
    project
        .send("PUT", &rule, Some(&calls(0.5, Some(0.1), &links)))
        .await;
    project.evaluate(&pool).await;
    assert!(
        project.sent().get(1).is_some_and(|(_, to)| to.is_empty()),
        "{:?}",
        project.sent()
    );
    let told: Vec<_> = project
        .notices
        .notices()
        .into_iter()
        .filter(|n| n.kind == NotificationKind::OrganizationAlert)
        .collect();
    assert_eq!(told.len(), 1, "the owner told once");
    assert_eq!(told[0].project_id, None, "on the organization's feed");
}

/// ⚠ A connector that hears nothing takes none of the day's room.
#[sqlx::test]
async fn a_stopped_connector_takes_none_of_the_days_deliveries(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, Arc::new(Deliveries(2))).await;
    if project.state.redis.connection().await.is_none() {
        eprintln!("skipping {}: Redis is not reachable", module_path!());
        return;
    }
    // The stopped one first by id, where the room is handed out.
    let mut pair = [Uuid::new_v4(), Uuid::new_v4()];
    pair.sort_unstable();
    let [stopped, heard] = pair;
    project.notices.stop(stopped);
    let links = json!([{ "connection_id": stopped }, { "connection_id": heard }]);
    let (status, made) = project
        .send("POST", RULES, Some(&calls(5.0, Some(0.0), &links)))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let rule = format!("{RULES}/{}", made["id"].as_str().expect("an id"));

    project.evaluate(&pool).await;
    project
        .send("PUT", &rule, Some(&calls(0.5, Some(0.1), &links)))
        .await;
    project.evaluate(&pool).await;
    let sent: Vec<Vec<Uuid>> = project.sent().into_iter().map(|(_, to)| to).collect();
    assert_eq!(
        sent,
        [vec![heard], vec![heard]],
        "the warning and the rise each the day's delivery of one"
    );
    let told = project
        .notices
        .notices()
        .into_iter()
        .filter(|n| n.kind == NotificationKind::OrganizationAlert)
        .count();
    assert_eq!(told, 1, "the owner told once the day's two are made");
}

/// A link that reads two ways is refused before anything is written.
#[sqlx::test]
async fn a_link_that_reads_two_ways_is_refused(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, Arc::new(NoLimits)).await;
    let one = Uuid::new_v4();
    let too_many: Vec<Value> = (0..21)
        .map(|_| json!({ "connection_id": Uuid::new_v4() }))
        .collect();
    for links in [
        json!([{ "connection_id": one }, { "connection_id": one }]),
        json!([{ "connection_id": one, "level": "page" }]),
        json!([{ "connection_id": one, "hours": { "days": [8], "from": "09:00", "to": "18:00" } }]),
        Value::Array(too_many),
    ] {
        let (status, refused) = project
            .send("POST", RULES, Some(&calls(5.0, None, &links)))
            .await;
        assert!(status.is_client_error(), "{links}: {status} {refused}");
    }
}

/// Room given back to a day whose count is gone writes no count, which would outlive its day.
#[sqlx::test]
async fn room_given_back_to_a_day_whose_count_is_gone_writes_nothing(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, Arc::new(Deliveries(5))).await;
    let Some(mut redis) = project.state.redis.connection().await else {
        eprintln!("skipping {}: Redis is not reachable", module_path!());
        return;
    };
    let taken = deliveries::take(&project.state, &project.organization, 3).await;
    assert_eq!(taken.room(), 3);
    let key = format!(
        "telemetry:alert-deliveries:{}:{}",
        project.organization,
        Utc::now().date_naive()
    );
    let gone: i64 = redis::cmd("DEL")
        .arg(&key)
        .query_async(&mut redis)
        .await
        .expect("the count gone");
    assert_eq!(gone, 1, "the room was counted under {key}");
    deliveries::settle(&project.state, &project.organization, taken, 0).await;
    let kept: i64 = redis::cmd("EXISTS")
        .arg(&key)
        .query_async(&mut redis)
        .await
        .expect("a read");
    assert_eq!(kept, 0);
}
