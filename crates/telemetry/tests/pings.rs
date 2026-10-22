//! The ping lane as a crontab reaches it: monitors a first ping makes, runs
//! from `/start` to their end, the monitor's state and due time after them,
//! and the lane's limits and refusals; pings racing the console's changes,
//! moves and purges; finished runs reaching ClickHouse through the outbox;
//! and the purges, the move and the purge sweep.
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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;
use uuid::Uuid;

use common::{
    Answer, AuthStub, Ceiling, NoticeLog, PausedOrganization, PlanLanes, SECRET, StandingAsked,
};
use telmoni_shared::audit::lock_chain;
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits, Notice, Notifications, Telemetry};
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::types::{PING_KEY_PREFIX, PING_KEY_RANDOM_LEN};
use telmoni_shared::{
    MonitorState, NotificationKind, OrganizationId, OrganizationRole, OrganizationStatus,
    ProjectId, SpanKind, TelmoniError,
};
use telmoni_telemetry::ceilings;
use telmoni_telemetry::db::{self, MonitorRow, TelemetryLane};
use telmoni_telemetry::handler::pings::NEW_MONITORS_MAX;
use telmoni_telemetry::outbox::{self, FinishedRun, Settled};
use telmoni_telemetry::seam::TelemetrySeam;
use telmoni_telemetry::store::SpanRow;
use telmoni_telemetry::sweep::{self, Swept};
use telmoni_telemetry::{AppState, ping_router, router};

const MONITORS: &str = "/internal/v1/telemetry/monitors";
const PING_KEYS: &str = "/internal/v1/telemetry/ping-keys";
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

/// A project as `project` makes one, its pings waiting out a move of it for
/// `move_wait` before they give up.
fn project_waiting_out_moves(pool: &PgPool, move_wait: Duration) -> Project {
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
    let state = common::state_waiting_out_moves(pool, auth, notices.clone(), move_wait);
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

    /// One of the project's monitors, as the console reads it, without what
    /// its page reads beside it — its open runs and incidents, which
    /// `monitors.rs` holds — so it compares with a change's answer.
    async fn monitor(&self, slug: &str) -> Value {
        let (status, mut body) = self
            .send(self.request("GET", &format!("{MONITORS}/{slug}"), None))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let page = body.as_object_mut().expect("a monitor");
        for field in ["open_runs", "incidents"] {
            assert!(page.remove(field).is_some(), "{field} on its page");
        }
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

    /// Two `GET` pings, the second sent while the first waits to settle its
    /// monitor, held up by a session locking the monitors' rows; both
    /// answers, in order. The first must change its monitor's state, or it
    /// waits on nothing.
    async fn pings_racing(
        &self,
        pool: &PgPool,
        first: &str,
        second: &str,
    ) -> ((StatusCode, String), (StatusCode, String)) {
        let mut holder = pool.begin().await.expect("the holder's transaction");
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .expect("the holder's session");
        let held =
            sqlx::query("SELECT 1 FROM telemetry.monitors WHERE project_id = $1 FOR NO KEY UPDATE")
                .bind(&self.project)
                .execute(&mut *holder)
                .await
                .expect("hold the project's monitors")
                .rows_affected();
        assert!(held > 0, "the project has no monitor to hold");
        tokio::join!(self.ping(first), async {
            let first_pid = waiting_on(
                pool,
                holder_pid,
                "the first ping waits to write its monitor",
            )
            .await;
            let (second, ()) = tokio::join!(self.ping(second), async {
                waiting_on(pool, first_pid, "the second ping waits on the first's run").await;
                holder.commit().await.expect("the holder lets go");
            });
            second
        })
    }

    /// A `GET` ping and the console's delete of its monitor, the delete sent
    /// once the ping has ended its runs: a session of `pool`'s holds the
    /// outbox, so the ping, its runs' rows deleted, waits there to file them;
    /// the delete is sent once it does, and the session lets go once the
    /// delete waits on the ping. The ping's answer and the delete's status.
    async fn ping_as_its_monitor_is_deleted(
        &self,
        pool: &PgPool,
        path: &str,
        slug: &str,
    ) -> ((StatusCode, String), StatusCode) {
        let mut holder = pool.begin().await.expect("the holder's transaction");
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .expect("the holder's session");
        sqlx::query("LOCK TABLE telemetry.outbox IN SHARE MODE")
            .execute(&mut *holder)
            .await
            .expect("hold the outbox");
        let delete = self.request("DELETE", &format!("{MONITORS}/{slug}"), None);
        tokio::join!(self.ping(path), async {
            let ping_pid = waiting_on(pool, holder_pid, "the ping waits to file its run").await;
            let ((deleted, _), ()) = tokio::join!(self.send(delete), async {
                waiting_on(pool, ping_pid, "the delete waits on the ping").await;
                holder.commit().await.expect("the holder lets go");
            });
            deleted
        })
    }

    /// A move of the project to `to`, held up on the project's ping key — a
    /// session of `pool`'s holds the key's row — once it has held the
    /// monitors and rewritten what comes before the key, and `beside`, run
    /// once the move waits there; the session lets go once `beside` waits on
    /// the move. Whether the move found a settings row, and what `beside`
    /// came to.
    async fn moving_beside<T>(
        &self,
        pool: &PgPool,
        to: &OrganizationId,
        beside: impl Future<Output = T>,
    ) -> (bool, T) {
        let mut holder = pool.begin().await.expect("the holder's transaction");
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .expect("the holder's session");
        sqlx::query("SELECT 1 FROM telemetry.ping_keys WHERE project_id = $1 FOR UPDATE")
            .bind(&self.project)
            .execute(&mut *holder)
            .await
            .expect("hold the key");
        let mut mover = maintenance_scope(&self.state.db, TelemetryLane)
            .await
            .expect("the lane");
        tokio::join!(
            async {
                let moved = db::move_project(&mut mover, &self.project, to)
                    .await
                    .expect("moved");
                mover.commit().await.expect("the move commits");
                moved
            },
            async {
                let mover_pid = waiting_on(pool, holder_pid, "the move waits on the key").await;
                let (done, ()) = tokio::join!(beside, async {
                    waiting_on(pool, mover_pid, "the writer beside it waits on the move").await;
                    holder.commit().await.expect("the holder lets go");
                });
                done
            },
        )
    }

    /// A `GET` ping sent while the console's delete of `slug` holds the
    /// monitor and is taking its runs: a session of `pool`'s holds one of
    /// them, `held`, so the delete waits on it there, the monitor and the
    /// rest taken; the ping is sent once it does, and the session lets go
    /// once the ping waits on the delete. The ping's answer and the delete's
    /// status.
    async fn ping_as_a_delete_holds_its_monitor(
        &self,
        pool: &PgPool,
        path: &str,
        slug: &str,
        held: &str,
    ) -> ((StatusCode, String), StatusCode) {
        let mut holder = pool.begin().await.expect("the holder's transaction");
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .expect("the holder's session");
        sqlx::query(
            "SELECT 1 FROM telemetry.open_runs WHERE project_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(&self.project)
        .bind(Uuid::parse_str(held).expect("a run's id"))
        .execute(&mut *holder)
        .await
        .expect("hold the run");
        let delete = self.request("DELETE", &format!("{MONITORS}/{slug}"), None);
        let ((deleted, _), answer) = tokio::join!(self.send(delete), async {
            let delete_pid = waiting_on(pool, holder_pid, "the delete waits on the run held").await;
            let (answer, ()) = tokio::join!(self.ping(path), async {
                waiting_on(pool, delete_pid, "the ping waits on the delete").await;
                holder.commit().await.expect("the holder lets go");
            });
            answer
        });
        (answer, deleted)
    }

    /// A `GET` ping, and changes to its monitor sent while the ping, its
    /// monitor read, waits on a session holding `table`; the ping's answer
    /// and each change's status and body.
    async fn ping_as_its_monitor_changes(
        &self,
        pool: &PgPool,
        path: &str,
        table: &str,
        changes: Vec<Request<Body>>,
    ) -> ((StatusCode, String), Vec<(StatusCode, Value)>) {
        let mut holder = pool.begin().await.expect("the holder's transaction");
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .expect("the holder's session");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "LOCK TABLE telemetry.{table} IN SHARE MODE"
        )))
        .execute(&mut *holder)
        .await
        .expect("hold the table");
        tokio::join!(self.ping(path), async {
            waiting_on(pool, holder_pid, "the ping waits to write its run").await;
            let mut changed = Vec::new();
            for change in changes {
                changed.push(
                    tokio::time::timeout(Duration::from_secs(10), self.send(change))
                        .await
                        .expect("a change waited on the ping"),
                );
            }
            holder.commit().await.expect("the holder lets go");
            changed
        })
    }

    /// A `GET` ping naming `run`, held up by a session opening a run under
    /// that id, which it rolls back once `beside` has run; the ping's answer
    /// and what `beside` came to.
    async fn ping_as_its_run_is_opened<T>(
        &self,
        pool: &PgPool,
        path: &str,
        run: &str,
        monitor: Uuid,
        beside: impl Future<Output = T>,
    ) -> ((StatusCode, String), T) {
        let mut opener = pool.begin().await.expect("the opener's transaction");
        let opener_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *opener)
            .await
            .expect("the opener's session");
        let opening = sqlx::query(
            "INSERT INTO telemetry.open_runs (id, project_id, organization_id, monitor_id, \
                 agent, source, ends_by)
             SELECT $1, project_id, organization_id, id, agent, 'ping', now() + interval '1 day'
               FROM telemetry.monitors WHERE id = $2",
        )
        .bind(Uuid::parse_str(run).expect("a run's id"))
        .bind(monitor)
        .execute(&mut *opener)
        .await
        .expect("a run being opened")
        .rows_affected();
        assert_eq!(opening, 1, "no monitor to open a run of");
        tokio::join!(self.ping(path), async {
            waiting_on(pool, opener_pid, "the ping waits on the run being opened").await;
            let done = beside.await;
            opener.rollback().await.expect("the opener lets go");
            done
        })
    }

    /// A `GET` ping and the console's pause of `slug`, the pause written but
    /// not committed when the ping comes to settle the monitor; the ping's
    /// answer and the pause's status.
    async fn ping_as_a_pause_commits(
        &self,
        pool: &PgPool,
        path: &str,
        slug: &str,
    ) -> ((StatusCode, String), StatusCode) {
        let mut outbox = pool.begin().await.expect("a holder's transaction");
        let outbox_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *outbox)
            .await
            .expect("a holder's session");
        sqlx::query("LOCK TABLE telemetry.outbox IN SHARE MODE")
            .execute(&mut *outbox)
            .await
            .expect("hold the outbox");
        let mut chain = pool.begin().await.expect("a holder's transaction");
        let chain_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *chain)
            .await
            .expect("a holder's session");
        sqlx::query("LOCK TABLE audit.events IN SHARE MODE")
            .execute(&mut *chain)
            .await
            .expect("hold the chain's records");
        let pause = self.request("POST", &format!("{MONITORS}/{slug}/pause"), None);
        tokio::join!(self.ping(path), async {
            waiting_on(pool, outbox_pid, "the ping waits to file its run").await;
            let paused = tokio::time::timeout(Duration::from_secs(10), self.send(pause));
            let (paused, ()) = tokio::join!(paused, async {
                let pause_pid =
                    waiting_on(pool, chain_pid, "the pause waits to record itself").await;
                outbox.commit().await.expect("the ping goes on");
                waiting_on(pool, pause_pid, "the ping waits on the pause").await;
                chain.commit().await.expect("the pause goes on");
            });
            paused.expect("the pause was answered").0
        })
    }

    /// A change to `slug` sent from the console while a ping's settling holds
    /// the monitor, which then settles it healthy and due at `due`; the
    /// change's status and body, and the monitor as the settling left it.
    async fn change_as_a_ping_settles(
        &self,
        pool: &PgPool,
        slug: &str,
        change: Request<Body>,
        due: DateTime<Utc>,
    ) -> ((StatusCode, Value), MonitorRow) {
        let id = self
            .monitor(slug)
            .await
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .expect("a monitor's id");
        let mut ping = project_scope(&self.state.db, &self.project)
            .await
            .expect("the ping's scope")
            .bind_organization(&self.organization)
            .await
            .expect("bound to its organization");
        let ping_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(ping.conn())
            .await
            .expect("the ping's session");
        db::monitor_to_settle(&mut ping, &self.project, id)
            .await
            .expect("hold the monitor to settle it")
            .expect("the monitor");
        let changed = tokio::time::timeout(Duration::from_secs(10), self.send(change));
        let (changed, settled) = tokio::join!(changed, async {
            waiting_on(pool, ping_pid, "the change waits on the ping's settling").await;
            let settled = db::settle_monitor_run(
                &mut ping,
                &self.project,
                id,
                MonitorState::Healthy,
                Some(due),
            )
            .await
            .expect("settled as a ping settles it")
            .expect("the monitor");
            ping.commit().await.expect("the settling commits");
            settled
        });
        (changed.expect("the change waited on the settling"), settled)
    }

    /// A `GET` ping sent while a purge, the project's or its organization's,
    /// is stopped between its deletes; the ping's answer, what the purge came
    /// to, and whether a ping could have held the project in place meanwhile.
    async fn ping_as_a_purge_runs(
        &self,
        pool: &PgPool,
        path: &str,
        whole_organization: bool,
    ) -> ((StatusCode, String), sqlx::Result<u64>, bool) {
        let (monitors, monitors_pid) = holding(pool, "telemetry.monitors").await;
        let (outbox, outbox_pid) = holding(pool, "telemetry.outbox").await;
        let mut purger = maintenance_scope(&self.state.db, TelemetryLane)
            .await
            .expect("the lane");
        let purger_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(purger.conn())
            .await
            .expect("the purge's session");
        let probed = std::slice::from_ref(&self.project);
        let (purged, (answer, held)) = tokio::join!(
            async {
                let purged = if whole_organization {
                    db::purge_organization(&mut purger, &self.organization).await
                } else {
                    db::purge_project(&mut purger, &self.project).await
                };
                match purged {
                    Ok(purged) => purger.commit().await.map(|()| purged),
                    Err(e) => {
                        purger.rollback().await.expect("the purge lets go");
                        Err(e)
                    }
                }
            },
            async {
                waiting_on(pool, monitors_pid, "the purge waits to delete the monitors").await;
                let held_first = !holdable(pool, probed).await.is_empty();
                monitors.commit().await.expect("the monitors let go");
                waiting_on(pool, outbox_pid, "the purge waits to delete the outbox").await;
                let held_then = !holdable(pool, probed).await.is_empty();
                let (answer, _) = tokio::join!(self.ping(path), async {
                    let waited =
                        waits_on_within(pool, purger_pid, Duration::from_millis(150)).await;
                    outbox.commit().await.expect("the outbox lets go");
                    waited
                });
                (answer, held_first || held_then)
            },
        );
        (answer, purged, held)
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
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "UPDATE telemetry.{table} SET organization_id = organization_id
                      WHERE project_id = $1"
                )))
                .bind(&self.project)
                .execute(writer)
                .await,
            ),
            Lock::Purged(table) => (
                table,
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DELETE FROM telemetry.{table} WHERE project_id = $1"
                )))
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

/// When one of the project's open runs started, and when it was last heard.
async fn clocks_of(
    pool: &PgPool,
    project: &ProjectId,
    run: &str,
) -> (DateTime<Utc>, DateTime<Utc>) {
    sqlx::query_as(
        "SELECT started_at, last_heard_at FROM telemetry.open_runs
          WHERE project_id = $1 AND id = $2",
    )
    .bind(project)
    .bind(Uuid::parse_str(run).expect("a run's id"))
    .fetch_one(pool)
    .await
    .expect("read a run's clocks")
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

/// How many seconds an outbox row is held for yet.
async fn held_for(pool: &PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT CAST(extract(epoch FROM lease_until - now()) AS BIGINT)
           FROM telemetry.outbox WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("read how long a row is held")
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
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT organization_id FROM telemetry.{table} WHERE project_id = $1"
    )))
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

/// A transaction of `pool`'s holding `table` in `SHARE` mode, which every
/// write to the table waits on, and its session.
async fn holding(pool: &PgPool, table: &str) -> (sqlx::Transaction<'static, sqlx::Postgres>, i32) {
    let mut holder = pool.begin().await.expect("a holder's transaction");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .expect("a holder's session");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "LOCK TABLE {table} IN SHARE MODE"
    )))
    .execute(&mut *holder)
    .await
    .expect("hold the table");
    (holder, pid)
}

/// Those of `projects` a ping could hold in place now.
async fn holdable(pool: &PgPool, projects: &[ProjectId]) -> Vec<ProjectId> {
    let mut held = Vec::new();
    for project in projects {
        let mut probe = pool.begin().await.expect("a probe's transaction");
        if project_move::try_hold_in_place(&mut probe, project)
            .await
            .expect("try the project")
        {
            held.push(project.clone());
        }
        probe.rollback().await.expect("the probe lets go");
    }
    held
}

/// Whether a session comes to wait on `pid`'s locks within `within`, for a
/// step one may or may not come to wait at.
async fn waits_on_within(pool: &PgPool, pid: i32, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                             WHERE $1 = ANY (pg_blocking_pids(pid)))",
        )
        .bind(pid)
        .fetch_one(pool)
        .await
        .expect("ask who waits");
        if waiting || std::time::Instant::now() >= deadline {
            return waiting;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The tables a purge empties in which the project has a row still.
async fn left_of(pool: &PgPool, project: &ProjectId) -> Vec<&'static str> {
    let mut left = Vec::new();
    for table in [
        "monitors",
        "open_runs",
        "ping_keys",
        "outbox",
        "project_settings",
    ] {
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM telemetry.{table} WHERE project_id = $1"
        )))
        .bind(project)
        .fetch_one(pool)
        .await
        .expect("count the project's rows");
        if rows > 0 {
            left.push(table);
        }
    }
    left
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

/// The project's incidents, oldest first: id, cause, detail, how each closed
/// if it has, and whether it is flapping.
async fn incidents_of(
    pool: &PgPool,
    project: &ProjectId,
) -> Vec<(Uuid, String, String, Option<String>, bool)> {
    sqlx::query_as(
        "SELECT id, cause, detail, resolution, flapping FROM telemetry.incidents
          WHERE project_id = $1
          ORDER BY opened_at, id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read a project's incidents")
}

/// The notices waiting in the project's outbox, oldest first.
async fn notices_of(pool: &PgPool, project: &ProjectId) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM telemetry.outbox
          WHERE project_id = $1 AND kind = 'notice'
          ORDER BY id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read a project's outbox")
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

/// A first ping whose agent another monitor of the project carries makes
/// nothing, and gets a 409 in the lane's own words, not the console's.
#[sqlx::test]
async fn a_first_ping_whose_agent_another_monitor_carries_is_told_its_slug(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(json!({ "name": "Report", "slug": "report-a", "agent": "nightly-report" }))
        .await;
    let (status, body) = p.ping(&format!("/{key}/nightly-report")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.contains(
            "this project has a monitor for that agent under another slug; ping that slug"
        ),
        "{body}"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
}

/// A monitor its first ping made starts once its owner saves its settings,
/// and not before: a resume is refused with how to start it, changing and
/// recording nothing; a save starts it `healthy`, due under the timing it
/// sets and held to the bounds it sets in place of the hour's stall timeout
/// it was given, the start recorded on the chain beside what changed — and
/// recorded where nothing else did — after which its pings move it as any
/// monitor's do.
#[sqlx::test]
async fn a_monitor_a_ping_made_starts_once_its_owner_saves_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    p.ping_ok(&format!("/{key}/weekly-digest")).await;
    let made = p.monitor("nightly-report").await;
    assert_eq!(made["state"], "new");

    let (status, body) = p
        .send(p.request("POST", &format!("{MONITORS}/nightly-report/resume"), None))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("save its settings to start it")),
        "{body}"
    );
    assert_eq!(p.monitor("nightly-report").await, made);
    assert_eq!(
        recorded(&pool, &p.organization).await.len(),
        2,
        "the two discoveries alone"
    );

    // A pause of `new` is answered as it is — paused until saved already —
    // so a resume after it still asks for the settings.
    let (status, paused) = p
        .send(p.request("POST", &format!("{MONITORS}/nightly-report/pause"), None))
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused, made, "answered as it is");
    let (status, body) = p
        .send(p.request("POST", &format!("{MONITORS}/nightly-report/resume"), None))
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a pause let the resume skip the settings: {body}"
    );
    assert_eq!(
        recorded(&pool, &p.organization).await.len(),
        2,
        "nothing recorded"
    );

    let (status, saved) = p
        .send(p.request(
            "PUT",
            &format!("{MONITORS}/nightly-report"),
            Some(json!({
                "name": "Nightly report",
                "schedule": "0 2 * * *",
                "timezone": "Europe/London",
                "grace_s": 600,
                "bounds": { "stall_after_s": 120 },
            })),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["id"], made["id"]);
    assert_eq!(saved["state"], "healthy");
    assert!(
        saved["next_due_at"].is_string(),
        "due under the timing saved"
    );
    assert_eq!(saved["bounds"]["stall_after_s"], 120);
    let chain = recorded(&pool, &p.organization).await;
    assert_eq!(chain.len(), 3, "{chain:?}");
    assert_eq!(
        (chain[2].0.as_str(), chain[2].1.as_str()),
        ("updated", OWNER)
    );
    assert_eq!(
        chain[2].2["state"],
        json!({ "from": "new", "to": "healthy" })
    );
    assert_eq!(
        chain[2].2["changed"],
        json!(["name", "schedule", "timezone", "grace_s", "stall_after_s"])
    );

    // Saved as its ping made it, it starts all the same, and the start is
    // recorded though nothing else changed.
    let (status, saved) = p
        .send(p.request(
            "PUT",
            &format!("{MONITORS}/weekly-digest"),
            Some(json!({
                "name": "weekly-digest",
                "bounds": { "stall_after_s": 3600 },
            })),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["state"], "healthy");
    assert!(saved["next_due_at"].is_null(), "no timing, so never due");
    let chain = recorded(&pool, &p.organization).await;
    assert_eq!(chain.len(), 4, "{chain:?}");
    assert_eq!(
        chain[3].2["state"],
        json!({ "from": "new", "to": "healthy" })
    );
    assert_eq!(chain[3].2["changed"], json!([]));

    // Started, its pings move it as any monitor's do.
    p.ping_ok(&format!("/{key}/nightly-report/fail")).await;
    assert_eq!(p.monitor("nightly-report").await["state"], "failed");
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    assert_eq!(p.monitor("nightly-report").await["state"], "healthy");
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

    // Resumed, a success with no run open keeps the monitor's state, and so
    // the clock of its last change, and moves its due time on; a failure
    // moves both.
    let (status, resumed) = p
        .send(p.request("POST", &format!("{MONITORS}/hourly-sync/resume"), None))
        .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["state"], "healthy");
    assert_eq!(monitor["state_changed_at"], resumed["state_changed_at"]);
    assert!(at(&monitor["next_due_at"]) > at(&resumed["next_due_at"]));
    p.ping_ok(&format!("/{key}/hourly-sync/fail")).await;
    let failed = p.monitor("hourly-sync").await;
    assert_eq!(failed["state"], "failed");
    assert!(at(&failed["state_changed_at"]) > at(&monitor["state_changed_at"]));
}

/// A monitor keeps its last run as it ends — the run an end ping meant,
/// one of no duration, or one a `/start` or a bare end ping found never
/// ended — with its verdict and what its checkpoints reported, as the span
/// has them; a run ended late, when it was last heard, never replaces one
/// that ended since.
#[sqlx::test]
async fn a_monitor_keeps_the_run_that_ended_last(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let made = p.make(hourly()).await;
    assert_eq!(made["last_run"], Value::Null, "none until one has ended");

    // A run from `/start` to its end: kept with what its checkpoints said.
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={run}&steps=2&cost=0.5"
    ))
    .await;
    assert_eq!(
        p.monitor("hourly-sync").await["last_run"],
        Value::Null,
        "still going"
    );
    p.ping_ok(&format!("/{key}/hourly-sync?rid={run}")).await;
    let monitor = p.monitor("hourly-sync").await;
    let last = &monitor["last_run"];
    assert_eq!(last["run_id"].as_str(), Some(run.as_str()), "{monitor}");
    assert_eq!(last["status"], "ok");
    assert_eq!(last["steps"], 2);
    assert_eq!(last["cost_usd"], 0.5);
    assert!(at(&last["started_at"]) < at(&last["ended_at"]));
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(last["started_at"], finished[0]["started_at"]);
    assert_eq!(
        last["ended_at"], finished[0]["ended_at"],
        "as the span has it"
    );

    // A `/fail` naming a run not open: one of no duration, ended error with
    // nothing reported, is the last.
    let fresh = rid();
    let older = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={older}"))
        .await;
    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={older}&steps=1"
    ))
    .await;
    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={fresh}&exit_code=3"))
        .await;
    let last = p.monitor("hourly-sync").await["last_run"].clone();
    assert_eq!(last["run_id"].as_str(), Some(fresh.as_str()), "{last}");
    assert_eq!(last["status"], "error");
    assert_eq!(last["started_at"], last["ended_at"]);
    assert!(last["steps"].is_null());
    assert!(last["cost_usd"].is_null());

    // A bare `/start` ends the older run, which never said it ended, at
    // the moment it was last heard — before the run of no duration ended —
    // so the run kept stands; the run opened is not kept until it ends.
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["state"], "running", "{monitor}");
    assert_eq!(
        monitor["last_run"], last,
        "a run ended late replaces nothing"
    );
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 3);
    assert_eq!(finished[2]["run_id"].as_str(), Some(older.as_str()));
    assert!(at(&finished[2]["ended_at"]) < at(&last["ended_at"]));

    // A bare end ping ends the newest run with its verdict, now, and the
    // rest as `error` when last heard: the newest is the last.
    let newest = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={newest}"))
        .await;
    assert_eq!(open_runs(&pool, &p.project).await.len(), 2);
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["state"], "healthy", "{monitor}");
    assert_eq!(
        monitor["last_run"]["run_id"].as_str(),
        Some(newest.as_str()),
        "{monitor}"
    );
    assert_eq!(monitor["last_run"]["status"], "ok");
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 5);

    // A paused monitor keeps its runs, so it keeps its last run too.
    let (status, paused) = p
        .send(p.request("POST", &format!("{MONITORS}/hourly-sync/pause"), None))
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    let while_paused = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={while_paused}"))
        .await;
    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={while_paused}"))
        .await;
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["state"], "paused", "{monitor}");
    assert_eq!(
        monitor["last_run"]["run_id"].as_str(),
        Some(while_paused.as_str()),
        "{monitor}"
    );
    assert_eq!(monitor["last_run"]["status"], "error");
}

/// A run that ends well is the monitor's last success, kept through the
/// failures after it; an end ping tried again moves it nothing, and the next
/// run that ends well moves it on.
#[sqlx::test]
async fn a_monitor_keeps_its_last_success_through_failures(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let made = p.make(hourly()).await;
    assert_eq!(made["last_success_at"], Value::Null, "none until one has");

    let first = rid();
    p.ping_ok(&format!("/{key}/hourly-sync?rid={first}")).await;
    let monitor = p.monitor("hourly-sync").await;
    let success = monitor["last_success_at"].clone();
    assert_eq!(success, monitor["last_run"]["ended_at"], "{monitor}");
    p.ping_ok(&format!("/{key}/hourly-sync?rid={first}")).await;
    assert_eq!(
        p.monitor("hourly-sync").await["last_success_at"],
        success,
        "a copy moves it nothing"
    );

    for _ in 0..2 {
        p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={}", rid()))
            .await;
    }
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["last_run"]["status"], "error", "{monitor}");
    assert_eq!(
        monitor["last_success_at"], success,
        "a failure moves nothing"
    );

    let later = rid();
    p.ping_ok(&format!("/{key}/hourly-sync?rid={later}")).await;
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(
        monitor["last_run"]["run_id"].as_str(),
        Some(later.as_str()),
        "{monitor}"
    );
    assert_eq!(monitor["last_success_at"], monitor["last_run"]["ended_at"]);
    assert!(at(&monitor["last_success_at"]) > at(&success));
}

/// An end ping tried again — `telmoni run` repeats one that got no answer —
/// finds its run ended already and files a run of no duration under the same
/// id, which the outbox drops; the monitor keeps the record the first ping
/// left, never the copy, while a run of another id is the last as before.
#[sqlx::test]
async fn an_end_ping_tried_again_never_replaces_the_run_it_already_ended(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={run}&steps=2&cost=0.5"
    ))
    .await;
    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={run}&exit_code=3"))
        .await;
    let first = p.monitor("hourly-sync").await["last_run"].clone();
    assert_eq!(first["run_id"].as_str(), Some(run.as_str()), "{first}");
    assert_eq!(first["status"], "error");
    assert_eq!(first["steps"], 2);
    assert!(at(&first["started_at"]) < at(&first["ended_at"]));

    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={run}&exit_code=3"))
        .await;
    assert_eq!(
        p.monitor("hourly-sync").await["last_run"],
        first,
        "a copy replaces nothing"
    );
    p.ping_ok(&format!("/{key}/hourly-sync?rid={run}")).await;
    let monitor = p.monitor("hourly-sync").await;
    assert_eq!(monitor["last_run"], first, "{monitor}");
    assert_eq!(
        monitor["state"], "healthy",
        "the state follows the ping as before"
    );

    let other = rid();
    p.ping_ok(&format!("/{key}/hourly-sync?rid={other}")).await;
    let last = p.monitor("hourly-sync").await["last_run"].clone();
    assert_eq!(last["run_id"].as_str(), Some(other.as_str()), "{last}");
    assert_eq!(last["started_at"], last["ended_at"]);
}

/// A run ending in error opens an incident, once, saying what moved the
/// monitor and the exit code; runs starting and failing again under it
/// change it nothing; a run ending well resolves it, saying how long it
/// stood; and the next failure opens another. Each notice reaches
/// notifications through the outbox under its own dedup key.
#[sqlx::test]
async fn a_failing_run_opens_an_incident_and_a_run_ending_well_resolves_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let made = p.make(hourly()).await;
    let monitor_id = made["id"].as_str().expect("an id").to_owned();

    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    p.ping_ok(&format!("/{key}/hourly-sync/fail?exit_code=3"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    let incidents = incidents_of(&pool, &p.project).await;
    assert_eq!(incidents.len(), 1, "{incidents:?}");
    let (id, cause, detail, resolution, flapping) = &incidents[0];
    assert_eq!(
        (
            cause.as_str(),
            detail.as_str(),
            resolution.as_deref(),
            *flapping
        ),
        ("failed", "run_failed", None, false)
    );
    let notices = notices_of(&pool, &p.project).await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    let opened = &notices[0];
    assert_eq!(opened["kind"], "incident_opened");
    assert_eq!(opened["title"], "hourly-sync failed");
    assert!(
        opened["body"]
            .as_str()
            .expect("a body")
            .contains("exit code 3"),
        "{}",
        opened["body"]
    );
    assert_eq!(opened["metadata"]["incident_id"], id.to_string());
    assert_eq!(opened["metadata"]["monitor_id"], monitor_id);
    assert_eq!(opened["metadata"]["cause"], "failed");
    assert_eq!(opened["metadata"]["detail"], "run_failed");
    assert_eq!(opened["metadata"]["exit_code"], 3);
    assert_eq!(opened["metadata"]["flapping"], false);
    assert_eq!(
        opened["dedup_key"],
        format!("telemetry:incident:{id}:opened")
    );

    // Another run starting, and failing again: the same incident, told once.
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    p.ping_ok(&format!("/{key}/hourly-sync/fail")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    assert_eq!(incidents_of(&pool, &p.project).await.len(), 1);
    assert_eq!(notices_of(&pool, &p.project).await.len(), 1);

    // A run ending well resolves it.
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    let incidents = incidents_of(&pool, &p.project).await;
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].3.as_deref(), Some("recovered"));
    let notices = notices_of(&pool, &p.project).await;
    assert_eq!(notices.len(), 2, "{notices:?}");
    let resolved = &notices[1];
    assert_eq!(resolved["kind"], "incident_resolved");
    assert_eq!(resolved["title"], "hourly-sync recovered");
    assert_eq!(resolved["metadata"]["resolution"], "recovered");
    assert_eq!(
        resolved["metadata"]["incident_id"],
        opened["metadata"]["incident_id"]
    );
    assert!(
        resolved["metadata"]["duration_s"]
            .as_i64()
            .is_some_and(|s| s >= 0),
        "{resolved}"
    );
    assert_eq!(
        resolved["dedup_key"],
        format!("telemetry:incident:{id}:resolved")
    );

    // The next failure is an incident of its own.
    p.ping_ok(&format!("/{key}/hourly-sync/fail?exit_code=1"))
        .await;
    let incidents = incidents_of(&pool, &p.project).await;
    assert_eq!(incidents.len(), 2, "{incidents:?}");
    assert_ne!(incidents[1].0, incidents[0].0);
    assert!(incidents[1].3.is_none());

    // Through the outbox to notifications, each once; the runs' rows wait on
    // a store this suite has none of.
    let settled = outbox::tick_once(&p.state).await.expect("a tick");
    assert_eq!(settled.landed, 3, "{settled:?}");
    let told = p.notices.notices();
    assert_eq!(
        told.iter().map(|n| n.kind).collect::<Vec<_>>(),
        [
            NotificationKind::IncidentOpened,
            NotificationKind::IncidentResolved,
            NotificationKind::IncidentOpened,
        ]
    );
    assert!(
        told.iter()
            .all(|n| n.project_id.as_ref() == Some(&p.project)
                && n.organization_id == p.organization)
    );
    assert_eq!(
        told[0].dedup_key.as_deref(),
        Some(format!("telemetry:incident:{id}:opened").as_str())
    );
}

/// A monitor flipping in and out of trouble more than six times in an hour
/// is flapping: the incident the seventh flip opens stays open, marked so,
/// through the runs that follow, so the job costs one incident from then on,
/// and the project hears of the flapping once.
#[sqlx::test]
async fn a_flapping_monitor_keeps_one_incident_open(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;

    // Three incidents opened and recovered: six flips.
    for _ in 0..3 {
        p.ping_ok(&format!("/{key}/hourly-sync/fail")).await;
        p.ping_ok(&format!("/{key}/hourly-sync")).await;
    }
    let incidents = incidents_of(&pool, &p.project).await;
    assert_eq!(incidents.len(), 3, "{incidents:?}");
    assert!(
        incidents
            .iter()
            .all(|i| i.3.as_deref() == Some("recovered") && !i.4),
        "{incidents:?}"
    );

    // The seventh flip opens an incident flapping.
    p.ping_ok(&format!("/{key}/hourly-sync/fail")).await;
    let incidents = incidents_of(&pool, &p.project).await;
    assert_eq!(incidents.len(), 4, "{incidents:?}");
    assert!(incidents[3].3.is_none() && incidents[3].4, "{incidents:?}");
    let notices = notices_of(&pool, &p.project).await;
    assert_eq!(notices.len(), 7, "{notices:?}");
    assert_eq!(notices[6]["metadata"]["flapping"], true);
    assert!(
        notices[6]["body"]
            .as_str()
            .expect("a body")
            .contains("flipped in and out of trouble"),
        "{}",
        notices[6]["body"]
    );

    // Healthy and failing again, twice over: the incident stays open and
    // nothing more is told.
    for _ in 0..2 {
        p.ping_ok(&format!("/{key}/hourly-sync")).await;
        assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
        p.ping_ok(&format!("/{key}/hourly-sync/fail")).await;
        assert_eq!(p.monitor("hourly-sync").await["state"], "failed");
    }
    let incidents = incidents_of(&pool, &p.project).await;
    assert_eq!(incidents.len(), 4, "{incidents:?}");
    assert!(incidents[3].3.is_none() && incidents[3].4, "{incidents:?}");
    assert_eq!(notices_of(&pool, &p.project).await.len(), 7);
}

/// A pause closes the open incident as paused, telling so, and a resume
/// opens none: a paused monitor is its owner's.
#[sqlx::test]
async fn a_pause_closes_the_open_incident_as_paused(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    p.ping_ok(&format!("/{key}/hourly-sync/fail?exit_code=2"))
        .await;
    assert_eq!(incidents_of(&pool, &p.project).await.len(), 1);

    let (status, paused) = p
        .send(p.request("POST", &format!("{MONITORS}/hourly-sync/pause"), None))
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    let incidents = incidents_of(&pool, &p.project).await;
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].3.as_deref(), Some("paused"), "{incidents:?}");
    let notices = notices_of(&pool, &p.project).await;
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert_eq!(notices[1]["kind"], "incident_resolved");
    assert_eq!(notices[1]["title"], "hourly-sync paused");
    assert_eq!(notices[1]["metadata"]["resolution"], "paused");
    assert_eq!(notices[1]["metadata"]["cause"], "failed");

    let (status, resumed) = p
        .send(p.request("POST", &format!("{MONITORS}/hourly-sync/resume"), None))
        .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["state"], "healthy");
    assert_eq!(incidents_of(&pool, &p.project).await.len(), 1);
    assert_eq!(notices_of(&pool, &p.project).await.len(), 2);
}

/// A checkpoint's steps and spend are totals so far, which never fall: one
/// lower than the run holds — an older checkpoint sent again, a retried
/// curl — leaves it where it stands, each total on its own, so the run ends
/// with the most it reported whatever order its checkpoints came in.
#[sqlx::test]
async fn a_checkpoint_never_moves_its_runs_progress_back(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    let run = rid();
    let checkpoint = |query: &str| format!("/{key}/hourly-sync/checkpoint?rid={run}&{query}");

    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    p.ping_ok(&checkpoint("steps=4&cost=0.5")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "over_bound");

    // The checkpoint before it, sent again: neither total falls, and the
    // monitor stays past its bound.
    p.ping_ok(&checkpoint("steps=2&cost=0.25")).await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(
        (runs[0].1, runs[0].2),
        (4, 0.5),
        "an older checkpoint moved the run's progress back"
    );
    assert_eq!(p.monitor("hourly-sync").await["state"], "over_bound");

    // Each total on its own: the one that grew is kept beside the one that
    // did not, and one not sent stands.
    p.ping_ok(&checkpoint("steps=3&cost=0.75")).await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!((runs[0].1, runs[0].2), (4, 0.75));
    p.ping_ok(&checkpoint("steps=6")).await;
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!((runs[0].1, runs[0].2), (6, 0.75));

    // The run ends with the most it reported, an older checkpoint sent last.
    p.ping_ok(&checkpoint("steps=5&cost=0.5")).await;
    p.ping_ok(&format!("/{key}/hourly-sync?rid={run}")).await;
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["steps"], 6);
    assert_eq!(finished[0]["cost_usd"], 0.75);
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
    let heard = db::hear_open_run(&mut tx, &p.project, namesake, hourly, None, None)
        .await
        .expect("a write");
    assert!(
        heard.is_none(),
        "another monitor's run was heard as this one's"
    );
    let heard = db::hear_open_run(&mut tx, &p.project, namesake, other, None, None)
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

/// A monitor deleted as its run ends waits for the ping, which holds the
/// monitor from its read before it takes any of its runs, as every writer of
/// both takes the monitor first: so the two never wait on each other in a
/// circle, which Postgres breaks by failing one of them, a 500 for the
/// crontab or for the console. The ping is answered `OK` and files its run,
/// the delete goes through and is recorded, a run a `/start` opened as its
/// monitor went goes with it, and another monitor's run stays.
#[sqlx::test]
async fn a_monitor_deleted_as_its_run_ends_waits_on_no_ping_in_a_circle(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    p.make(json!({
        "name": "Other sync",
        "slug": "other-sync",
        "interval_s": 3600,
        "grace_s": 60,
    }))
    .await;
    let survivor = rid();
    p.ping_ok(&format!("/{key}/other-sync/start?rid={survivor}"))
        .await;
    let left = |runs: Vec<(Uuid, i32, f64)>| -> Vec<String> {
        runs.into_iter().map(|(id, _, _)| id.to_string()).collect()
    };

    // An end ping, its run's row deleted, waits to file the run while the
    // delete comes for the monitor.
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    let (ended, deleted) = p
        .ping_as_its_monitor_is_deleted(
            &pool,
            &format!("/{key}/hourly-sync?rid={run}"),
            "hourly-sync",
        )
        .await;
    assert_eq!((ended.0, ended.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(deleted, StatusCode::NO_CONTENT);
    assert_eq!(monitors_of(&pool, &p.project).await, 1, "the other monitor");
    assert_eq!(
        left(open_runs(&pool, &p.project).await),
        std::slice::from_ref(&survivor)
    );
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["run_id"].as_str(), Some(run.as_str()));
    assert_eq!(finished[0]["status"], "ok");

    // A bare `/start` waits to file the run it found open, ended as
    // `error`, while the delete comes for the monitor: the run it opens
    // then goes with the monitor.
    p.make(hourly()).await;
    p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
    let crashed: Vec<String> = left(open_runs(&pool, &p.project).await)
        .into_iter()
        .filter(|id| *id != survivor)
        .collect();
    assert_eq!(crashed.len(), 1);
    let (started, deleted) = p
        .ping_as_its_monitor_is_deleted(&pool, &format!("/{key}/hourly-sync/start"), "hourly-sync")
        .await;
    assert_eq!((started.0, started.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(deleted, StatusCode::NO_CONTENT);
    assert_eq!(monitors_of(&pool, &p.project).await, 1, "the other monitor");
    assert_eq!(
        left(open_runs(&pool, &p.project).await),
        [survivor],
        "the run the start opened outlived its monitor, or another's went"
    );
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 2);
    assert_eq!(finished[1]["run_id"].as_str(), Some(crashed[0].as_str()));
    assert_eq!(finished[1]["status"], "error");

    let actions: Vec<String> = recorded(&pool, &p.organization)
        .await
        .into_iter()
        .map(|(action, _, _)| action)
        .collect();
    assert_eq!(
        actions,
        ["created", "created", "deleted", "created", "deleted"]
    );
}

/// A monitor's delete waits for a ping in flight, which holds the monitor
/// from its read before it takes any of its runs: here a `/start` held up on
/// a run another transaction is opening under the id it names, for another
/// monitor, the delete sent meanwhile. Once the other lets go, the ping opens
/// its run and is answered `OK`, and the delete goes through, the run with
/// it — where a ping that held nothing of its monitor's would let a delete
/// take the monitor while the ping went on to the monitor's runs.
#[sqlx::test]
async fn a_monitors_delete_waits_for_the_pings_holding_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    let other = p
        .make(json!({
            "name": "Other sync",
            "slug": "other-sync",
            "interval_s": 3600,
            "grace_s": 60,
        }))
        .await;
    let other = Uuid::parse_str(other["id"].as_str().expect("an id")).expect("a monitor's id");

    // The other monitor's run, opened under the id the ping names and not
    // yet committed: the ping waits on it holding its own monitor alone.
    let run = rid();
    let mut opener = pool.begin().await.expect("the opener's transaction");
    let opener_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *opener)
        .await
        .expect("the opener's session");
    sqlx::query(
        "INSERT INTO telemetry.open_runs (id, project_id, organization_id, monitor_id, agent, \
             source, ends_by)
         VALUES ($1, $2, $3, $4, 'other-sync', 'ping', now() + interval '1 day')",
    )
    .bind(Uuid::parse_str(&run).expect("a run's id"))
    .bind(&p.project)
    .bind(&p.organization)
    .bind(other)
    .execute(&mut *opener)
    .await
    .expect("a run being opened");
    let path = format!("/{key}/hourly-sync/start?rid={run}");
    let delete = p.request("DELETE", &format!("{MONITORS}/hourly-sync"), None);
    let (started, deleted) = tokio::join!(p.ping(&path), async {
        let ping_pid =
            waiting_on(&pool, opener_pid, "the ping waits on the run being opened").await;
        let ((deleted, _), ()) = tokio::join!(p.send(delete), async {
            waiting_on(&pool, ping_pid, "the delete waits on the ping").await;
            opener.rollback().await.expect("the other lets go");
        });
        deleted
    });
    assert_eq!((started.0, started.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(deleted, StatusCode::NO_CONTENT);
    assert_eq!(monitors_of(&pool, &p.project).await, 1, "the other monitor");
    assert!(
        open_runs(&pool, &p.project).await.is_empty(),
        "the run the ping opened outlived its monitor"
    );
}

/// A ping that comes as a monitor's delete holds the monitor, its runs on
/// the way out, waits for the delete holding none of them, and finds no
/// monitor by its slug: it makes the monitor anew, as a first ping does, and
/// is answered `OK` — an end ping and a `/start` alike — where a ping that
/// read its monitor without holding it would go on to its runs beside the
/// delete: an end ping to meet the cascade on the run it ends, in a circle,
/// and a `/start` to fail its run's foreign key on a monitor gone since it
/// read it.
#[sqlx::test]
async fn a_ping_beside_its_monitors_delete_waits_and_makes_it_anew(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;

    // An end ping for one of the monitor's runs, the delete held up on
    // another: a run of no duration, filed under the monitor made anew.
    let (ended, held) = (rid(), rid());
    for run in [&ended, &held] {
        p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
            .await;
    }
    let (answer, deleted) = p
        .ping_as_a_delete_holds_its_monitor(
            &pool,
            &format!("/{key}/hourly-sync?rid={ended}"),
            "hourly-sync",
            &held,
        )
        .await;
    assert_eq!((answer.0, answer.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(deleted, StatusCode::NO_CONTENT);
    assert_eq!(p.monitor("hourly-sync").await["state"], "new");
    assert!(open_runs(&pool, &p.project).await.is_empty());
    let finished = finished_runs(&pool, &p.project).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["run_id"].as_str(), Some(ended.as_str()));
    assert_eq!(finished[0]["started_at"], finished[0]["ended_at"]);

    // A `/start` naming a run of its own, as the monitor made anew is
    // deleted in turn: its run opened under the monitor made after it.
    let (other, held, opened) = (rid(), rid(), rid());
    for run in [&other, &held] {
        p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
            .await;
    }
    let (answer, deleted) = p
        .ping_as_a_delete_holds_its_monitor(
            &pool,
            &format!("/{key}/hourly-sync/start?rid={opened}"),
            "hourly-sync",
            &held,
        )
        .await;
    assert_eq!((answer.0, answer.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(deleted, StatusCode::NO_CONTENT);
    assert_eq!(p.monitor("hourly-sync").await["state"], "new");
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0.to_string(), opened);

    let actions: Vec<String> = recorded(&pool, &p.organization)
        .await
        .into_iter()
        .map(|(action, _, _)| action)
        .collect();
    assert_eq!(
        actions,
        ["created", "deleted", "created", "deleted", "created"]
    );
}

/// A ping settles its monitor as it stands once locked, not as it read it:
/// a pause, a save, a rename or another ping's settling committed in between
/// is kept, never undone.
#[sqlx::test]
async fn a_ping_settles_its_monitor_as_it_stands_not_as_it_read_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    let monitor = format!("{MONITORS}/hourly-sync");
    let pause = || p.request("POST", &format!("{monitor}/pause"), None);
    let resume = || p.request("POST", &format!("{monitor}/resume"), None);
    let saved = |interval_s: i32, name: &str| {
        let mut saved = hourly();
        saved["interval_s"] = json!(interval_s);
        saved["name"] = json!(name);
        p.request("PUT", &monitor, Some(saved))
    };

    // An end ping as its monitor is paused: the run filed, the monitor left
    // paused and due at no time.
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    let (ended, changed) = p
        .ping_as_its_monitor_changes(
            &pool,
            &format!("/{key}/hourly-sync?rid={run}"),
            "outbox",
            vec![pause()],
        )
        .await;
    assert_eq!((ended.0, ended.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(changed[0].0, StatusCode::OK, "{}", changed[0].1);
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "paused", "{settled}");
    assert_eq!(settled["next_due_at"], Value::Null, "{settled}");
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 1);

    // A `/start` as it is paused again: the run opened, the monitor left
    // paused.
    let (resumed, body) = p.send(resume()).await;
    assert_eq!(resumed, StatusCode::OK, "{body}");
    let run = rid();
    let (started, changed) = p
        .ping_as_its_monitor_changes(
            &pool,
            &format!("/{key}/hourly-sync/start?rid={run}"),
            "open_runs",
            vec![pause()],
        )
        .await;
    assert_eq!((started.0, started.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(changed[0].0, StatusCode::OK, "{}", changed[0].1);
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "paused", "{settled}");
    assert_eq!(settled["next_due_at"], Value::Null, "{settled}");
    assert_eq!(open_runs(&pool, &p.project).await.len(), 1);

    // Its end, as a pause is still being written when the ping comes to
    // settle the monitor: the ping waits for the pause, and leaves it be.
    let (resumed, body) = p.send(resume()).await;
    assert_eq!(resumed, StatusCode::OK, "{body}");
    let (ended, paused) = p
        .ping_as_a_pause_commits(
            &pool,
            &format!("/{key}/hourly-sync?rid={run}"),
            "hourly-sync",
        )
        .await;
    assert_eq!((ended.0, ended.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(paused, StatusCode::OK);
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "paused", "{settled}");
    assert_eq!(settled["next_due_at"], Value::Null, "{settled}");
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 2);

    // A run's end, as the monitor's timing moves from hourly to daily: due a
    // day on from the end, not an hour.
    let (resumed, body) = p.send(resume()).await;
    assert_eq!(resumed, StatusCode::OK, "{body}");
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    let (ended, changed) = p
        .ping_as_its_monitor_changes(
            &pool,
            &format!("/{key}/hourly-sync?rid={run}"),
            "outbox",
            vec![saved(86_400, "Hourly sync")],
        )
        .await;
    assert_eq!((ended.0, ended.1.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(changed[0].0, StatusCode::OK, "{}", changed[0].1);
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "healthy", "{settled}");
    let due_in = at(&settled["next_due_at"]) - Utc::now();
    assert!(due_in > chrono::Duration::hours(23), "{settled}");

    // A `/start` as it moves back to hourly: due when the save said.
    let run = rid();
    let (started, changed) = p
        .ping_as_its_monitor_changes(
            &pool,
            &format!("/{key}/hourly-sync/start?rid={run}"),
            "open_runs",
            vec![saved(3_600, "Hourly sync")],
        )
        .await;
    assert_eq!((started.0, started.1.as_str()), (StatusCode::OK, "OK"));
    let (saved_hourly, body) = &changed[0];
    assert_eq!(*saved_hourly, StatusCode::OK, "{body}");
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "running", "{settled}");
    assert_eq!(settled["next_due_at"], body["next_due_at"], "{settled}");

    // Its end, as the monitor is renamed: settled as if nothing had changed,
    // due an hour on from the end.
    let (ended, changed) = p
        .ping_as_its_monitor_changes(
            &pool,
            &format!("/{key}/hourly-sync?rid={run}"),
            "outbox",
            vec![saved(3_600, "Hourly sync, renamed")],
        )
        .await;
    assert_eq!((ended.0, ended.1.as_str()), (StatusCode::OK, "OK"));
    let (renamed, body) = &changed[0];
    assert_eq!(*renamed, StatusCode::OK, "{body}");
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["name"], "Hourly sync, renamed");
    assert_eq!(settled["state"], "healthy", "{settled}");
    assert!(
        at(&settled["next_due_at"]) > at(&body["next_due_at"]),
        "{settled}"
    );
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 4);

    // A `/fail` as the monitor is paused and resumed: failed, its clocks
    // after the resume's.
    let run = rid();
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
        .await;
    let (failed, changed) = p
        .ping_as_its_monitor_changes(
            &pool,
            &format!("/{key}/hourly-sync/fail?rid={run}"),
            "outbox",
            vec![pause(), resume()],
        )
        .await;
    assert_eq!((failed.0, failed.1.as_str()), (StatusCode::OK, "OK"));
    for (status, body) in &changed {
        assert_eq!(*status, StatusCode::OK, "{body}");
    }
    let resumed = &changed[1].1;
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "failed", "{settled}");
    assert!(
        at(&settled["state_changed_at"]) > at(&resumed["state_changed_at"]),
        "{settled}"
    );
    assert!(
        at(&settled["updated_at"]) > at(&resumed["updated_at"]),
        "{settled}"
    );
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 5);

    // A `/start` as another ping's settling moves the due time on: it keeps
    // the moved due time, not the one it read.
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    let id = Uuid::parse_str(
        p.monitor("hourly-sync").await["id"]
            .as_str()
            .expect("an id"),
    )
    .expect("a monitor's id");
    let opened = rid();
    let (started, due) = p
        .ping_as_its_run_is_opened(
            &pool,
            &format!("/{key}/hourly-sync/start?rid={opened}"),
            &opened,
            id,
            async {
                p.ping_ok(&format!("/{key}/hourly-sync")).await;
                p.monitor("hourly-sync").await["next_due_at"].clone()
            },
        )
        .await;
    assert_eq!((started.0, started.1.as_str()), (StatusCode::OK, "OK"));
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "running", "{settled}");
    assert_eq!(settled["next_due_at"], due, "{settled}");
    assert_eq!(finished_runs(&pool, &p.project).await.len(), 7);

    // A `/start` as another run's checkpoint takes the monitor past its step
    // cap: it stays over its bound.
    for (status, body) in [p.send(pause()).await, p.send(resume()).await] {
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let next = rid();
    let (started, ()) = p
        .ping_as_its_run_is_opened(
            &pool,
            &format!("/{key}/hourly-sync/start?rid={next}"),
            &next,
            id,
            p.ping_ok(&format!(
                "/{key}/hourly-sync/checkpoint?rid={opened}&steps=4"
            )),
        )
        .await;
    assert_eq!((started.0, started.1.as_str()), (StatusCode::OK, "OK"));
    let settled = p.monitor("hourly-sync").await;
    assert_eq!(settled["state"], "over_bound", "{settled}");
    assert_eq!(open_runs(&pool, &p.project).await.len(), 2);
}

/// A console save or pause is written over the monitor as a ping settling
/// meanwhile left it: a kept timing keeps the due time the ping moved to, a
/// pause records the state the ping left, and no clock goes back.
#[sqlx::test]
async fn a_change_the_console_saves_is_written_over_the_monitor_as_a_ping_left_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    let monitor = format!("{MONITORS}/hourly-sync");
    let saved = |interval_s: i32, name: &str| {
        let mut saved = hourly();
        saved["interval_s"] = json!(interval_s);
        saved["name"] = json!(name);
        p.request("PUT", &monitor, Some(saved))
    };
    // A run come at its due time, five minutes gone, and not yet ended.
    let started = || async {
        p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
        sqlx::query(
            "UPDATE telemetry.monitors SET next_due_at = now() - interval '5 minutes'
              WHERE project_id = $1",
        )
        .bind(&p.project)
        .execute(&pool)
        .await
        .expect("the monitor due five minutes ago");
    };

    // A rename as the run's end settles the monitor: due when the ping said.
    started().await;
    let due = Utc::now() + chrono::Duration::hours(1);
    let ((status, renamed), settled) = p
        .change_as_a_ping_settles(
            &pool,
            "hourly-sync",
            saved(3_600, "Hourly sync, renamed"),
            due,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Hourly sync, renamed");
    assert_eq!(renamed["state"], "healthy", "{renamed}");
    assert_eq!(
        Some(at(&renamed["next_due_at"])),
        settled.next_due_at,
        "{renamed}"
    );
    assert!(
        at(&renamed["updated_at"]) >= settled.updated_at,
        "{renamed}"
    );
    assert_eq!(p.monitor("hourly-sync").await, renamed);

    // A save that moves it to daily as the next run's end settles it: due a
    // day on, under the timing saved.
    started().await;
    let ((status, daily), settled) = p
        .change_as_a_ping_settles(
            &pool,
            "hourly-sync",
            saved(86_400, "Hourly sync, renamed"),
            Utc::now() + chrono::Duration::hours(1),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{daily}");
    assert_eq!(daily["state"], "healthy", "{daily}");
    let due_in = at(&daily["next_due_at"]) - Utc::now();
    assert!(due_in > chrono::Duration::hours(23), "{daily}");
    assert!(at(&daily["updated_at"]) >= settled.updated_at, "{daily}");

    // A pause as the next run's end settles it: paused from healthy, its
    // clocks no older than the ping's.
    started().await;
    let pause = p.request("POST", &format!("{monitor}/pause"), None);
    let ((status, paused), settled) = p
        .change_as_a_ping_settles(&pool, "hourly-sync", pause, due)
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["state"], "paused", "{paused}");
    assert_eq!(paused["next_due_at"], Value::Null, "{paused}");
    assert!(
        at(&paused["state_changed_at"]) >= settled.state_changed_at,
        "{paused}"
    );
    assert!(at(&paused["updated_at"]) >= settled.updated_at, "{paused}");
    let (action, _, metadata) = recorded(&pool, &p.organization)
        .await
        .pop()
        .expect("the pause's record");
    assert_eq!(action, "updated");
    assert_eq!(
        metadata["state"],
        json!({ "from": "healthy", "to": "paused" }),
        "{metadata}"
    );
}

/// A ping a move kept out files its rows where the project went, and is held
/// to the new organization's standing, asked with nothing held.
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

    // Moved into an organization being deleted: refused as an unknown key,
    // and nothing written.
    let closing = OrganizationId::new();
    let (t, auth) = project_and_auth(&pool);
    auth.stand(&closing, Some(OrganizationStatus::PendingDeletion));
    let key = t.mint().await;
    let (status, body) = t
        .ping_resolved_before_a_move(&pool, &format!("/{key}/nightly-report"), &closing)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(monitors_of(&pool, &t.project).await, 0);
    assert!(outbox_of(&pool, &t.project).await.is_empty());
    let held = auth.standing_held();
    assert_eq!(
        held.len(),
        2,
        "the standings of the organization the key named and of the one the project went to: {held:?}"
    );
    assert_eq!(
        held[1], 0,
        "the new organization's standing asked holding a lock"
    );

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

/// A move waits for the pings in flight and takes their rows with the rest,
/// a first ping waiting on its chain lock included, in no circle.
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

/// Pings a move keeps out hold no connection while they wait: more of them
/// than the pool holds leave the move's own work the connection it needs,
/// and each is answered once the move lets go, filed where the project went.
/// They wait a minute here, not two seconds, so a busy machine cannot make
/// one give up.
#[sqlx::test]
async fn pings_a_move_keeps_out_hold_no_connection(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let elsewhere = OrganizationId::new();
    let p = project_waiting_out_moves(&pool, Duration::from_secs(60));
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
    // Looked at while the pings are kept out, over and over.
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
    let moved = tokio::time::timeout(Duration::from_secs(20), async {
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

/// A ping that reads no run under the `rid` it names, and finds one opened
/// beside it when it goes to open its own — by a copy of the same `/start`,
/// a retry sent while the first was still being answered, or by the
/// `/start` or `/checkpoint` it raced, in either order — hears that run as a
/// ping that read it open would: both are answered `OK`, one run is open, a
/// checkpoint's progress is heard on it, and the monitor's state follows the
/// run as it stands, past its cap or not, where the second was refused as
/// though the run were another monitor's. A run opened beside it under
/// another of the project's monitors is that refusal still.
#[sqlx::test]
async fn a_ping_racing_its_runs_first_ping_hears_the_run(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    p.make(json!({
        "name": "Other sync",
        "slug": "other-sync",
        "interval_s": 3600,
        "grace_s": 60,
    }))
    .await;
    let ok = |answer: &(StatusCode, String)| answer.0 == StatusCode::OK && answer.1 == "OK";

    // A `/start` sent again while its first copy is still being answered:
    // the copy hears the run, at its own later clock.
    let retried = rid();
    let start = format!("/{key}/hourly-sync/start?rid={retried}");
    let (first, second) = p.pings_racing(&pool, &start, &start).await;
    assert!(ok(&first), "{first:?}");
    assert!(ok(&second), "the copy was refused: {second:?}");
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1, "one run, not a second");
    assert_eq!(runs[0].0.to_string(), retried);
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    let (started, heard) = clocks_of(&pool, &p.project, &retried).await;
    assert!(heard > started, "the copy did not hear the run");

    // A hearing is stamped by the database's clock as it is written, so a
    // copy's write that lands after a later ping's never moves the run's
    // last hearing back, and its start, the server's clock, stands.
    let hourly = Uuid::parse_str(
        p.monitor("hourly-sync").await["id"]
            .as_str()
            .expect("an id"),
    )
    .expect("a monitor's id");
    let mut tx = telmoni_shared::db::tenant_session::project_scope(&pool, &p.project)
        .await
        .expect("a scope");
    db::hear_open_run(
        &mut tx,
        &p.project,
        Uuid::parse_str(&retried).expect("a run's id"),
        hourly,
        None,
        None,
    )
    .await
    .expect("a write")
    .expect("the run");
    tx.commit().await.expect("committed");
    let (started_again, heard_again) = clocks_of(&pool, &p.project, &retried).await;
    assert_eq!(started_again, started, "the start moved");
    assert!(heard_again >= heard, "the run's last hearing moved back");
    p.ping_ok(&format!("/{key}/hourly-sync?rid={retried}"))
        .await;

    // A `/checkpoint` sent before its `/start` has committed: heard on the
    // run the `/start` opened, with what it reports.
    let overtaken = rid();
    let (first, second) = p
        .pings_racing(
            &pool,
            &format!("/{key}/hourly-sync/start?rid={overtaken}"),
            &format!("/{key}/hourly-sync/checkpoint?rid={overtaken}&steps=4&cost=0.5"),
        )
        .await;
    assert!(ok(&first), "{first:?}");
    assert!(ok(&second), "the checkpoint was refused: {second:?}");
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1, "one run, not a second");
    assert_eq!(runs[0].0.to_string(), overtaken);
    assert_eq!(
        (runs[0].1, runs[0].2),
        (4, 0.5),
        "the checkpoint's progress"
    );
    assert_eq!(p.monitor("hourly-sync").await["state"], "over_bound");
    p.ping_ok(&format!("/{key}/hourly-sync?rid={overtaken}"))
        .await;

    // The `/start` a checkpoint overtook, the run past its step cap: heard on
    // the run the checkpoint opened, the monitor left past its bound, where
    // a `/start` going by the monitor as it read it would set it running.
    let reversed = rid();
    let (first, second) = p
        .pings_racing(
            &pool,
            &format!("/{key}/hourly-sync/checkpoint?rid={reversed}&steps=4"),
            &format!("/{key}/hourly-sync/start?rid={reversed}"),
        )
        .await;
    assert!(ok(&first), "{first:?}");
    assert!(ok(&second), "the start was refused: {second:?}");
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1, "one run, not a second");
    assert_eq!((runs[0].0.to_string(), runs[0].1), (reversed.clone(), 4));
    assert_eq!(p.monitor("hourly-sync").await["state"], "over_bound");
    p.ping_ok(&format!("/{key}/hourly-sync?rid={reversed}"))
        .await;

    // The same without a race: a `/start` sent again for a run past its cap,
    // once another run's end has set the monitor healthy, puts it back past
    // its bound, as the run's next checkpoint would.
    let (capped, beside) = (rid(), rid());
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={capped}"))
        .await;
    p.ping_ok(&format!(
        "/{key}/hourly-sync/checkpoint?rid={capped}&steps=4"
    ))
    .await;
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={beside}"))
        .await;
    p.ping_ok(&format!("/{key}/hourly-sync?rid={beside}")).await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "healthy");
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={capped}"))
        .await;
    assert_eq!(p.monitor("hourly-sync").await["state"], "over_bound");
    p.ping_ok(&format!("/{key}/hourly-sync?rid={capped}")).await;

    // Opened beside it under another monitor, the run is that monitor's:
    // the 409, and the run stands as the first ping opened it.
    let taken = rid();
    let (first, second) = p
        .pings_racing(
            &pool,
            &format!("/{key}/hourly-sync/start?rid={taken}"),
            &format!("/{key}/other-sync/start?rid={taken}"),
        )
        .await;
    assert!(ok(&first), "{first:?}");
    assert_eq!(second.0, StatusCode::CONFLICT, "{second:?}");
    assert!(second.1.contains("under another monitor"), "{second:?}");
    let runs = open_runs(&pool, &p.project).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0.to_string(), taken);
    assert_eq!(p.monitor("hourly-sync").await["state"], "running");
    assert_eq!(p.monitor("other-sync").await["state"], "healthy");
    assert_eq!(
        finished_runs(&pool, &p.project).await.len(),
        5,
        "the five runs ended, and nothing else"
    );
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
        // Logged once for the ping that ends the run, however its settling
        // goes: not again for the row it holds, where nothing moved it.
        let (capturing, logs) = telmoni_shared::test_util::captured_logs();
        p.ping_ok(&format!("/{key}/hourly-sync{end}")).await;
        drop(capturing);
        assert_eq!(
            logs.text()
                .matches("a saved monitor's timing cannot be read")
                .count(),
            1,
            "{unreadable}"
        );
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

/// A ping tells the organization's open consoles that the project's monitors
/// changed — on the channel the console subscribes to, once the ping is in,
/// naming the project and nothing of the ping; a refused ping tells nothing.
#[sqlx::test]
async fn a_ping_tells_the_organizations_open_consoles_of_its_monitors(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = OrganizationId::new();
    let project = ProjectId::new();
    let auth = AuthStub::probing(&pool);
    auth.seat(
        OWNER,
        &organization,
        &project,
        OrganizationRole::Owner,
        None,
    );
    let notices = NoticeLog::new();
    let state = common::state_with_redis_and_notices(&pool, auth, notices.clone());
    if state.redis.connection().await.is_none() {
        eprintln!(
            "skipping {}: Redis is not reachable (REDIS_URL, or the compose file's)",
            module_path!()
        );
        return;
    }
    let p = Project::over(state, notices, organization, project);
    let key = p.mint().await;

    // The console's subscription (`web/lib/events/subscriber.ts`): before the
    // ping, on the organization's channel, RESP3 as a push message needs.
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

    p.ping_ok(&format!("/{key}/nightly-report")).await;
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
    assert_eq!(
        nudge["data"].as_object().map(serde_json::Map::len),
        Some(2),
        "the project, and nothing of the ping"
    );

    let unknown = format!("{PING_KEY_PREFIX}{}", "0".repeat(PING_KEY_RANDOM_LEN));
    let (status, _) = p.ping(&format!("/{unknown}/nightly-report")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), heard.recv())
            .await
            .is_err(),
        "a refused ping told the console"
    );
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
        if status == StatusCode::METHOD_NOT_ALLOWED {
            assert_eq!(
                response
                    .headers()
                    .get("allow")
                    .and_then(|v| v.to_str().ok()),
                Some("GET, HEAD, POST"),
                "{method} {sent}"
            );
        }
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

/// A path axum cannot decode is refused as the lane refuses: a slug that is
/// no slug gets its 400 problem, echoing none of it, a key the door's 404 —
/// never axum's plain text — and nothing is recorded.
#[sqlx::test]
async fn a_path_that_does_not_decode_is_refused_as_the_lane_refuses(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let shape = format!(
        "a monitor's slug is lowercase letters and digits joined by single hyphens, up to {} \
         characters",
        telmoni_shared::slug::MAX_LEN
    );
    for (sent, status, detail) in [
        (
            format!("/{key}/%FF"),
            StatusCode::BAD_REQUEST,
            shape.as_str(),
        ),
        (
            format!("/{key}/%FF/start"),
            StatusCode::BAD_REQUEST,
            shape.as_str(),
        ),
        (
            "/%FF/nightly-report".to_owned(),
            StatusCode::NOT_FOUND,
            "no ping key by that value",
        ),
    ] {
        let (got, media_type, body) = p.send_ping("GET", &sent, &[], Body::empty()).await;
        assert_eq!(got, status, "{sent}: {body}");
        assert!(
            media_type.starts_with("application/problem+json"),
            "{sent}: {media_type}"
        );
        let problem: Value = serde_json::from_str(&body).expect("a problem document");
        assert_eq!(problem["detail"], detail, "{sent}");
        assert!(
            !body.contains("Invalid UTF-8"),
            "{sent}: axum's words: {body}"
        );
    }
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
    assert!(finished_runs(&pool, &p.project).await.is_empty());
}

/// Past the plan's limit a ping to a slug no monitor has is refused with the
/// ceiling's own 402 — what `curl -f` turns into cron's mail — naming what
/// lifts it and never the key; nothing is made or recorded, and no notice is
/// raised for it: the answer is the message, and the owner's is the day's.
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

/// A ping for an organization being deleted, or gone, is refused as an
/// unknown key and writes nothing; auth unable to say refuses nothing, so
/// the lane stays up through auth's outage.
#[sqlx::test]
async fn a_ping_for_an_organization_being_deleted_is_refused_as_an_unknown_key(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report/start")).await;
    let waiting = outbox_of(&pool, &p.project).await;
    assert_eq!(waiting.len(), 1, "the first ping's notice: {waiting:?}");

    for standing in [Some(OrganizationStatus::PendingDeletion), None] {
        auth.stand(&p.organization, standing);
        for path in [
            format!("/{key}/nightly-report"),
            format!("/{key}/nightly-report/fail?exit_code=1"),
            format!("/{key}/weekly-digest"),
            format!("/{key}/Weekly_Digest"),
            format!("/{key}/nightly-report/checkpoint?steps=many"),
        ] {
            let (status, body) = p.ping(&path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{standing:?} {path}: {body}");
            assert!(
                body.contains("no ping key by that value"),
                "{standing:?} {path}: {body}"
            );
        }
    }
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
    assert_eq!(
        open_runs(&pool, &p.project).await.len(),
        1,
        "a ping for a closed organization ended its run"
    );
    assert_eq!(
        outbox_of(&pool, &p.project).await,
        waiting,
        "a ping for a closed organization wrote to the outbox"
    );

    auth.stand(&p.organization, Some(OrganizationStatus::PendingDeletion))
        .failing_standing(true);
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    assert!(
        open_runs(&pool, &p.project).await.is_empty(),
        "a ping auth could not place was refused"
    );
    // Nor one auth answers past its second: the answer is not waited for.
    auth.failing_standing(false)
        .delaying_standing(Duration::from_secs(3));
    let started = std::time::Instant::now();
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "a ping waited out auth past its second"
    );
    let held = auth.standing_held();
    assert_eq!(held.len(), 13, "one ask a ping: {held:?}");
    assert!(
        held.iter().all(|&locks| locks == 0),
        "the standing asked holding a lock: {held:?}"
    );
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
/// time — five seconds, doubled each try, five minutes at most — and taken
/// by no tick within its wait, counted, and after its twelfth try left —
/// nothing takes it, the depth counts it stuck — until an operator's
/// `release outbox` gives it its tries back, which it spends again.
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

    let waits = [5, 10, 20, 40, 80, 160, 300, 300, 300, 300, 300, 300];
    for (attempt, wait) in (1..=12).zip(waits) {
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
        let held = held_for(&pool, id).await;
        assert!(
            (wait - 2..=wait).contains(&held),
            "try {attempt}: held for {held}s, not {wait}s"
        );
        assert_eq!(
            outbox::tick_once(&p.state).await.expect("a tick"),
            Settled::default(),
            "try {attempt}: taken within its wait"
        );
        assert_eq!(tries_of(&pool, &p.project).await, [(attempt, true)]);
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

/// A notice, and the run beside it, go nowhere once their organization is
/// being deleted, or is gone, since they would outlive its purges.
#[sqlx::test]
async fn a_notice_for_an_organization_being_deleted_is_dropped(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (p, auth) = project_and_auth(&pool);
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;

    auth.stand(&p.organization, Some(OrganizationStatus::PendingDeletion));
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 2, held: 0 },
        "the notice and the run's row dropped as landed"
    );
    assert!(
        p.notices.notices().is_empty(),
        "a notice was written to a feed the deletion purges"
    );
    assert!(kinds_waiting(&pool, &p.project).await.is_empty());

    // Pinged while the organization stood, and landed once it is gone.
    auth.stand(&p.organization, Some(OrganizationStatus::Active));
    p.ping_ok(&format!("/{key}/weekly-digest")).await;
    auth.stand(&p.organization, None);
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 2, held: 0 }
    );
    assert!(
        p.notices.notices().is_empty(),
        "a notice was written for an organization that is gone"
    );
    assert!(kinds_waiting(&pool, &p.project).await.is_empty());
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

    // Said where it is to the batch's run and to the notice's ask, then
    // moved as the notice is written.
    auth.move_after(&p.project, &OrganizationId::new(), 2);
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

    // Asked once for the batch's runs, which land first, and once for its
    // first notice, never for its second.
    auth.failing_homes(true);
    let asked = auth.homes_asked();
    assert_eq!(
        outbox::tick_once(&p.state).await.expect("a tick"),
        Settled { landed: 0, held: 4 }
    );
    assert_eq!(
        auth.homes_asked() - asked,
        2,
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
        2,
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

    // Said where it is to the batch's run, which lands first, and to the
    // notice's ask, then gone as the notice is written.
    auth.forget_after(&p.project, 2);
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

    // Answered for the batch's run and for the notice's ask, then lost once
    // the notice is written.
    auth.failing_homes_after(2);
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

    // Said where it is to the batch's run and to the notice's ask, then
    // gone as the notice is written.
    auth.forget_after(&p.project, 2);
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

/// A project of `organization`'s, seated for its owner, over `state` and
/// the `auth` behind it: two of a suite's projects under one loop.
fn project_over(
    state: &Arc<AppState>,
    auth: &AuthStub,
    notices: &Arc<NoticeLog>,
    organization: &OrganizationId,
) -> Project {
    let project = ProjectId::new();
    auth.seat(OWNER, organization, &project, OrganizationRole::Owner, None);
    Project::over(
        Arc::clone(state),
        Arc::clone(notices),
        organization.clone(),
        project,
    )
}

/// A run lands nowhere once its organization is being deleted, or its
/// project is gone, whose spans are then marked deleted again.
#[sqlx::test]
async fn a_run_whose_organization_is_closing_or_whose_project_is_gone_lands_nowhere(pool: PgPool) {
    let Some(clickhouse) = common::clickhouse_or_skip(
        "a_run_whose_organization_is_closing_or_whose_project_is_gone_lands_nowhere",
    )
    .await
    else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let notices = NoticeLog::new();
    let state = common::state_with_stores_and_notices(&pool, auth.clone(), notices.clone());

    let p = project_over(&state, &auth, &notices, &OrganizationId::new());
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report")).await;
    auth.stand(&p.organization, Some(OrganizationStatus::PendingDeletion));
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 2, held: 0 },
        "the notice and the run dropped as landed"
    );
    assert!(outbox_of(&pool, &p.project).await.is_empty());
    assert_eq!(
        common::rows_of(&clickhouse.store, &p.project).await,
        (0, 0),
        "a run landed for an organization being deleted"
    );

    let q = project_over(&state, &auth, &notices, &OrganizationId::new());
    let key = q.mint().await;
    q.ping_ok(&format!("/{key}/nightly-report")).await;
    // The earlier try: the run written, then cut off with its row leased.
    let earlier: Vec<SpanRow> = finished_runs(&pool, &q.project)
        .await
        .into_iter()
        .map(|run| {
            serde_json::from_value::<FinishedRun>(run)
                .expect("a finished run")
                .row(&q.project)
        })
        .collect();
    assert_eq!(earlier.len(), 1);
    clickhouse
        .store
        .insert_spans(&earlier)
        .await
        .expect("the earlier try's write");
    assert_eq!(common::rows_of(&clickhouse.store, &q.project).await, (1, 1));

    auth.forget(&q.project);
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 2, held: 0 },
        "the notice and the run dropped as landed"
    );
    assert!(outbox_of(&pool, &q.project).await.is_empty());
    common::eventually("the earlier try's run is marked deleted", || async {
        common::rows_of(&clickhouse.store, &q.project).await == (0, 0)
    })
    .await;
}

/// A project deleted while its run is written has its spans marked deleted
/// again once the row is in.
#[sqlx::test]
async fn a_run_written_as_its_project_is_deleted_goes_with_it(pool: PgPool) {
    let Some(clickhouse) =
        common::clickhouse_or_skip("a_run_written_as_its_project_is_deleted_goes_with_it").await
    else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let notices = NoticeLog::new();
    let state = common::state_with_stores_and_notices(&pool, auth.clone(), notices.clone());
    let p = project_over(&state, &auth, &notices, &OrganizationId::new());
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report/start")).await;
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 1, held: 0 },
        "the first ping's notice"
    );
    p.ping_ok(&format!("/{key}/nightly-report/fail?exit_code=1"))
        .await;

    auth.forget_after(&p.project, 1);
    let asked = auth.homes_asked();
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 1, held: 0 }
    );
    assert_eq!(
        auth.homes_asked() - asked,
        2,
        "asked before the insert, and again once the row was in"
    );
    assert!(outbox_of(&pool, &p.project).await.is_empty());
    common::eventually(
        "the run written as its project went is marked deleted",
        || async { common::rows_of(&clickhouse.store, &p.project).await == (0, 0) },
    )
    .await;
}

/// Auth unable to say where a run's project is hands the row back with its
/// try; unable to say after the write, the next tick drops the row rather
/// than write the run twice.
#[sqlx::test]
async fn a_run_auth_cannot_place_waits_with_its_try(pool: PgPool) {
    let Some(clickhouse) =
        common::clickhouse_or_skip("a_run_auth_cannot_place_waits_with_its_try").await
    else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let notices = NoticeLog::new();
    let state = common::state_with_stores_and_notices(&pool, auth.clone(), notices.clone());
    let p = project_over(&state, &auth, &notices, &OrganizationId::new());
    let key = p.mint().await;
    p.ping_ok(&format!("/{key}/nightly-report/start")).await;
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 1, held: 0 },
        "the first ping's notice"
    );
    p.ping_ok(&format!("/{key}/nightly-report/fail?exit_code=1"))
        .await;

    for unanswered in ["where the project is", "how its organization stands"] {
        auth.failing_homes(unanswered == "where the project is")
            .failing_standing(unanswered == "how its organization stands");
        assert_eq!(
            outbox::tick_once(&state).await.expect("a tick"),
            Settled { landed: 0, held: 1 },
            "{unanswered}"
        );
        assert_eq!(
            tries_of(&pool, &p.project).await,
            [(0, false)],
            "{unanswered}: the row not handed back with its try"
        );
        assert_eq!(
            common::rows_of(&clickhouse.store, &p.project).await,
            (0, 0),
            "{unanswered}: the run written"
        );
    }

    auth.failing_standing(false).failing_homes_after(1);
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 0, held: 1 }
    );
    assert_eq!(tries_of(&pool, &p.project).await, [(0, false)]);
    assert_eq!(
        common::rows_of(&clickhouse.store, &p.project).await,
        (1, 1),
        "the run not written"
    );

    auth.failing_homes(false);
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 1, held: 0 }
    );
    assert!(outbox_of(&pool, &p.project).await.is_empty());
    assert_eq!(
        common::rows_of(&clickhouse.store, &p.project).await,
        (1, 1),
        "the run written twice"
    );
}

/// A gone project's run whose mark fails waits with its try for the next
/// tick, and the batch's other gone projects are handed back unmarked.
#[sqlx::test]
async fn a_mark_clickhouse_does_not_take_hands_the_run_back(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let notices = NoticeLog::new();
    let state =
        common::state_with_notices(&pool, auth.clone(), Arc::new(NoLimits), notices.clone());
    let gone = [
        project_over(&state, &auth, &notices, &OrganizationId::new()),
        project_over(&state, &auth, &notices, &OrganizationId::new()),
    ];
    for p in &gone {
        let key = p.mint().await;
        p.ping_ok(&format!("/{key}/nightly-report/start")).await;
        p.ping_ok(&format!("/{key}/nightly-report/fail?exit_code=1"))
            .await;
    }
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 2, held: 2 },
        "the notices landed, the runs waiting on ClickHouse"
    );

    for p in &gone {
        auth.forget(&p.project);
    }
    let (capturing, logs) = telmoni_shared::test_util::captured_logs();
    assert_eq!(
        outbox::tick_once(&state).await.expect("a tick"),
        Settled { landed: 0, held: 2 }
    );
    drop(capturing);
    for p in &gone {
        assert_eq!(
            tries_of(&pool, &p.project).await,
            [(0, false)],
            "the row not handed back with its try"
        );
    }
    assert_eq!(
        logs.text().matches("did not mark").count(),
        1,
        "a mark tried after one failed"
    );
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
        let organizations: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT DISTINCT organization_id FROM telemetry.{table} WHERE project_id = $1"
        )))
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
        purged, 4,
        "the outbox row, the key, the monitor and the settings row, the run going with its \
         monitor by the cascade"
    );
    for table in [
        "ping_keys",
        "open_runs",
        "outbox",
        "monitors",
        "project_settings",
    ] {
        let left: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM telemetry.{table} WHERE project_id = $1"
        )))
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
    assert_eq!(purged, 4);
    assert!(open_runs(&pool, &q.project).await.is_empty());
}

/// A purge, a project's or an organization's, waits for a ping in flight
/// before it deletes anything, so the ping's outbox row goes with the rest.
#[sqlx::test]
async fn a_purge_waits_for_the_pings_in_flight_before_it_takes_their_runs(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    for whole_organization in [false, true] {
        let p = project(&pool);
        let key = p.mint().await;
        p.make(hourly()).await;
        let run = rid();
        p.ping_ok(&format!("/{key}/hourly-sync/start?rid={run}"))
            .await;

        // The run's row held, so the end ping waits there with its monitor.
        let mut holder = pool.begin().await.expect("the holder's transaction");
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .expect("the holder's session");
        sqlx::query("SELECT 1 FROM telemetry.open_runs WHERE project_id = $1 FOR UPDATE")
            .bind(&p.project)
            .execute(&mut *holder)
            .await
            .expect("hold the run");
        let mut purge = maintenance_scope(&p.state.db, TelemetryLane)
            .await
            .expect("the lane");
        let path = format!("/{key}/hourly-sync?rid={run}");
        let (ended, ()) = tokio::join!(p.ping(&path), async {
            let ping_pid = waiting_on(&pool, holder_pid, "the ping waits on its run").await;
            let ((), ()) = tokio::join!(
                async {
                    let purged = if whole_organization {
                        db::purge_organization(&mut purge, &p.organization).await
                    } else {
                        db::purge_project(&mut purge, &p.project).await
                    };
                    assert!(purged.expect("purged") > 0);
                    purge.commit().await.expect("the purge commits");
                },
                async {
                    waiting_on(&pool, ping_pid, "the purge waits on the ping").await;
                    holder.commit().await.expect("the holder lets go");
                },
            );
        });
        assert_eq!((ended.0, ended.1.as_str()), (StatusCode::OK, "OK"));
        assert_eq!(monitors_of(&pool, &p.project).await, 0);
        assert!(open_runs(&pool, &p.project).await.is_empty());
        assert!(
            outbox_of(&pool, &p.project).await.is_empty(),
            "the run the ping ended outlived the purge"
        );
    }
}

/// A first ping sent during a purge, a project's or an organization's, is
/// kept out until the purge commits, then finds its key gone, a 404, rather
/// than deadlocking with it.
#[sqlx::test]
async fn a_purge_keeps_out_the_first_pings_beside_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    for whole_organization in [false, true] {
        for shares_its_run in [false, true] {
            let p = project(&pool);
            let key = p.mint().await;
            // A run of another monitor's under the id the first ping names,
            // or another monitor for the agent its monitor would carry.
            let path = if shares_its_run {
                p.make(json!({ "name": "Report", "slug": "report-a" }))
                    .await;
                let run = rid();
                p.ping_ok(&format!("/{key}/report-a/start?rid={run}")).await;
                format!("/{key}/nightly-report/start?rid={run}")
            } else {
                p.make(json!({ "name": "Report", "slug": "report-a", "agent": "nightly-report" }))
                    .await;
                format!("/{key}/nightly-report")
            };
            let (answer, purged, held) = p
                .ping_as_a_purge_runs(&pool, &path, whole_organization)
                .await;
            assert_eq!(answer.0, StatusCode::NOT_FOUND, "{path}: {}", answer.1);
            assert!(purged.expect("purged") > 0, "{path}");
            assert!(
                !held,
                "{path}: a ping held the project in place beside the purge"
            );
            assert!(
                left_of(&pool, &p.project).await.is_empty(),
                "{path}: outlived the purge in {:?}",
                left_of(&pool, &p.project).await
            );
        }
    }
}

/// A purge waits at most two seconds for a project held in place, even where
/// the role sets no `lock_timeout`, and then fails, the rows standing.
#[sqlx::test]
async fn a_purge_waits_two_seconds_at_most_for_a_project_held_in_place(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    p.mint().await;
    let mut in_place = pool.begin().await.expect("a ping's transaction");
    assert!(
        project_move::try_hold_in_place(&mut in_place, &p.project)
            .await
            .expect("try the project")
    );
    let mut purger = maintenance_scope(&p.state.db, TelemetryLane)
        .await
        .expect("the lane");
    let started = std::time::Instant::now();
    let purged = tokio::time::timeout(
        Duration::from_secs(10),
        db::purge_project(&mut purger, &p.project),
    )
    .await
    .expect("the purge waited on past its lock's time");
    let waited = started.elapsed();
    let refused = purged.expect_err("a purge beside a project held in place");
    assert_eq!(
        refused
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03"),
        "{refused}"
    );
    assert!(
        (Duration::from_secs(2)..Duration::from_secs(4)).contains(&waited),
        "{waited:?}"
    );
    purger.rollback().await.expect("the purge lets go");
    in_place.rollback().await.expect("the ping ends");
    assert_eq!(
        left_of(&pool, &p.project).await,
        ["ping_keys", "project_settings"]
    );
}

/// A project's purge deletes its ping keys first, on their own, so a purge
/// cut short leaves no key answering pings; the rest waits for the sweep.
#[sqlx::test]
async fn a_projects_purge_closes_its_ping_lane_before_anything_can_fail(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    p.make(hourly()).await;
    p.ping_ok(&format!("/{key}/hourly-sync")).await;
    let refused = TelemetrySeam(p.state.clone())
        .purge_project(&p.project)
        .await
        .expect_err("a purge over a store that does not answer");
    assert_eq!(
        refused.to_problem_details().type_uri,
        "/errors/telemetry/store-unavailable"
    );
    for path in [
        format!("/{key}/hourly-sync"),
        format!("/{key}/nightly-report"),
    ] {
        let (status, body) = p.ping(&path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    assert_eq!(
        left_of(&pool, &p.project).await,
        ["monitors", "outbox", "project_settings"],
        "the rest stands for the sweep, and no first ping made more"
    );
}

/// The sweep finishes a purge cut short once auth no longer has the project,
/// and leaves a project auth still has alone, wherever its row is filed; a
/// pass auth or ClickHouse cannot answer changes nothing.
#[sqlx::test]
async fn a_projects_purge_cut_short_is_finished_by_the_sweep(pool: PgPool) {
    let Some(clickhouse) =
        common::clickhouse_or_skip("a_projects_purge_cut_short_is_finished_by_the_sweep").await
    else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let notices = NoticeLog::new();
    let live = common::state_with_stores_and_notices(&pool, auth.clone(), notices.clone());
    // The same module over a ClickHouse that does not answer.
    let away = common::state_with_notices(&pool, auth.clone(), Arc::new(NoLimits), notices.clone());
    let organization = OrganizationId::new();
    let gone = project_over(&live, &auth, &notices, &organization);
    let kept = project_over(&live, &auth, &notices, &organization);
    for p in [&gone, &kept] {
        let key = p.mint().await;
        p.make(hourly()).await;
        p.ping_ok(&format!("/{key}/hourly-sync/start")).await;
        clickhouse
            .store
            .insert_spans(&[common::span(&p.project, SpanKind::Run, "hourly-sync", 100)])
            .await
            .expect("a span of the project's");
    }
    sqlx::query("UPDATE telemetry.project_settings SET organization_id = $2 WHERE project_id = $1")
        .bind(&kept.project)
        .bind(OrganizationId::new())
        .execute(&pool)
        .await
        .expect("the kept project's row filed under another organization");

    // Deleted, and its purge cut short by a ClickHouse that does not answer:
    // its ping keys gone, the rest left.
    auth.forget(&gone.project);
    TelemetrySeam(away.clone())
        .purge_project(&gone.project)
        .await
        .expect_err("a purge over a store that does not answer");
    let left = left_of(&pool, &gone.project).await;
    assert_eq!(left, ["monitors", "open_runs", "project_settings"]);

    // A pass auth cannot answer, and one ClickHouse does not: nothing goes.
    auth.failing_homes(true);
    sweep::sweep(&live)
        .await
        .expect_err("a pass auth cannot answer");
    auth.failing_homes(false);
    let refused = sweep::sweep(&away)
        .await
        .expect_err("a pass over a store that does not answer");
    assert_eq!(
        refused.to_problem_details().type_uri,
        "/errors/telemetry/store-unavailable"
    );
    assert_eq!(left_of(&pool, &gone.project).await, left);
    assert_eq!(
        common::rows_of(&clickhouse.store, &gone.project).await,
        (1, 1)
    );

    // A pass that can: the purge finished, and nothing of the other's gone.
    assert_eq!(
        sweep::sweep(&live).await.expect("a pass"),
        Swept { purged: 1, left: 0 }
    );
    assert!(left_of(&pool, &gone.project).await.is_empty());
    common::eventually("the deleted project's spans are marked deleted", || async {
        common::rows_of(&clickhouse.store, &gone.project).await == (0, 0)
    })
    .await;
    assert_eq!(
        left_of(&pool, &kept.project).await,
        ["monitors", "open_runs", "ping_keys", "project_settings"]
    );
    assert_eq!(
        common::rows_of(&clickhouse.store, &kept.project).await,
        (1, 1)
    );
    assert_eq!(
        sweep::sweep(&live).await.expect("a pass"),
        Swept::default(),
        "nothing is left to finish"
    );
}

/// A purge the sweep cannot finish now is left for the next pass, and the
/// pass goes on to the rest.
#[sqlx::test]
async fn a_purge_the_sweep_cannot_finish_now_waits_for_the_next_pass(pool: PgPool) {
    let Some(_clickhouse) =
        common::clickhouse_or_skip("a_purge_the_sweep_cannot_finish_now_waits_for_the_next_pass")
            .await
    else {
        return;
    };
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let notices = NoticeLog::new();
    let state = common::state_with_stores_and_notices(&pool, auth.clone(), notices.clone());
    let organization = OrganizationId::new();
    for _ in 0..2 {
        let p = project_over(&state, &auth, &notices, &organization);
        p.make(hourly()).await;
        auth.forget(&p.project);
    }
    let read: Vec<ProjectId> =
        sqlx::query_scalar("SELECT project_id FROM telemetry.project_settings ORDER BY project_id")
            .fetch_all(&pool)
            .await
            .expect("the rows in the order a pass reads them");
    assert_eq!(read.len(), 2, "{read:?}");
    let held = read.first().expect("the row read first");
    let other = read.last().expect("the row read after it");
    let mut in_place = pool.begin().await.expect("a ping's transaction");
    assert!(
        project_move::try_hold_in_place(&mut in_place, held)
            .await
            .expect("hold the project in place")
    );
    assert_eq!(
        sweep::sweep(&state).await.expect("a pass"),
        Swept { purged: 1, left: 1 }
    );
    assert!(left_of(&pool, other).await.is_empty());
    assert_eq!(left_of(&pool, held).await, ["monitors", "project_settings"]);
    in_place.rollback().await.expect("the ping ends");
    assert_eq!(
        sweep::sweep(&state).await.expect("a pass"),
        Swept { purged: 1, left: 0 }
    );
    assert!(left_of(&pool, held).await.is_empty());
}

/// The sweep reads every settings row, a page at a time, asking auth once a
/// page.
#[sqlx::test]
async fn the_sweep_reads_every_page_of_settings_rows(pool: PgPool) {
    let Some(_clickhouse) =
        common::clickhouse_or_skip("the_sweep_reads_every_page_of_settings_rows").await
    else {
        return;
    };
    let auth = AuthStub::probing(&pool);
    let state = common::state_with_stores(&pool, auth.clone());
    let organization = OrganizationId::new();
    let projects: Vec<ProjectId> = (0..501).map(|_| ProjectId::new()).collect();
    for project in &projects {
        auth.seat(OWNER, &organization, project, OrganizationRole::Owner, None);
    }
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id)
         SELECT unnest($1::text[]), $2",
    )
    .bind(&projects)
    .bind(&organization)
    .execute(&pool)
    .await
    .expect("a settings row for each project");
    let ends: Vec<ProjectId> = sqlx::query_scalar(
        "(SELECT project_id FROM telemetry.project_settings ORDER BY project_id LIMIT 1)
         UNION ALL
         (SELECT project_id FROM telemetry.project_settings ORDER BY project_id DESC LIMIT 1)",
    )
    .fetch_all(&pool)
    .await
    .expect("the rows read first and last");
    for project in &ends {
        auth.forget(project);
    }
    let asked = auth.homes_asked();
    assert_eq!(
        sweep::sweep(&state).await.expect("a pass"),
        Swept { purged: 2, left: 0 }
    );
    assert_eq!(
        auth.homes_asked() - asked,
        2,
        "a page of 500, then one of 1"
    );
    for project in &ends {
        assert!(left_of(&pool, project).await.is_empty());
    }
    let standing: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.project_settings")
        .fetch_one(&pool)
        .await
        .expect("count the settings rows");
    assert_eq!(standing, 499);
}

/// A database that stops answering ends the sweep's pass where it stopped.
#[sqlx::test]
async fn a_pass_that_loses_its_database_ends_there(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::probing(&pool);
    let notices = NoticeLog::new();
    let state =
        common::state_with_notices(&pool, auth.clone(), Arc::new(NoLimits), notices.clone());
    let organization = OrganizationId::new();
    for _ in 0..2 {
        let p = project_over(&state, &auth, &notices, &organization);
        p.make(hourly()).await;
        auth.forget(&p.project);
    }
    // The page read, auth's answer still to come when the pool closes.
    auth.delaying_homes(Duration::from_millis(500));
    let asked = auth.homes_asked();
    let (_logging, logs) = telmoni_shared::test_util::captured_logs();
    let (ended, ()) = tokio::join!(sweep::sweep(&state), async {
        common::eventually("the pass asks auth", || async {
            auth.homes_asked() > asked
        })
        .await;
        state.db.close().await;
    });
    assert!(
        matches!(ended, Err(TelmoniError::Database(sqlx::Error::PoolClosed))),
        "{ended:?}"
    );
    assert!(
        !logs
            .text()
            .contains("a deleted project's purge failed again"),
        "a project was tried as though the database had answered"
    );
}

/// An organization's purge holds every project its rows name, one whose
/// settings row is filed elsewhere included, and takes only the rows filed
/// under the organization.
#[sqlx::test]
async fn an_organizations_purge_holds_every_project_its_rows_name(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (organization, elsewhere) = (OrganizationId::new(), OrganizationId::new());
    let tables = ["monitors", "ping_keys", "outbox", "project_settings"];
    let mut named = Vec::new();
    for named_by in tables {
        let p = project_under(&pool, organization.clone(), Arc::new(NoLimits));
        let key = p.mint().await;
        p.make(hourly()).await;
        p.ping_ok(&format!("/{key}/hourly-sync")).await;
        for table in tables.iter().filter(|table| **table != named_by) {
            let refiled = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE telemetry.{table} SET organization_id = $2 WHERE project_id = $1"
            )))
            .bind(&p.project)
            .bind(&elsewhere)
            .execute(&pool)
            .await
            .expect("re-file the project's rows")
            .rows_affected();
            assert!(refiled > 0, "{named_by}: no row in {table} to re-file");
        }
        named.push((named_by, p.project.clone(), p.state.clone()));
    }
    let projects: Vec<ProjectId> = named
        .iter()
        .map(|(_, project, _)| project.clone())
        .collect();
    let (monitors, monitors_pid) = holding(&pool, "telemetry.monitors").await;
    let mut purger = maintenance_scope(&named[0].2.db, TelemetryLane)
        .await
        .expect("the lane");
    let (purged, held) = tokio::join!(
        async {
            let purged = db::purge_organization(&mut purger, &organization)
                .await
                .expect("purged");
            purger.commit().await.expect("the purge commits");
            purged
        },
        async {
            waiting_on(
                &pool,
                monitors_pid,
                "the purge waits to delete the monitors",
            )
            .await;
            let held = holdable(&pool, &projects).await;
            monitors.commit().await.expect("the monitors let go");
            held
        },
    );
    assert!(held.is_empty(), "held in place beside the purge: {held:?}");
    assert_eq!(purged, 4, "one row named the organization in each project");
    for (named_by, project, _) in &named {
        let filed_elsewhere: Vec<&str> = [
            "monitors",
            "open_runs",
            "ping_keys",
            "outbox",
            "project_settings",
        ]
        .into_iter()
        .filter(|table| *table != *named_by && *table != "open_runs")
        .collect();
        assert_eq!(left_of(&pool, project).await, filed_elsewhere, "{named_by}");
    }
}

/// The hourly monitor made in `p`, with a run of its open and another
/// ended and waiting in the outbox: a row in each table a move rewrites.
async fn a_run_open_and_one_filed(pool: &PgPool, p: &Project, key: &str) {
    p.make(hourly()).await;
    p.ping_ok(&format!("/{key}/hourly-sync/start?rid={}", rid()))
        .await;
    p.ping_ok(&format!("/{key}/hourly-sync/fail?rid={}", rid()))
        .await;
    assert_eq!(open_runs(pool, &p.project).await.len(), 1);
    assert_eq!(finished_runs(pool, &p.project).await.len(), 1);
}

/// A move holds its monitors before it rewrites anything, so a monitor's
/// delete or the project's purge beside it waits for the move, rather than
/// deadlocking with it on the runs or the outbox.
#[sqlx::test]
async fn a_move_and_a_delete_or_a_purge_beside_it_wait_in_turn(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let elsewhere = OrganizationId::new();

    // The monitor's delete, beside the move.
    let p = project(&pool);
    let key = p.mint().await;
    a_run_open_and_one_filed(&pool, &p, &key).await;
    let delete = p.request("DELETE", &format!("{MONITORS}/hourly-sync"), None);
    let (moved, (deleted, _)) = p.moving_beside(&pool, &elsewhere, p.send(delete)).await;
    assert!(moved);
    assert_eq!(deleted, StatusCode::NO_CONTENT);
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
    assert!(open_runs(&pool, &p.project).await.is_empty());
    assert!(all_filed_under(&pool, "ping_keys", &p.project, &elsewhere).await);
    assert!(all_filed_under(&pool, "outbox", &p.project, &elsewhere).await);

    // The project's purge, beside the move.
    let q = project(&pool);
    let key = q.mint().await;
    a_run_open_and_one_filed(&pool, &q, &key).await;
    let mut purger = maintenance_scope(&q.state.db, TelemetryLane)
        .await
        .expect("the lane");
    let purge = async {
        let purged = db::purge_project(&mut purger, &q.project).await;
        purger.commit().await.expect("the purge commits");
        purged
    };
    let (moved, purged) = q.moving_beside(&pool, &elsewhere, purge).await;
    assert!(moved);
    assert!(purged.expect("purged") > 0);
    for table in [
        "monitors",
        "open_runs",
        "ping_keys",
        "outbox",
        "project_settings",
    ] {
        assert!(
            filed_under(&pool, table, &q.project).await.is_empty(),
            "{table}"
        );
    }
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

/// A body left unread — announced past the cap, or chunked past it with more
/// to come — ends the connection once the answer is out, so its rest is never
/// read as the next request; one read whole keeps the connection.
#[sqlx::test]
async fn a_body_left_unread_ends_the_connection_and_one_read_whole_keeps_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let key = p.mint().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("its address");
    let pings = p.pings.clone();
    tokio::spawn(async move { axum::serve(listener, pings).await });
    let post = |headers: &str, body: &[u8]| {
        let mut request =
            format!("POST /{key}/nightly-report HTTP/1.1\r\nhost: localhost\r\n{headers}\r\n\r\n")
                .into_bytes();
        request.extend_from_slice(body);
        request
    };
    let bare =
        format!("GET /{key}/nightly-report HTTP/1.1\r\nhost: localhost\r\n\r\n").into_bytes();
    let dropped = Some(("HTTP/1.1 200 OK".to_owned(), "OK; body dropped".to_owned()));
    let ok = Some(("HTTP/1.1 200 OK".to_owned(), "OK".to_owned()));

    // Read whole, the connection carries the next ping.
    let mut whole = TcpStream::connect(address).await.expect("a connection");
    assert_eq!(
        exchange(&mut whole, &post("content-length: 64", &[b'x'; 64])).await,
        dropped
    );
    assert_eq!(
        exchange(&mut whole, &bare).await,
        ok,
        "a body read whole keeps the connection"
    );

    // Announced past the cap, so not read at all: answered, then the
    // connection ends.
    let mut announced = TcpStream::connect(address).await.expect("a connection");
    assert_eq!(
        exchange(
            &mut announced,
            &post("content-length: 10485760", &vec![b'x'; 32 * 1024])
        )
        .await,
        dropped
    );
    assert_eq!(
        exchange(&mut announced, &bare).await,
        None,
        "a body left unread ends the connection"
    );

    // Chunked past the cap, its chunk still arriving: read to the cap, then
    // the same. (A chunk whose rest is in hand already is finished cheaply,
    // and the connection kept; nothing here depends on it.)
    let mut chunked = TcpStream::connect(address).await.expect("a connection");
    let mut body = format!("{:x}\r\n", 1024 * 1024).into_bytes();
    body.extend_from_slice(&vec![b'x'; 150 * 1024]);
    assert_eq!(
        exchange(&mut chunked, &post("transfer-encoding: chunked", &body)).await,
        dropped
    );
    assert_eq!(
        exchange(&mut chunked, &bare).await,
        None,
        "a chunked body past the cap ends the connection"
    );

    assert_eq!(
        finished_runs(&pool, &p.project).await.len(),
        4,
        "no byte of a body became a ping"
    );
}

/// `request`, sent on `stream` as a crontab's `curl` sends one, and the
/// answer's status line and body — or `None` when the connection ends before
/// an answer comes.
async fn exchange(stream: &mut TcpStream, request: &[u8]) -> Option<(String, String)> {
    tokio::time::timeout(Duration::from_secs(10), async {
        stream.write_all(request).await.ok()?;
        let mut answer = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = stream
                .read(&mut buffer)
                .await
                .ok()
                .filter(|read| *read > 0)?;
            answer.extend_from_slice(buffer.get(..read)?);
            let Some(end) = answer.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(answer.get(..end)?).into_owned();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            if answer.len() >= end + 4 + length {
                let body = String::from_utf8_lossy(answer.get(end + 4..end + 4 + length)?);
                return Some((head.lines().next()?.to_owned(), body.into_owned()));
            }
        }
    })
    .await
    .expect("answered, or ended, within ten seconds")
}

/// The organization-wide notices a suite's ceilings raised, oldest first.
fn organization_alerts(p: &Project) -> Vec<common::LoggedNotice> {
    p.notices
        .notices()
        .into_iter()
        .filter(|notice| notice.kind == NotificationKind::OrganizationAlert)
        .collect()
}

/// A plan's pings a minute hold its organization's monitors together: six
/// minutes' worth goes through at once, the next is refused with how long
/// to wait and recorded nowhere, and the owner is told once that day.
#[sqlx::test]
async fn pings_past_the_plans_ceiling_are_refused_unrecorded_and_told_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project_under(&pool, OrganizationId::new(), Arc::new(PlanLanes::hobby()));
    let key = p.mint().await;
    p.make(hourly()).await;
    for n in 0..12 {
        let (status, body) = p.ping(&format!("/{key}/hourly-sync?rid={}", rid())).await;
        assert_eq!(status, StatusCode::OK, "ping {n} of the burst: {body}");
    }
    let before = p.monitor("hourly-sync").await["last_run"].clone();
    let (status, media_type, body) = p
        .send_ping(
            "GET",
            &format!("/{key}/hourly-sync?rid={}", rid()),
            &[],
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(
        media_type.starts_with("application/problem+json"),
        "{media_type}"
    );
    let problem: Value = serde_json::from_str(&body).expect("a problem document");
    assert_eq!(problem["type"], "/errors/tenant/rate-limited");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("2 a minute")),
        "{problem}"
    );
    assert!(
        problem["retry_after_secs"]
            .as_u64()
            .is_some_and(|s| (1..=30).contains(&s))
    );
    assert_eq!(
        p.monitor("hourly-sync").await["last_run"],
        before,
        "a refused ping is recorded nowhere"
    );

    ceilings::tell(&p.state).await;
    ceilings::tell(&p.state).await;
    let told = organization_alerts(&p);
    assert_eq!(told.len(), 1, "told once a day: {told:?}");
    assert_eq!(told[0].organization_id, p.organization);
    assert_eq!(told[0].project_id, None, "on the organization's feed");
    assert_eq!(told[0].metadata["lane"], "pings");
    assert_eq!(told[0].metadata["refused"], 1);
    assert!(told[0].body.contains("refused once"), "{}", told[0].body);
}

/// A plan's pings hold its schedules too, two pings a run: a monitor whose
/// schedule would take the organization's past them is refused, a 402 naming
/// what lifts them, one that expects no run needs none, and a schedule may
/// always come down.
#[sqlx::test]
async fn a_schedule_past_the_plans_pings_is_refused_and_one_may_come_down(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    // No monitor ceiling, so what refuses the sixth is the pings alone.
    let lanes = PlanLanes {
        monitors: None,
        ..PlanLanes::hobby()
    };
    let p = project_under(&pool, OrganizationId::new(), Arc::new(lanes));
    for n in 0..5 {
        p.make(json!({ "name": format!("Job {n}"), "schedule": "*/5 * * * *", "grace_s": 60 }))
            .await;
    }
    let (status, body) = p
        .send(p.request(
            "POST",
            MONITORS,
            Some(json!({ "name": "Job 5", "schedule": "*/5 * * * *", "grace_s": 60 })),
        ))
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["type"], "/errors/plan/limit-reached");
    assert_eq!(body["entitlement"], "pro");
    assert_eq!(body["detail"], "Move to Pro for more pings.");
    p.make(json!({ "name": "On demand" })).await;

    let (status, body) = p
        .send(p.request(
            "PUT",
            &format!("{MONITORS}/job-0"),
            Some(json!({ "name": "Job 0", "schedule": "* * * * *", "grace_s": 60 })),
        ))
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    let (status, body) = p
        .send(p.request(
            "PUT",
            &format!("{MONITORS}/job-0"),
            Some(json!({ "name": "Job 0", "schedule": "*/10 * * * *", "grace_s": 60 })),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    p.make(json!({ "name": "Job 5", "interval_s": 600, "grace_s": 60 }))
        .await;

    // Past the ceiling once the plan holds fewer, a schedule may still come
    // down, and none may go up.
    let auth = AuthStub::probing(&pool);
    auth.seat(
        OWNER,
        &p.organization,
        &p.project,
        OrganizationRole::Owner,
        None,
    );
    let fewer = PlanLanes {
        pings_per_minute: 1,
        monitors: None,
        ..PlanLanes::hobby()
    };
    let notices = NoticeLog::new();
    let state = common::state_with_notices(&pool, auth, Arc::new(fewer), notices.clone());
    let q = Project::over(state, notices, p.organization.clone(), p.project.clone());
    let (status, body) = q
        .send(q.request(
            "PUT",
            &format!("{MONITORS}/job-1"),
            Some(json!({ "name": "Job 1", "schedule": "*/15 * * * *", "grace_s": 60 })),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = q
        .send(q.request(
            "PUT",
            &format!("{MONITORS}/job-2"),
            Some(json!({ "name": "Job 2", "schedule": "*/4 * * * *", "grace_s": 60 })),
        ))
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
}

/// A ping naming a monitor the plan has no room for is refused as before,
/// and counted for the owner's notice of the day with the slug it named.
#[sqlx::test]
async fn a_ping_refused_for_the_plans_monitors_is_told_with_its_slug(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let lanes = PlanLanes {
        monitors: Some(0),
        ..PlanLanes::hobby()
    };
    let p = project_under(&pool, OrganizationId::new(), Arc::new(lanes));
    let key = p.mint().await;
    let (status, body) = p.ping(&format!("/{key}/nightly-sync")).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");

    ceilings::tell(&p.state).await;
    let told = organization_alerts(&p);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].metadata["lane"], "monitors");
    assert!(told[0].body.contains("nightly-sync"), "{}", told[0].body);
}

/// The day's data out is counted where an organization is sent bytes, and
/// refused once spent, a 429 until midnight UTC; a binary that sells nothing
/// counts nothing.
#[sqlx::test]
async fn data_out_past_the_day_is_refused_until_midnight(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project_under(&pool, OrganizationId::new(), Arc::new(PlanLanes::hobby()));
    let seam = TelemetrySeam(p.state.clone());
    assert!(seam.admit_data_out(&p.organization).await.is_ok());
    seam.spend_data_out(&p.organization, 4_999_999).await;
    assert!(seam.admit_data_out(&p.organization).await.is_ok());
    seam.spend_data_out(&p.organization, 1).await;
    let problem = seam
        .admit_data_out(&p.organization)
        .await
        .expect_err("the day is spent")
        .to_problem_details();
    assert_eq!(problem.type_uri, "/errors/tenant/rate-limited");
    assert_eq!(problem.status, 429);
    let detail = problem.detail.unwrap_or_default();
    assert!(
        detail.contains("5 MB") && detail.contains("midnight UTC"),
        "{detail}"
    );
    assert!(
        problem
            .extensions
            .get("retry_after_secs")
            .and_then(Value::as_u64)
            .is_some_and(|s| (1..=86_400).contains(&s))
    );
    ceilings::tell(&p.state).await;
    let told = organization_alerts(&p);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].metadata["lane"], "data_out");

    let q = project(&pool);
    let seam = TelemetrySeam(q.state.clone());
    seam.spend_data_out(&q.organization, u64::MAX).await;
    assert!(
        seam.admit_data_out(&q.organization).await.is_ok(),
        "nothing is counted without a plan"
    );
}

/// The plan's ceilings over Redis, as a tier counts them: pings past the
/// burst refused, the slugs a day's notice names kept to ten however many
/// are refused, the day's data out spent, and each lane told once.
#[sqlx::test]
async fn the_ceilings_hold_over_redis_as_on_a_tier(pool: PgPool) {
    use redis::AsyncCommands as _;
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
    let lanes = PlanLanes {
        monitors: Some(0),
        ..PlanLanes::hobby()
    };
    let notices = NoticeLog::new();
    let state = common::state_with_redis_under(&pool, auth, Arc::new(lanes), notices.clone());
    let Some(mut redis) = state.redis.connection().await else {
        eprintln!(
            "skipping {}: Redis is not reachable (REDIS_URL, or the compose file's)",
            module_path!()
        );
        return;
    };
    let p = Project::over(state, notices, organization, project);
    let key = p.mint().await;
    for n in 0..12 {
        let (status, body) = p.ping(&format!("/{key}/sync-{n}")).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "ping {n}: {body}");
    }
    let (status, body) = p.ping(&format!("/{key}/sync-12")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    let day = chrono::Utc::now().date_naive();
    let slugs: u64 = redis
        .scard(format!("telemetry:refused:{day}:{}:slugs", p.organization))
        .await
        .expect("the day's slugs");
    assert_eq!(slugs, 10);

    let seam = TelemetrySeam(p.state.clone());
    seam.spend_data_out(&p.organization, 5_000_000).await;
    assert!(seam.admit_data_out(&p.organization).await.is_err());

    ceilings::tell(&p.state).await;
    ceilings::tell(&p.state).await;
    let mut told: Vec<String> = organization_alerts(&p)
        .iter()
        .filter(|notice| notice.organization_id == p.organization)
        .map(|notice| {
            notice.metadata["lane"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    told.sort();
    assert_eq!(told, ["data_out", "monitors", "pings"]);
}
