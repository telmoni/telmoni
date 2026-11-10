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
use telmoni_shared::acting::{Acting, ActingProject};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{IncidentAct, IncidentActed, NoLimits, Telemetry};
use telmoni_shared::test_util::{
    apply_audit_migrations, holding_chain, until_queued_on_advisory_locks,
};
use telmoni_shared::{
    AuthError, NotificationKind, OrganizationId, OrganizationRole, OrganizationStatus, ProjectId,
    Role, SpanKind, TelmoniError, UserId,
};
use telmoni_telemetry::rules::{evaluate, incidents};
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::{AppState, router};

/// A project with a member seated on it, the module over the stores with
/// its notices logged, and one model call of the project's this minute.
struct Project {
    app: axum::Router,
    state: Arc<AppState>,
    auth: Arc<AuthStub>,
    notices: Arc<NoticeLog>,
    organization: OrganizationId,
    project: ProjectId,
}

impl Project {
    async fn new(pool: &PgPool, ch: &common::ClickHouse) -> Self {
        apply_audit_migrations(pool).await;
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
        let state = common::state_with_stores_noticed(
            pool,
            auth.clone(),
            Arc::new(NoLimits),
            notices.clone(),
        );
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
            auth,
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
    let id = project.incidents().await[0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let take = format!("/internal/v1/telemetry/incidents/{id}/acknowledge");
    let (status, taken) = project.send("POST", &take, None).await;
    assert_eq!(status, StatusCode::OK, "{taken}");

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
        (
            &incidents[0]["acknowledged_at"],
            &incidents[0]["acknowledged_by"]
        ),
        (&Value::Null, &Value::Null),
        "reopened untaken"
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

/// ⚠ A press of an incident notice's buttons acts as the console does, as
/// the person it is for: each opening's notice carries its incident;
/// Acknowledge takes it, filed as the console's own Acknowledge is; Mute
/// makes its alert an hour's window, once; and a person with no seat, or an
/// incident gone, is refused with nothing written.
#[sqlx::test]
async fn a_press_acts_on_an_incident_as_the_console_does(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    for name in ["Calls A", "Calls B"] {
        let mut body = calls_over(0.0, None);
        body["name"] = json!(name);
        let (status, made) = project
            .send("POST", "/internal/v1/telemetry/rules", Some(&body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
    }
    project.evaluate(&pool).await;
    let opened = project.incidents().await;
    let id_of = |incident: &Value| -> Uuid {
        incident["id"]
            .as_str()
            .and_then(|id| id.parse().ok())
            .expect("an id")
    };
    let mut ids: Vec<Uuid> = opened.iter().map(id_of).collect();
    ids.sort();
    let mut carried: Vec<Uuid> = project
        .notices
        .notices()
        .iter()
        .filter_map(|notice| notice.incident.as_ref().map(|(id, _)| *id))
        .collect();
    carried.sort();
    assert_eq!(carried, ids, "each opening carries its incident");

    let (status, taken) = project
        .send(
            "POST",
            &format!("/internal/v1/telemetry/incidents/{}/acknowledge", ids[0]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{taken}");
    let seam = TelemetrySeam(project.state.clone());
    let member = Acting {
        user_id: UserId::try_new("user_member").expect("an id"),
        organization_id: project.organization.clone(),
        organization_role: Some(OrganizationRole::Member),
        project: Some(ActingProject {
            project_id: project.project.clone(),
            role: Role::Member,
        }),
        session_id: None,
        expires_at: Utc::now().timestamp() + 60,
    };
    let press = |id, act| seam.act_on_incident(&member, id, act);
    assert_eq!(
        press(ids[1], IncidentAct::Acknowledge)
            .await
            .expect("taken"),
        IncidentActed::Acknowledged
    );
    assert_eq!(
        press(ids[1], IncidentAct::Acknowledge)
            .await
            .expect("answered"),
        IncidentActed::AcknowledgedAlready
    );
    let filed: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT actor_id, action, in_project, (metadata - 'alert' - 'rule_id')::text
           FROM audit.events WHERE organization_id = $1 AND resource_kind = 'incident'
          ORDER BY seq",
    )
    .bind(&project.organization)
    .fetch_all(&pool)
    .await
    .expect("the chain");
    assert_eq!(
        filed.len(),
        2,
        "each taken once, the second press filed nothing"
    );
    assert_eq!(filed[0], filed[1], "a press is filed as the console's own");

    let muted = press(ids[0], IncidentAct::MuteForAnHour)
        .await
        .expect("muted");
    let IncidentActed::Muted { until } = muted else {
        panic!("not muted: {muted:?}");
    };
    let left = until - Utc::now();
    assert!(
        left > TimeDelta::minutes(59) && left <= TimeDelta::hours(1),
        "{left}"
    );
    assert_eq!(
        press(ids[0], IncidentAct::MuteForAnHour)
            .await
            .expect("answered"),
        IncidentActed::MutedAlready { until }
    );
    let (_, windows) = project
        .send("GET", "/internal/v1/telemetry/mutes", None)
        .await;
    let windows = windows["mutes"].as_array().cloned().unwrap_or_default();
    assert_eq!(windows.len(), 1, "muted once");
    let alert = opened
        .iter()
        .find(|incident| id_of(incident) == ids[0])
        .map(|incident| incident["rule_id"].clone());
    assert_eq!(Some(windows[0]["rule_id"].clone()), alert, "its own alert");

    let stranger = Acting {
        user_id: UserId::try_new("user_stranger").expect("an id"),
        ..member.clone()
    };
    let refused = seam
        .act_on_incident(&stranger, ids[1], IncidentAct::MuteForAnHour)
        .await;
    assert!(
        matches!(refused, Err(TelmoniError::Authz(_))),
        "{refused:?}"
    );
    let gone = press(Uuid::now_v7(), IncidentAct::MuteForAnHour).await;
    assert!(
        matches!(gone, Err(TelmoniError::Auth(AuthError::NotFound(_)))),
        "{gone:?}"
    );
    let (_, windows) = project
        .send("GET", "/internal/v1/telemetry/mutes", None)
        .await;
    assert_eq!(
        windows["mutes"].as_array().map(Vec::len),
        Some(1),
        "nothing more made"
    );
}

/// ⚠ A press of Mute reads the windows in force under the organization's
/// chain lock: one made while it waited for the lock is the one it answers,
/// and it makes none of its own.
#[sqlx::test]
async fn a_press_of_mute_reads_the_windows_under_the_chain_lock(pool: PgPool) {
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
    let opened = project.incidents().await;
    let incident: Uuid = opened[0]["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("an id");
    let rule: Uuid = opened[0]["rule_id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("its alert");
    let member = Acting {
        user_id: UserId::try_new("user_member").expect("an id"),
        organization_id: project.organization.clone(),
        organization_role: Some(OrganizationRole::Member),
        project: Some(ActingProject {
            project_id: project.project.clone(),
            role: Role::Member,
        }),
        session_id: None,
        expires_at: Utc::now().timestamp() + 60,
    };

    let held = holding_chain(&pool, &project.organization).await;
    let seam = TelemetrySeam(project.state.clone());
    let press = tokio::spawn(async move {
        seam.act_on_incident(&member, incident, IncidentAct::MuteForAnHour)
            .await
    });
    until_queued_on_advisory_locks(&pool, 1).await;
    let until = Utc::now() + TimeDelta::hours(2);
    sqlx::query(
        "INSERT INTO telemetry.mutes (id, project_id, rule_id, starts_at, ends_at, created_by)
         VALUES ($1, $2, $3, now(), $4, 'user_member')",
    )
    .bind(Uuid::now_v7())
    .bind(&project.project)
    .bind(rule)
    .bind(until)
    .execute(&pool)
    .await
    .expect("a window made while the press waits");
    held.commit().await.expect("the lock let go");

    let acted = press.await.expect("the press ran").expect("answered");
    let IncidentActed::MutedAlready { until: answered } = acted else {
        panic!("a second window: {acted:?}");
    };
    assert!(
        (answered - until).abs() < TimeDelta::seconds(1),
        "{answered}"
    );
    let windows: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.mutes")
        .fetch_one(&pool)
        .await
        .expect("the windows");
    assert_eq!(windows, 1, "none of its own");
}

/// ⚠ A breach a member resolved stays quiet at its severity, and a rise past it opens it again.
#[sqlx::test]
async fn a_rise_past_the_severity_a_member_resolved_opens_it_again(pool: PgPool) {
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
    project.evaluate(&pool).await;
    let id = project.incidents().await[0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let (status, resolved) = project
        .send(
            "POST",
            &format!("/internal/v1/telemetry/incidents/{id}/resolve"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");

    project.evaluate(&pool).await;
    assert_eq!(project.incidents().await[0]["status"], "resolved");
    assert_eq!(project.told().len(), 2, "a warning held stays quiet");

    let (_, _) = project
        .send("PUT", &rule, Some(&calls_over(0.5, Some(0.1))))
        .await;
    project.evaluate(&pool).await;
    let incidents = project.incidents().await;
    assert_eq!(incidents.len(), 1, "reopened, not a second");
    assert_eq!(
        (&incidents[0]["status"], &incidents[0]["severity"]),
        (&json!("open"), &json!("alert"))
    );
    assert_eq!(
        project.told()[2],
        (NotificationKind::IncidentOpened, true),
        "the rise is told"
    );
}

/// ⚠ An incident keeps each connection once and its last twenty traces, one shown again last.
#[sqlx::test]
async fn an_incident_keeps_its_connections_once_and_its_last_twenty_traces(pool: PgPool) {
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
    let traces: Vec<String> = (0..25)
        .map(|_| Uuid::new_v4().simple().to_string())
        .collect();
    let mut tx = project_scope(&project.state.db, &project.project)
        .await
        .expect("the project's scope")
        .bind_organization(&project.organization)
        .await
        .expect("and its organization's, which every write binds");
    let told = &project.project;
    incidents::told(&mut tx, told, id, &[b, a], None)
        .await
        .expect("kept");
    incidents::told(&mut tx, told, id, &[a], Some(&traces[0]))
        .await
        .expect("kept");
    for trace in &traces[1..] {
        incidents::told(&mut tx, told, id, &[], Some(trace))
            .await
            .expect("kept");
    }
    incidents::told(&mut tx, told, id, &[], Some(&traces[10]))
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
    let mut last: Vec<String> = traces[5..]
        .iter()
        .filter(|trace| **trace != traces[10])
        .cloned()
        .collect();
    last.push(traces[10].clone());
    assert_eq!(
        kept["traces"],
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
    // Only once the pass waits on that row is it a race the member wins.
    let mut waiting = false;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        waiting = sqlx::query_scalar(
            "SELECT count(*) > 0 FROM pg_stat_activity
              WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&pool)
        .await
        .expect("who waits");
        if waiting {
            break;
        }
    }
    assert!(waiting, "the pass never reached the member's row");
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

/// ⚠ An empty window opens an incident at its first reading, once, where the alert asks.
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
    let incidents = project.incidents().await;
    assert_eq!(incidents.len(), 1, "{incidents:?}");
    assert_eq!(
        (&incidents[0]["rule_name"], &incidents[0]["severity"]),
        (&json!("Told"), &json!("no_data"))
    );
    assert_eq!(project.told(), [(NotificationKind::IncidentOpened, false)]);

    project.evaluate(&pool).await;
    assert_eq!(project.incidents().await.len(), 1, "the one incident");
    assert_eq!(project.told().len(), 1, "told once a spell");

    // Resolved by hand while the window stays empty: not opened again this spell.
    let id = project.incidents().await[0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let resolve = format!("/internal/v1/telemetry/incidents/{id}/resolve");
    let (status, resolved) = project.send("POST", &resolve, None).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    let told = project.told().len();
    project.evaluate(&pool).await;
    assert_eq!(project.incidents().await[0]["status"], "resolved");
    assert_eq!(project.told().len(), told, "opened again within its spell");
}

/// ⚠ A reading of no data leaves an open incident standing and reminded,
/// and follows it there, told, where the alert asks to be told of no data.
#[sqlx::test]
async fn a_reading_of_no_data_leaves_an_open_incident_standing(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    for (name, missing) in [("Shown", "no_data"), ("Told", "notify_no_data")] {
        let body = json!({
            "name": name, "kind": "model_call", "measure": "count", "aggregation": "count",
            "window": "hour", "operator": "gt", "alert_threshold": 0.5,
            "missing_data": missing, "renotify_minutes": 1,
        });
        let (status, made) = project
            .send("POST", "/internal/v1/telemetry/rules", Some(&body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
    }
    project.evaluate(&pool).await;
    assert_eq!(
        project.told(),
        [(NotificationKind::IncidentOpened, false); 2]
    );

    // The window empties, the alerts reading tool calls, of which there are
    // none; and each is due to be told again.
    sqlx::query(
        "UPDATE telemetry.rules SET kind = 'tool_call', told_at = now() - interval '2 minutes'",
    )
    .execute(&pool)
    .await
    .expect("an empty window");
    project.evaluate(&pool).await;
    let incidents = project.incidents().await;
    let standing = |name: &str| {
        incidents
            .iter()
            .find(|incident| incident["rule_name"] == name)
            .map(|incident| (incident["status"].clone(), incident["severity"].clone()))
    };
    assert_eq!(standing("Shown"), Some((json!("open"), json!("alert"))));
    assert_eq!(standing("Told"), Some((json!("open"), json!("no_data"))));
    assert_eq!(
        project.told()[2..],
        [(NotificationKind::IncidentOpened, true); 2],
        "reminded, and told of no data"
    );
    let reminder = project
        .notices
        .notices()
        .into_iter()
        .find(|notice| notice.reminded && notice.title.starts_with("Shown"))
        .expect("the reminder");
    assert!(
        reminder
            .body
            .contains("has no value now; its incident stands past the alert threshold"),
        "{}",
        reminder.body
    );
}

/// A window holding no span, read as 0, is told as the 0 it crossed at.
#[sqlx::test]
async fn an_empty_window_read_as_zero_is_told_as_zero(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let body = json!({
        "name": "Tools quiet", "kind": "tool_call", "measure": "count", "aggregation": "count",
        "window": "hour", "operator": "lt", "alert_threshold": 1,
    });
    let (status, made) = project
        .send("POST", "/internal/v1/telemetry/rules", Some(&body))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    project.evaluate(&pool).await;
    let notices = project.notices.notices();
    assert_eq!(notices.len(), 1, "one notice");
    assert!(
        notices[0]
            .body
            .contains(" is 0, past the alert threshold of 1."),
        "{}",
        notices[0].body
    );
    assert_eq!(notices[0].metadata["value"], json!(0.0));
}

/// ⚠ A budget with no tag sums its window's full hours from the hourly
/// totals, each call scaled by its trace's sample rate, so an hour whose spans
/// a short retention purged still counts.
#[sqlx::test]
async fn a_budget_counts_the_hours_whose_spans_are_gone(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let earlier = Utc::now() - TimeDelta::hours(3);
    ch.store
        .insert_spans(&[SpanRow {
            model: "claude-sonnet-5-5".into(),
            cost: Some(2.0),
            sample_rate: Some(0.25),
            ..common::received(
                &project.project,
                SpanKind::ModelCall,
                "chat claude-sonnet-5-5",
                1_200,
                earlier,
            )
        }])
        .await
        .expect("the call is kept");
    ch.migrator
        .query(
            "DELETE FROM telemetry.spans WHERE project_id = ? \
             AND received_at < fromUnixTimestamp64Micro(?)",
        )
        .bind(project.project.as_str())
        .bind((earlier + TimeDelta::minutes(1)).timestamp_micros())
        .execute()
        .await
        .expect("the hour's spans purged");
    let body = json!({
        "name": "Daily budget", "kind": "agent", "measure": "cost", "aggregation": "sum",
        "window": "day", "operator": "gt", "alert_threshold": 5,
    });
    let (status, made) = project
        .send("POST", "/internal/v1/telemetry/rules", Some(&body))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    project.evaluate(&pool).await;
    let notices = project.notices.notices();
    assert_eq!(notices.len(), 1, "the budget breached");
    assert_eq!(
        notices[0].metadata["value"],
        json!(8.0),
        "one trace in four, $2 each"
    );
}

/// ⚠ An organization being erased has its alerts neither evaluated nor told.
#[sqlx::test]
async fn an_organization_being_erased_tells_nothing(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let (status, made) = project
        .send(
            "POST",
            "/internal/v1/telemetry/rules",
            Some(&calls_over(0.5, None)),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    project.auth.stand(
        &project.organization,
        Some(OrganizationStatus::PendingDeletion),
    );
    project.evaluate(&pool).await;
    assert!(project.incidents().await.is_empty(), "an incident opened");
    assert!(project.told().is_empty(), "told while being erased");
}
