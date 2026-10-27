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
use telmoni_shared::acting::{Acting, ActingProject};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    AlertCeilings, Grants, NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role,
    SpanKind, TelmoniError, UserId,
};
use telmoni_telemetry::handler::mutes::MUTES_ENDED_KEPT;
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

/// Every row of the project's organization's audit chain, oldest first, each
/// filed by its member inside the project: what was done to which kind of
/// thing, and how.
async fn filed(pool: &PgPool, project: &Project) -> Vec<(String, String, String, Value)> {
    let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT actor_id, action, resource_kind, resource_id, in_project, metadata::text
           FROM audit.events WHERE organization_id = $1 ORDER BY seq",
    )
    .bind(&project.organization)
    .fetch_all(pool)
    .await
    .expect("the chain");
    rows.into_iter()
        .map(|(actor, action, kind, id, in_project, metadata)| {
            assert_eq!(
                (actor.as_str(), in_project.as_str()),
                ("user_member", project.project.as_str())
            );
            let metadata = serde_json::from_str(&metadata).expect("details are JSON");
            (action, kind, id, metadata)
        })
        .collect()
}

/// ⚠ Every change a member makes to an alert, a window or an incident is
/// filed in the organization's audit chain, once, and a change to nothing is not.
#[sqlx::test]
async fn a_members_changes_to_alerting_are_filed_in_the_chain(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch).await;
    let connector = Uuid::new_v4();
    let rule = project.alert("Calls", 0.5, None, connector).await;
    let rule_id = rule.rsplit('/').next().expect("an id").to_owned();
    let unmute = format!("{rule}/connectors/{connector}/unmute");
    let (status, heard) = project.send("POST", &unmute, None).await;
    assert_eq!(status, StatusCode::OK, "{heard}");
    let body = json!({
        "name": "Calls", "kind": "model_call", "measure": "count", "aggregation": "count",
        "window": "hour", "operator": "gt", "alert_threshold": 0.5, "warning_threshold": 0.1,
        "connectors": [],
    });
    let (status, saved) = project.send("PUT", &rule, Some(&body)).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    for (verb, times) in [("pause", 2), ("resume", 1)] {
        for _ in 0..times {
            let (status, answered) = project.send("POST", &format!("{rule}/{verb}"), None).await;
            assert_eq!(status, StatusCode::OK, "{answered}");
        }
    }
    let ends_at = Utc::now() + TimeDelta::hours(1);
    let (status, window) = project
        .send(
            "POST",
            MUTES,
            Some(&json!({ "rule_id": rule_id, "ends_at": ends_at })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{window}");
    let window = window["id"].as_str().expect("an id").to_owned();
    for _ in 0..2 {
        let (status, ended) = project
            .send("POST", &format!("{MUTES}/{window}/end"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{ended}");
    }
    project.evaluate(&pool).await;
    let (_, listed) = project
        .send("GET", "/internal/v1/telemetry/incidents", None)
        .await;
    let incident = listed["incidents"][0]["id"]
        .as_str()
        .expect("an incident")
        .to_owned();
    for verb in ["acknowledge", "resolve", "resolve"] {
        let path = format!("/internal/v1/telemetry/incidents/{incident}/{verb}");
        let (status, answered) = project.send("POST", &path, None).await;
        assert_eq!(status, StatusCode::OK, "{answered}");
    }
    let (status, _) = project.send("DELETE", &rule, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let filed = filed(&pool, &project).await;
    let what: Vec<(&str, &str, &str)> = filed
        .iter()
        .map(|(action, kind, id, _)| (action.as_str(), kind.as_str(), id.as_str()))
        .collect();
    let (rule_id, window, incident) = (rule_id.as_str(), window.as_str(), incident.as_str());
    assert_eq!(
        what,
        [
            ("created", "alert", rule_id),
            ("updated", "alert", rule_id),
            ("updated", "alert", rule_id),
            ("updated", "alert", rule_id),
            ("updated", "alert", rule_id),
            ("created", "maintenance_window", window),
            ("updated", "maintenance_window", window),
            ("updated", "incident", incident),
            ("updated", "incident", incident),
            ("deleted", "alert", rule_id),
        ]
    );
    let details: Vec<&Value> = filed.iter().map(|(_, _, _, metadata)| metadata).collect();
    assert_eq!(details[0], &json!({ "name": "Calls" }));
    assert_eq!(
        details[1],
        &json!({ "name": "Calls", "unmuted": connector })
    );
    assert_eq!(
        details[2],
        &json!({ "name": "Calls", "changed": ["warning_threshold", "connectors"] })
    );
    assert_eq!(
        details[3],
        &json!({ "name": "Calls", "paused": { "from": null, "to": "person" } })
    );
    assert_eq!(
        details[4],
        &json!({ "name": "Calls", "paused": { "from": "person", "to": null } })
    );
    assert_eq!(details[5]["rule_id"], json!(rule_id));
    assert!(details[6]["ended_at"].is_string(), "{}", details[6]);
    assert_eq!(
        details[8],
        &json!({ "alert": "Calls", "rule_id": rule_id, "status": "resolved" })
    );
    assert_eq!(details[9], &json!({ "name": "Calls" }));
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
    let (again, ended_again) = mutes::end(
        &mut tx,
        &project.project,
        id.parse().expect("a window's id"),
        &UserId::new(),
    )
    .await
    .expect("answered");
    let again = again.expect("the window");
    assert!(!ended_again, "ended once already");
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

/// A member seated on a project with no alert, its module over Postgres alone.
fn bare(pool: &PgPool) -> (axum::Router, Arc<AppState>, OrganizationId, ProjectId) {
    let auth = AuthStub::new();
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    auth.seat(
        "user_member",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    let state = common::state(pool, auth);
    (router(state.clone()), state, organization, project)
}

/// The member's request on that project, answered with its status and body.
async fn bare_send(
    app: &axum::Router,
    organization: &OrganizationId,
    project: &ProjectId,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(SERVICE_SECRET_HEADER, SECRET)
        .header("authorization", "Bearer tok_user_member")
        .header("x-organization-id", organization.as_str())
        .header("x-project-id", project.as_str())
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .expect("a request");
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
        serde_json::from_slice(&bytes).expect("JSON")
    };
    (status, body)
}

/// ⚠ A project holds fifty windows that have not ended, so one in force is always listed.
#[sqlx::test]
async fn a_project_holds_fifty_windows_that_have_not_ended(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, _, organization, project) = bare(&pool);
    let ahead = |hours: i64| {
        json!({
            "starts_at": Utc::now() + TimeDelta::hours(hours),
            "ends_at": Utc::now() + TimeDelta::hours(hours + 1),
        })
    };
    let mut first = None;
    for hours in 1..=50 {
        let body = ahead(hours);
        let (status, made) =
            bare_send(&app, &organization, &project, "POST", MUTES, Some(&body)).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        first = first.or_else(|| made["id"].as_str().map(str::to_owned));
    }
    let body = ahead(51);
    let (status, _) = bare_send(&app, &organization, &project, "POST", MUTES, Some(&body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "the fifty-first");
    let end = format!("{MUTES}/{}/end", first.expect("a window"));
    let (status, ended) = bare_send(&app, &organization, &project, "POST", &end, None).await;
    assert_eq!(status, StatusCode::OK, "{ended}");
    let (status, _) = bare_send(&app, &organization, &project, "POST", MUTES, Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "room once one has ended");
}

/// ⚠ A project keeps as many ended windows as its list answers, the oldest going as another ends.
#[sqlx::test]
async fn a_project_keeps_its_newest_ended_windows(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, _, organization, project) = bare(&pool);
    sqlx::query(
        "INSERT INTO telemetry.mutes (id, project_id, starts_at, ends_at, created_by)
         SELECT gen_random_uuid(), $1, now() - make_interval(hours => n + 1),
                now() - make_interval(hours => n), 'user_member'
           FROM generate_series(1, $2::int) AS n",
    )
    .bind(&project)
    .bind(MUTES_ENDED_KEPT)
    .execute(&pool)
    .await
    .expect("windows ended an hour apart");
    let ends_at = Utc::now() + TimeDelta::hours(1);
    let (status, window) = bare_send(
        &app,
        &organization,
        &project,
        "POST",
        MUTES,
        Some(&json!({ "ends_at": ends_at })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{window}");
    let end = format!("{MUTES}/{}/end", window["id"].as_str().expect("an id"));
    let (status, ended) = bare_send(&app, &organization, &project, "POST", &end, None).await;
    assert_eq!(status, StatusCode::OK, "{ended}");

    let kept: (i64, i64) = sqlx::query_as(
        "SELECT count(*), floor(extract(epoch FROM now() - min(ends_at)) / 3600)::bigint
           FROM telemetry.mutes WHERE project_id = $1",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .expect("the windows kept");
    assert_eq!(
        kept,
        (MUTES_ENDED_KEPT, MUTES_ENDED_KEPT - 1),
        "the newest kept, the oldest gone"
    );
}

/// ⚠ A window in force is listed past any number ended before they began.
#[sqlx::test]
async fn a_window_in_force_is_listed_past_those_ended_ahead_of_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, _, organization, project) = bare(&pool);
    let now = json!({ "ends_at": Utc::now() + TimeDelta::hours(1) });
    let (status, held) = bare_send(&app, &organization, &project, "POST", MUTES, Some(&now)).await;
    assert_eq!(status, StatusCode::CREATED, "{held}");
    let ahead = json!({
        "starts_at": Utc::now() + TimeDelta::days(365),
        "ends_at": Utc::now() + TimeDelta::days(366),
    });
    for _ in 0..200 {
        let (status, made) =
            bare_send(&app, &organization, &project, "POST", MUTES, Some(&ahead)).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        let end = format!("{MUTES}/{}/end", made["id"].as_str().expect("an id"));
        let (status, ended) = bare_send(&app, &organization, &project, "POST", &end, None).await;
        assert_eq!(status, StatusCode::OK, "{ended}");
    }
    let (status, listed) = bare_send(&app, &organization, &project, "GET", MUTES, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let listed: Vec<&str> = listed["mutes"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|mute| mute["id"].as_str())
        .collect();
    assert!(
        listed.contains(&held["id"].as_str().expect("an id")),
        "the window in force fell off the list"
    );
}

/// ⚠ A window resolved before its project moved is refused, the project left
/// filed under the organization it went to.
#[sqlx::test]
async fn a_window_resolved_before_the_project_moved_is_refused(pool: PgPool) {
    let auth = AuthStub::probing(&pool);
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    auth.seat(
        "user_member",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    let app = router(common::state(&pool, auth.clone()));
    // As a transfer leaves it: telemetry's move committed, auth's not yet.
    let went_to = OrganizationId::new();
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id) VALUES ($1, $2)",
    )
    .bind(&project)
    .bind(&went_to)
    .execute(&pool)
    .await
    .expect("the moved project's settings");
    auth.answer_again(Ok(Acting {
        user_id: UserId::try_new("user_member").expect("a user id"),
        organization_id: went_to.clone(),
        organization_role: Some(OrganizationRole::Member),
        project: Some(ActingProject {
            project_id: project.clone(),
            role: Role::Member,
        }),
        session_id: None,
        expires_at: Utc::now().timestamp() + 3600,
    }));

    let body = json!({ "ends_at": Utc::now() + TimeDelta::hours(1) });
    let (status, made) = bare_send(&app, &organization, &project, "POST", MUTES, Some(&body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{made}");
    let filed: String = sqlx::query_scalar(
        "SELECT organization_id FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .expect("the settings row");
    assert_eq!(
        filed,
        went_to.to_string(),
        "filed back under the organization it left"
    );
    assert_eq!(
        auth.chain_held(),
        [true],
        "asked again outside the chain lock"
    );
}

/// ⚠ An alert paused by a request resolved before its project moved is
/// refused, and nothing is filed under the organization it left.
#[sqlx::test]
async fn an_alert_paused_as_its_project_moved_is_refused_and_not_filed(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    auth.seat(
        "user_member",
        &organization,
        &project,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    let app = router(common::state(&pool, auth.clone()));
    let body = json!({
        "name": "Calls", "kind": "model_call", "measure": "count", "aggregation": "count",
        "window": "hour", "operator": "gt", "alert_threshold": 0.5,
    });
    let (status, made) = bare_send(&app, &organization, &project, "POST", RULES, Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let rule = format!("{RULES}/{}", made["id"].as_str().expect("an id"));
    // As a transfer leaves it: auth's answer under the lock names where it went.
    auth.answer_again(Ok(Acting {
        user_id: UserId::try_new("user_member").expect("a user id"),
        organization_id: OrganizationId::new(),
        organization_role: Some(OrganizationRole::Member),
        project: Some(ActingProject {
            project_id: project.clone(),
            role: Role::Member,
        }),
        session_id: None,
        expires_at: Utc::now().timestamp() + 3600,
    }));

    let pause = format!("{rule}/pause");
    let (status, refused) = bare_send(&app, &organization, &project, "POST", &pause, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let (_, read) = bare_send(&app, &organization, &project, "GET", &rule, None).await;
    assert_eq!(read["paused"], Value::Null, "{read}");
    let filed: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit.events WHERE organization_id = $1 ORDER BY seq",
    )
    .bind(&organization)
    .fetch_all(&pool)
    .await
    .expect("the chain");
    assert_eq!(filed, ["created"], "the pause not filed");
}

/// ⚠ A window over every alert goes with its organization, though its project has no alert.
#[sqlx::test]
async fn a_projects_window_goes_with_its_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, state, organization, project) = bare(&pool);
    let body = json!({ "ends_at": Utc::now() + TimeDelta::days(30) });
    let (status, made) = bare_send(&app, &organization, &project, "POST", MUTES, Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");

    let mut tx = telmoni_shared::db::tenant_session::maintenance_scope(
        &state.db,
        telmoni_telemetry::db::TelemetryLane,
    )
    .await
    .expect("the lane");
    telmoni_telemetry::db::purge_organization(&mut tx, &organization)
        .await
        .expect("purged");
    tx.commit().await.expect("commit");
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry.mutes WHERE project_id = $1")
            .bind(&project)
            .fetch_one(&pool)
            .await
            .expect("a count");
    assert_eq!(left, 0, "the window outlived its organization");
}
