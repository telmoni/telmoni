//! What an incident names of its agents' changes, through the module's
//! router and its loops: a breach's notice and its incident naming the
//! release its agent changed to in the day before it opened, and none where
//! nothing changed; a change older than the day, one after the opening, one
//! hidden, another agent's and another project's left out; an alert over
//! every agent naming each change's agent, and one held to an environment
//! keeping to it; a monitor's and an objective's naming their own agent's;
//! five named and the rest counted, inside the body's bound; a change filed
//! late on the incident alone; a reopening keeping its window; and changes
//! that cannot be read leaving the notice and the incident standing.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, ClickHouse, LoggedNotice, NoticeLog, SECRET, clickhouse_or_skip};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::middleware::api_key::{ApiKeyGate, refuse_as_problem, require_api_key};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    ApiKeyScope, NotificationKind, OrganizationId, OrganizationRole, ProjectId, Role, SpanKind,
    SpanStatus,
};
use telmoni_telemetry::changes::{self, Opened, Scope};
use telmoni_telemetry::rules::evaluate;
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::{AppState, monitors, router, v1_reads};

const RULES: &str = "/internal/v1/telemetry/rules";
const INCIDENTS: &str = "/internal/v1/telemetry/incidents";
const MONITORS: &str = "/internal/v1/telemetry/monitors";
const READ_KEY: &str = "tm_read_only_key";
const SUPPORT: &str = "support";
/// The agent a monitor watches.
const AGENT: &str = "nightly-triage";
const LEAD: &str = "Changed in the 24 hours before it opened:";

fn at(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .expect("a time")
        .with_timezone(&Utc)
}

struct Project {
    app: Router,
    state: Arc<AppState>,
    notices: Arc<NoticeLog>,
    organization: OrganizationId,
    project: ProjectId,
}

impl Project {
    /// A project whose notices link the console, a member seated on it and a
    /// key that reads it, holding `spans`.
    async fn new(
        pool: &PgPool,
        ch: &ClickHouse,
        spans: impl FnOnce(&ProjectId) -> Vec<SpanRow>,
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
        auth.key(READ_KEY, &organization, &project, &[ApiKeyScope::Read]);
        let notices = NoticeLog::new();
        let state = common::state_with_console(pool, auth, notices.clone());
        ch.store
            .insert_spans(&spans(&project))
            .await
            .expect("the spans are kept");
        let gate = ApiKeyGate {
            auth: state.auth.clone(),
            scope: ApiKeyScope::Read,
            refuse: refuse_as_problem,
        };
        let app = router(state.clone()).nest(
            "/v1",
            v1_reads(state.clone())
                .route_layer(axum::middleware::from_fn_with_state(gate, require_api_key)),
        );
        Self {
            app,
            state,
            notices,
            organization,
            project,
        }
    }

    async fn answer(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.app.clone().oneshot(request).await.expect("an answer");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// The member's request through the console's lane.
    async fn send(&self, method: &str, uri: &str, body: &Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", "Bearer tok_user_member")
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.project.as_str())
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request");
        self.answer(request).await
    }

    /// A read with the project's key on `/v1`.
    async fn keyed(&self, uri: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .uri(format!("/v1{uri}"))
            .header("authorization", format!("Bearer {READ_KEY}"))
            .body(Body::empty())
            .expect("a request");
        self.answer(request).await
    }

    /// What `body` makes through `lane`: its id.
    async fn make(&self, lane: &str, body: &Value) -> String {
        let (status, made) = self.send("POST", lane, body).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        made.get("id")
            .and_then(Value::as_str)
            .expect("an id")
            .to_owned()
    }

    async fn evaluate(&self, pool: &PgPool) {
        sqlx::query("UPDATE telemetry.rules SET evaluated_at = NULL")
            .execute(pool)
            .await
            .expect("due again");
        evaluate::pass(&self.state).await.expect("a pass");
    }

    /// The project's incidents, as the console's list answers them.
    async fn incidents(&self) -> Vec<Value> {
        let (status, listed) = self.send("GET", INCIDENTS, &Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        listed
            .get("incidents")
            .and_then(Value::as_array)
            .expect("incidents")
            .clone()
    }

    /// The project's one incident.
    async fn incident(&self) -> Value {
        let mut listed = self.incidents().await;
        assert_eq!(listed.len(), 1, "{listed:?}");
        listed.remove(0)
    }

    /// The notice titled `title`, the `nth` of them.
    fn told(&self, title: &str, nth: usize) -> LoggedNotice {
        self.notices
            .notices()
            .into_iter()
            .filter(|notice| notice.title == title)
            .nth(nth)
            .expect("a notice of that title")
    }
}

/// A span of `agent`'s trace `trace` in production, ended `ago` seconds since.
fn span(
    project: &ProjectId,
    agent: &str,
    trace: Uuid,
    kind: SpanKind,
    name: &str,
    ago: i64,
) -> SpanRow {
    SpanRow {
        trace_id: trace.simple().to_string(),
        agent: agent.into(),
        environment: "production".into(),
        ..common::received(
            project,
            kind,
            name,
            1_000,
            Utc::now() - TimeDelta::seconds(ago),
        )
    }
}

/// A trace of `agent` that failed and one that did not: half its traces failed.
fn failing(project: &ProjectId, agent: &str) -> Vec<SpanRow> {
    vec![
        SpanRow {
            status: SpanStatus::Error,
            error_type: "TimeoutError".into(),
            ..span(project, agent, Uuid::new_v4(), SpanKind::Agent, "trip", 10)
        },
        span(project, agent, Uuid::new_v4(), SpanKind::Agent, "trip", 5),
    ]
}

/// An alert on the share of traces failed past `threshold`, read through `filters`.
fn failures(name: &str, filters: &Value, threshold: f64) -> Value {
    json!({
        "name": name, "kind": "agent", "measure": "failures", "aggregation": "avg",
        "window": "hour", "operator": "gt", "alert_threshold": threshold, "filters": filters,
    })
}

/// A change as its row keeps it.
#[derive(Clone, Copy)]
struct Changed<'a> {
    agent: &'a str,
    environment: &'a str,
    kind: &'a str,
    prompt: Option<&'a str>,
    value: &'a str,
    mark: &'a str,
}

/// `agent`'s release `value` in production, shown.
const fn release<'a>(agent: &'a str, value: &'a str) -> Changed<'a> {
    Changed {
        agent,
        environment: "production",
        kind: "release",
        prompt: None,
        value,
        mark: "shown",
    }
}

/// `change` filed on `project`, first carried at `changed_at`.
async fn changed(
    pool: &PgPool,
    project: &ProjectId,
    change: Changed<'_>,
    changed_at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO telemetry.changes
             (id, project_id, agent, environment, kind, prompt, value, identity, changed_at,
              trace_id, seen_at, mark)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $9, $11)",
    )
    .bind(Uuid::new_v4())
    .bind(project)
    .bind(change.agent)
    .bind(change.environment)
    .bind(change.kind)
    .bind(change.prompt)
    .bind(change.value)
    .bind(format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ))
    .bind(changed_at)
    .bind(Uuid::new_v4().simple().to_string())
    .bind(change.mark)
    .execute(pool)
    .await
    .expect("a change filed");
}

/// The time `ago` before now, and twenty seconds, so its whole hours hold.
fn before(ago: TimeDelta) -> DateTime<Utc> {
    Utc::now() - ago - TimeDelta::seconds(20)
}

/// How long before `incident` opened its change to `value` came, in a notice's words.
fn ago(incident: &Value, value: &str) -> String {
    let time = |of: &Value| {
        of.as_str()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .expect("a time")
    };
    let change = incident["changes"]
        .as_array()
        .and_then(|all| all.iter().find(|change| change["value"] == value))
        .expect("a change to the value");
    match (time(&incident["opened_at"]) - time(&change["changed_at"])).num_minutes() {
        ..=0 => "under a minute before".to_owned(),
        1 => "1 minute before".to_owned(),
        minutes => format!("{minutes} minutes before"),
    }
}

/// A notice's lines.
fn lines(notice: &LoggedNotice) -> Vec<&str> {
    notice.body.lines().collect()
}

/// The lines of a notice that name changes: its last, after their lead.
fn named(notice: &LoggedNotice) -> Vec<&str> {
    lines(notice)
        .into_iter()
        .skip_while(|line| *line != LEAD)
        .skip(1)
        .collect()
}

/// The values an incident's changes name, in order.
fn values(incident: &Value) -> Vec<&str> {
    incident
        .get("changes")
        .and_then(Value::as_array)
        .expect("its changes, read")
        .iter()
        .map(|change| {
            change
                .get("value")
                .and_then(Value::as_str)
                .expect("a value")
        })
        .collect()
}

/// ⚠ An incident names its agent's changes of the day before it, in its notice and on every lane.
#[sqlx::test]
async fn an_incident_names_what_its_agent_changed_to_in_the_day_before(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |project| failing(project, SUPPORT)).await;
    let p = &project.project;
    let minutes = TimeDelta::minutes;
    changed(&pool, p, release(SUPPORT, "2026.11.3"), before(minutes(14))).await;
    let as_it_opens = Utc::now() - TimeDelta::seconds(2);
    changed(
        &pool,
        p,
        release(SUPPORT, "the-minute-it-opened"),
        as_it_opens,
    )
    .await;
    let model = Changed {
        kind: "model",
        mark: "capped",
        ..release(SUPPORT, "gpt-4o-mini")
    };
    changed(&pool, p, model, before(TimeDelta::hours(3))).await;
    let prompt = Changed {
        kind: "prompt",
        prompt: Some("triage"),
        ..release(SUPPORT, "3")
    };
    changed(&pool, p, prompt, before(minutes(23 * 60 + 50))).await;
    changed(
        &pool,
        p,
        release(SUPPORT, "older-than-the-day"),
        before(minutes(24 * 60 + 5)),
    )
    .await;
    changed(
        &pool,
        p,
        release(SUPPORT, "after-it-opened"),
        Utc::now() + minutes(10),
    )
    .await;
    let hidden = Changed {
        mark: "hidden",
        ..release(SUPPORT, "past-its-days-cap")
    };
    changed(&pool, p, hidden, before(minutes(5))).await;
    changed(
        &pool,
        p,
        release("billing", "another-agents"),
        before(minutes(5)),
    )
    .await;
    changed(
        &pool,
        &ProjectId::new(),
        release(SUPPORT, "another-projects"),
        before(minutes(5)),
    )
    .await;

    project
        .make(
            RULES,
            &failures("Failing support", &json!({ "agent": [SUPPORT] }), 0.1),
        )
        .await;
    project.evaluate(&pool).await;

    let opened = project.told("Failing support: alert", 0);
    assert_eq!(opened.kind, NotificationKind::IncidentOpened);
    let said = lines(&opened);
    assert!(
        said.get(1)
            .is_some_and(|line| line.starts_with("The latest failed trace: ")),
        "{}",
        opened.body
    );
    assert!(
        said.get(2)
            .is_some_and(|traces| traces.starts_with("Traces: ")),
        "{}",
        opened.body
    );
    let incident = project.incident().await;
    let newest = format!(
        "release the-minute-it-opened in production, {}",
        ago(&incident, "the-minute-it-opened")
    );
    let earlier = format!(
        "release 2026.11.3 in production, {}",
        ago(&incident, "2026.11.3")
    );
    assert_eq!(
        said.get(3..),
        Some(
            &[
                LEAD,
                newest.as_str(),
                earlier.as_str(),
                "model gpt-4o-mini in production, 3 hours before, and more that day",
                "prompt triage version 3 in production, 23 hours before",
            ][..]
        ),
        "the newest first, the body's last lines: {}",
        opened.body
    );
    assert!(earlier.ends_with(" minutes before"), "{earlier}");

    assert_eq!(
        values(&incident),
        ["the-minute-it-opened", "2026.11.3", "gpt-4o-mini", "3"]
    );
    assert_eq!(incident["changes_more"], false);
    let listed = &incident["changes"];
    assert_eq!(
        listed[1],
        json!({
            "agent": SUPPORT, "environment": "production", "kind": "release",
            "value": "2026.11.3", "changed_at": listed[1]["changed_at"],
            "trace_id": listed[1]["trace_id"], "capped": false,
        }),
        "a change as the changes lane answers one"
    );
    assert_eq!(listed[2]["capped"], true);
    assert_eq!(
        (listed[3]["kind"].clone(), listed[3]["prompt"].clone()),
        (json!("prompt"), json!("triage"))
    );

    // Every lane that answers the incident answers its changes with it.
    let id = incident["id"].as_str().expect("an id");
    let one = format!("{INCIDENTS}/{id}");
    for (method, uri) in [
        ("GET", one.clone()),
        ("POST", format!("{one}/acknowledge")),
        ("POST", format!("{one}/resolve")),
    ] {
        let (status, answered) = project.send(method, &uri, &Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{method} {uri}: {answered}");
        assert_eq!(answered["changes"], incident["changes"], "{method} {uri}");
        assert_eq!(answered["changes_more"], false, "{method} {uri}");
    }
    let (status, keyed) = project.keyed("/incidents").await;
    assert_eq!(status, StatusCode::OK, "{keyed}");
    assert_eq!(keyed["incidents"][0]["changes"], incident["changes"]);
    let (status, keyed) = project.keyed(&format!("/incidents/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{keyed}");
    assert_eq!(keyed["changes"], incident["changes"]);
}

/// ⚠ An incident with no change names none: its changes are empty, not missing.
#[sqlx::test]
async fn an_incident_with_no_change_names_none(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |project| failing(project, SUPPORT)).await;
    changed(
        &pool,
        &project.project,
        release("billing", "another-agents"),
        before(TimeDelta::minutes(5)),
    )
    .await;
    project
        .make(
            RULES,
            &failures("Failing support", &json!({ "agent": [SUPPORT] }), 0.1),
        )
        .await;
    project.evaluate(&pool).await;

    let opened = project.told("Failing support: alert", 0);
    let said = lines(&opened);
    assert_eq!(
        said.len(),
        3,
        "its words, its trace and its changes: {}",
        opened.body
    );
    assert!(!opened.body.contains("Changed in"), "{}", opened.body);
    let incident = project.incident().await;
    assert_eq!(incident["changes"], json!([]));
    assert_eq!(incident["changes_more"], false);
}

/// ⚠ An alert over every agent says whose each change is; an environment's alert names its own.
#[sqlx::test]
async fn an_alert_over_every_agent_names_each_changes_agent(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |project| failing(project, SUPPORT)).await;
    let p = &project.project;
    changed(
        &pool,
        p,
        release(SUPPORT, "2026.11.3"),
        before(TimeDelta::minutes(14)),
    )
    .await;
    let staged = Changed {
        environment: "staging",
        ..release("billing", "2.0.0")
    };
    changed(&pool, p, staged, before(TimeDelta::minutes(30))).await;
    let unplaced = Changed {
        environment: "",
        ..release("billing", "2.0.1")
    };
    changed(&pool, p, unplaced, before(TimeDelta::hours(2))).await;

    project
        .make(RULES, &failures("Every agent", &json!({}), 0.1))
        .await;
    project
        .make(
            RULES,
            &failures(
                "In production",
                &json!({ "environment": ["production"] }),
                0.1,
            ),
        )
        .await;
    project.evaluate(&pool).await;

    let incidents = project.incidents().await;
    let of = |name: &str| {
        incidents
            .iter()
            .find(|incident| incident["rule_name"] == name)
            .unwrap_or_else(|| panic!("no incident of {name}"))
    };
    let every = of("Every agent");
    assert_eq!(
        named(&project.told("Every agent: alert", 0)),
        [
            format!(
                "support: release 2026.11.3 in production, {}",
                ago(every, "2026.11.3")
            ),
            format!("billing: release 2.0.0 in staging, {}", ago(every, "2.0.0")),
            "billing: release 2.0.1, 2 hours before".to_owned(),
        ]
    );
    assert_eq!(
        named(&project.told("In production: alert", 0)),
        [format!(
            "support: release 2026.11.3 in production, {}",
            ago(of("In production"), "2026.11.3")
        )],
        "its filter's environment alone, its agent named since it watches every one"
    );
    assert_eq!(values(of("Every agent")), ["2026.11.3", "2.0.0", "2.0.1"]);
    assert_eq!(values(of("In production")), ["2026.11.3"]);
}

/// ⚠ A monitor's incident names its own agent's changes, against when it opened.
#[sqlx::test]
async fn a_monitors_incident_names_its_agents_changes(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |project| {
        let started = at("2026-11-10T02:00:02Z");
        vec![SpanRow {
            agent: AGENT.into(),
            started_at: started,
            ended_at: started,
            units: 0,
            ..common::received(project, SpanKind::TraceStart, AGENT, 0, started)
        }]
    })
    .await;
    project
        .make(
            MONITORS,
            &json!({ "agent": AGENT, "cron": "0 2 * * *", "time_zone": "UTC", "grace_seconds": 1800 }),
        )
        .await;
    let p = &project.project;
    let staged = Changed {
        environment: "staging",
        ..release(AGENT, "2026.11.3")
    };
    changed(&pool, p, staged, at("2026-11-11T01:47:00Z")).await;
    changed(
        &pool,
        p,
        release(SUPPORT, "another-agents"),
        at("2026-11-11T01:50:00Z"),
    )
    .await;
    changed(
        &pool,
        p,
        release(AGENT, "after-it-opened"),
        at("2026-11-11T02:10:00Z"),
    )
    .await;

    let tick = async |now: &str| {
        monitors::tick_at(&project.state, at(now))
            .await
            .expect("a tick");
    };
    tick("2026-11-10T02:00:10Z").await;
    tick("2026-11-11T02:01:00Z").await;
    let late = project.told(&format!("{AGENT}: late"), 0);
    assert_eq!(
        lines(&late).get(1..),
        Some(&[LEAD, "release 2026.11.3 in staging, 14 minutes before"][..]),
        "its own agent's, in any environment, unnamed since it watches one: {}",
        late.body
    );

    tick("2026-11-11T02:30:00Z").await;
    let down = project.told(&format!("{AGENT}: down"), 0);
    assert_eq!(
        named(&down),
        ["release 2026.11.3 in staging, 14 minutes before"],
        "said again against when it opened, and nothing since: {}",
        down.body
    );
    let incident = project.incident().await;
    assert_eq!(incident["agent"], AGENT);
    assert_eq!(values(&incident), ["2026.11.3"]);
}

/// ⚠ An objective's incident names its agent's changes, its day read to the instant.
#[sqlx::test]
async fn an_objectives_incident_names_its_agents_changes_to_the_instant(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |_| Vec::new()).await;
    let rule = project
        .make(
            RULES,
            &json!({
                "name": "Support succeeds", "kind": "agent", "measure": "burn",
                "aggregation": "avg", "filters": { "agent": [SUPPORT] }, "window": "hour",
                "operator": "gte", "alert_threshold": 14.4,
                "objective": { "target": 0.99, "days": 30 },
            }),
        )
        .await;
    let opened = (Utc::now() - TimeDelta::hours(2)).trunc_subsecs(0);
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO telemetry.incidents
             (id, rule_id, project_id, organization_id, status, severity, value, opened_at)
         VALUES ($1, $2::uuid, $3, $4, 'open', 'alert', 20, $5)",
    )
    .bind(id)
    .bind(&rule)
    .bind(&project.project)
    .bind(&project.organization)
    .bind(opened)
    .execute(&pool)
    .await
    .expect("an incident");
    let p = &project.project;
    let day = TimeDelta::hours(24);
    changed(&pool, p, release(SUPPORT, "as-it-opened"), opened).await;
    changed(
        &pool,
        p,
        release(SUPPORT, "a-microsecond-after"),
        opened + TimeDelta::microseconds(1),
    )
    .await;
    changed(
        &pool,
        p,
        release(SUPPORT, "a-second-inside-the-day"),
        opened - day + TimeDelta::seconds(1),
    )
    .await;
    changed(
        &pool,
        p,
        release(SUPPORT, "a-day-before-to-the-instant"),
        opened - day,
    )
    .await;
    changed(
        &pool,
        p,
        release("billing", "another-agents"),
        opened - TimeDelta::hours(1),
    )
    .await;

    let (status, incident) = project
        .send("GET", &format!("{INCIDENTS}/{id}"), &Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{incident}");
    assert_eq!(
        values(&incident),
        ["as-it-opened", "a-second-inside-the-day"]
    );
}

/// ⚠ A notice names five clipped changes inside its bound and counts the rest.
#[sqlx::test]
async fn a_notice_names_five_changes_inside_its_bound_and_counts_the_rest(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    // Names as long as each may be, so the lines, the trace and the changes
    // together would pass the body's bound.
    let agent = "a".repeat(128);
    let project = Project::new(&pool, &ch, |project| {
        let failed = Uuid::new_v4();
        vec![
            span(
                project,
                &agent,
                failed,
                SpanKind::Step,
                &"s".repeat(200),
                20,
            ),
            SpanRow {
                status: SpanStatus::Error,
                error_type: "E".repeat(128),
                ..span(project, &agent, failed, SpanKind::Agent, "trip", 10)
            },
            span(project, &agent, Uuid::new_v4(), SpanKind::Agent, "trip", 5),
        ]
    })
    .await;
    let p = &project.project;
    let (long, place) = ("v".repeat(1_024), "e".repeat(128));
    for n in 1..=7 {
        let version: String = format!("{n}{long}").chars().take(1_024).collect();
        let prompt = Changed {
            agent: &agent,
            environment: &place,
            kind: "prompt",
            prompt: Some(&long),
            value: &version,
            mark: "capped",
        };
        changed(&pool, p, prompt, before(TimeDelta::hours(n))).await;
    }
    for n in 1..=7 {
        changed(
            &pool,
            p,
            release(SUPPORT, &format!("1.0.{n}")),
            before(TimeDelta::minutes(n)),
        )
        .await;
    }
    let others: Vec<String> = (0..5).map(|n| format!("{n}{}", "b".repeat(127))).collect();
    let mut watched = vec![agent.clone()];
    watched.extend(others);
    project
        .make(
            RULES,
            &failures("Long names", &json!({ "agent": watched }), 0.1),
        )
        .await;
    project
        .make(
            RULES,
            &failures(
                "Failing support",
                &json!({ "agent": [SUPPORT, agent] }),
                0.1,
            ),
        )
        .await;
    project.evaluate(&pool).await;

    // The log itself refuses a body past 4,000 characters, as notifications does.
    let long_names = project.told("Long names: alert", 0);
    let said = named(&long_names);
    let count = said.last().copied().unwrap_or_default();
    let shown = said.len().saturating_sub(1);
    assert!(
        (1..5).contains(&shown),
        "fewer than five fit: {}",
        long_names.body
    );
    assert_eq!(
        count,
        format!("and {} more on the incident", 7 - shown),
        "what did not fit is counted: {}",
        long_names.body
    );
    assert!(
        said.iter()
            .take(shown)
            .all(|line| line.chars().count() < 560 && line.contains('…')),
        "each name clipped: {}",
        long_names.body
    );
    assert!(long_names.body.chars().count() <= 4_000);
    assert!(
        lines(&long_names)
            .iter()
            .any(|line| line.starts_with("Traces: ")),
        "its link kept, before them: {}",
        long_names.body
    );

    let short = project.told("Failing support: alert", 0);
    let said = named(&short);
    assert_eq!(said.len(), 6, "{}", short.body);
    assert_eq!(
        said.last().copied(),
        Some("and 9 more on the incident"),
        "fourteen in its day, five named"
    );
    let incidents = project.incidents().await;
    let of_support = incidents
        .iter()
        .find(|incident| incident["rule_name"] == "Failing support")
        .expect("its incident");
    assert_eq!(
        said.first().map(|line| (*line).to_owned()),
        Some(format!(
            "support: release 1.0.1 in production, {}",
            ago(of_support, "1.0.1")
        ))
    );
    assert_eq!(values(of_support).len(), 14, "its incident names them all");
    assert_eq!(of_support["changes_more"], false);
}

/// ⚠ A change filed after the notice went is named on the incident alone.
#[sqlx::test]
async fn a_change_filed_late_is_named_on_the_incident_alone(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |project| failing(project, SUPPORT)).await;
    project
        .make(
            RULES,
            &failures("Failing support", &json!({ "agent": [SUPPORT] }), 0.1),
        )
        .await;
    project.evaluate(&pool).await;
    assert!(named(&project.told("Failing support: alert", 0)).is_empty());
    let incident = project.incident().await;
    assert_eq!(incident["changes"], json!([]));

    let opened: DateTime<Utc> =
        serde_json::from_value(incident["opened_at"].clone()).expect("when it opened");
    changed(
        &pool,
        &project.project,
        release(SUPPORT, "2026.11.3"),
        opened - TimeDelta::minutes(1),
    )
    .await;
    assert_eq!(values(&project.incident().await), ["2026.11.3"]);
}

/// ⚠ A reopened incident keeps its first day: a release since its recovery is not named.
#[sqlx::test]
async fn a_reopened_incident_keeps_the_day_before_it_first_opened(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |project| failing(project, SUPPORT)).await;
    let p = &project.project;
    changed(
        &pool,
        p,
        release(SUPPORT, "before-it-opened"),
        before(TimeDelta::minutes(14)),
    )
    .await;
    let filters = json!({ "agent": [SUPPORT] });
    let rule = project
        .make(RULES, &failures("Failing support", &filters, 0.1))
        .await;
    let rule = format!("{RULES}/{rule}");
    project.evaluate(&pool).await;
    let first = project.incident().await;

    project
        .send("PUT", &rule, &failures("Failing support", &filters, 0.9))
        .await;
    project.evaluate(&pool).await;
    assert_eq!(project.incident().await["status"], "resolved");
    changed(&pool, p, release(SUPPORT, "since-it-recovered"), Utc::now()).await;
    project
        .send("PUT", &rule, &failures("Failing support", &filters, 0.1))
        .await;
    project.evaluate(&pool).await;

    let again = project.incident().await;
    assert_eq!(
        (
            again["id"].clone(),
            again["reopened"].clone(),
            again["opened_at"].clone()
        ),
        (first["id"].clone(), json!(1), first["opened_at"].clone()),
        "the same incident, opened when it first was"
    );
    assert_eq!(values(&again), ["before-it-opened"]);
    let said_again = project.told("Failing support: alert", 1);
    assert!(
        said_again.reminded,
        "a reopening is said to the connectors alone"
    );
    assert_eq!(
        named(&said_again),
        [format!(
            "release before-it-opened in production, {}",
            ago(&again, "before-it-opened")
        )],
        "against when it first opened: {}",
        said_again.body
    );
}

/// ⚠ Changes that cannot be read hold back neither the notice nor the incident's answer.
#[sqlx::test]
async fn changes_that_cannot_be_read_leave_the_notice_and_the_incident_standing(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |project| failing(project, SUPPORT)).await;
    changed(
        &pool,
        &project.project,
        release(SUPPORT, "2026.11.3"),
        before(TimeDelta::minutes(14)),
    )
    .await;
    project
        .make(
            RULES,
            &failures("Failing support", &json!({ "agent": [SUPPORT] }), 0.1),
        )
        .await;
    sqlx::query("ALTER TABLE telemetry.changes RENAME TO changes_away")
        .execute(&pool)
        .await
        .expect("the table away");
    project.evaluate(&pool).await;

    let opened = project.told("Failing support: alert", 0);
    assert_eq!(opened.kind, NotificationKind::IncidentOpened);
    assert_eq!(
        lines(&opened).len(),
        3,
        "its words, its trace and its changes: {}",
        opened.body
    );
    let incident = project.incident().await;
    assert_eq!(incident["status"], "open");
    assert_eq!(
        incident["changes"],
        Value::Null,
        "not read, which is not none"
    );
    assert_eq!(incident["changes_more"], false);
    let id = incident["id"].as_str().expect("an id");
    let (status, one) = project
        .send("GET", &format!("{INCIDENTS}/{id}"), &Value::Null)
        .await;
    assert_eq!((status, &one["changes"]), (StatusCode::OK, &Value::Null));

    sqlx::query("ALTER TABLE telemetry.changes_away RENAME TO changes")
        .execute(&pool)
        .await
        .expect("the table back");
    assert_eq!(values(&project.incident().await), ["2026.11.3"]);
}

/// ⚠ Many incidents' changes are one read, each its own scope's and day's, twenty at most.
#[sqlx::test]
async fn many_incidents_changes_are_one_read(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let project = Project::new(&pool, &ch, |_| Vec::new()).await;
    let p = &project.project;
    let noon = at("2026-11-05T12:00:00Z");
    for n in 1..=21 {
        changed(
            &pool,
            p,
            release(SUPPORT, &format!("1.0.{n}")),
            noon - TimeDelta::minutes(n),
        )
        .await;
    }
    let staged = Changed {
        environment: "staging",
        ..release("billing", "2.0.0")
    };
    changed(&pool, p, staged, noon - TimeDelta::hours(30)).await;
    changed(
        &pool,
        p,
        release("billing", "2.0.1"),
        noon - TimeDelta::hours(31),
    )
    .await;
    changed(
        &pool,
        &ProjectId::new(),
        release(SUPPORT, "another-projects"),
        noon - TimeDelta::minutes(1),
    )
    .await;

    let opened = |agents: &[&str], environments: &[&str], opened_at| Opened {
        incident_id: Uuid::new_v4(),
        opened_at,
        scope: Scope {
            agents: agents.iter().map(|agent| (*agent).to_owned()).collect(),
            environments: environments.iter().map(|one| (*one).to_owned()).collect(),
        },
    };
    let asked = [
        opened(&[SUPPORT], &[], noon),
        opened(&[], &["staging"], noon - TimeDelta::hours(8)),
        opened(&["billing", SUPPORT], &[], noon - TimeDelta::hours(8)),
        opened(&["nobody"], &[], noon),
    ];
    let mut tx = project_scope(&project.state.db, p)
        .await
        .expect("its scope");
    let around = changes::around_incidents(&mut tx, p, &asked)
        .await
        .expect("one read");
    tx.commit().await.expect("committed");

    let of = |n: usize| {
        let (changes, more) = around
            .get(&asked[n].incident_id)
            .cloned()
            .unwrap_or_default();
        let values: Vec<String> = changes.into_iter().map(|change| change.value).collect();
        (values, more)
    };
    let (support, more) = of(0);
    assert_eq!(support.len(), 20, "{support:?}");
    assert!(more, "the twenty-first says there are more");
    assert_eq!(
        (
            support.first().map(String::as_str),
            support.last().map(String::as_str)
        ),
        (Some("1.0.1"), Some("1.0.20")),
        "the newest first, never another project's"
    );
    assert_eq!(
        of(1),
        (vec!["2.0.0".to_owned()], false),
        "its environment's alone"
    );
    assert_eq!(
        of(2),
        (vec!["2.0.0".to_owned(), "2.0.1".to_owned()], false),
        "both its agents', in its own day"
    );
    assert!(
        !around.contains_key(&asked[3].incident_id),
        "none to answer"
    );
    assert!(
        changes::around_incidents(
            &mut project_scope(&project.state.db, p)
                .await
                .expect("its scope"),
            p,
            &[]
        )
        .await
        .expect("no read")
        .is_empty()
    );
}
