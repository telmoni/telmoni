//! The ping lane as a crontab reaches it: a ping to a slug no monitor has
//! makes the monitor, paused as `new`, recorded on the chain as made by the
//! key, and the owner told once through the outbox; a run is opened by its
//! `/start`, heard by its `/checkpoint`s, which hold it to its monitor's
//! step cap, and ended by a bare ping or a `/fail`, when it waits in the
//! outbox as a finished span and the monitor's state and next due time
//! follow, or as `error` by the next ping that finds it never said it ended;
//! a ping whose run ends beside it changing nothing; a ping waiting on no
//! other writer in a circle; the project held in place while a ping writes,
//! so a move waits for the pings in flight, keeping new ones out meanwhile,
//! and takes their rows, and a ping a move keeps out holds no connection
//! while it waits, files its rows where the project went once let in, its
//! new organization's standing asked with nothing held, or gives up as on
//! the chain lock; the run a ping names by `rid`; every run's ceiling, and
//! the stall timeout a discovered
//! monitor is given; a monitor whose saved timing cannot be read refusing
//! none of its pings; a body is dropped and said so, the query held to
//! numbers, the methods and paths to the lane's, and no answer one a cache
//! may keep; past the
//! plan's limit, or past the monitors nobody has set up that a project may
//! hold, nothing is made and the ping is refused with the bound's words; a
//! paused organization takes no ping; a finished run reaches ClickHouse
//! through the outbox; and the purges and a move take the keys, runs and
//! outbox rows with the project.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

use common::{Answer, AuthStub, Ceiling, NoticeLog, PausedOrganization, SECRET, StandingAsked};
use telmoni_shared::audit::lock_chain;
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits, Notice, Notifications};
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{
    NotificationKind, OrganizationId, OrganizationRole, OrganizationStatus, ProjectId,
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

/// A project as `project` makes one, with auth kept to say what a test needs
/// said of where the project is and how its organization stands.
fn project_and_auth(pool: &PgPool) -> (Project, Arc<AuthStub>) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::probing(pool);
    auth.seat(
        OWNER,
        &organization,
        &project,
        OrganizationRole::Owner,
        None,
    );
    let notices = NoticeLog::new();
    let state = common::state_with_notices(pool, auth.clone(), Arc::new(NoLimits), notices.clone());
    (Project::over(state, notices, organization, project), auth)
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
    /// its media type and its body — every answer, whatever it says, held
    /// to saying that no cache may keep it.
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
        assert_eq!(
            cache_control(&response),
            Some("no-store"),
            "{method} {path}: an answer a cache may keep"
        );
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

    /// A `GET` ping sent while its run is ended beside it: the project's one
    /// open run deleted, as an end ping deletes its row, by a session of
    /// `pool`'s that commits once the ping has read the run open and waits
    /// on its row to write it.
    async fn ping_as_its_run_ends(&self, pool: &PgPool, path: &str) -> (StatusCode, String) {
        let mut end = pool.begin().await.expect("the end's transaction");
        let end_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *end)
            .await
            .expect("the end's session");
        let ended = sqlx::query("DELETE FROM telemetry.open_runs WHERE project_id = $1")
            .bind(&self.project)
            .execute(&mut *end)
            .await
            .expect("end the open run")
            .rows_affected();
        assert_eq!(ended, 1, "one run open, to end beside the ping");
        let (answer, ()) = tokio::join!(self.ping(path), async {
            common::eventually("the ping waits on its run's row", move || async move {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                                     WHERE $1 = ANY (pg_blocking_pids(pid)))",
                )
                .bind(end_pid)
                .fetch_one(pool)
                .await
                .expect("ask who waits on the end")
            })
            .await;
            end.commit().await.expect("the end commits");
        });
        answer
    }

    /// A `GET` ping sent while another writer, a session of `pool`'s, takes
    /// `order`'s locks: the first before the ping is sent, the rest once the
    /// ping waits on it, then its commit. A ping holding a lock the writer
    /// wants next would wait on it in a circle, which Postgres breaks by
    /// aborting the side that waited first: the ping, a 500.
    async fn ping_as_locks_are_taken(
        &self,
        pool: &PgPool,
        path: &str,
        order: &[Lock],
    ) -> (StatusCode, String) {
        let (first, rest) = order.split_first().expect("a lock to hold first");
        let mut writer = pool.begin().await.expect("the writer's transaction");
        let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *writer)
            .await
            .expect("the writer's session");
        self.take(&mut writer, *first).await;
        let (answer, ()) = tokio::join!(self.ping(path), async {
            common::eventually("the ping waits on the writer", move || async move {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                                     WHERE $1 = ANY (pg_blocking_pids(pid)))",
                )
                .bind(writer_pid)
                .fetch_one(pool)
                .await
                .expect("ask who waits on the writer")
            })
            .await;
            for lock in rest {
                self.take(&mut writer, *lock).await;
            }
            writer.commit().await.expect("the writer commits");
        });
        answer
    }

    /// A `GET` ping sent while the project moves to `to`: a writer, a session
    /// of `pool`'s, takes `held` before the ping is sent; once the ping waits
    /// on it, a move as auth and telemetry make one — the project's move lock
    /// alone, then the organization's chain lock, as an accept takes them,
    /// then every row the project has handed to `to` — is started on a
    /// transaction opened before the ping, held to waiting on the ping alone,
    /// and then the writer commits. The ping's answer, once the move is done
    /// too.
    async fn ping_as_the_project_moves(
        &self,
        pool: &PgPool,
        path: &str,
        held: Lock,
        to: &OrganizationId,
    ) -> (StatusCode, String) {
        let mut writer = pool.begin().await.expect("the writer's transaction");
        let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *writer)
            .await
            .expect("the writer's session");
        self.take(&mut writer, held).await;
        let mut mover = maintenance_scope(&self.state.db, TelemetryLane)
            .await
            .expect("the lane");
        let (answer, ()) = tokio::join!(self.ping(path), async {
            let ping_pid = waiting_on(pool, writer_pid, "the ping waits on the writer").await;
            let ((), ()) = tokio::join!(
                async {
                    project_move::lock_for_move(mover.conn(), &self.project)
                        .await
                        .expect("the move lock");
                    lock_chain(mover.conn(), &self.organization)
                        .await
                        .expect("the chain lock");
                    db::move_project(&mut mover, &self.project, to)
                        .await
                        .expect("moved");
                    mover.commit().await.expect("the move commits");
                },
                async {
                    let mover_pid = waiting_on(pool, ping_pid, "the move waits on the ping").await;
                    let blocking: Vec<i32> = sqlx::query_scalar("SELECT pg_blocking_pids($1)")
                        .bind(mover_pid)
                        .fetch_one(pool)
                        .await
                        .expect("ask what the move waits on");
                    assert_eq!(blocking, [ping_pid], "the move waits on the ping alone");
                    writer.commit().await.expect("the writer commits");
                },
            );
        });
        answer
    }

    /// A `GET` ping whose key is resolved before the project moves to `to`
    /// and whose transaction comes after: a move as auth and telemetry make
    /// one — the project's move lock alone, then the organization's chain
    /// lock, as an accept takes them, then every row the project has handed
    /// to `to` — holds the project from before the ping is sent until the
    /// ping has resolved its key, touching it, and has been kept out a while;
    /// the ping answered only once the move commits.
    async fn ping_resolved_before_a_move(
        &self,
        pool: &PgPool,
        path: &str,
        to: &OrganizationId,
    ) -> (StatusCode, String) {
        untouched(pool, &self.project).await;
        let mut mover = maintenance_scope(&self.state.db, TelemetryLane)
            .await
            .expect("the lane");
        project_move::lock_for_move(mover.conn(), &self.project)
            .await
            .expect("the move lock");
        lock_chain(mover.conn(), &self.organization)
            .await
            .expect("the chain lock");
        let project = &self.project;
        let committing = AtomicBool::new(false);
        let kept_out = async {
            let answer = self.ping(path).await;
            assert!(
                committing.load(Ordering::SeqCst),
                "the ping was let in before the move committed"
            );
            answer
        };
        let (answer, ()) = tokio::join!(kept_out, async {
            common::eventually("the ping resolves its key", move || async move {
                sqlx::query_scalar::<_, bool>(
                    "SELECT last_ping_at IS NOT NULL FROM telemetry.ping_keys
                      WHERE project_id = $1 AND retired_at IS NULL",
                )
                .bind(project)
                .fetch_one(pool)
                .await
                .expect("read the live key")
            })
            .await;
            // Long enough for the ping to have tried the project and let go:
            // the move is to commit while the ping is kept out.
            tokio::time::sleep(Duration::from_millis(300)).await;
            db::move_project(&mut mover, project, to)
                .await
                .expect("moved");
            committing.store(true, Ordering::SeqCst);
            mover.commit().await.expect("the move commits");
        });
        answer
    }

    /// One of the writer's locks: the organization's chain lock, or every row
    /// the project has in a table — rewritten to the organization each names
    /// already, or deleted as a purge deletes them.
    async fn take(&self, writer: &mut sqlx::PgConnection, lock: Lock) {
        let (table, taken) = match lock {
            Lock::Chain => {
                lock_chain(writer, &self.organization)
                    .await
                    .expect("the chain lock");
                return;
            }
            Lock::Rows(table) => (
                table,
                sqlx::query(&format!(
                    "UPDATE telemetry.{table} SET organization_id = organization_id
                      WHERE project_id = $1"
                ))
                .bind(&self.project)
                .execute(writer)
                .await,
            ),
            Lock::Purged(table) => (
                table,
                sqlx::query(&format!(
                    "DELETE FROM telemetry.{table} WHERE project_id = $1"
                ))
                .bind(&self.project)
                .execute(writer)
                .await,
            ),
            Lock::Retired => (
                "ping_keys",
                sqlx::query(
                    "UPDATE telemetry.ping_keys SET retired_at = now()
                      WHERE project_id = $1 AND retired_at IS NULL",
                )
                .bind(&self.project)
                .execute(writer)
                .await,
            ),
        };
        let taken = taken.expect("the project's rows").rows_affected();
        assert!(taken > 0, "the project has no row in {table} to take");
    }
}

/// A lock a writer beside a ping takes: the organization's chain lock; the
/// project's rows in one table, rewritten in place or purged; or its live
/// keys, retired.
#[derive(Debug, Clone, Copy)]
enum Lock {
    Chain,
    Rows(&'static str),
    Purged(&'static str),
    Retired,
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

/// The organizations a project's rows in a table are filed under.
async fn filed_under(pool: &PgPool, table: &str, project: &ProjectId) -> Vec<String> {
    sqlx::query_scalar(&format!(
        "SELECT organization_id FROM telemetry.{table} WHERE project_id = $1"
    ))
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read where the project's rows are filed")
}

/// The one session found waiting on `pid`'s locks, polled for as `what`.
async fn waiting_on(pool: &PgPool, pid: i32, what: &str) -> i32 {
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
    let waiting: Vec<i32> = sqlx::query_scalar(
        "SELECT pid FROM pg_stat_activity WHERE $1 = ANY (pg_blocking_pids(pid))",
    )
    .bind(pid)
    .fetch_all(pool)
    .await
    .expect("the sessions waiting");
    assert_eq!(waiting.len(), 1, "{what}: {waiting:?} wait");
    waiting.first().copied().expect("the session waiting")
}

/// Whether every row the project has in `table` is filed under
/// `organization`, and it has one at least.
async fn all_filed_under(
    pool: &PgPool,
    table: &str,
    project: &ProjectId,
    organization: &OrganizationId,
) -> bool {
    let filed = filed_under(pool, table, project).await;
    !filed.is_empty() && filed.iter().all(|o| o == organization.as_str())
}

/// The project's live key as if no ping had come under it in the last
/// minute, so the next ping touches its row.
async fn untouched(pool: &PgPool, project: &ProjectId) {
    let keys = sqlx::query(
        "UPDATE telemetry.ping_keys SET last_ping_at = NULL
          WHERE project_id = $1 AND retired_at IS NULL",
    )
    .bind(project)
    .execute(pool)
    .await
    .expect("forget the key's last ping")
    .rows_affected();
    assert_eq!(keys, 1, "the project's one live key");
}

/// A run id a ping names.
fn rid() -> String {
    Uuid::new_v4().to_string()
}

/// What an answer tells a cache on the way, where it says anything.
fn cache_control(response: &axum::response::Response) -> Option<&str> {
    response
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
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

/// A ping that read its run open and finds it ended beside it when it
/// writes — the end committed while the ping waited on the run's row —
/// changes nothing: a `/checkpoint`, naming its run or meaning the newest,
/// and a `/start` sent again are answered `OK`, open no run, hand nothing
/// over and leave the monitor as it stood, where a ping that took its run
/// for heard would set it running. The write is held to the monitor the
/// run was read under, so a namesake another monitor opened under the id
/// in that gap is not taken for it.
#[sqlx::test]
async fn a_ping_whose_run_ends_beside_it_changes_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;

    let (named, retried) = (rid(), rid());
    for (start, racing) in [
        (
            format!("/{key}/hourly-sync/start?rid={named}"),
            format!("/{key}/hourly-sync/checkpoint?rid={named}&steps=2"),
        ),
        (
            format!("/{key}/hourly-sync/start"),
            format!("/{key}/hourly-sync/checkpoint?steps=2"),
        ),
        (
            format!("/{key}/hourly-sync/start?rid={retried}"),
            format!("/{key}/hourly-sync/start?rid={retried}"),
        ),
    ] {
        p.ping_ok(&start).await;
        // Another run's end fails the monitor, so a ping that took its run
        // for heard would set it running again.
        p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={}", rid()))
            .await;
        assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
        let (status, body) = p.ping_as_its_run_ends(&pool, &racing).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"), "{racing}");
        assert!(
            open_runs(&pool, &p.project).await.is_empty(),
            "{racing}: the run was opened again"
        );
        assert_eq!(
            p.monitor("hourly-sync").await["state"],
            "failed",
            "{racing}"
        );
    }
    assert_eq!(
        finished_runs(&pool, &p.project).await.len(),
        3,
        "the `/fail`s' runs alone: nothing a ping found ended beside it was handed over"
    );

    // The gap between the read and the write cannot be held open from
    // outside, so the namesake is put to the write itself.
    let namesake = rid();
    p.ping_ok(&format!("/{key}/other-sync/start?rid={namesake}"))
        .await;
    let id_of = |monitor: Value| {
        Uuid::parse_str(monitor["id"].as_str().expect("an id")).expect("a monitor's id")
    };
    let hourly = id_of(p.monitor("hourly-sync").await);
    let other = id_of(p.monitor("other-sync").await);
    let namesake = Uuid::parse_str(&namesake).expect("a run's id");
    let mut tx = telmoni_shared::db::tenant_session::project_scope(&pool, &p.project)
        .await
        .expect("a scope");
    let heard = db::hear_open_run(
        &mut tx,
        &p.project,
        namesake,
        hourly,
        Utc::now(),
        None,
        None,
    )
    .await
    .expect("a write");
    assert!(
        heard.is_none(),
        "another monitor's run was heard as this one's"
    );
    let heard = db::hear_open_run(&mut tx, &p.project, namesake, other, Utc::now(), None, None)
        .await
        .expect("a write");
    assert!(
        heard.is_some(),
        "the run under its own monitor was not heard"
    );
    tx.rollback().await.expect("rolled back");
}

/// A ping takes no lock on its key's row in its own transaction — the key is
/// touched where it is resolved, in a statement that holds nothing else — so
/// no writer that takes that row beside the project's other rows waits on a
/// ping in a circle over it. Each writer here is one of the project's: a
/// retirement under the chain lock, a settling's move with the settings row,
/// a transfer's move with the runs and the monitors, a purge with the
/// monitors. The moves here take their rows without the project's move lock,
/// which would keep the ping out of their way altogether, so that the ping
/// meets them row by row. Wherever in its own transaction a ping touched its
/// key — first, once its monitor is found, once its run is written, once its
/// monitor is — one of them would wait on it in a circle until Postgres
/// ended the ping, which waited first: a 500.
#[sqlx::test]
async fn a_ping_waits_on_no_writer_in_a_circle(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;

    // A first ping waits for the chain lock a retirement holds, and the
    // retirement takes the key's row beside it: the ping finds its key
    // retired, and makes nothing.
    let (status, body) = p
        .ping_as_locks_are_taken(
            &pool,
            &format!("/{key}/nightly-report"),
            &[Lock::Chain, Lock::Retired],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the chain lock, then the key: {body}"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
    let key = p.mint().await;

    // A first ping waits on its key's row, which a settling's move holds with
    // no chain lock, and the move takes the settings row beside it.
    untouched(&pool, &p.project).await;
    let (status, body) = p
        .ping_as_locks_are_taken(
            &pool,
            &format!("/{key}/weekly-report"),
            &[Lock::Rows("ping_keys"), Lock::Rows("project_settings")],
        )
        .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "OK"),
        "the keys, then the settings row"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, 1);

    // An end ping waits on its run's row, which a move holds, and the move
    // takes the key's row and the monitor's beside it.
    p.make(hourly()).await;
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    untouched(&pool, &p.project).await;
    let (status, body) = p
        .ping_as_locks_are_taken(
            &pool,
            &format!("/{key}/hourly-sync"),
            &[
                Lock::Rows("open_runs"),
                Lock::Rows("ping_keys"),
                Lock::Rows("monitors"),
            ],
        )
        .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "OK"),
        "the runs, then the keys, then the monitors"
    );
    assert!(open_runs(&pool, &p.project).await.is_empty());
    let ended = p.monitor("hourly-sync").await;
    assert_eq!(ended["state"], "healthy", "{ended}");

    // With no run open, a ping that moves its monitor's due time waits on
    // its key's row, which a move holds, and the move takes the monitor's
    // beside it.
    untouched(&pool, &p.project).await;
    let (status, body) = p
        .ping_as_locks_are_taken(
            &pool,
            &format!("/{key}/hourly-sync"),
            &[Lock::Rows("ping_keys"), Lock::Rows("monitors")],
        )
        .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "OK"),
        "the keys, then the monitors"
    );
    assert_ne!(
        p.monitor("hourly-sync").await["next_due_at"],
        ended["next_due_at"],
        "the ping wrote its monitor"
    );
    let touched: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_ping_at FROM telemetry.ping_keys
          WHERE project_id = $1 AND retired_at IS NULL",
    )
    .bind(&p.project)
    .fetch_one(&pool)
    .await
    .expect("read the live key");
    assert!(touched.is_some(), "the ping touched its key");

    // A `/start` waits on its key's row, which a purge holds, and the purge
    // takes the monitors beside it — whose row the `/start` would hold a
    // share of, had it opened its run before it touched its key: the ping
    // finds its key gone, and writes nothing into the project.
    untouched(&pool, &p.project).await;
    let (status, body) = p
        .ping_as_locks_are_taken(
            &pool,
            &format!("/{key}/hourly-sync/start"),
            &[Lock::Purged("ping_keys"), Lock::Purged("monitors")],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the keys, then the monitors, purged: {body}"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
    assert!(open_runs(&pool, &p.project).await.is_empty());
}

/// A ping a move kept out files its rows where the project went: the move
/// holds the project's move lock alone, taken before any chain lock as a
/// transfer takes it, and the ping reads its key's organization only once it
/// holds the project in place. So a ping resolved before the move and let in
/// after it is neither refused nor filed under the organization that gave
/// the project up, whose purge would take its rows and the new one's never
/// find them — and it is held to the new organization's standing, asked with
/// nothing held, so no move of the project waits on the answer.
#[sqlx::test]
async fn a_ping_a_move_kept_out_is_filed_where_the_project_went(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let elsewhere = OrganizationId::new();

    // A `/start` to a monitor its first ping made opens its run there.
    let p = project(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    let (status, body) = p
        .ping_resolved_before_a_move(&pool, &format!("/{key}/nightly-report/start"), &elsewhere)
        .await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(
        filed_under(&pool, "open_runs", &p.project).await,
        [elsewhere.to_string()],
        "the run filed under the organization that gave the project up"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, 1);

    // A first ping makes its monitor there, records it on that organization's
    // chain, and tells its owner there.
    let q = project(&pool);
    let key = q.mint().await;
    let (status, body) = q
        .ping_resolved_before_a_move(&pool, &format!("/{key}/nightly-report"), &elsewhere)
        .await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"));
    for table in ["monitors", "outbox", "project_settings"] {
        assert!(
            all_filed_under(&pool, table, &q.project, &elsewhere).await,
            "{table}: {:?}",
            filed_under(&pool, table, &q.project).await
        );
    }
    assert_eq!(recorded(&pool, &elsewhere).await.len(), 1);
    assert!(recorded(&pool, &q.organization).await.is_empty());

    // Moved into a paused organization: a 402, and nothing written.
    let paused = OrganizationId::new();
    let r = project_under(
        &pool,
        OrganizationId::new(),
        Arc::new(PausedOrganization(paused.clone())),
    );
    let key = r.mint().await;
    let (status, body) = r
        .ping_resolved_before_a_move(&pool, &format!("/{key}/nightly-report"), &paused)
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(monitors_of(&pool, &r.project).await, 0);
    assert!(outbox_of(&pool, &r.project).await.is_empty());

    // The new organization's standing asked once, with nothing held.
    let next = OrganizationId::new();
    let standing = StandingAsked::of(&pool, &next);
    let s = project_under(&pool, OrganizationId::new(), standing.clone());
    let key = s.mint().await;
    let (status, body) = s
        .ping_resolved_before_a_move(&pool, &format!("/{key}/nightly-report"), &next)
        .await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(
        standing.held(),
        [0],
        "the standing asked with an advisory lock held"
    );
    assert!(all_filed_under(&pool, "monitors", &s.project, &next).await);
}

/// A move waits for the pings in flight and takes their rows with the rest:
/// a ping holds its project in place from the start of its transaction to
/// its commit, so the move is held until the ping has written — never let
/// past a run the ping had not committed, which would stay filed under the
/// organization that gave the project up, nor commit a row the ping is about
/// to update, which the row policy would refuse the ping, a 500. A first
/// ping waiting for its organization's chain lock holds the move back the
/// same way, in no circle: a move takes the project's lock before any chain
/// lock.
#[sqlx::test]
async fn a_move_waits_for_the_pings_in_flight_and_takes_their_rows(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let elsewhere = OrganizationId::new();

    // A `/start` writes its run, then waits on its monitor's row to settle it.
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    let (status, body) = p
        .ping_as_the_project_moves(
            &pool,
            &format!("/{key}/hourly-sync/start"),
            Lock::Rows("monitors"),
            &elsewhere,
        )
        .await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"));
    for table in ["open_runs", "monitors", "ping_keys", "project_settings"] {
        assert!(
            all_filed_under(&pool, table, &p.project, &elsewhere).await,
            "{table}: {:?}",
            filed_under(&pool, table, &p.project).await
        );
    }

    // A first ping waits for its organization's chain lock.
    let q = project(&pool);
    let key = q.mint().await;
    let (status, body) = q
        .ping_as_the_project_moves(
            &pool,
            &format!("/{key}/nightly-report"),
            Lock::Chain,
            &elsewhere,
        )
        .await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"));
    for table in ["monitors", "outbox", "ping_keys", "project_settings"] {
        assert!(
            all_filed_under(&pool, table, &q.project, &elsewhere).await,
            "{table}: {:?}",
            filed_under(&pool, table, &q.project).await
        );
    }
}

/// A ping waits out a move no longer than it waits for the chain lock: past
/// the two seconds, a 409 that says to try again, and nothing written. Pings
/// holding the same project in place never keep each other out.
#[sqlx::test]
async fn a_ping_waits_on_a_move_no_longer_than_on_the_chain_lock(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    // Held in place by another ping until this one is answered, so a ping it
    // kept out could end only in the 409 below.
    let mut beside = pool.begin().await.expect("another ping's transaction");
    assert!(
        project_move::try_hold_in_place(&mut beside, &p.project)
            .await
            .expect("try the project")
    );
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    beside.rollback().await.expect("the other ping ends");

    let mut moving = pool.begin().await.expect("a transaction to hold the lock");
    project_move::lock_for_move(&mut moving, &p.project)
        .await
        .expect("the move lock");
    let sent = std::time::Instant::now();
    let (status, body) = tokio::time::timeout(
        Duration::from_secs(10),
        p.ping(&format!("/{key}/nightly-report")),
    )
    .await
    .expect("the ping gave up on the move");
    // The two seconds are counted once the key is resolved, after `sent`;
    // the ceiling leaves room for a slow machine and none for a longer wait.
    let waited = sent.elapsed();
    assert!(
        (Duration::from_secs(2)..Duration::from_secs(4)).contains(&waited),
        "{waited:?}"
    );
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.contains("the project is moving to another organization"),
        "{body}"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
    moving.rollback().await.expect("let the lock go");
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    assert_eq!(monitors_of(&pool, &p.project).await, 2);
}

/// Pings a move keeps out hold no connection while they wait: of more of
/// them than the module's pool holds, sent while a move holds the project,
/// none ever waits on the lock, a wait that would hold its connection, and
/// the move's own work — telemetry's half of a transfer, on the same pool,
/// while auth holds the lock — gets the connection it needs. None is let in
/// before the move lets go, and each is answered once it does, filed where
/// the project went.
#[sqlx::test]
async fn pings_a_move_keeps_out_hold_no_connection(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let elsewhere = OrganizationId::new();
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    let mut moving = pool.begin().await.expect("a transaction to hold the lock");
    project_move::lock_for_move(&mut moving, &p.project)
        .await
        .expect("the move lock");
    let mut kept_out = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (lane, path) = (p.pings.clone(), format!("/{key}/hourly-sync"));
        kept_out.spawn(async move {
            lane.oneshot(Request::get(path).body(Body::empty()).expect("a request"))
                .await
                .expect("the lane answers")
                .status()
        });
    }
    // Looked at while the pings are kept out, over and over, and let go well
    // inside their two seconds.
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks
              WHERE locktype = 'advisory' AND NOT granted
                AND database = (SELECT oid FROM pg_database
                                 WHERE datname = current_database())",
        )
        .fetch_one(&pool)
        .await
        .expect("count the waits on an advisory lock");
        assert_eq!(waiting, 0, "a ping kept out waits on the lock");
    }
    let moved = tokio::time::timeout(Duration::from_secs(1), async {
        let mut tx = maintenance_scope(&p.state.db, TelemetryLane).await?;
        db::move_project(&mut tx, &p.project, &elsewhere).await?;
        tx.commit().await
    })
    .await;
    assert!(
        moved.is_ok_and(|done| done.is_ok()),
        "the move's own work was kept from the pool"
    );
    assert!(
        finished_runs(&pool, &p.project).await.is_empty(),
        "a ping let in while the move held the project"
    );
    moving.rollback().await.expect("let the lock go");
    while let Some(answered) = kept_out.join_next().await {
        assert_eq!(answered.expect("a ping"), StatusCode::OK);
    }
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 8);
    assert!(all_filed_under(&pool, "outbox", &p.project, &elsewhere).await);
}

/// A move waiting on the pings in flight keeps new ones out until it is done,
/// rather than let them in ahead of it, which a steady stream of pings could
/// do until the move gave up: a ping sent while a move waits on another is
/// let in only once the move commits, and files its run where the project
/// went.
#[sqlx::test]
async fn a_move_waiting_on_the_pings_in_flight_keeps_new_ones_out(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let elsewhere = OrganizationId::new();
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    let mut in_flight = pool.begin().await.expect("a ping's transaction");
    let in_flight_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *in_flight)
        .await
        .expect("the ping's session");
    assert!(
        project_move::try_hold_in_place(&mut in_flight, &p.project)
            .await
            .expect("try the project")
    );
    let mut mover = maintenance_scope(&p.state.db, TelemetryLane)
        .await
        .expect("the lane");
    let committing = AtomicBool::new(false);
    let ((), (status, body)) = tokio::join!(
        async {
            project_move::lock_for_move(mover.conn(), &p.project)
                .await
                .expect("the move lock");
            db::move_project(&mut mover, &p.project, &elsewhere)
                .await
                .expect("moved");
            committing.store(true, Ordering::SeqCst);
            mover.commit().await.expect("the move commits");
        },
        async {
            waiting_on(&pool, in_flight_pid, "the move waits on the ping in flight").await;
            let path = format!("/{key}/hourly-sync");
            let (answer, ()) = tokio::join!(p.ping(&path), async {
                // Long enough for the new ping to have been kept out.
                tokio::time::sleep(Duration::from_millis(300)).await;
                in_flight.rollback().await.expect("the ping in flight ends");
            });
            assert!(
                committing.load(Ordering::SeqCst),
                "the new ping was let in ahead of the waiting move"
            );
            answer
        },
    );
    assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"));
    assert!(all_filed_under(&pool, "outbox", &p.project, &elsewhere).await);
}

/// A `rid` is a run's own name within its project: a script that derives
/// one from its own name — `uuidgen --sha1` on the job's name and the day,
/// the stable UUID the manual gives — opens its run beside another tenant's
/// of the same id, each heard and ended alone and neither told of the other.
/// Within a project the id is one monitor's run: a `/start` naming it under
/// another monitor is refused.
#[sqlx::test]
async fn a_rid_is_a_runs_own_name_within_its_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let a = project(&pool);
    let b = project(&pool);
    a.make(hourly()).await;
    b.make(hourly()).await;
    let a_key = a.mint().await;
    let b_key = b.mint().await;
    let shared = rid();

    // Two tenants open a run by the same id, each in its own project.
    a.ping_ok(&format!("/{a_key}/hourly-sync/start?rid={shared}"))
        .await;
    b.ping_ok(&format!("/{b_key}/hourly-sync/start?rid={shared}"))
        .await;
    for p in [&a, &b] {
        let runs = open_runs(&pool, &p.project).await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0.to_string(), shared);
    }

    // Heard in the project its ping came under alone.
    b.ping_ok(&format!(
        "/{b_key}/hourly-sync/checkpoint?rid={shared}&steps=2"
    ))
    .await;
    assert_eq!(open_runs(&pool, &a.project).await[0].1, 0);
    assert_eq!(open_runs(&pool, &b.project).await[0].1, 2);

    // Ended in its own project alone, as the run it was there.
    a.ping_ok(&format!("/{a_key}/hourly-sync?rid={shared}"))
        .await;
    assert!(open_runs(&pool, &a.project).await.is_empty());
    assert_eq!(open_runs(&pool, &b.project).await.len(), 1);
    let finished = finished_runs(&pool, &a.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["run_id"].as_str(), Some(shared.as_str()));
    assert_eq!(finished[0]["status"], "ok");
    assert!(finished_runs(&pool, &b.project).await.is_empty());
    assert_eq!(a.monitor("hourly-sync").await["state"], "healthy");
    assert_eq!(b.monitor("hourly-sync").await["state"], "running");

    // Within the project the id is the monitor's run: under another
    // monitor it is refused, and the run stands as it was.
    let (status, body) = b
        .ping(&format!("/{b_key}/nightly-report/start?rid={shared}"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("under another monitor"), "{body}");
    let runs = open_runs(&pool, &b.project).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].1, 2);
    b.ping_ok(&format!(
        "/{b_key}/hourly-sync/fail?rid={shared}&exit_code=1"
    ))
    .await;
    assert!(open_runs(&pool, &b.project).await.is_empty());
    let finished = finished_runs(&pool, &b.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["status"], "error");
    assert_eq!(finished[0]["steps"], 2);
    assert_eq!(finished[0]["exit_code"], 1);
    assert_eq!(b.monitor("hourly-sync").await["state"], "failed");
}

/// A monitor whose timing was read as it was saved and cannot be read now —
/// as only a change of ours leaves one: a zone the database no longer
/// names, a line a stricter `croner` refuses, one whose search no longer
/// ends — refuses none of its pings. Each run is ended and handed to the
/// outbox, the monitor's state follows it, and it is due at no time rather
/// than at one already passed; saved again, the timing is read as it is
/// sent, and the monitor is due again once it can be.
#[sqlx::test]
async fn a_monitor_whose_saved_timing_cannot_be_read_refuses_no_ping(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    for (schedule, interval_s, timezone, end, state) in [
        (None, Some(3600), "Mars/Olympus", "", "healthy"),
        (
            Some("every day at nine"),
            None,
            "UTC",
            "/fail?exit_code=2",
            "failed",
        ),
        (Some("0 0 30 2 *"), None, "UTC", "", "healthy"),
    ] {
        let unreadable = format!("{schedule:?} {interval_s:?} {timezone}");
        // Written straight into the row, due an hour on, as a change of ours
        // would leave it.
        let written = sqlx::query(
            "UPDATE telemetry.monitors
                SET schedule = $2, interval_s = $3, timezone = $4,
                    next_due_at = now() + interval '1 hour'
              WHERE project_id = $1 AND slug = 'hourly-sync'",
        )
        .bind(&p.project)
        .bind(schedule)
        .bind(interval_s)
        .bind(timezone)
        .execute(&pool)
        .await
        .expect("write the monitor's timing")
        .rows_affected();
        assert_eq!(written, 1, "{unreadable}");
        p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
        p.ping_ok(&format!("/{key}/hourly-sync{end}")).await;
        assert!(
            open_runs(&pool, &p.project).await.is_empty(),
            "{unreadable}: the run's end was rolled back"
        );
        let monitor = p.monitor("hourly-sync").await;
        assert_eq!(monitor["state"], state, "{unreadable}");
        assert!(
            monitor["next_due_at"].is_null(),
            "{unreadable}: due at a time its timing could not give: {monitor}"
        );
    }
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(
        finished
            .iter()
            .map(|run| run["status"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["ok", "error", "ok"]
    );
    assert_eq!(finished[1]["exit_code"], 2);

    // Saved again, the timing is read and searched as it is sent, the line
    // that never comes refused though the row holds it, and the same timing
    // a readable one gives the monitor its due time back.
    let one = format!("{MONITORS}/hourly-sync");
    let (status, problem) = p
        .send(p.request(
            "PUT",
            &one,
            Some(json!({ "name": "Hourly sync", "schedule": "0 0 30 2 *", "grace_s": 60 })),
        ))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("never comes"),
        "{problem}"
    );
    let mended = sqlx::query(
        "UPDATE telemetry.monitors SET schedule = NULL, interval_s = 3600
          WHERE project_id = $1 AND slug = 'hourly-sync'",
    )
    .bind(&p.project)
    .execute(&pool)
    .await
    .expect("a readable timing, as a fix of ours would leave it")
    .rows_affected();
    assert_eq!(mended, 1, "the same timing the save sends, so it stands");
    let (status, saved) = p.send(p.request("PUT", &one, Some(hourly()))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert!(
        saved["next_due_at"].is_string(),
        "due again under the same timing: {saved}"
    );
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

/// No answer the lane gives is one a cache may keep: a `GET` records a run,
/// and a bare ping's URL is the same each run, so a cache on the way that
/// kept a `200` would answer the next run's ping in the lane's place. Each
/// kind of answer says so — an `OK` to `GET`, `HEAD` or `POST`, a refusal,
/// axum's own for a path that does not decode, a key the lane does not
/// hold, a path or a method it does not serve — and each `OK` was a ping the
/// lane heard, `HEAD`'s as `GET`'s.
#[sqlx::test]
async fn no_answer_of_the_lane_is_one_a_cache_may_keep(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let path = format!("/{key}/nightly-report");
    let unknown = format!("/{}/nightly-report", telmoni_shared::types::mint_ping_key());
    for (method, sent, status) in [
        ("GET", path.clone(), StatusCode::OK),
        ("HEAD", path.clone(), StatusCode::OK),
        ("POST", format!("{path}/start"), StatusCode::OK),
        ("GET", format!("{path}/fail?exit_code=1"), StatusCode::OK),
        (
            "GET",
            format!("{path}/checkpoint?steps=four"),
            StatusCode::BAD_REQUEST,
        ),
        ("GET", format!("/{key}/%FF"), StatusCode::BAD_REQUEST),
        ("GET", unknown, StatusCode::NOT_FOUND),
        ("GET", format!("{path}/stop"), StatusCode::NOT_FOUND),
        ("PUT", path.clone(), StatusCode::METHOD_NOT_ALLOWED),
    ] {
        let response = p
            .pings
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&sent)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("the lane answers");
        assert_eq!(response.status(), status, "{method} {sent}");
        assert_eq!(
            cache_control(&response),
            Some("no-store"),
            "{method} {sent}"
        );
    }
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(
        finished
            .iter()
            .map(|run| run["status"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["ok", "ok", "error"],
        "a ping of no duration to GET and to HEAD, and the run POST opened"
    );
}

/// Past the plan's limit a ping to a slug no monitor has is refused with the
/// ceiling's own 402 — what `curl -f` turns into cron's mail — naming what
/// lifts it and never the key; nothing is made or recorded, and no notice is
/// raised for it: the answer is the message, and the owner's digest is the
/// limiter's (roadmap 2.11).
#[sqlx::test]
async fn past_the_plans_limit_a_ping_is_refused_with_the_ceilings_words(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project_under(&pool, OrganizationId::new(), Ceiling::of(&pool, 1));
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

/// The row a ping's discovery writes on the chain records the request's id
/// only well formed: on the lane alone, as a suite mounts it, a caller's free
/// text is no id at all; behind the binary's layers the lane sees no id of a
/// caller's (`crates/telmoni/tests/health.rs`).
#[sqlx::test]
async fn a_discovery_records_no_free_text_for_its_request(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    for (slug, sent) in [
        ("nightly-report", "it's free text, of any length"),
        ("weekly-digest", "req-1"),
    ] {
        let answered = p
            .pings
            .clone()
            .oneshot(
                Request::get(format!("/{key}/{slug}"))
                    .header("x-request-id", sent)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("the lane answers");
        assert_eq!(answered.status(), StatusCode::OK, "{slug}");
    }
    let recorded: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT request_id FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'monitor'
          ORDER BY seq",
    )
    .bind(&p.organization)
    .fetch_all(&pool)
    .await
    .expect("read the chain");
    assert_eq!(recorded, [None, Some("req-1".to_owned())]);
}

/// The kinds of the rows waiting in the project's outbox, oldest first.
async fn kinds_waiting(pool: &PgPool, project: &ProjectId) -> Vec<String> {
    outbox_of(pool, project)
        .await
        .into_iter()
        .map(|(kind, _)| kind)
        .collect()
}

/// A notice goes nowhere once its organization's deletion is marked: the
/// deletion's purges take that organization's feeds, and a notice written
/// after them would outlive them, the monitor's slug and the key's prefix in
/// its body. The row is dropped as landed; the run's row beside it waits on
/// the store as before. Nor once the organization is gone, between auth
/// saying where the project is and saying how the organization stands.
#[sqlx::test]
async fn a_notice_for_an_organization_being_deleted_is_dropped(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    auth.stand(&p.organization, Some(OrganizationStatus::PendingDeletion));
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 },
        "the notice dropped as landed, the run's row handed back"
    );
    assert!(
        p.notices.notices().is_empty(),
        "a notice was written to a feed the deletion purges"
    );
    assert_eq!(kinds_waiting(&pool, &p.project).await, ["span"]);

    p.ping_ok(&format!("/{key}/weekly-digest")).await;
    auth.stand(&p.organization, None);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 2 }
    );
    assert!(
        p.notices.notices().is_empty(),
        "a notice was written for an organization that is gone"
    );
    assert_eq!(kinds_waiting(&pool, &p.project).await, ["span", "span"]);
}

/// A notice goes nowhere once auth has no such project — deleted, or its
/// organization's deletion done — and the project is purged from
/// notifications once more, which takes whatever an earlier try wrote after
/// the deletion's own purge and was cut off before it could ask again.
#[sqlx::test]
async fn a_notice_for_a_project_that_is_gone_is_dropped(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    // The earlier try: written, then cut off with its row still leased.
    let waiting = outbox_of(&pool, &p.project).await;
    let (_, queued) = waiting
        .iter()
        .find(|(kind, _)| kind == "notice")
        .expect("the notice's row");
    p.notices
        .emit(
            &p.organization,
            Some(&p.project),
            Notice {
                kind: NotificationKind::MonitorDiscovered,
                subject_user_id: None,
                title: queued["title"].as_str().expect("a title"),
                body: queued["body"].as_str().expect("a body"),
                metadata: queued["metadata"].clone(),
                dedup_key: queued["dedup_key"].as_str(),
            },
        )
        .await
        .expect("the earlier try's write");

    auth.forget(&p.project);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 },
        "the notice dropped as landed, the run's row handed back"
    );
    assert!(
        p.notices.notices().is_empty(),
        "a notice outlived a project that is gone"
    );
    assert_eq!(p.notices.purged(), std::slice::from_ref(&p.project));
    assert_eq!(kinds_waiting(&pool, &p.project).await, ["span"]);
}

/// A notice follows its project: a row leased before a transfer committed
/// names the organization the project left, and the notice is filed where
/// auth holds the project now, as that organization stands — the one it
/// left being deleted changes nothing — and nothing is purged.
#[sqlx::test]
async fn a_notice_follows_its_project_to_the_organization_holding_it_now(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    let other = OrganizationId::new();
    auth.move_to(&p.project, &other)
        .stand(&p.organization, Some(OrganizationStatus::PendingDeletion));
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 }
    );
    let notices = p.notices.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].organization_id, other);
    assert_eq!(notices[0].project_id.as_ref(), Some(&p.project));
    assert!(
        p.notices.purged().is_empty(),
        "a project that moved was purged"
    );
}

/// A transfer committing between the ask and the write leaves the notice
/// under the organization the project left, the residue `land_notice`
/// names, and purges nothing: the project is still there, and a purge would
/// take what its new organization holds of it.
#[sqlx::test]
async fn a_notice_whose_project_moves_as_it_is_written_purges_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    auth.move_after(&p.project, &OrganizationId::new(), 1);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 }
    );
    let notices = p.notices.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].organization_id, p.organization);
    assert!(
        p.notices.purged().is_empty(),
        "a project that moved was purged"
    );
}

/// Auth unable to say where a notice's project is, or how its organization
/// stands, is an outage of ours: the row is handed back with its try and
/// nothing written, the batch's other notices handed back unasked rather
/// than each waiting on auth in turn, and every notice lands once auth
/// answers.
#[sqlx::test]
async fn a_notice_auth_cannot_place_waits_with_its_try(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    p.ping_ok(&format!("/{key}/weekly-digest")).await;

    auth.failing_homes(true);
    let asked = auth.homes_asked();
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 0, held: 4 }
    );
    assert_eq!(
        auth.homes_asked() - asked,
        1,
        "auth asked again for the second notice"
    );
    assert_eq!(
        tries_of(&pool, &p.project).await,
        [(0, false); 4],
        "where the project is unanswered: every row handed back"
    );

    auth.failing_homes(false).failing_standing(true);
    let asked = auth.homes_asked();
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 0, held: 4 }
    );
    assert_eq!(
        auth.homes_asked() - asked,
        1,
        "auth asked again for the second notice"
    );
    assert_eq!(
        tries_of(&pool, &p.project).await,
        [(0, false); 4],
        "the organization's standing unanswered: every row handed back"
    );
    assert!(p.notices.notices().is_empty());

    auth.failing_standing(false);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 2, held: 2 }
    );
    assert_eq!(p.notices.notices().len(), 2);
    assert_eq!(kinds_waiting(&pool, &p.project).await, ["span", "span"]);
}

/// A project deleted as its notice is written — the deletion committing
/// after the loop asked where the project is, and notifications' purge of
/// it run before the notice lands — would leave the notice behind both:
/// asked again once the notice is written, auth has no such project, and the
/// loop purges it from notifications once more, which takes the notice.
#[sqlx::test]
async fn a_notice_written_as_its_project_is_deleted_goes_with_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    auth.forget_after(&p.project, 1);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 }
    );
    assert!(
        p.notices.notices().is_empty(),
        "the notice outlived its project"
    );
    assert_eq!(p.notices.purged(), std::slice::from_ref(&p.project));
}

/// Auth lost on the ask after the write says nothing of a deletion: nothing
/// is purged, since a purge takes the project's connectors and feed with it,
/// and the row is handed back with its try; the next tick writes the notice
/// again, which its dedup key keeps one notice, and asks again.
#[sqlx::test]
async fn a_notice_auth_cannot_place_once_written_is_tried_again(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    auth.failing_homes_after(1);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 0, held: 2 }
    );
    assert_eq!(tries_of(&pool, &p.project).await, [(0, false), (0, false)]);
    assert_eq!(p.notices.notices().len(), 1, "the notice was written");
    assert!(
        p.notices.purged().is_empty(),
        "a project auth could not place was purged"
    );

    auth.failing_homes(false);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 }
    );
    assert_eq!(
        p.notices.notices().len(),
        1,
        "written again, still one notice"
    );
    assert!(p.notices.purged().is_empty());
}

/// A purge that fails, or does not end within auth's budget for it, hands
/// the row back, its notice written, and the next tick, finding the project
/// gone, purges again and drops the row.
#[sqlx::test]
async fn a_purge_that_fails_or_does_not_end_in_time_hands_the_notice_back(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    auth.forget_after(&p.project, 1);
    p.notices.failing_purges(true);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 0, held: 2 }
    );
    assert_eq!(tries_of(&pool, &p.project).await, [(0, false), (0, false)]);
    assert_eq!(p.notices.notices().len(), 1, "the notice was written");

    p.notices.failing_purges(false);
    p.notices.stalling_purges(true);
    let settled = tokio::time::timeout(Duration::from_secs(15), outbox::tick_once(&p.state))
        .await
        .expect("a stalled purge held the tick past its budget")
        .expect("a tick");
    assert_eq!(settled, Settled { landed: 0, held: 2 });
    assert_eq!(tries_of(&pool, &p.project).await, [(0, false), (0, false)]);
    assert_eq!(p.notices.notices().len(), 1, "the notice was purged");

    p.notices.stalling_purges(false);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 1, held: 1 }
    );
    assert!(
        p.notices.notices().is_empty(),
        "the notice outlived its project"
    );
    assert_eq!(p.notices.purged(), std::slice::from_ref(&p.project));
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

/// The plan's ceiling is read within a second, so a deployment slow to
/// answer it — billing's pool spent — holds the lane on it no longer: an
/// answer not had in time is no ceiling, and the monitor is made.
#[sqlx::test]
async fn a_ceiling_not_answered_in_time_is_no_ceiling(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let slow = Ceiling::answered_after(&pool, 0, Duration::from_secs(3));
    let p = project_under(&pool, OrganizationId::new(), slow.clone());
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    assert_eq!(
        monitors_of(&pool, &p.project).await,
        1,
        "no ceiling was held"
    );
    assert_eq!(
        slow.asked(),
        [false],
        "asked once, with the chain lock not held"
    );
}

/// The ceiling is read before the organization's chain lock is taken, so
/// nobody else in the organization waits on the answer: with the lock held
/// elsewhere, a ping to a new slug is refused for the lock as every other
/// writer is, the ceiling asked all the same and never under it.
#[sqlx::test]
async fn the_ceiling_is_read_before_the_chain_lock(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let quick = Ceiling::of(&pool, 5);
    let p = project_under(&pool, OrganizationId::new(), quick.clone());
    let key = p.mint().await;
    let mut held = pool.begin().await.expect("a transaction to hold the lock");
    lock_chain(&mut held, &p.organization)
        .await
        .expect("the chain lock");
    let (status, body) = p.ping(&format!("/{key}/nightly-report")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.contains("another change to this project's organization is under way"),
        "{body}"
    );
    assert_eq!(
        quick.asked(),
        [true],
        "asked once, before the lane tried the lock"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
    held.rollback().await.expect("let the lock go");
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    assert_eq!(quick.asked(), [true, false]);
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
}
