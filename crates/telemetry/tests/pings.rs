//! The ping lane as a crontab reaches it: a ping to a slug no monitor has
//! makes the monitor, paused as `new`, recorded on the chain as made by the
//! key, and the owner told once through the outbox; a run is opened by its
//! `/start`, heard by its `/checkpoint`s, which hold it to its monitor's
//! step cap, and ended by a bare ping or a `/fail`, when it waits in the
//! outbox as a finished span and the monitor's state and next due time
//! follow, or as `error` by the next ping that finds it never said it ended;
//! the run a ping names by `rid`; every run's ceiling, and the stall timeout
//! a discovered monitor is given; a body is dropped and said so, the
//! query held to numbers, the methods and paths to the lane's; past the
//! plan's limit, or past the monitors nobody has set up that a project may
//! hold, nothing is made and the ping is refused with the bound's words; a
//! paused organization takes no ping; a finished run reaches ClickHouse
//! through the outbox; and the purges and a move take the keys, runs and
//! outbox rows with the project.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{Answer, AuthStub, NoticeLog, PausedOrganization, SECRET};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    Grants, MonitorCeiling, NotificationKind, OrganizationId, OrganizationRole, ProjectId,
    TelmoniError,
};
use telmoni_telemetry::db::{self, TelemetryLane};
use telmoni_telemetry::handler::pings::NEW_MONITORS_MAX;
use telmoni_telemetry::outbox::{self, Settled};
use telmoni_telemetry::{AppState, ping_router, router};

const MONITORS: &str = "/internal/telemetry/monitors";
const PING_KEYS: &str = "/internal/telemetry/ping-keys";
const OWNER: &str = "user_owner";

/// The module's two routers, auth as it holds it, the notices it hands
/// over, and a project the roster seats its owner on.
struct Project {
    app: Router,
    pings: Router,
    state: Arc<AppState>,
    notices: Arc<NoticeLog>,
    organization: OrganizationId,
    project: ProjectId,
}

fn project(pool: &PgPool) -> Project {
    project_under(pool, OrganizationId::new(), Arc::new(NoLimits))
}

/// A project of `organization`, under `entitlements`, over no store.
fn project_under(
    pool: &PgPool,
    organization: OrganizationId,
    entitlements: Arc<dyn Entitlements>,
) -> Project {
    let project = ProjectId::new();
    let auth = AuthStub::probing(pool);
    auth.seat(
        OWNER,
        &organization,
        &project,
        OrganizationRole::Owner,
        None,
    );
    let notices = NoticeLog::new();
    let state = common::state_with_notices(pool, auth, entitlements, notices.clone());
    Project::over(state, notices, organization, project)
}

impl Project {
    fn over(
        state: Arc<AppState>,
        notices: Arc<NoticeLog>,
        organization: OrganizationId,
        project: ProjectId,
    ) -> Self {
        Self {
            app: router(state.clone()),
            pings: ping_router(state.clone()),
            state,
            notices,
            organization,
            project,
        }
    }

    /// A request as the console sends one, as the owner.
    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{OWNER}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.project.as_str());
        match body {
            Some(body) => builder
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .expect("a request")
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("the router answers");
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

    /// The project's ping key, minted by its owner.
    async fn mint(&self) -> String {
        let (status, body) = self.send(self.request("POST", PING_KEYS, None)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body.get("key")
            .and_then(Value::as_str)
            .expect("the key")
            .to_owned()
    }

    /// A monitor made in the console.
    async fn make(&self, monitor: Value) -> Value {
        let (status, body) = self
            .send(self.request("POST", MONITORS, Some(monitor)))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body
    }

    /// One of the project's monitors, as the console reads it.
    async fn monitor(&self, slug: &str) -> Value {
        let (status, body) = self
            .send(self.request("GET", &format!("{MONITORS}/{slug}"), None))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    /// A ping as a crontab sends one: no header but those given, the key in
    /// the path, on the lane as the binary nests it. The answer's status,
    /// its media type and its body.
    async fn send_ping(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Body,
    ) -> (StatusCode, String, String) {
        let mut builder = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = self
            .pings
            .clone()
            .oneshot(builder.body(body).expect("a request"))
            .await
            .expect("the lane answers");
        let status = response.status();
        let media_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        (
            status,
            media_type,
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }

    /// A `GET` ping, as `curl` sends one; `OK` is what it expects back.
    async fn ping(&self, path: &str) -> (StatusCode, String) {
        let (status, _, body) = self.send_ping("GET", path, &[], Body::empty()).await;
        (status, body)
    }

    async fn ping_ok(&self, path: &str) {
        let (status, media_type, body) = self.send_ping("GET", path, &[], Body::empty()).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"), "{path}");
        assert!(media_type.starts_with("text/plain"), "{path}: {media_type}");
    }
}

/// Entitlements under which every organization holds `limit` monitors.
struct Ceiling(u32);

#[async_trait]
impl Entitlements for Ceiling {
    async fn grants(&self, organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        let _ = organization_id;
        Ok(Grants::unlimited())
    }

    async fn monitor_ceiling(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<MonitorCeiling>, TelmoniError> {
        let _ = organization_id;
        Ok(Some(MonitorCeiling {
            limit: self.0,
            entitlement: "pro".into(),
            detail: "Move to Pro for more.".into(),
        }))
    }
}

/// A monitor as the console makes one: hourly, a minute of grace, held to a
/// stall timeout and a step cap of three.
fn hourly() -> Value {
    json!({
        "name": "Hourly sync",
        "slug": "hourly-sync",
        "interval_s": 3600,
        "grace_s": 60,
        "bounds": { "stall_after_s": 120, "max_steps": 3 },
    })
}

async fn monitors_of(pool: &PgPool, project: &ProjectId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM telemetry.monitors WHERE project_id = $1")
        .bind(project)
        .fetch_one(pool)
        .await
        .expect("count a project's monitors")
}

/// The project's open runs, oldest first: each run's id, steps and spend.
async fn open_runs(pool: &PgPool, project: &ProjectId) -> Vec<(Uuid, i32, f64)> {
    sqlx::query_as(
        "SELECT id, steps, cost_usd FROM telemetry.open_runs
          WHERE project_id = $1
          ORDER BY started_at, id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read a project's open runs")
}

/// Each open run's ceiling, in seconds past its start, oldest first.
async fn ceilings_of(pool: &PgPool, project: &ProjectId) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT CAST(extract(epoch FROM ends_by - started_at) AS BIGINT)
           FROM telemetry.open_runs
          WHERE project_id = $1
          ORDER BY started_at, id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read a project's open runs' ceilings")
}

/// What waits in the project's outbox, oldest first: each row's kind and
/// payload.
async fn outbox_of(pool: &PgPool, project: &ProjectId) -> Vec<(String, Value)> {
    sqlx::query_as("SELECT kind, payload FROM telemetry.outbox WHERE project_id = $1 ORDER BY id")
        .bind(project)
        .fetch_all(pool)
        .await
        .expect("read a project's outbox")
}

/// The finished runs waiting in the project's outbox, oldest first.
async fn finished_runs(pool: &PgPool, project: &ProjectId) -> Vec<Value> {
    outbox_of(pool, project)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "span")
        .map(|(_, payload)| payload)
        .collect()
}

/// Each row waiting in the project's outbox, oldest first: how many tries
/// it has been counted, and whether it is held for a wait or a lease now.
async fn tries_of(pool: &PgPool, project: &ProjectId) -> Vec<(i32, bool)> {
    sqlx::query_as(
        "SELECT attempts, lease_until IS NOT NULL AND lease_until > now()
           FROM telemetry.outbox WHERE project_id = $1 ORDER BY id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read a project's outbox tries")
}

/// What the organization's chain records of its monitors, in order: the
/// action, who did it, and the row's metadata.
async fn recorded(pool: &PgPool, organization: &OrganizationId) -> Vec<(String, String, Value)> {
    sqlx::query_as(
        "SELECT action, actor_id, metadata FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'monitor'
          ORDER BY seq",
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .expect("read the chain")
}

/// A run id a ping names.
fn rid() -> String {
    Uuid::new_v4().to_string()
}

/// A clock as a lane or a payload wrote it.
fn at(value: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value.as_str().expect("a clock"))
        .expect("an RFC 3339 clock")
        .with_timezone(&Utc)
}

/// The run's row as this suite reads it back from ClickHouse.
#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct RunRow {
    kind: String,
    name: String,
    agent: String,
    status: String,
    attributes: Vec<(String, String)>,
}

#[sqlx::test]
async fn a_ping_to_a_slug_no_monitor_has_makes_it_and_tells_the_owner_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;

    p.ping_ok(&format!("/{key}/nightly-report")).await;
    let monitor = p.monitor("nightly-report").await;
    assert_eq!(monitor["state"], "new", "{monitor}");
    assert_eq!(monitor["name"], "nightly-report");
    assert_eq!(monitor["agent"], "nightly-report");
    assert!(monitor["schedule"].is_null());
    assert!(monitor["next_due_at"].is_null());
    assert_eq!(
        monitor["bounds"]["stall_after_s"], 3600,
        "an hour's stall timeout until its owner sets one, so its runs are visited"
    );
    assert!(monitor["bounds"]["max_steps"].is_null());

    // Recorded on the chain as made by the key, which the row names.
    let chain = recorded(&pool, &p.organization).await;
    assert_eq!(chain.len(), 1, "{chain:?}");
    assert_eq!(chain[0].0, "created");
    assert_eq!(chain[0].1, "external:ping-key");
    assert_eq!(chain[0].2["slug"], "nightly-report");
    assert!(chain[0].2["ping_key_id"].is_string(), "{}", chain[0].2);

    // The notice and the run of no duration wait in the outbox, in that
    // order; the loop hands the notice over, and the run's row waits on a
    // store this suite has none of.
    let waiting = outbox_of(&pool, &p.project).await;
    assert_eq!(
        waiting
            .iter()
            .map(|(kind, _)| kind.as_str())
            .collect::<Vec<_>>(),
        ["notice", "span"]
    );
    assert_eq!(waiting[1].1["status"], "ok");
    assert_eq!(
        waiting[1].1["started_at"], waiting[1].1["ended_at"],
        "no duration"
    );
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 }
    );
    let notices = p.notices.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].kind, NotificationKind::MonitorDiscovered);
    assert_eq!(notices[0].project_id.as_ref(), Some(&p.project));
    assert_eq!(notices[0].organization_id, p.organization);
    assert_eq!(notices[0].metadata["slug"], "nightly-report");
    assert_eq!(notices[0].metadata["made"], true);
    assert_eq!(
        notices[0].metadata["monitor_id"].as_str(),
        monitor["id"].as_str()
    );
    assert_eq!(
        notices[0].dedup_key.as_deref(),
        Some(
            format!(
                "telemetry:monitor:{}:discovered",
                monitor["id"].as_str().expect("an id")
            )
            .as_str()
        )
    );
    assert!(
        !notices[0].body.contains(&key),
        "the key reached the feed: {}",
        notices[0].body
    );
    let waiting = outbox_of(&pool, &p.project).await;
    assert_eq!(
        waiting.len(),
        1,
        "the notice is gone, the run's row held: {waiting:?}"
    );

    // Every ping after the first is the monitor's run: nothing made, nothing
    // told again, and its state its owner's to set.
    p.ping_ok(&format!("/{key}/nightly-report/start")).await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    assert_eq!(recorded(&pool, &p.organization).await.len(), 1);
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
    assert_eq!(p.monitor("nightly-report").await["state"], "new");
    assert_eq!(
        outbox_of(&pool, &p.project)
            .await
            .iter()
            .filter(|(kind, _)| kind == "notice")
            .count(),
        0
    );

    // ⚠ A slug the console could not save is one a ping cannot name: a 400
    // that echoes none of it, and nothing made.
    for refused in ["Nightly", "nightly_report", "new", "schedule", "shit-2"] {
        let (status, body) = p.ping(&format!("/{key}/{refused}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
        assert!(!body.contains(refused), "{refused} was echoed: {body}");
    }
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
}

#[sqlx::test]
async fn a_run_is_opened_heard_and_ended_by_its_pings(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let made = p.make(hourly()).await;
    assert_eq!(made["state"], "healthy");
    let due_at_first = at(&made["next_due_at"]);

    // `/start` opens the run and the monitor runs; its due time stands.
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].1, runs[0].2), (0, 0.0));
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["state"], "running");
    assert_eq!(at(&monitor["next_due_at"]), due_at_first);

    // Each `/checkpoint` is heard, with what it reports; past the step cap
    // the monitor is over its bound.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?steps=2&cost=0.5"))
        .await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!((runs[0].1, runs[0].2), (2, 0.5));
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?steps=4"))
        .await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(
        (runs[0].1, runs[0].2),
        (4, 0.5),
        "a checkpoint keeps what it did not resend"
    );
    assert_eq!(p.monitor("hourly-sync").await["state"], "over_bound");

    // The bare ping ends it `ok`: the row gone, the finished run in the
    // outbox with what the checkpoints reported, the monitor healthy and
    // due again an hour from now.
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    assert!(open_runs(&pool, &p.project).await.is_empty());
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["state"], "healthy");
    let due_at_second = at(&monitor["next_due_at"]);
    assert!(
        due_at_second >= due_at_first,
        "{due_at_second} < {due_at_first}"
    );
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["agent"], "hourly-sync");
    assert_eq!(finished[0]["status"], "ok");
    assert_eq!(finished[0]["steps"], 4);
    assert_eq!(finished[0]["cost_usd"], 0.5);
    assert!(finished[0]["exit_code"].is_null());
    assert_eq!(
        finished[0]["run_id"].as_str().map(str::to_owned),
        Some(runs[0].0.to_string())
    );
    assert!(at(&finished[0]["started_at"]) < at(&finished[0]["ended_at"]));

    // `/fail` with no run open records one of no duration ended `error`,
    // with its exit code, and the monitor failed.
    p.ping_ok(&format!("/{key}/hourly-sync/fail?exit_code=3"))
        .await;
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["state"], "failed");
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 2);
    assert_eq!(finished[1]["status"], "error");
    assert_eq!(finished[1]["exit_code"], 3);
    assert!(finished[1]["steps"].is_null());
    assert_eq!(finished[1]["started_at"], finished[1]["ended_at"]);

    // A `/checkpoint` with no run open opens the run it reports on, since
    // its `/start` was lost; the next success recovers the monitor.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?steps=1"))
        .await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].1, 1);
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");

    // `?rid=` names the run when runs overlap: a `/start` sent again for it
    // is heard, not a second run; a `/start` naming another opens it
    // beside; a bare `/start` ends them both as `error`, since a run that
    // never said it ended is one that crashed, and opens its own; and each
    // ending ping ends the run it names, or the newest with its verdict and
    // whatever else is open as `error`.
    let first = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={first}"))
        .await;
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={first}"))
        .await;
    assert_eq!(open_runs(&pool, &p.project).await.len(), 1);
    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={first}&steps=1"
    ))
    .await;
    let second = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={second}"))
        .await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 2, "runs that name themselves overlap");
    assert_eq!(runs[0].0.to_string(), first);
    assert_eq!(runs[0].1, 1);
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1, "a bare start ended the runs it found open");
    assert_ne!(runs[0].0.to_string(), first);
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 5);
    assert_eq!(finished[3]["run_id"].as_str(), Some(first.as_str()));
    assert_eq!(finished[3]["status"], "error");
    assert_eq!(finished[3]["steps"], 1);
    assert!(finished[3]["exit_code"].is_null());
    assert_eq!(finished[4]["run_id"].as_str(), Some(second.as_str()));
    assert_eq!(finished[4]["status"], "error");
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    let fourth = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={fourth}"))
        .await;
    assert_eq!(open_runs(&pool, &p.project).await.len(), 2);
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    assert!(
        open_runs(&pool, &p.project).await.is_empty(),
        "a bare end ping ends them all"
    );
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 7);
    assert_eq!(
        finished[5]["status"], "error",
        "the run the ping did not mean"
    );
    assert_eq!(finished[6]["run_id"].as_str(), Some(fourth.as_str()));
    assert_eq!(
        finished[6]["status"], "ok",
        "the newest, which the ping meant"
    );
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    let (status, _) = p
        .ping(&format!("/{key}/hourly-sync/start?rid={}", "not-a-run"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A paused monitor keeps its runs and its state, and no due time.
    let (status, paused) = p
        .send(p.request("POST", &format!("{MONITORS}/hourly-sync/pause"), None))
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "paused");
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["state"], "paused");
    assert!(monitor["next_due_at"].is_null());
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 8);
}

/// A run that never said it ended — a job that crashed after its `/start`,
/// a `/checkpoint` that came after its run's end — is ended `error` with no
/// exit code by the next ping that finds it: a `/start` naming no run ends
/// what its monitor still has open, and an end ping naming none ends the
/// newest with its verdict and the rest the same way; a `/start` naming a
/// run leaves the others be. Every run carries its ceiling: four times what
/// its monitor expects, or a day.
#[sqlx::test]
async fn a_run_that_never_said_it_ended_is_ended_by_the_ping_after_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(json!({
        "name": "Long batch",
        "slug": "long-batch",
        "expected_duration_s": 8 * 60 * 60,
    }))
    .await;

    // The job crashed after its `/start`: the next `/start` ends that run,
    // at the moment it was last heard — its start.
    p.ping_ok(&format!("/{key}/long-batch/start")).await;
    let crashed = open_runs(&pool, &p.project).await;
    assert_eq!(crashed.len(), 1);
    assert_eq!(
        ceilings_of(&pool, &p.project).await,
        [32 * 60 * 60],
        "four times the expected duration"
    );
    p.ping_ok(&format!("/{key}/long-batch/start")).await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1);
    assert_ne!(runs[0].0, crashed[0].0);
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(
        finished[0]["run_id"].as_str().map(str::to_owned),
        Some(crashed[0].0.to_string())
    );
    assert_eq!(finished[0]["status"], "error");
    assert!(finished[0]["exit_code"].is_null());
    assert!(finished[0]["steps"].is_null());
    assert_eq!(finished[0]["started_at"], finished[0]["ended_at"]);
    assert_eq!(p.monitor("long-batch").await["state"], "running");

    // The run ends; a `/checkpoint` sent again after that — a retried curl
    // — opens a run as a lost `/start` would, which nothing but the next
    // ping ends: a `/fail` naming no run ends the newest with its verdict
    // and that one, and the run a `/start` named beside it, as `error`.
    p.ping_ok(&format!("/{key}/long-batch/checkpoint?steps=2"))
        .await;
    p.ping_ok(&format!("/{key}/long-batch")).await;
    assert!(open_runs(&pool, &p.project).await.is_empty());
    p.ping_ok(&format!("/{key}/long-batch/checkpoint?steps=5"))
        .await;
    let late = open_runs(&pool, &p.project).await;
    assert_eq!(late.len(), 1);
    assert_eq!(late[0].1, 5);
    let named = rid();
    p.ping_ok(&format!("/{key}/long-batch/start?rid={named}"))
        .await;
    let last = rid();
    p.ping_ok(&format!("/{key}/long-batch/start?rid={last}"))
        .await;
    assert_eq!(
        open_runs(&pool, &p.project).await.len(),
        3,
        "a start naming a run leaves the others be"
    );
    p.ping_ok(&format!("/{key}/long-batch/fail?exit_code=9"))
        .await;
    assert!(open_runs(&pool, &p.project).await.is_empty());
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 5, "{finished:?}");
    assert_eq!(finished[1]["status"], "ok");
    assert_eq!(finished[1]["steps"], 2);
    assert_eq!(
        finished[2]["run_id"].as_str().map(str::to_owned),
        Some(late[0].0.to_string()),
        "the late checkpoint's run, oldest of the abandoned"
    );
    assert_eq!(finished[2]["status"], "error");
    assert_eq!(finished[2]["steps"], 5);
    assert!(finished[2]["exit_code"].is_null());
    assert_eq!(finished[3]["run_id"].as_str(), Some(named.as_str()));
    assert_eq!(finished[3]["status"], "error");
    assert!(finished[3]["exit_code"].is_null());
    assert_eq!(finished[4]["run_id"].as_str(), Some(last.as_str()));
    assert_eq!(finished[4]["status"], "error");
    assert_eq!(finished[4]["exit_code"], 9);
    assert_eq!(p.monitor("long-batch").await["state"], "failed");

    // A monitor a ping made expects nothing: its runs' ceiling is a day.
    p.ping_ok(&format!("/{key}/found-by-ping/start")).await;
    assert_eq!(ceilings_of(&pool, &p.project).await, [24 * 60 * 60]);
}

#[sqlx::test]
async fn a_pings_body_is_dropped_its_query_held_to_numbers_and_the_lane_to_its_shape(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let path = format!("/{key}/nightly-report");

    // A body is content: dropped, and the answer says so; none is `OK`.
    let (status, media_type, body) = p
        .send_ping(
            "POST",
            &path,
            &[("content-type", "text/plain")],
            Body::from("the job's log, which is content"),
        )
        .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "OK; body dropped")
    );
    assert!(media_type.starts_with("text/plain"));
    let (status, _, body) = p
        .send_ping(
            "POST",
            &path,
            &[
                ("content-type", "application/x-www-form-urlencoded"),
                ("content-length", "17"),
            ],
            Body::from("steps=4&cost=0.08"),
        )
        .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "OK; body dropped"),
        "a form body is never read as the query"
    );
    assert!(
        open_runs(&pool, &p.project).await.is_empty(),
        "the body was read as a checkpoint"
    );
    let (status, _, body) = p
        .send_ping(
            "POST",
            &path,
            &[("content-length", "10485760")],
            Body::from(vec![b'x'; 200 * 1024]),
        )
        .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "OK; body dropped"),
        "a body past the cap still counts"
    );
    let (status, _, body) = p.send_ping("POST", &path, &[], Body::empty()).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(
        finished_runs(&pool, &p.project).await.len(),
        4,
        "each ping counted"
    );

    // ⚠ The query is numbers alone: a value that is not is a 400 that names
    // its field, echoes none of it, and records nothing.
    for (query, field) in [
        ("steps=four%3Cscript%3E", "steps"),
        ("steps=-1", "steps"),
        ("cost=%241%3Cscript%3E", "cost"),
        ("exit_code=one%3Cscript%3E", "exit_code"),
        ("rid=run-1%3Cscript%3E", "rid"),
        ("steps=1&steps=2", "steps"),
    ] {
        let (status, media_type, body) = p
            .send_ping(
                "GET",
                &format!("{path}/checkpoint?{query}"),
                &[],
                Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
        assert!(
            media_type.starts_with("application/problem+json"),
            "{query}: {media_type}"
        );
        assert!(body.contains(field), "{query}: {body}");
        assert!(!body.contains("script"), "{query} was echoed: {body}");
    }
    assert!(open_runs(&pool, &p.project).await.is_empty());

    // The lane's shape: `GET` or `POST` and nothing else, and no path beside
    // the four.
    let (status, _, body) = p.send_ping("PUT", &path, &[], Body::empty()).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{body}");
    let (status, _, body) = p
        .send_ping("DELETE", &format!("{path}/start"), &[], Body::empty())
        .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{body}");
    for missing in [
        format!("/{key}"),
        format!("{path}/stop"),
        format!("{path}/start/again"),
    ] {
        let (status, media_type, body) = p.send_ping("GET", &missing, &[], Body::empty()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}: {body}");
        assert!(
            media_type.starts_with("application/problem+json"),
            "{missing}: {media_type}"
        );
        assert!(!body.contains(&key), "{missing} echoed the key: {body}");
    }
}

/// Past the plan's limit a ping to a slug no monitor has is refused with the
/// ceiling's own 402 — what `curl -f` turns into cron's mail — naming what
/// lifts it and never the key; nothing is made or recorded, and no notice is
/// raised for it: the answer is the message, and the owner's digest is the
/// limiter's (roadmap 2.11).
#[sqlx::test]
async fn past_the_plans_limit_a_ping_is_refused_with_the_ceilings_words(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project_under(&pool, OrganizationId::new(), Arc::new(Ceiling(1)));
    let key = p.mint().await;

    p.ping_ok(&format!("/{key}/nightly-report")).await;
    assert_eq!(monitors_of(&pool, &p.project).await, 1);

    for path in [
        format!("/{key}/weekly-digest/start"),
        format!("/{key}/weekly-digest"),
        format!("/{key}/weekly-digest/fail?exit_code=1"),
    ] {
        let (status, media_type, body) = p.send_ping("GET", &path, &[], Body::empty()).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{path}: {body}");
        assert!(
            media_type.starts_with("application/problem+json"),
            "{path}: {media_type}"
        );
        let problem: Value = serde_json::from_str(&body).expect("a problem document");
        assert_eq!(problem["type"], "/errors/plan/limit-reached", "{body}");
        assert_eq!(problem["entitlement"], "pro", "{body}");
        assert_eq!(problem["detail"], "Move to Pro for more.", "{body}");
        assert!(!body.contains(&key), "{path} echoed the key: {body}");
    }
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
    assert!(
        open_runs(&pool, &p.project).await.is_empty(),
        "a run nobody could hold"
    );
    let (status, _) = p
        .send(p.request("GET", &format!("{MONITORS}/weekly-digest"), None))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The first monitor's own notice is all the feed gets.
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 },
        "the first monitor's notice, and its run's row waiting on a store"
    );
    let notices = p.notices.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].metadata["slug"], "nightly-report");
    assert_eq!(recorded(&pool, &p.organization).await.len(), 1);

    // The monitor the project has is held to nothing: its pings go on.
    p.ping_ok(&format!("/{key}/nightly-report/start")).await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;
}

/// A project holds at most `NEW_MONITORS_MAX` monitors its pings made and
/// nobody has set up: a ping that would make one more is refused — a 409
/// that says what to do and echoes nothing — until one is set up or deleted,
/// so a slug that carries a date cannot fill a project.
#[sqlx::test]
async fn a_ping_past_the_monitors_nobody_set_up_is_refused_until_one_is(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    for n in 0..NEW_MONITORS_MAX {
        p.ping_ok(&format!("/{key}/job-{n}")).await;
    }
    assert_eq!(monitors_of(&pool, &p.project).await, NEW_MONITORS_MAX);

    let dated = format!("/{key}/job-2026-10-14");
    let (status, media_type, body) = p.send_ping("GET", &dated, &[], Body::empty()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        media_type.starts_with("application/problem+json"),
        "{media_type}"
    );
    assert!(body.contains("set them up"), "{body}");
    assert!(
        !body.contains("2026-10-14") && !body.contains(&key),
        "the slug or the key was echoed: {body}"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, NEW_MONITORS_MAX);
    // A monitor the project has is still heard.
    p.ping_ok(&format!("/{key}/job-0/start")).await;

    // Set up, a monitor is no longer one nobody set up: the next slug is made.
    let (status, saved) = p
        .send(p.request(
            "PUT",
            &format!("{MONITORS}/job-0"),
            Some(json!({ "name": "Job 0" })),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["state"], "healthy");
    p.ping_ok(&dated).await;
    assert_eq!(monitors_of(&pool, &p.project).await, NEW_MONITORS_MAX + 1);
}

#[sqlx::test]
async fn a_paused_organization_takes_no_ping(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = OrganizationId::new();
    let p = project_under(
        &pool,
        organization.clone(),
        Arc::new(PausedOrganization(organization)),
    );
    // The key is minted on a live organization's terms: the stub pauses the
    // organization for everything new, so the key is written straight in.
    let (status, body) = p.send(p.request("POST", PING_KEYS, None)).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    let key = telmoni_shared::types::mint_ping_key();
    let mut tx = telmoni_shared::db::tenant_session::project_scope(&pool, &p.project)
        .await
        .expect("a scope")
        .bind_organization(&p.organization)
        .await
        .expect("bound");
    db::insert_ping_key(
        &mut tx,
        &p.project,
        &p.organization,
        Uuid::new_v4(),
        &telmoni_shared::digest::sha256_hex(key.as_bytes()),
        &key.chars().take(20).collect::<String>(),
    )
    .await
    .expect("a key");
    tx.commit().await.expect("committed");

    let (status, body) = p.ping(&format!("/{key}/nightly-report")).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
}

#[sqlx::test]
async fn a_finished_run_reaches_clickhouse_through_the_outbox(pool: PgPool) {
    let Some(clickhouse) =
        common::clickhouse_or_skip("a_finished_run_reaches_clickhouse_through_the_outbox").await
    else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::probing(&pool);
    auth.seat(
        OWNER,
        &organization,
        &project,
        OrganizationRole::Owner,
        None,
    );
    let notices = NoticeLog::new();
    let state = common::state_with_stores_and_notices(&pool, auth, notices.clone());
    let p = Project::over(state, notices, organization, project);
    let key = p.mint().await;

    p.ping_ok(&format!("/{key}/nightly-report/start")).await;
    p.ping_ok(&format!(
        "/{key}/nightly-report/checkpoint?steps=2&cost=0.25"
    ))
    .await;
    p.ping_ok(&format!("/{key}/nightly-report/fail?exit_code=1"))
        .await;
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 2, held: 0 },
        "the notice and the run's row"
    );
    assert!(outbox_of(&pool, &p.project).await.is_empty());
    assert_eq!(p.notices.notices().len(), 1);

    common::eventually("the run's row is in ClickHouse", || async {
        common::rows_of(&clickhouse.store, &p.project).await == (1, 1)
    })
    .await;
    let only = telmoni_telemetry::store::Projects::one(p.project.clone());
    let row: RunRow = clickhouse
        .store
        .read(
            &only,
            "SELECT kind, name, agent, status, attributes FROM telemetry.spans \
             WHERE project_id IN {projects:Array(String)}",
        )
        .fetch_one()
        .await
        .expect("the run's row");
    assert_eq!(
        (
            row.kind.as_str(),
            row.name.as_str(),
            row.agent.as_str(),
            row.status.as_str()
        ),
        ("run", "nightly-report", "nightly-report", "error")
    );
    assert_eq!(
        row.attributes,
        vec![
            ("telmoni.run.steps".to_owned(), "2".to_owned()),
            ("telmoni.run.cost_usd".to_owned(), "0.25".to_owned()),
            ("process.exit.code".to_owned(), "1".to_owned()),
        ]
    );
}

/// A run reaches `spans` once, however many rows carry it: an end ping sent
/// again once its first copy got through files a second run of no duration
/// under the same `rid`, and two such rows may wait in one batch; the loop
/// drops each copy before the insert rather than write it twice, and
/// answers every ping `OK`, since the run did end.
#[sqlx::test]
async fn a_run_ended_twice_is_one_row_of_spans(pool: PgPool) {
    let Some(clickhouse) =
        common::clickhouse_or_skip("a_run_ended_twice_is_one_row_of_spans").await
    else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::probing(&pool);
    auth.seat(
        OWNER,
        &organization,
        &project,
        OrganizationRole::Owner,
        None,
    );
    let notices = NoticeLog::new();
    let state = common::state_with_stores_and_notices(&pool, auth, notices.clone());
    let p = Project::over(state, notices, organization, project);
    let key = p.mint().await;
    let spans = || async { common::rows_of(&clickhouse.store, &p.project).await.0 };

    let run = rid();
    p.ping_ok(&format!("/{key}/nightly-report/start?rid={run}"))
        .await;
    p.ping_ok(&format!("/{key}/nightly-report?rid={run}")).await;
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 2, held: 0 },
        "the notice and the run's row"
    );
    common::eventually("the run's row is in ClickHouse", || async {
        spans().await == 1
    })
    .await;

    // The end ping sent again, its first copy through: a run of no duration
    // under the same id waits in the outbox, and is dropped as landed.
    p.ping_ok(&format!("/{key}/nightly-report?rid={run}")).await;
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 1);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 0 }
    );
    assert!(outbox_of(&pool, &p.project).await.is_empty());
    assert_eq!(spans().await, 1, "a retried end ping wrote a second row");

    // Two copies in one batch: the older is written, the other dropped.
    let again = rid();
    p.ping_ok(&format!(
        "/{key}/nightly-report/fail?rid={again}&exit_code=3"
    ))
    .await;
    p.ping_ok(&format!(
        "/{key}/nightly-report/fail?rid={again}&exit_code=3"
    ))
    .await;
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 2);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 2, held: 0 }
    );
    assert!(outbox_of(&pool, &p.project).await.is_empty());
    common::eventually("the second run's row is in ClickHouse", || async {
        spans().await == 2
    })
    .await;
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled::default()
    );
    assert_eq!(spans().await, 2, "a copy in one batch wrote a second row");
}

/// A store that cannot be reached is the store's fault: the run's row is
/// handed back for the next tick with its try returned, tick after tick,
/// and never counted stuck. Nothing listens where this suite's ClickHouse
/// would be.
#[sqlx::test]
async fn a_store_outage_hands_a_run_back_with_its_try(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    // The first tick hands the notice over; every tick hands the run back.
    // Thirteen is one past the cap, had the tries counted.
    for tick in 1..=13 {
        let settled = outbox::tick_once(&p.state).await.expect("a tick");
        assert_eq!(settled.held, 1, "tick {tick}: {settled:?}");
        assert_eq!(
            settled.landed,
            u32::from(tick == 1),
            "tick {tick}: {settled:?}"
        );
        assert_eq!(
            tries_of(&pool, &p.project).await,
            [(0, false)],
            "tick {tick}: the row waits with its try, unleased"
        );
    }
    let (pending, stuck, oldest) = outbox::depth(&p.state).await.expect("the depth");
    assert_eq!((pending, stuck), (1, 0));
    assert!(oldest.is_some(), "the row to land is the oldest");
}

/// A row no version of this code wrote is the row's fault: held longer each
/// time, counted, and after its twelfth try left — nothing takes it, the
/// depth counts it stuck — until an operator's `release outbox` gives it its
/// tries back, which it spends again.
#[sqlx::test]
async fn a_row_that_does_not_parse_waits_out_its_tries_until_released(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO telemetry.outbox (id, project_id, organization_id, kind, payload)
         VALUES ($1, $2, $3, 'span', '{}')",
    )
    .bind(id)
    .bind(&p.project)
    .bind(&p.organization)
    .execute(&pool)
    .await
    .expect("a row no version of this code wrote");

    for attempt in 1..=12 {
        assert_eq!(
            outbox::tick_once(&p.state).await.expect("a tick"),
            Settled { landed: 0, held: 1 },
            "try {attempt}"
        );
        assert_eq!(
            tries_of(&pool, &p.project).await,
            [(attempt, true)],
            "try {attempt}: held for its wait"
        );
        // Its wait over.
        sqlx::query(
            "UPDATE telemetry.outbox SET lease_until = now() - interval '1 second' WHERE id = $1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .expect("the wait over");
    }
    // Past its tries: nothing takes it, and the depth counts it stuck.
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled::default()
    );
    assert_eq!(
        outbox::depth(&p.state).await.expect("the depth"),
        (0, 1, None)
    );

    // Released: tried again, held again, and not touched by another release.
    assert_eq!(outbox::release(&p.state).await.expect("the release"), 1);
    assert_eq!(outbox::depth(&p.state).await.expect("the depth").0, 1);
    assert_eq!(tries_of(&pool, &p.project).await, [(0, false)]);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 0, held: 1 }
    );
    assert_eq!(tries_of(&pool, &p.project).await, [(1, true)]);
    assert_eq!(
        outbox::release(&p.state).await.expect("the release"),
        0,
        "a row within its tries is not touched"
    );
}

/// Notifications unable to write a notice is an outage of ours: the row is
/// handed back with its try. A notice it refuses is the row's fault: held
/// and counted. Both beside a run's row the unreachable store hands back.
#[sqlx::test]
async fn a_refused_notice_counts_a_try_and_one_notifications_could_not_write_does_not(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.notices.answer(Answer::Down);
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 0, held: 2 }
    );
    assert_eq!(
        tries_of(&pool, &p.project).await,
        [(0, false), (0, false)],
        "notifications down and ClickHouse unreachable: both handed back"
    );

    p.notices.answer(Answer::Refused);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 0, held: 2 }
    );
    assert_eq!(
        tries_of(&pool, &p.project).await,
        [(1, true), (0, false)],
        "a refused notice is held and counted; the run's row handed back again"
    );

    // Taken once its wait is over, the run's row still waiting on the store.
    p.notices.answer(Answer::Taken);
    sqlx::query(
        "UPDATE telemetry.outbox SET lease_until = now() - interval '1 second'
          WHERE project_id = $1 AND kind = 'notice'",
    )
    .bind(&p.project)
    .execute(&pool)
    .await
    .expect("the wait over");
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 }
    );
    assert_eq!(p.notices.notices().len(), 1);
    assert_eq!(tries_of(&pool, &p.project).await, [(0, false)]);
}

#[sqlx::test]
async fn the_purges_and_a_move_take_the_keys_runs_and_outbox_rows_with_the_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report/start")).await;
    assert_eq!(open_runs(&pool, &p.project).await.len(), 1);
    assert_eq!(outbox_of(&pool, &p.project).await.len(), 1);

    // Moved: every row of the project's names the organization that holds
    // it now.
    let other = OrganizationId::new();
    let mut tx = maintenance_scope(&p.state.db, TelemetryLane)
        .await
        .expect("the lane");
    assert!(
        db::move_project(&mut tx, &p.project, &other)
            .await
            .expect("moved")
    );
    tx.commit().await.expect("committed");
    for table in ["ping_keys", "open_runs", "outbox", "monitors"] {
        let organizations: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT DISTINCT organization_id FROM telemetry.{table} WHERE project_id = $1"
        ))
        .bind(&p.project)
        .fetch_all(&pool)
        .await
        .expect("read the table");
        assert_eq!(organizations, vec![other.to_string()], "{table}");
    }

    // Purged: nothing of the project's left in any table.
    let mut tx = maintenance_scope(&p.state.db, TelemetryLane)
        .await
        .expect("the lane");
    let purged = db::purge_project(&mut tx, &p.project)
        .await
        .expect("purged");
    tx.commit().await.expect("committed");
    assert_eq!(
        purged, 5,
        "the outbox row, the run, the key, the monitor and the settings row"
    );
    for table in [
        "ping_keys",
        "open_runs",
        "outbox",
        "monitors",
        "project_settings",
    ] {
        let left: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM telemetry.{table} WHERE project_id = $1"
        ))
        .bind(&p.project)
        .fetch_one(&pool)
        .await
        .expect("count");
        assert_eq!(left, 0, "{table}");
    }
    let (status, _) = p.ping(&format!("/{key}/nightly-report")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a purged key opens nothing");

    // An organization's purge finds its projects' rows the same way.
    let q = project(&pool);
    let key = q.mint().await;
    q.ping_ok(&format!("/{key}/nightly-report/start")).await;
    let mut tx = maintenance_scope(&q.state.db, TelemetryLane)
        .await
        .expect("the lane");
    let purged = db::purge_organization(&mut tx, &q.organization)
        .await
        .expect("purged");
    tx.commit().await.expect("committed");
    assert_eq!(purged, 5);
}
