//! The ticker as it finds what no ping says: a monitor due is late while its
//! grace runs and failed past it, a run quiet past its stall timeout stalls
//! its monitor, a run past its ceiling is ended as `error` and fails it, the
//! owner's states stand, and every rule is decided from the rows as they
//! stand under the ticker's locks; the ticker beside a move, a delete, a
//! ping's settling, another tick and a lost database, waiting on none of
//! them; and the consoles told once a project. Every tick here is held to
//! ten seconds, so a ticker that waits on anybody fails its test.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, NoticeLog, SECRET};
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::NoLimits;
use telmoni_shared::test_util::{apply_audit_migrations, captured_logs};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, TelmoniError};
use telmoni_telemetry::db::TelemetryLane;
use telmoni_telemetry::ticker::{self, Ticked};
use telmoni_telemetry::{AppState, ping_router, router};

const MONITORS: &str = "/internal/v1/telemetry/monitors";
const PING_KEYS: &str = "/internal/v1/telemetry/ping-keys";
const OWNER: &str = "user_owner";

/// The module's two routers over a project the roster seats its owner on.
struct Project {
    app: Router,
    pings: Router,
    state: Arc<AppState>,
    organization: OrganizationId,
    project: ProjectId,
}

fn project(pool: &PgPool) -> Project {
    let auth = AuthStub::probing(pool);
    let state =
        common::state_with_notices(pool, auth.clone(), Arc::new(NoLimits), NoticeLog::new());
    project_over(&state, &auth, &OrganizationId::new())
}

/// A project of `organization` over `state`, seated on `auth`.
fn project_over(state: &Arc<AppState>, auth: &AuthStub, organization: &OrganizationId) -> Project {
    let project = ProjectId::new();
    auth.seat(OWNER, organization, &project, OrganizationRole::Owner, None);
    Project {
        app: router(state.clone()),
        pings: ping_router(state.clone()),
        state: state.clone(),
        organization: organization.clone(),
        project,
    }
}

impl Project {
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

    /// A console action on a monitor, `pause` or `resume`, answered `OK`.
    async fn act(&self, slug: &str, action: &str) -> Value {
        let (status, body) = self
            .send(self.request("POST", &format!("{MONITORS}/{slug}/{action}"), None))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    /// A `GET` ping, as `curl` sends one, answered `OK`.
    async fn ping_ok(&self, path: &str) {
        let response = self
            .pings
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("the lane answers");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        assert_eq!(
            (status, String::from_utf8_lossy(&bytes).as_ref()),
            (StatusCode::OK, "OK"),
            "{path}"
        );
    }

    /// One tick, held to ten seconds: a ticker that waits on anybody fails
    /// here.
    async fn tick(&self) -> Ticked {
        tokio::time::timeout(Duration::from_secs(10), ticker::tick_once(&self.state))
            .await
            .expect("the tick waited on something")
            .expect("a tick")
    }
}

/// A monitor as the console makes one: hourly, a minute of grace, held to a
/// stall timeout of two minutes and a step cap of three.
fn hourly(slug: &str) -> Value {
    json!({
        "name": slug,
        "slug": slug,
        "interval_s": 3600,
        "grace_s": 60,
        "bounds": { "stall_after_s": 120, "max_steps": 3 },
    })
}

/// A tick that moved `moved` monitors, ended `ended` runs and left none.
fn ticked(moved: u32, ended: u32) -> Ticked {
    Ticked {
        moved,
        ended,
        left: 0,
    }
}

/// A clock as a lane wrote it.
fn at(value: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value.as_str().expect("a clock"))
        .expect("an RFC 3339 clock")
        .with_timezone(&Utc)
}

/// The monitor's due time written straight into its row, `ago` seconds
/// back, as time passing would leave it.
async fn due(pool: &PgPool, project: &ProjectId, slug: &str, ago: i64) {
    let written = sqlx::query(
        "UPDATE telemetry.monitors SET next_due_at = now() - make_interval(secs => $3)
          WHERE project_id = $1 AND slug = $2",
    )
    .bind(project)
    .bind(slug)
    .bind(ago)
    .execute(pool)
    .await
    .expect("write the monitor's due time")
    .rows_affected();
    assert_eq!(written, 1, "{slug}");
}

/// The run last heard `ago` seconds back, as silence would leave it.
async fn quiet(pool: &PgPool, project: &ProjectId, run: &str, ago: i64) {
    let written = sqlx::query(
        "UPDATE telemetry.open_runs SET last_heard_at = now() - make_interval(secs => $3)
          WHERE project_id = $1 AND id = $2",
    )
    .bind(project)
    .bind(Uuid::parse_str(run).expect("a run's id"))
    .bind(ago)
    .execute(pool)
    .await
    .expect("write when the run was last heard")
    .rows_affected();
    assert_eq!(written, 1, "{run}");
}

/// The run's ceiling a minute back, as a run that outlived it would find.
async fn past_ceiling(pool: &PgPool, project: &ProjectId, run: &str) {
    let written = sqlx::query(
        "UPDATE telemetry.open_runs SET ends_by = now() - interval '1 minute'
          WHERE project_id = $1 AND id = $2",
    )
    .bind(project)
    .bind(Uuid::parse_str(run).expect("a run's id"))
    .execute(pool)
    .await
    .expect("write the run's ceiling")
    .rows_affected();
    assert_eq!(written, 1, "{run}");
}

/// The project's open runs, oldest first: each run's id, and when it was
/// last heard.
async fn open_runs(pool: &PgPool, project: &ProjectId) -> Vec<(Uuid, DateTime<Utc>)> {
    sqlx::query_as(
        "SELECT id, last_heard_at FROM telemetry.open_runs
          WHERE project_id = $1
          ORDER BY started_at, id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read a project's open runs")
}

/// The finished runs waiting in the project's outbox, oldest first.
async fn finished_runs(pool: &PgPool, project: &ProjectId) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM telemetry.outbox
          WHERE project_id = $1 AND kind = 'span'
          ORDER BY id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read a project's outbox")
}

fn rid() -> String {
    Uuid::new_v4().to_string()
}

/// A monitor due is late while its grace runs, failed once it has run out
/// — its next due time found again then — and a tick that finds nothing
/// changed writes nothing; one late or failed already stays so at its next
/// due time, and a ping ending well is what recovers it.
#[sqlx::test]
async fn a_monitor_due_is_late_within_its_grace_and_failed_past_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let made = p.make(hourly("hourly-sync")).await;
    assert_eq!(p.tick().await, ticked(0, 0), "due an hour on: nothing");

    // Due half a minute ago, a minute of grace: late, due at the same time.
    due(&pool, &p.project, "hourly-sync", 30).await;
    let before = p.monitor("hourly-sync").await;
    assert_eq!(p.tick().await, ticked(1, 0));
    let late = p.monitor("hourly-sync").await;
    assert_eq!(late["state"], "late", "{late}");
    assert_eq!(late["next_due_at"], before["next_due_at"], "{late}");
    assert!(at(&late["state_changed_at"]) > at(&made["state_changed_at"]));
    assert_eq!(
        p.tick().await,
        ticked(0, 0),
        "late already, within its grace"
    );
    assert_eq!(
        p.monitor("hourly-sync").await["updated_at"],
        late["updated_at"]
    );

    // Past its grace: failed, and due again an hour on.
    due(&pool, &p.project, "hourly-sync", 120).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    let failed = p.monitor("hourly-sync").await;
    assert_eq!(failed["state"], "failed", "{failed}");
    assert!(
        at(&failed["next_due_at"]) > Utc::now() + chrono::Duration::minutes(55),
        "{failed}"
    );
    assert!(at(&failed["state_changed_at"]) > at(&late["state_changed_at"]));
    assert_eq!(p.tick().await, ticked(0, 0), "failed, and not due");

    // Failed at its next due time: within its grace nothing is read, and
    // past it the due time alone moves on.
    due(&pool, &p.project, "hourly-sync", 30).await;
    assert_eq!(p.tick().await, ticked(0, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    due(&pool, &p.project, "hourly-sync", 120).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    let still = p.monitor("hourly-sync").await;
    assert_eq!(still["state"], "failed", "{still}");
    assert_eq!(still["state_changed_at"], failed["state_changed_at"]);
    assert!(at(&still["next_due_at"]) > Utc::now());

    // Healthy straight to failed where the grace had run out before any
    // tick saw it due: a tick a day late calls nothing late first.
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    due(&pool, &p.project, "hourly-sync", 120).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");

    // Late, then its run comes: the lane recovers it, and the ticker finds
    // nothing more.
    due(&pool, &p.project, "hourly-sync", 30).await;
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    due(&pool, &p.project, "hourly-sync", 30).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "late");
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    let recovered = p.monitor("hourly-sync").await;
    assert_eq!(recovered["state"], "healthy", "{recovered}");
    assert!(at(&recovered["next_due_at"]) > Utc::now());
    assert_eq!(p.tick().await, ticked(0, 0));

    // No grace is no grace: due a moment ago, it has failed already.
    let (status, saved) = p
        .send(p.request(
            "PUT",
            &format!("{MONITORS}/hourly-sync"),
            Some(json!({ "name": "hourly-sync", "interval_s": 3600, "grace_s": 0 })),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    due(&pool, &p.project, "hourly-sync", 1).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
}

/// The loop ticks on its own, at the cadence `TELEMETRY_TICK_SECS` sets
/// rather than the five seconds it takes unset.
#[sqlx::test]
async fn the_loop_ticks_at_the_cadence_it_is_set_to(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let state = common::state_ticking_every(&pool, auth.clone(), 1);
    let p = project_over(&state, &auth, &OrganizationId::new());
    p.make(hourly("hourly-sync")).await;
    due(&pool, &p.project, "hourly-sync", 30).await;

    let started = std::time::Instant::now();
    let ticker = tokio::spawn(ticker::run(state.clone()));
    common::eventually("the loop marks the monitor late", || async {
        p.monitor("hourly-sync").await["state"] == "late"
    })
    .await;
    ticker.abort();
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
}

/// A monitor with a run going is never late or failed for a due time, since
/// the run's own bounds hold it: its due time alone moves on past its grace.
#[sqlx::test]
async fn a_monitor_with_a_run_going_is_never_late(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");

    due(&pool, &p.project, "hourly-sync", 30).await;
    assert_eq!(p.tick().await, ticked(0, 0), "within its grace, nothing");
    let running = p.monitor("hourly-sync").await;
    assert_eq!(running["state"], "running", "{running}");
    due(&pool, &p.project, "hourly-sync", 120).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    let moved = p.monitor("hourly-sync").await;
    assert_eq!(moved["state"], "running", "{moved}");
    assert_eq!(moved["state_changed_at"], running["state_changed_at"]);
    assert!(at(&moved["next_due_at"]) > Utc::now(), "{moved}");

    // Past its step cap, the same.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?steps=4"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "over_bound");
    due(&pool, &p.project, "hourly-sync", 120).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    let over = p.monitor("hourly-sync").await;
    assert_eq!(over["state"], "over_bound", "{over}");
    assert!(at(&over["next_due_at"]) > Utc::now(), "{over}");
}

/// A run quiet past its stall timeout stalls its monitor, its due time kept
/// and the run still open for the ping that recovers it; a run with no
/// stall timeout is never quiet; and a monitor its owner holds, paused or
/// not yet set up, keeps its state.
#[sqlx::test]
async fn a_run_quiet_past_its_stall_timeout_stalls_its_monitor(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let made = p.make(hourly("hourly-sync")).await;
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    quiet(&pool, &p.project, &run, 60).await;
    assert_eq!(p.tick().await, ticked(0, 0), "quiet a minute, two allowed");

    quiet(&pool, &p.project, &run, 180).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    let stalled = p.monitor("hourly-sync").await;
    assert_eq!(stalled["state"], "stalled", "{stalled}");
    assert_eq!(stalled["next_due_at"], made["next_due_at"], "{stalled}");
    assert_eq!(open_runs(&pool, &p.project).await.len(), 1);
    assert_eq!(p.tick().await, ticked(0, 0), "stalled already");

    // Heard again, the lane puts it back to running, and the tick finds it
    // heard.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?rid={run}&steps=1"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    assert_eq!(p.tick().await, ticked(0, 0));

    // Past its step cap and then quiet: stalled, the graver.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?rid={run}&steps=4"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "over_bound");
    quiet(&pool, &p.project, &run, 180).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");

    // Paused with its quiet run: paused it stays, and due at no time; resumed
    // with the run still quiet, the next tick stalls it again.
    p.act("hourly-sync", "pause").await;
    assert_eq!(p.tick().await, ticked(0, 0));
    let paused = p.monitor("hourly-sync").await;
    assert_eq!(paused["state"], "paused", "{paused}");
    assert!(paused["next_due_at"].is_null(), "{paused}");
    p.act("hourly-sync", "resume").await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");
    p.ping_ok(&format!("/{key}/hourly-sync?rid={run}")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    assert_eq!(p.tick().await, ticked(0, 0));

    // A monitor with no stall timeout: its run is never quiet.
    p.make(json!({ "name": "unbounded", "slug": "unbounded" }))
        .await;
    let free = rid();
    p.ping_ok(&format!("/{key}/unbounded/start?rid={free}"))
        .await;
    quiet(&pool, &p.project, &free, 24 * 60 * 60).await;
    assert_eq!(p.tick().await, ticked(0, 0));
    assert_eq!(p.monitor("unbounded").await["state"], "running");

    // One its first ping made, with the hour's stall timeout a discovered
    // monitor is given: its run is visited, and its state is its owner's.
    let found = rid();
    p.ping_ok(&format!("/{key}/nightly-job/start?rid={found}"))
        .await;
    assert_eq!(p.monitor("nightly-job").await["state"], "new");
    quiet(&pool, &p.project, &found, 2 * 60 * 60).await;
    assert_eq!(p.tick().await, ticked(0, 0));
    assert_eq!(p.monitor("nightly-job").await["state"], "new");
    assert_eq!(open_runs(&pool, &p.project).await.len(), 2);
}

/// A run past its ceiling is ended as `error` with no exit code — its span
/// ending when the run was last heard, with what its checkpoints reported —
/// and its monitor fails as a `/fail` leaves it, due again; a run still
/// within its ceiling goes on beside it, and a monitor its owner holds has
/// its run ended all the same, its state kept.
#[sqlx::test]
async fn a_run_past_its_ceiling_is_ended_as_error_and_fails_its_monitor(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;
    let (first, second) = (rid(), rid());
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={first}"))
        .await;
    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={first}&steps=2&cost=0.5"
    ))
    .await;
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={second}"))
        .await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 2);
    let heard = runs[0].1;
    assert_eq!(p.tick().await, ticked(0, 0), "both within their ceilings");

    past_ceiling(&pool, &p.project, &first).await;
    assert_eq!(p.tick().await, ticked(1, 1));
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1, "the second goes on");
    assert_eq!(runs[0].0.to_string(), second);
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["run_id"].as_str(), Some(first.as_str()));
    assert_eq!(finished[0]["agent"], "hourly-sync");
    assert_eq!(finished[0]["status"], "error");
    assert!(finished[0]["exit_code"].is_null());
    assert_eq!(finished[0]["steps"], 2);
    assert_eq!(finished[0]["cost_usd"], 0.5);
    assert_eq!(at(&finished[0]["ended_at"]), heard, "ended when last heard");
    let failed = p.monitor("hourly-sync").await;
    assert_eq!(failed["state"], "failed", "{failed}");
    assert!(at(&failed["next_due_at"]) > Utc::now(), "{failed}");
    assert_eq!(p.tick().await, ticked(0, 0));

    // The second's checkpoint puts the monitor back to running; its end
    // ends it well.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?rid={second}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    p.ping_ok(&format!("/{key}/hourly-sync?rid={second}")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 2);

    // A paused monitor's run past its ceiling is ended, and the monitor
    // left paused; so is one its first ping made, left new.
    let paused = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={paused}"))
        .await;
    p.act("hourly-sync", "pause").await;
    let found = rid();
    p.ping_ok(&format!("/{key}/nightly-job/start?rid={found}"))
        .await;
    past_ceiling(&pool, &p.project, &paused).await;
    past_ceiling(&pool, &p.project, &found).await;
    assert_eq!(p.tick().await, ticked(0, 2));
    assert!(open_runs(&pool, &p.project).await.is_empty());
    let paused = p.monitor("hourly-sync").await;
    assert_eq!(paused["state"], "paused", "{paused}");
    assert!(
        paused["next_due_at"].is_null(),
        "due though paused: {paused}"
    );
    let new = p.monitor("nightly-job").await;
    assert_eq!(new["state"], "new", "{new}");
    assert!(new["next_due_at"].is_null(), "due though new: {new}");
    // Visited by id, so the two end in either order.
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 4);
    let mut agents: Vec<&str> = finished[2..]
        .iter()
        .map(|run| run["agent"].as_str().unwrap_or_default())
        .collect();
    agents.sort_unstable();
    assert_eq!(agents, ["hourly-sync", "nightly-job"]);
    for run in &finished[2..] {
        assert_eq!(run["status"], "error", "{run}");
        assert!(run["steps"].is_null(), "{run}");
    }
}

/// A saved timing the ticker cannot read is logged, once, and leaves its
/// monitor due at no time; the tick goes on to the rest.
#[sqlx::test]
async fn a_timing_that_cannot_be_read_leaves_its_monitor_due_at_no_time(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let state =
        common::state_with_notices(&pool, auth.clone(), Arc::new(NoLimits), NoticeLog::new());
    let broken = project_over(&state, &auth, &OrganizationId::new());
    let fine = project_over(&state, &auth, &OrganizationId::new());
    broken.make(hourly("hourly-sync")).await;
    fine.make(hourly("hourly-sync")).await;
    let written = sqlx::query(
        "UPDATE telemetry.monitors SET timezone = 'Mars/Olympus'
          WHERE project_id = $1 AND slug = 'hourly-sync'",
    )
    .bind(&broken.project)
    .execute(&pool)
    .await
    .expect("write a zone the database does not name")
    .rows_affected();
    assert_eq!(written, 1);
    due(&pool, &broken.project, "hourly-sync", 120).await;
    due(&pool, &fine.project, "hourly-sync", 120).await;

    let (capturing, logs) = captured_logs();
    assert_eq!(broken.tick().await, ticked(2, 0));
    drop(capturing);
    assert_eq!(
        logs.text()
            .matches("a saved monitor's timing cannot be read")
            .count(),
        1
    );
    let unread = broken.monitor("hourly-sync").await;
    assert_eq!(unread["state"], "failed", "{unread}");
    assert!(unread["next_due_at"].is_null(), "{unread}");
    let read = fine.monitor("hourly-sync").await;
    assert_eq!(read["state"], "failed", "{read}");
    assert!(read["next_due_at"].is_string(), "{read}");
    assert_eq!(
        broken.tick().await,
        ticked(0, 0),
        "due at no time is off the read"
    );
}

/// A project a move or a purge holds is left for a later tick, at once, and
/// the tick goes on to the rest; once the move is done, the next tick takes
/// it.
#[sqlx::test]
async fn a_project_a_move_holds_is_left_to_the_next_tick(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let state =
        common::state_with_notices(&pool, auth.clone(), Arc::new(NoLimits), NoticeLog::new());
    let moving = project_over(&state, &auth, &OrganizationId::new());
    let still = project_over(&state, &auth, &OrganizationId::new());
    moving.make(hourly("hourly-sync")).await;
    still.make(hourly("hourly-sync")).await;
    due(&pool, &moving.project, "hourly-sync", 120).await;
    due(&pool, &still.project, "hourly-sync", 120).await;

    let mut mover = maintenance_scope(&state.db, TelemetryLane)
        .await
        .expect("the lane");
    project_move::lock_for_move(mover.conn(), &moving.project)
        .await
        .expect("the move lock");
    assert_eq!(
        moving.tick().await,
        Ticked {
            moved: 1,
            ended: 0,
            left: 1
        }
    );
    assert_eq!(moving.monitor("hourly-sync").await["state"], "healthy");
    assert_eq!(still.monitor("hourly-sync").await["state"], "failed");
    mover.rollback().await.expect("the move lets go");
    assert_eq!(moving.tick().await, ticked(1, 0));
    assert_eq!(moving.monitor("hourly-sync").await["state"], "failed");
}

/// A monitor another writer holds — a ping settling it, the console
/// changing it, its delete — is left for a later tick without a wait, the
/// end of its runs rolled back with it, and the tick goes on to the rest.
#[sqlx::test]
async fn a_monitor_another_writer_holds_is_left_without_waiting(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("held")).await;
    p.make(hourly("free")).await;
    let run = rid();
    p.ping_ok(&format!("/{key}/held/start?rid={run}")).await;
    past_ceiling(&pool, &p.project, &run).await;
    due(&pool, &p.project, "free", 120).await;

    // Held as a ping's settling or the console's change holds it.
    for lock in ["FOR NO KEY UPDATE", "FOR UPDATE"] {
        let mut holder = pool.begin().await.expect("the holder's transaction");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT 1 FROM telemetry.monitors WHERE project_id = $1 AND slug = 'held' {lock}"
        )))
        .bind(&p.project)
        .execute(&mut *holder)
        .await
        .expect("hold the monitor");
        due(&pool, &p.project, "free", 120).await;
        assert_eq!(
            p.tick().await,
            Ticked {
                moved: 1,
                ended: 0,
                left: 1
            },
            "{lock}"
        );
        assert_eq!(open_runs(&pool, &p.project).await.len(), 1, "{lock}");
        assert!(finished_runs(&pool, &p.project).await.is_empty(), "{lock}");
        assert_eq!(p.monitor("held").await["state"], "running", "{lock}");
        assert_eq!(p.monitor("free").await["state"], "failed", "{lock}");
        holder.rollback().await.expect("the holder lets go");
    }
    assert_eq!(p.tick().await, ticked(1, 1));
    assert!(open_runs(&pool, &p.project).await.is_empty());
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 1);
    assert_eq!(p.monitor("held").await["state"], "failed");
}

/// A run a ping holds — ending it, as an end ping deletes its row — is left
/// to the ping, the monitor's other runs past their ceiling ended beside it
/// without a wait.
#[sqlx::test]
async fn a_run_a_ping_holds_is_left_to_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;
    let (held, free) = (rid(), rid());
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={held}"))
        .await;
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={free}"))
        .await;
    past_ceiling(&pool, &p.project, &held).await;
    past_ceiling(&pool, &p.project, &free).await;

    let mut holder = pool.begin().await.expect("the holder's transaction");
    sqlx::query("SELECT 1 FROM telemetry.open_runs WHERE project_id = $1 AND id = $2 FOR UPDATE")
        .bind(&p.project)
        .bind(Uuid::parse_str(&held).expect("a run's id"))
        .execute(&mut *holder)
        .await
        .expect("hold the run");
    assert_eq!(p.tick().await, ticked(1, 1));
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0.to_string(), held);
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    holder.rollback().await.expect("the holder lets go");
    let again = p.tick().await;
    assert_eq!((again.ended, again.left), (1, 0), "{again:?}");
    assert!(open_runs(&pool, &p.project).await.is_empty());
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 2);
}

/// Every rule is decided from the rows as they stand under the ticker's
/// locks, never from the tick's read: a pause committed since stands, a run
/// heard since is not stalled, and a run ended since is not ended twice.
#[sqlx::test]
async fn the_ticker_decides_from_the_rows_as_they_stand(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;

    // Paused since the read.
    due(&pool, &p.project, "hourly-sync", 120).await;
    let found = ticker::found(&p.state).await.expect("the read");
    assert_eq!(found.len(), 1);
    p.act("hourly-sync", "pause").await;
    assert_eq!(
        ticker::visit_each(&p.state, &found)
            .await
            .expect("the visits"),
        ticked(0, 0)
    );
    let paused = p.monitor("hourly-sync").await;
    assert_eq!(paused["state"], "paused", "{paused}");
    assert!(paused["next_due_at"].is_null(), "{paused}");
    p.act("hourly-sync", "resume").await;

    // Heard since the read.
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    quiet(&pool, &p.project, &run, 180).await;
    let found = ticker::found(&p.state).await.expect("the read");
    assert_eq!(found.len(), 1);
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?rid={run}"))
        .await;
    assert_eq!(
        ticker::visit_each(&p.state, &found)
            .await
            .expect("the visits"),
        ticked(0, 0)
    );
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");

    // Ended since the read.
    past_ceiling(&pool, &p.project, &run).await;
    let found = ticker::found(&p.state).await.expect("the read");
    assert_eq!(found.len(), 1);
    p.ping_ok(&format!("/{key}/hourly-sync?rid={run}")).await;
    assert_eq!(
        ticker::visit_each(&p.state, &found)
            .await
            .expect("the visits"),
        ticked(0, 0)
    );
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["status"], "ok");
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");

    // Saved to a later due time since the read: not due now, so nothing;
    // and deleted since the read: nothing, and no error.
    due(&pool, &p.project, "hourly-sync", 120).await;
    let found = ticker::found(&p.state).await.expect("the read");
    let (status, saved) = p
        .send(p.request(
            "PUT",
            &format!("{MONITORS}/hourly-sync"),
            Some(json!({ "name": "hourly-sync", "interval_s": 7200, "grace_s": 60 })),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        ticker::visit_each(&p.state, &found)
            .await
            .expect("the visits"),
        ticked(0, 0)
    );
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    due(&pool, &p.project, "hourly-sync", 120).await;
    let found = ticker::found(&p.state).await.expect("the read");
    let (status, _) = p
        .send(p.request("DELETE", &format!("{MONITORS}/hourly-sync"), None))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        ticker::visit_each(&p.state, &found)
            .await
            .expect("the visits"),
        ticked(0, 0),
        "gone is nothing to retry"
    );
}

/// Two ticks at once — two replicas — move each monitor once, and neither
/// waits on the other.
#[sqlx::test]
async fn two_ticks_at_once_move_each_monitor_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let slugs: Vec<String> = (0..5).map(|n| format!("job-{n}")).collect();
    for slug in &slugs {
        p.make(hourly(slug)).await;
        due(&pool, &p.project, slug, 120).await;
    }
    let (first, second) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(ticker::tick_once(&p.state), ticker::tick_once(&p.state))
    })
    .await
    .expect("a tick waited on the other");
    let (first, second) = (first.expect("a tick"), second.expect("a tick"));
    assert_eq!(first.moved + second.moved, 5, "{first:?} {second:?}");
    assert_eq!(first.ended + second.ended, 0);
    for slug in &slugs {
        let monitor = p.monitor(slug).await;
        assert_eq!(monitor["state"], "failed", "{monitor}");
        assert!(at(&monitor["next_due_at"]) > Utc::now(), "{monitor}");
    }
    assert_eq!(p.tick().await, ticked(0, 0));
}

/// A tick that moves anything of a project's tells the organization's open
/// consoles once for the project, as a ping tells them; one that moves
/// nothing tells nothing.
#[sqlx::test]
async fn a_tick_tells_the_organizations_open_consoles_once_a_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = OrganizationId::new();
    let auth = AuthStub::probing(&pool);
    let state = common::state_with_redis_and_notices(&pool, auth.clone(), NoticeLog::new());
    if state.redis.connection().await.is_none() {
        eprintln!(
            "skipping {}: Redis is not reachable (REDIS_URL, or the compose file's)",
            module_path!()
        );
        return;
    }
    let p = project_over(&state, &auth, &organization);
    p.make(hourly("one")).await;
    p.make(hourly("two")).await;
    due(&pool, &p.project, "one", 120).await;
    due(&pool, &p.project, "two", 120).await;

    let (told, mut heard) = tokio::sync::mpsc::unbounded_channel();
    let mut console = redis::Client::open(format!("{}/?protocol=resp3", common::redis_url()))
        .expect("a Redis URL")
        .get_multiplexed_async_connection_with_config(
            &redis::AsyncConnectionConfig::new().set_push_sender(told),
        )
        .await
        .expect("the console's connection");
    console
        .subscribe(format!("bfev:organization:{}", p.organization))
        .await
        .expect("subscribed");

    assert_eq!(p.tick().await, ticked(2, 0));
    let nudge = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let push = heard.recv().await.expect("the subscription holds");
            if push.kind == redis::PushKind::Message {
                let payload = push.data.into_iter().nth(1).expect("the message");
                return redis::from_redis_value::<String>(payload).expect("a string");
            }
        }
    })
    .await
    .expect("told within five seconds");
    let nudge: Value = serde_json::from_str(&nudge).expect("json");
    assert_eq!(nudge["type"], "monitors:changed");
    assert_eq!(nudge["data"]["organizationId"], p.organization.as_str());
    assert_eq!(nudge["data"]["projectId"], p.project.as_str());
    assert!(
        tokio::time::timeout(Duration::from_millis(300), heard.recv())
            .await
            .is_err(),
        "told twice for one project"
    );
    assert_eq!(p.tick().await, ticked(0, 0));
    assert!(
        tokio::time::timeout(Duration::from_millis(300), heard.recv())
            .await
            .is_err(),
        "a tick that moved nothing told the console"
    );

    // A run ended on a monitor its owner holds moves no state, and is told
    // all the same, since the project's runs changed.
    let key = p.mint().await;
    let found = rid();
    p.ping_ok(&format!("/{key}/nightly-job/start?rid={found}"))
        .await;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), heard.recv())
            .await
            .is_ok(),
        "the first ping told the console"
    );
    past_ceiling(&pool, &p.project, &found).await;
    assert_eq!(p.tick().await, ticked(0, 1));
    let told = tokio::time::timeout(Duration::from_secs(5), heard.recv())
        .await
        .expect("told within five seconds")
        .expect("the subscription holds");
    assert_eq!(told.kind, redis::PushKind::Message);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), heard.recv())
            .await
            .is_err(),
        "told twice for one run"
    );
}

/// A database that stops answering ends the tick where it stopped.
#[sqlx::test]
async fn a_tick_that_loses_its_database_ends_there(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    p.make(hourly("hourly-sync")).await;
    due(&pool, &p.project, "hourly-sync", 120).await;
    p.state.db.close().await;
    let ended = tokio::time::timeout(Duration::from_secs(10), ticker::tick_once(&p.state))
        .await
        .expect("the tick waited on a closed pool");
    assert!(
        matches!(ended, Err(TelmoniError::Database(sqlx::Error::PoolClosed))),
        "{ended:?}"
    );
}

/// A stalled monitor, or a failed one with a quiet run, past its grace has
/// its due time found again all the same, so it leaves the tick's read
/// rather than being read every tick until the quiet run's ceiling.
#[sqlx::test]
async fn a_stalled_monitor_past_its_grace_leaves_the_read(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    quiet(&pool, &p.project, &run, 180).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");
    due(&pool, &p.project, "hourly-sync", 120).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    let stalled = p.monitor("hourly-sync").await;
    assert_eq!(stalled["state"], "stalled", "{stalled}");
    assert!(at(&stalled["next_due_at"]) > Utc::now(), "{stalled}");
    assert_eq!(p.tick().await, ticked(0, 0), "off the read");
    assert!(ticker::found(&p.state).await.expect("the read").is_empty());

    // Failed, with the quiet run still open: failed it stays, the graver,
    // off the read until its grace runs out, and due again then.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?rid={run}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={}", rid()))
        .await;
    let failed = p.monitor("hourly-sync").await;
    assert_eq!(failed["state"], "failed", "{failed}");
    assert_eq!(open_runs(&pool, &p.project).await.len(), 1);
    quiet(&pool, &p.project, &run, 180).await;
    assert_eq!(p.tick().await, ticked(0, 0));
    assert!(ticker::found(&p.state).await.expect("the read").is_empty());
    due(&pool, &p.project, "hourly-sync", 120).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    let still = p.monitor("hourly-sync").await;
    assert_eq!(still["state"], "failed", "{still}");
    assert_eq!(still["state_changed_at"], failed["state_changed_at"]);
    assert!(at(&still["next_due_at"]) > Utc::now(), "{still}");
    assert_eq!(p.tick().await, ticked(0, 0));

    // Heard, the run puts the monitor back to running, and quiet again it
    // stalls it.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?rid={run}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    quiet(&pool, &p.project, &run, 180).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");
}

/// Runs that overlap do not trade the monitor's state: stalled on a run
/// still quiet, it stays stalled through the other run's checkpoints, its
/// step cap and its end, runs again once the quiet run is heard, and fails
/// or runs again once a ping ends the quiet run.
#[sqlx::test]
async fn overlapping_runs_do_not_trade_the_state(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let made = p.make(hourly("hourly-sync")).await;
    let (quiet_one, live_one) = (rid(), rid());
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={quiet_one}"))
        .await;
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={live_one}"))
        .await;
    quiet(&pool, &p.project, &quiet_one, 180).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");

    // The live run heard, past its cap, and ended well: stalled throughout,
    // its end moving the due time on, and nothing for the tick.
    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={live_one}&steps=1"
    ))
    .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");
    assert_eq!(p.tick().await, ticked(0, 0));
    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={live_one}&steps=4"
    ))
    .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");
    p.ping_ok(&format!("/{key}/hourly-sync?rid={live_one}"))
        .await;
    let ended = p.monitor("hourly-sync").await;
    assert_eq!(ended["state"], "stalled", "{ended}");
    assert!(
        at(&ended["next_due_at"]) > at(&made["next_due_at"]),
        "{ended}"
    );
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 1);
    assert_eq!(p.tick().await, ticked(0, 0));

    // The quiet run heard: running again.
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?rid={quiet_one}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");

    // Quiet again and failed by its ping: failed. Another quiet, ended by
    // a bare `/start` as one that never said it ended: running.
    quiet(&pool, &p.project, &quiet_one, 180).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={quiet_one}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    let another = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={another}"))
        .await;
    quiet(&pool, &p.project, &another, 180).await;
    assert_eq!(p.tick().await, ticked(1, 0));
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    assert_eq!(open_runs(&pool, &p.project).await.len(), 1);
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 3);
    assert_eq!(p.tick().await, ticked(0, 0));
}

/// A ping holding its monitor from its read, before it settles it, holds
/// the ticker for nothing: the tick ends the monitor's run and writes it,
/// and the ping's settling, which waits on the tick alone, is through once
/// the tick has committed.
#[sqlx::test]
async fn a_ping_holding_its_monitor_holds_the_ticker_for_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    past_ceiling(&pool, &p.project, &run).await;

    let mut holder = pool.begin().await.expect("the ping's transaction");
    sqlx::query(
        "SELECT 1 FROM telemetry.monitors WHERE project_id = $1 AND slug = 'hourly-sync'
            FOR KEY SHARE",
    )
    .bind(&p.project)
    .execute(&mut *holder)
    .await
    .expect("hold the monitor as a ping does");
    assert_eq!(p.tick().await, ticked(1, 1));
    assert!(open_runs(&pool, &p.project).await.is_empty());
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    let settling = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query(
            "SELECT 1 FROM telemetry.monitors WHERE project_id = $1 AND slug = 'hourly-sync'
                FOR NO KEY UPDATE",
        )
        .bind(&p.project)
        .execute(&mut *holder),
    )
    .await;
    assert!(
        settling.is_ok_and(|held| held.is_ok()),
        "the ping's settling waited on the tick"
    );
    holder.rollback().await.expect("the ping lets go");
}

/// A run both quiet and past its ceiling is ended, not counted quiet: its
/// monitor fails rather than stalls.
#[sqlx::test]
async fn a_run_both_quiet_and_past_its_ceiling_is_ended(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    quiet(&pool, &p.project, &run, 180).await;
    past_ceiling(&pool, &p.project, &run).await;
    assert_eq!(p.tick().await, ticked(1, 1));
    assert!(open_runs(&pool, &p.project).await.is_empty());
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["status"], "error");
    assert_eq!(p.tick().await, ticked(0, 0));
}

/// A tick visits a page of monitors, by project and id, and leaves the rest
/// to the next.
#[sqlx::test]
async fn a_tick_visits_a_page_and_the_next_the_rest(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    p.make(hourly("job-0")).await;
    due(&pool, &p.project, "job-0", 120).await;
    // Written straight into the table, as five hundred saves would leave
    // them: a page and one more, every one past its grace.
    let written = sqlx::query(
        "INSERT INTO telemetry.monitors
             (id, project_id, organization_id, slug, name, agent, interval_s, grace_s, state,
              next_due_at)
         SELECT gen_random_uuid(), $1, $2, 'job-' || n, 'job-' || n, 'job-' || n, 3600, 60,
                'healthy', now() - interval '2 minutes'
           FROM generate_series(1, 500) AS n",
    )
    .bind(&p.project)
    .bind(&p.organization)
    .execute(&pool)
    .await
    .expect("write a page of monitors")
    .rows_affected();
    assert_eq!(written, 500);
    let tick = || async {
        tokio::time::timeout(Duration::from_secs(60), ticker::tick_once(&p.state))
            .await
            .expect("the tick waited on something")
            .expect("a tick")
    };
    assert_eq!(tick().await, ticked(500, 0));
    assert_eq!(tick().await, ticked(1, 0));
    assert_eq!(tick().await, ticked(0, 0));
    let failed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.monitors
          WHERE project_id = $1 AND state = 'failed' AND next_due_at > now()",
    )
    .bind(&p.project)
    .fetch_one(&pool)
    .await
    .expect("count the monitors failed and due again");
    assert_eq!(failed, 501);
}

/// The one session found waiting on `pid`'s locks, polled for as `what`.
async fn waits_on(pool: &PgPool, pid: i32, what: &str) {
    common::eventually(what, move || async move {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                             WHERE $1 = ANY (pg_blocking_pids(pid)))",
        )
        .bind(pid)
        .fetch_one(pool)
        .await
        .expect("ask who waits")
    })
    .await;
}

/// A checkpoint hearing a run already quiet, beside the tick that stalls
/// its monitor for it: the ping, with nothing of what it read to change,
/// still settles the monitor from the row as the tick left it, so the
/// monitor runs rather than stays stalled on a run just heard.
#[sqlx::test]
async fn a_checkpoint_beside_the_tick_stalling_its_run_leaves_the_monitor_running(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    quiet(&pool, &p.project, &run, 180).await;

    // Every write to the runs waits on the holder, so the checkpoint, its
    // run read, waits to hear it while the tick stalls the monitor.
    let mut holder = pool.begin().await.expect("the holder's transaction");
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .expect("the holder's session");
    sqlx::query("LOCK TABLE telemetry.open_runs IN SHARE MODE")
        .execute(&mut *holder)
        .await
        .expect("hold the runs");
    let path = format!("/{key}/hourly-sync/checkpoint?rid={run}&steps=1");
    let ((), meanwhile) = tokio::join!(p.ping_ok(&path), async {
        waits_on(&pool, holder_pid, "the checkpoint waits to hear its run").await;
        let meanwhile = p.tick().await;
        assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");
        holder.commit().await.expect("the holder lets go");
        meanwhile
    });
    assert_eq!(meanwhile, ticked(1, 0));
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "running", "{settled}");
    assert_eq!(p.tick().await, ticked(0, 0));
}

/// A visit ends a hundred runs past their ceiling at most, so a ping or a
/// delete waiting on it is bounded; the next tick finds the monitor again
/// for the rest.
#[sqlx::test]
async fn a_visit_ends_a_hundred_runs_at_most(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    p.make(hourly("hourly-sync")).await;
    // Written straight into the table, as a day's `/start`s with no end
    // would leave them: a visit's worth and one more, every one past its
    // ceiling.
    let written = sqlx::query(
        "INSERT INTO telemetry.open_runs
             (id, project_id, organization_id, monitor_id, agent, source, ends_by)
         SELECT gen_random_uuid(), $1, $2, m.id, m.agent, 'ping', now() - interval '1 minute'
           FROM telemetry.monitors m, generate_series(1, 101)
          WHERE m.project_id = $1 AND m.slug = 'hourly-sync'",
    )
    .bind(&p.project)
    .bind(&p.organization)
    .execute(&pool)
    .await
    .expect("write the runs")
    .rows_affected();
    assert_eq!(written, 101);
    assert_eq!(p.tick().await, ticked(1, 100));
    assert_eq!(open_runs(&pool, &p.project).await.len(), 1);
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 100);
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    let again = p.tick().await;
    assert_eq!((again.ended, again.left), (1, 0), "{again:?}");
    assert!(open_runs(&pool, &p.project).await.is_empty());
    assert_eq!(p.tick().await, ticked(0, 0));
}

/// A failed monitor with a run still quiet stays failed while another run
/// goes, as the ticker leaves it, and a run's end decides anew: ended well
/// with the quiet run still there, it is stalled; the quiet run heard, it
/// runs.
#[sqlx::test]
async fn a_failed_monitor_with_a_quiet_run_stays_failed_while_another_goes(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly("hourly-sync")).await;
    let (quiet_one, live_one) = (rid(), rid());
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={quiet_one}"))
        .await;
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={live_one}"))
        .await;
    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={}", rid()))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    quiet(&pool, &p.project, &quiet_one, 180).await;
    assert_eq!(p.tick().await, ticked(0, 0), "failed, the graver, stays");

    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={live_one}&steps=1"
    ))
    .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    assert_eq!(p.tick().await, ticked(0, 0));
    p.ping_ok(&format!("/{key}/hourly-sync?rid={live_one}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "stalled");
    assert_eq!(p.tick().await, ticked(0, 0));
    p.ping_ok(&format!("/{key}/hourly-sync/checkpoint?rid={quiet_one}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    assert_eq!(p.tick().await, ticked(0, 0));

    // Quiet again before any tick, its end ping's verdict decides: failed by
    // a `/fail`, and a bare end ping ending it well leaves nothing quiet.
    quiet(&pool, &p.project, &quiet_one, 180).await;
    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={quiet_one}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    let another = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={another}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    quiet(&pool, &p.project, &another, 180).await;
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    assert!(open_runs(&pool, &p.project).await.is_empty());
    assert_eq!(p.tick().await, ticked(0, 0));
}
