//! An alert's severity and its incidents, through the module's router and
//! its loop over the spans ClickHouse holds: a breach opens an incident told
//! once in the feed, a rise is told again to the connectors alone, a recovery
//! resolves it, and a breach within the hour reopens it; a severity held is
//! told again as often as the alert asks until a member takes the incident;
//! a window empty for as long as the window opens one where the alert asks,
//! and none where it does not; and each keeps where its notices went.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

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
use telmoni_shared::seam::NoLimits;
use telmoni_shared::{
    NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind,
};
use telmoni_telemetry::rules::{evaluate, incidents};
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::{AppState, router};

/// A project with a member seated on it, the module over the stores with
/// its notices logged, and one model call of the project's this minute.
struct Project {
    app: axum::Router,
    state: Arc<AppState>,
    notices: Arc<NoticeLog>,
    organization: OrganizationId,
    project: ProjectId,
}

impl Project {
    async fn new(pool: &PgPool, ch: &common::ClickHouse) -> Self {
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
        let state =
            common::state_with_stores_noticed(pool, auth, Arc::new(NoLimits), notices.clone());
        ch.store
            .insert_spans(&[SpanRow {
                model: "claude-sonnet-5-5".into(),
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

    /// Every alert due again, and one pass of the loop.
    async fn evaluate(&self, pool: &PgPool) {
        sqlx::query("UPDATE telemetry.rules SET evaluated_at = NULL")
            .execute(pool)
            .await
            .expect("due again");
        evaluate::pass(&self.state).await.expect("a pass");
    }

    async fn incidents(&self) -> Vec<Value> {
        let (status, listed) = self
            .send("GET", "/internal/v1/telemetry/incidents", None)
            .await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        listed
            .get("incidents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The notices told so far: their kind, and whether to the connectors alone.
    fn told(&self) -> Vec<(NotificationKind, bool)> {
        self.notices
            .notices()
            .iter()
            .map(|notice| (notice.kind, notice.reminded))
            .collect()
    }
}

/// The count of model calls over the last hour against `alert`, and a
/// warning at `warning`.
fn calls_over(alert: f64, warning: Option<f64>) -> Value {
    json!({
        "name": "Model calls",
        "kind": "model_call",
        "measure": "count",
        "aggregation": "count",
        "window": "hour",
        "operator": "gt",
        "alert_threshold": alert,
        "warning_threshold": warning,
    })
}

/// ⚠ A breach opens an incident told once in the feed; a rise to `alert` is
/// told again to the connectors alone; a recovery resolves it, told in the
/// feed; and a breach within the hour reopens it, told to the connectors.
#[sqlx::test]
async fn a_breach_opens_an_incident_its_recovery_resolves_it_and_a_relapse_reopens_it(
    pool: PgPool,
) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let (status, made) = project
        .send(
            "POST",
            "/internal/v1/telemetry/rules",
            Some(&calls_over(5.0, Some(0.0))),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let rule = format!(
        "/internal/v1/telemetry/rules/{}",
        made["id"].as_str().expect("an id")
    );
    assert_eq!(made["severity"], "unknown");

    project.evaluate(&pool).await;
    let (_, read) = project.send("GET", &rule, None).await;
    assert_eq!(read["severity"], "warning", "{read}");
    let incidents = project.incidents().await;
    assert_eq!(incidents.len(), 1);
    assert_eq!(
        (&incidents[0]["status"], &incidents[0]["severity"]),
        (&json!("open"), &json!("warning"))
    );
    assert_eq!(project.told(), [(NotificationKind::IncidentOpened, false)]);
    let opened = &project.notices.notices()[0];
    assert_eq!(opened.title, "Model calls: warning");
    assert!(
        opened.body.contains("past the warning threshold of 0"),
        "{}",
        opened.body
    );

    project.evaluate(&pool).await;
    assert_eq!(project.told().len(), 1, "a severity held is told no more");

    let (_, _) = project
        .send("PUT", &rule, Some(&calls_over(0.5, Some(0.1))))
        .await;
    project.evaluate(&pool).await;
    assert_eq!(project.incidents().await[0]["severity"], "alert");
    assert_eq!(
        project.told()[1],
        (NotificationKind::IncidentOpened, true),
        "a rise is told again, to the connectors alone"
    );

    let (_, _) = project
        .send("PUT", &rule, Some(&calls_over(100.0, None)))
        .await;
    project.evaluate(&pool).await;
    let incidents = project.incidents().await;
    assert_eq!(incidents[0]["status"], "resolved");
    assert_eq!(
        incidents[0]["resolved_by"],
        Value::Null,
        "resolved by its recovery"
    );
    assert_eq!(
        project.told()[2],
        (NotificationKind::IncidentResolved, false)
    );

    let (_, _) = project
        .send("PUT", &rule, Some(&calls_over(0.0, None)))
        .await;
    project.evaluate(&pool).await;
    let incidents = project.incidents().await;
    assert_eq!(incidents.len(), 1, "reopened, not a second");
    assert_eq!(
        (&incidents[0]["status"], &incidents[0]["reopened"]),
        (&json!("open"), &json!(1))
    );
    assert_eq!(
        project.told()[3],
        (NotificationKind::IncidentOpened, true),
        "the feed holds its opening already, so the connectors alone hear it"
    );
}

/// A severity held is told again as often as the alert asks, until a member
/// takes the incident; one resolved by hand is told so in the feed.
#[sqlx::test]
async fn a_held_breach_is_told_again_until_a_member_takes_it(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let mut body = calls_over(0.0, None);
    body["renotify_minutes"] = json!(1);
    let (status, made) = project
        .send("POST", "/internal/v1/telemetry/rules", Some(&body))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");

    project.evaluate(&pool).await;
    let before_due = |minutes: i64| {
        sqlx::query("UPDATE telemetry.rules SET told_at = now() - make_interval(mins => $1::int)")
            .bind(minutes)
            .execute(&pool)
    };
    before_due(0).await.expect("told just now");
    project.evaluate(&pool).await;
    assert_eq!(project.told().len(), 1, "not due within its minute");
    before_due(2).await.expect("told two minutes ago");
    project.evaluate(&pool).await;
    assert_eq!(project.told()[1], (NotificationKind::IncidentOpened, true));
    assert_eq!(project.notices.notices()[1].metadata["reminder"], true);
    assert!(project.notices.notices()[1].body.starts_with("Still: "));

    let id = project.incidents().await[0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let (status, taken) = project
        .send(
            "POST",
            &format!("/internal/v1/telemetry/incidents/{id}/acknowledge"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{taken}");
    assert_eq!(taken["status"], "acknowledged");
    assert!(taken["acknowledged_by"].is_string());
    before_due(2).await.expect("told two minutes ago");
    project.evaluate(&pool).await;
    assert_eq!(
        project.told().len(),
        2,
        "a taken incident is reminded of no more"
    );

    let (status, resolved) = project
        .send(
            "POST",
            &format!("/internal/v1/telemetry/incidents/{id}/resolve"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(
        project.told()[2],
        (NotificationKind::IncidentResolved, false)
    );
    let (_, again) = project
        .send(
            "POST",
            &format!("/internal/v1/telemetry/incidents/{id}/resolve"),
            None,
        )
        .await;
    assert_eq!(again["status"], "resolved");
    assert_eq!(project.told().len(), 3, "resolved once, told once");
}

/// ⚠ An incident keeps each connection once and its last twenty runs, one shown again last.
#[sqlx::test]
async fn an_incident_keeps_its_connections_once_and_its_last_twenty_runs(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let (status, made) = project
        .send(
            "POST",
            "/internal/v1/telemetry/rules",
            Some(&calls_over(0.0, None)),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    project.evaluate(&pool).await;
    let id: Uuid = project.incidents().await[0]["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("an incident");

    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let runs: Vec<Uuid> = (0..25).map(|_| Uuid::new_v4()).collect();
    let mut tx = project_scope(&project.state.db, &project.project)
        .await
        .expect("the project's scope");
    let told = &project.project;
    incidents::told(&mut tx, told, id, &[b, a], None)
        .await
        .expect("kept");
    incidents::told(&mut tx, told, id, &[a], Some(runs[0]))
        .await
        .expect("kept");
    for run in &runs[1..] {
        incidents::told(&mut tx, told, id, &[], Some(*run))
            .await
            .expect("kept");
    }
    incidents::told(&mut tx, told, id, &[], Some(runs[10]))
        .await
        .expect("kept");
    // Another project's incident of that id is none of this one's to write.
    incidents::told(&mut tx, &ProjectId::new(), id, &[Uuid::new_v4()], None)
        .await
        .expect("nothing written");
    tx.commit().await.expect("commit");

    let kept = &project.incidents().await[0];
    let mut connections = vec![a, b];
    connections.sort_unstable();
    assert_eq!(kept["told_to"], json!(connections), "each once");
    let mut last: Vec<Uuid> = runs[5..]
        .iter()
        .copied()
        .filter(|run| *run != runs[10])
        .collect();
    last.push(runs[10]);
    assert_eq!(
        kept["runs"],
        json!(last),
        "the last twenty, the one shown again moved last"
    );
}

/// ⚠ A recovery that meets a member's resolution of its incident tells nothing.
#[sqlx::test]
async fn a_recovery_racing_a_members_resolution_is_not_told(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let (status, made) = project
        .send(
            "POST",
            "/internal/v1/telemetry/rules",
            Some(&calls_over(0.0, None)),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let rule = format!(
        "/internal/v1/telemetry/rules/{}",
        made["id"].as_str().expect("an id")
    );
    project.evaluate(&pool).await;
    let id: Uuid = project.incidents().await[0]["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("an incident");
    project
        .send("PUT", &rule, Some(&calls_over(100.0, None)))
        .await;

    // The member's resolution holds the incident's row while the loop
    // recovers it, and lands first.
    let mut member = pool.begin().await.expect("a transaction");
    sqlx::query("SELECT 1 FROM telemetry.incidents WHERE id = $1 FOR UPDATE")
        .bind(id)
        .execute(&mut *member)
        .await
        .expect("held");
    let pass = {
        let (state, pool) = (project.state.clone(), pool.clone());
        tokio::spawn(async move {
            sqlx::query("UPDATE telemetry.rules SET evaluated_at = NULL")
                .execute(&pool)
                .await
                .expect("due again");
            evaluate::pass(&state).await.expect("a pass");
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    sqlx::query(
        "UPDATE telemetry.incidents SET status = 'resolved', resolved_at = now(),
                resolved_by = 'user_member'
          WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *member)
    .await
    .expect("resolved by the member");
    member.commit().await.expect("commit");
    pass.await.expect("the pass");

    assert_eq!(project.incidents().await[0]["resolved_by"], "user_member");
    assert!(
        project
            .told()
            .iter()
            .all(|(kind, _)| *kind != NotificationKind::IncidentResolved),
        "the member's resolution is the one told: {:?}",
        project.told()
    );
}

/// ⚠ A window empty for as long as the window opens an incident where the
/// alert asks to be told of no data, and none where it does not.
#[sqlx::test]
async fn a_window_empty_for_its_length_opens_an_incident_where_the_alert_asks(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    for (name, missing) in [("Told", "notify_no_data"), ("Shown", "no_data")] {
        let body = json!({
            "name": name, "kind": "tool_call", "measure": "count", "aggregation": "count",
            "window": "hour", "operator": "lt", "alert_threshold": 1, "missing_data": missing,
        });
        let (status, made) = project
            .send("POST", "/internal/v1/telemetry/rules", Some(&body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
    }
    project.evaluate(&pool).await;
    assert!(
        project.incidents().await.is_empty(),
        "not yet for its length"
    );

    sqlx::query("UPDATE telemetry.rules SET severity_since = now() - interval '61 minutes'")
        .execute(&pool)
        .await
        .expect("empty for an hour");
    project.evaluate(&pool).await;
    let incidents = project.incidents().await;
    assert_eq!(incidents.len(), 1, "{incidents:?}");
    assert_eq!(
        (&incidents[0]["rule_name"], &incidents[0]["severity"]),
        (&json!("Told"), &json!("no_data"))
    );
    assert_eq!(project.told(), [(NotificationKind::IncidentOpened, false)]);
}
