//! A project's starter alerts: three made by its first run with no click,
//! once, each filed as the service's and told to its feed; counted in the
//! plan's alerts, those past it not made and said so; read every five
//! minutes; and the failure share's set from the project's history once
//! fourteen days stand.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{DurationRound as _, TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, NoticeLog, SECRET, clickhouse_or_skip, eventually};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    AlertCeilings, ApiKeyScope, Grants, NotificationKind, OrganizationId, OrganizationRole,
    ProjectId, Role, SpanKind, SpanStatus, TelmoniError,
};
use telmoni_telemetry::db::TelemetryLane;
use telmoni_telemetry::rules::{db as rules_db, evaluate};
use telmoni_telemetry::{AppState, contract, otlp_router, router, starters};

const RULES: &str = "/internal/v1/telemetry/rules";
const OTLP: &str = "/otlp/v1/traces";
const KEYS: [&str; 3] = ["tm_ingest_first", "tm_ingest_second", "tm_ingest_third"];
const DETAIL: &str = "This organization's plan holds its alerts; more come with the plan above.";

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
            detail: DETAIL.into(),
        }))
    }
}

/// One organization, three of its projects, an ingest key on each, and a
/// member seated on each.
fn seated() -> (Arc<AuthStub>, OrganizationId, [ProjectId; 3]) {
    let auth = AuthStub::new();
    let organization = OrganizationId::new();
    let projects = [ProjectId::new(), ProjectId::new(), ProjectId::new()];
    for (project, key) in projects.iter().zip(KEYS) {
        auth.key(key, &organization, project, &[ApiKeyScope::Ingest]);
        auth.seat(
            "user_member",
            &organization,
            project,
            OrganizationRole::Member,
            Some(Role::Member),
        );
    }
    (auth, organization, projects)
}

/// The console's lanes and ingest's, as the binary nests them.
fn lanes(state: Arc<AppState>) -> Router {
    router(state.clone()).nest("/otlp", otlp_router(state))
}

/// One run exported under `key`, as an SDK sends it.
async fn export(app: &Router, key: &str) {
    let started = Utc::now() - TimeDelta::minutes(5);
    let nanos = |at: chrono::DateTime<Utc>| at.timestamp_nanos_opt().expect("in range").to_string();
    let trace = Uuid::new_v4().simple().to_string();
    let body = json!({ "resourceSpans": [{ "scopeSpans": [{ "scope": { "name": contract::SCOPE }, "spans": [{
        "traceId": trace, "spanId": "00f067aa0ba902b7", "name": "triage", "kind": 1,
        "startTimeUnixNano": nanos(started),
        "endTimeUnixNano": nanos(started + TimeDelta::seconds(2)),
        "attributes": [
            { "key": contract::RUN_ID, "value": { "stringValue": Uuid::now_v7().to_string() } },
            { "key": contract::AGENT, "value": { "stringValue": "triage" } },
            { "key": contract::OPERATION, "value": { "stringValue": "invoke_agent" } }
        ]
    }]}]}]});
    let answer = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(OTLP)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::OK, "the export lands");
}

/// A request from the member on `project`, as the console sends it.
async fn ask(
    app: &Router,
    method: &str,
    uri: &str,
    (organization, project): (&OrganizationId, &ProjectId),
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
        serde_json::from_slice(&bytes).expect("a JSON body")
    };
    (status, body)
}

/// The project's starters as their rows hold them, by name.
async fn starters_of(
    pool: &PgPool,
    project: &ProjectId,
) -> Vec<(String, String, f64, Option<f64>)> {
    sqlx::query_as(
        "SELECT name, measure, alert_threshold, warning_threshold FROM telemetry.rules
          WHERE project_id = $1 AND starter ORDER BY name",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("the starters read")
}

/// Whether the project's first run has been answered: its starters made, or
/// found no room for.
async fn answered(pool: &PgPool, project: &ProjectId) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT starters_made_at IS NOT NULL FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await
    .expect("the settings read")
    .unwrap_or(false)
}

/// The notices of the starters, in order.
fn told(notices: &NoticeLog) -> Vec<common::LoggedNotice> {
    notices
        .notices()
        .into_iter()
        .filter(|notice| notice.kind == NotificationKind::StarterAlerts)
        .collect()
}

/// ⚠ A project's first run makes its three starters with no click, each
/// filed as the service's and the feed told once; another export, another
/// process, or one deleted makes none again.
#[sqlx::test]
async fn a_projects_first_run_makes_its_three_starters_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (auth, organization, [project, ..]) = seated();
    let notices = NoticeLog::new();
    let state =
        common::state_with_stores_noticed(&pool, auth.clone(), Arc::new(NoLimits), notices.clone());
    let app = lanes(state.clone());

    export(&app, KEYS[0]).await;
    eventually("the first run makes three starters", || async {
        starters_of(&pool, &project).await.len() == 3
    })
    .await;
    assert_eq!(
        starters_of(&pool, &project).await,
        vec![
            (
                "Failing runs".to_owned(),
                "failures".to_owned(),
                0.1,
                Some(0.05)
            ),
            ("Looping runs".to_owned(), "repeats".to_owned(), 10.0, None),
            ("Stalled runs".to_owned(), "stalled".to_owned(), 1.0, None),
        ]
    );
    let filed: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT actor_id, action, resource_kind FROM audit.events
          WHERE organization_id = $1 ORDER BY seq",
    )
    .bind(&organization)
    .fetch_all(&pool)
    .await
    .expect("the chain reads");
    assert_eq!(
        filed,
        vec![
            (
                "service:starter-alerts".to_owned(),
                "created".to_owned(),
                "alert".to_owned()
            );
            3
        ]
    );
    let feed = told(&notices);
    assert_eq!(feed.len(), 1, "the feed is told once");
    assert_eq!(feed[0].project_id.as_ref(), Some(&project));
    assert!(
        feed[0]
            .body
            .contains("Failing runs, Looping runs and Stalled runs now watch its runs"),
        "{}",
        feed[0].body
    );

    let (status, list) = ask(&app, "GET", RULES, (&organization, &project), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let rules = list["rules"].as_array().expect("the alerts");
    assert!(
        rules.len() == 3 && rules.iter().all(|rule| rule["starter"] == json!(true)),
        "each is named a starter: {list}"
    );
    assert_eq!(list["starters_withheld"], json!([]));
    assert_eq!(list["starters_detail"], Value::Null);

    let looping = rules
        .iter()
        .find(|rule| rule["name"] == "Looping runs")
        .expect("the looping starter");
    let gone = format!("{RULES}/{}", looping["id"].as_str().expect("an id"));
    let (status, _) = ask(&app, "DELETE", &gone, (&organization, &project), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    export(&app, KEYS[0]).await;
    let another = lanes(common::state_with_stores_noticed(
        &pool,
        auth,
        Arc::new(NoLimits),
        NoticeLog::new(),
    ));
    export(&another, KEYS[0]).await;
    starters::make(&state, &organization, &project)
        .await
        .expect("a making asked again");
    assert_eq!(
        starters_of(&pool, &project).await.len(),
        2,
        "one deleted is never made again"
    );
    assert_eq!(told(&notices).len(), 1, "nor told again");
}

/// ⚠ A plan of ten holds a Hobby organization's two projects' six starters
/// and four alerts of its own, the fifth refused in the plan's words.
#[sqlx::test]
async fn a_hobby_organizations_two_projects_are_watched_with_four_alerts_still_to_make(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (auth, organization, [first, second, _]) = seated();
    let state =
        common::state_with_stores_noticed(&pool, auth, Arc::new(Holds(10)), NoticeLog::new());
    let app = lanes(state);

    export(&app, KEYS[0]).await;
    export(&app, KEYS[1]).await;
    for project in [&first, &second] {
        eventually("each project's first run makes three starters", || async {
            starters_of(&pool, project).await.len() == 3
        })
        .await;
    }
    for n in 1..=4 {
        let body = json!({ "name": format!("Our alert {n}"), "kind": "run", "measure": "count",
            "aggregation": "count", "window": "hour", "operator": "gt", "alert_threshold": 100 });
        let (status, made) = ask(&app, "POST", RULES, (&organization, &first), Some(&body)).await;
        assert_eq!(status, StatusCode::CREATED, "alert {n} of its own: {made}");
    }
    let body = json!({ "name": "One too many", "kind": "run", "measure": "count",
        "aggregation": "count", "window": "hour", "operator": "gt", "alert_threshold": 100 });
    let (status, refused) = ask(&app, "POST", RULES, (&organization, &second), Some(&body)).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{refused}");
    assert_eq!(refused["detail"], DETAIL);
}

/// ⚠ A starter that would pass the plan's number is not made, and the alerts
/// page and the feed say which, in the plan's own words; a project with no
/// room at all is told none were made.
#[sqlx::test]
async fn a_project_past_the_plans_number_is_made_without_its_starters_and_says_so(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let Some(_ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (auth, organization, [first, second, _]) = seated();
    let notices = NoticeLog::new();
    let state = common::state_with_stores_noticed(&pool, auth, Arc::new(Holds(2)), notices.clone());
    let app = lanes(state);

    export(&app, KEYS[0]).await;
    eventually("the first project's first run is answered", || async {
        answered(&pool, &first).await
    })
    .await;
    let made: Vec<String> = starters_of(&pool, &first)
        .await
        .into_iter()
        .map(|(name, ..)| name)
        .collect();
    assert_eq!(made, vec!["Failing runs", "Looping runs"]);
    let (_, list) = ask(&app, "GET", RULES, (&organization, &first), None).await;
    assert_eq!(list["starters_withheld"], json!(["stalled"]), "{list}");
    assert_eq!(list["starters_detail"], DETAIL);
    eventually("the feed is told", || async { !told(&notices).is_empty() }).await;
    let first_told = &told(&notices)[0];
    assert!(
        first_told.body.contains("Not made: Stalled runs.") && first_told.body.contains(DETAIL),
        "{}",
        first_told.body
    );

    export(&app, KEYS[1]).await;
    eventually("the second project's first run is answered", || async {
        answered(&pool, &second).await
    })
    .await;
    assert!(starters_of(&pool, &second).await.is_empty());
    let (_, list) = ask(&app, "GET", RULES, (&organization, &second), None).await;
    assert_eq!(
        list["starters_withheld"],
        json!(["failures", "repeats", "stalled"]),
        "{list}"
    );
    eventually("the second feed is told", || async {
        told(&notices).len() == 2
    })
    .await;
    let second_told = &told(&notices)[1];
    assert_eq!(
        second_told.title,
        "No starter alerts made: the plan is full"
    );
    assert!(
        second_told
            .body
            .contains("Not made: Failing runs, Looping runs and Stalled runs."),
        "{}",
        second_told.body
    );
}

/// A starter is leased five minutes after its last evaluation, and a
/// member's alert one minute after its own.
#[sqlx::test]
async fn a_starter_is_read_every_five_minutes(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (auth, organization, [project, ..]) = seated();
    let state = common::state(&pool, auth);
    let app = router(state.clone());
    let body = json!({ "name": "Ours", "kind": "run", "measure": "count",
        "aggregation": "count", "window": "hour", "operator": "gt", "alert_threshold": 100 });
    let (status, ours) = ask(&app, "POST", RULES, (&organization, &project), Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED, "{ours}");
    starters::make(&state, &organization, &project)
        .await
        .expect("the starters made");
    let due = |ago: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "UPDATE telemetry.rules SET evaluated_at = now() - make_interval(secs => $1)",
            )
            .bind(ago)
            .execute(&pool)
            .await
            .expect("the evaluations moved");
            let mut tx = maintenance_scope(&pool, TelemetryLane)
                .await
                .expect("the lane");
            let leased = rules_db::lease_due(&mut tx, (60, 300), Uuid::new_v4(), 120, 100)
                .await
                .expect("a lease");
            tx.rollback().await.expect("let go");
            let mut starters: Vec<bool> = leased.iter().map(|rule| rule.starter).collect();
            starters.sort_unstable();
            starters
        }
    };
    assert_eq!(
        due(120).await,
        vec![false],
        "two minutes on, the member's alone"
    );
    assert_eq!(
        due(360).await,
        vec![false, true, true, true],
        "six minutes on, the starters too"
    );
    assert_eq!(
        evaluate::STARTER_EVERY,
        std::time::Duration::from_secs(300),
        "the loop leases starters every five minutes"
    );
}

/// ⚠ Once fourteen days stand, the failure share's starter takes the 99th
/// percentile of its project's hours, as the form suggests; a project with
/// no run in them keeps the book's number, and one a member edited first
/// keeps theirs. Each is set once.
#[sqlx::test]
async fn the_failing_runs_starter_takes_its_projects_history_once_fourteen_days_stand(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let (auth, organization, [busy, quiet, edited]) = seated();
    let state =
        common::state_with_stores_noticed(&pool, auth, Arc::new(NoLimits), NoticeLog::new());
    let app = lanes(state.clone());
    for (key, project) in KEYS.iter().zip([&busy, &quiet, &edited]) {
        export(&app, key).await;
        eventually("each first run makes its starters", || async {
            starters_of(&pool, project).await.len() == 3
        })
        .await;
    }
    let (_, list) = ask(&app, "GET", RULES, (&organization, &edited), None).await;
    let failing = list["rules"]
        .as_array()
        .expect("the alerts")
        .iter()
        .find(|rule| rule["name"] == "Failing runs")
        .expect("the failing starter");
    let chosen = json!({ "name": "Failing runs", "kind": "run", "measure": "failures",
        "aggregation": "avg", "window": "hour", "operator": "gt", "alert_threshold": 0.3,
        "warning_threshold": 0.05, "missing_data": "zero" });
    let at = format!("{RULES}/{}", failing["id"].as_str().expect("an id"));
    let (status, saved) = ask(&app, "PUT", &at, (&organization, &edited), Some(&chosen)).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    sqlx::query("UPDATE telemetry.rules SET created_at = now() - interval '15 days'")
        .execute(&pool)
        .await
        .expect("the starters made fifteen days ago");

    // Twenty hours of the days before today, a quarter of each hour's runs failed.
    let today = Utc::now()
        .duration_trunc(TimeDelta::days(1))
        .expect("a day");
    let mut rows = Vec::new();
    for hour in 1..=20 {
        for n in 0..4 {
            let mut run = common::received(
                &busy,
                SpanKind::Run,
                "triage",
                1_000,
                today - TimeDelta::hours(hour) + TimeDelta::minutes(n),
            );
            run.trace_id = Uuid::new_v4().simple().to_string();
            run.span_id = format!("{:016x}", hour * 4 + n);
            if n == 0 {
                run.status = SpanStatus::Error;
            }
            rows.push(run);
        }
    }
    ch.store
        .insert_spans(&rows)
        .await
        .expect("the history is kept");

    evaluate::pass(&state).await.expect("a pass");
    let failing_of = |project: ProjectId| {
        let pool = pool.clone();
        async move {
            starters_of(&pool, &project)
                .await
                .into_iter()
                .find(|(name, ..)| name == "Failing runs")
                .map(|(_, _, alert, warning)| (alert, warning))
                .expect("the failing starter")
        }
    };
    assert_eq!(
        failing_of(busy.clone()).await,
        (0.25, None),
        "its own 99th percentile"
    );
    assert_eq!(
        failing_of(quiet.clone()).await,
        (0.1, Some(0.05)),
        "the book's number"
    );
    assert_eq!(
        failing_of(edited.clone()).await,
        (0.3, Some(0.05)),
        "the member's"
    );
    let tuned: Vec<(String, Value)> = sqlx::query_as(
        "SELECT actor_id, metadata->'changed' FROM audit.events
          WHERE organization_id = $1 AND action = 'updated' ORDER BY seq",
    )
    .bind(&organization)
    .fetch_all(&pool)
    .await
    .expect("the chain reads");
    assert_eq!(
        tuned,
        vec![
            ("user_member".to_owned(), json!(["alert_threshold"])),
            (
                "service:starter-alerts".to_owned(),
                json!(["alert_threshold", "warning_threshold"])
            ),
        ],
        "the member's edit and the service's tune, each filed"
    );
    let unset: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.rules WHERE measure = 'failures' AND tuned_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("the rows read");
    assert_eq!(unset, 0, "each is set once");
}
