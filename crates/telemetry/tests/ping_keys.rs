//! A project's ping keys through the module's own router: owners and admins
//! mint and retire them and members read them, each minting and retiring
//! recorded on the chain under the organization's lock, and one that cannot
//! have the lock in time refused with nothing changed; two may be live at
//! once and never three; a key of another shape, one the table does not
//! hold and one retired open nothing on the ping lane; the rotation retires
//! the older of two a week after its last ping once a newer one stands,
//! records it as the module's own act and tells the project, once and in the
//! same transaction, while nobody is told of a key a person retires; one
//! organization whose lock is busy holds up its own keys alone, and a key
//! whose project moves is retired where it went; and a ping touches its
//! key.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET};
use telmoni_shared::audit::lock_chain;
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::{apply_audit_migrations, captured_logs};
use telmoni_shared::types::{PING_KEY_PREFIX, PING_KEY_RANDOM_LEN};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, TelmoniError};
use telmoni_telemetry::db::{self, ResolvedPingKey, TelemetryLane};
use telmoni_telemetry::handler::ping_keys::{LIVE_MAX, Rotated, retire_each, retire_stale_keys};
use telmoni_telemetry::rotation::REPLACED_AFTER_S;
use telmoni_telemetry::{AppState, ping_router, router};

const LANE: &str = "/internal/telemetry/ping-keys";
const OWNER: &str = "user_owner";
const ADMIN: &str = "user_admin";
const MEMBER: &str = "user_member";

/// The module's two routers — the console's lanes and the ping lane — auth
/// as it holds it, and a project the roster seats its three people on.
struct Project {
    app: Router,
    pings: Router,
    state: Arc<AppState>,
    auth: Arc<AuthStub>,
    organization: OrganizationId,
    project: ProjectId,
}

fn project(pool: &PgPool) -> Project {
    project_of(pool, OrganizationId::new())
}

/// A project of `organization`, the roster seating its three people on it.
fn project_of(pool: &PgPool, organization: OrganizationId) -> Project {
    let project = ProjectId::new();
    let auth = AuthStub::probing(pool);
    for (user, role, seat) in [
        (OWNER, OrganizationRole::Owner, None),
        (ADMIN, OrganizationRole::Admin, None),
        (MEMBER, OrganizationRole::Member, Some(Role::Member)),
    ] {
        auth.seat(user, &organization, &project, role, seat);
    }
    let state = common::state(pool, auth.clone());
    Project {
        app: router(state.clone()),
        pings: ping_router(state.clone()),
        state,
        auth,
        organization,
        project,
    }
}

impl Project {
    /// A request as the console sends one.
    fn request(&self, method: &str, path: &str, user: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{user}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", self.project.as_str())
            .body(Body::empty())
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

    /// A key minted by `user`, shown this once.
    async fn mint(&self, user: &str) -> (StatusCode, Value) {
        self.send(self.request("POST", LANE, user)).await
    }

    /// The keys as `user` lists them.
    async fn list(&self, user: &str) -> Vec<Value> {
        let (status, body) = self.send(self.request("GET", LANE, user)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body.get("keys")
            .and_then(Value::as_array)
            .expect("keys")
            .clone()
    }

    /// A ping as a crontab sends one: no header, the key in the path, on the
    /// lane as the binary nests it.
    async fn ping(&self, path: &str) -> (StatusCode, String) {
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
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// What the organization's chain records of its ping keys, in order: the
/// action and who did it.
async fn recorded(pool: &PgPool, organization: &OrganizationId) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT action, actor_id FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'ping_key'
          ORDER BY seq",
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .expect("read the chain")
}

async fn monitors_of(pool: &PgPool, project: &ProjectId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM telemetry.monitors WHERE project_id = $1")
        .bind(project)
        .fetch_one(pool)
        .await
        .expect("count a project's monitors")
}

/// The notices waiting in the project's outbox, oldest first.
async fn notices_waiting(pool: &PgPool, project: &ProjectId) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM telemetry.outbox
          WHERE project_id = $1 AND kind = 'notice'
          ORDER BY id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("read the project's notices")
}

/// The first `n` characters of `s`.
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A key's clock moved back: its minting `created_days` ago, and its last
/// ping `pinged_days` ago, or never.
async fn aged(pool: &PgPool, id: &str, created_days: i32, pinged_days: Option<i32>) {
    let id: Uuid = id.parse().expect("a key's id");
    sqlx::query(
        "UPDATE telemetry.ping_keys
            SET created_at = now() - make_interval(days => $2),
                last_ping_at = CASE WHEN $3::int IS NULL THEN NULL
                                    ELSE now() - make_interval(days => $3) END
          WHERE id = $1",
    )
    .bind(id)
    .bind(created_days)
    .bind(pinged_days)
    .execute(pool)
    .await
    .expect("age a key");
}

#[sqlx::test]
async fn owners_and_admins_mint_and_retire_keys_and_members_read_them(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);

    let (status, _) = p.mint(MEMBER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member mints no key");
    assert!(p.list(MEMBER).await.is_empty(), "a member reads the keys");

    let (status, first) = p.mint(OWNER).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let key = first["key"].as_str().expect("the key, shown once");
    assert!(key.starts_with(PING_KEY_PREFIX), "{key}");
    assert_eq!(key.len(), PING_KEY_PREFIX.len() + PING_KEY_RANDOM_LEN);
    assert_eq!(
        first["prefix"].as_str(),
        Some(head(key, PING_KEY_PREFIX.len() + 8).as_str()),
        "the list shows the prefix and the first characters after it"
    );
    assert!(first["retired_at"].is_null());
    assert!(first["last_ping_at"].is_null());
    assert_eq!(
        p.auth.chain_held(),
        vec![true],
        "the key is minted under the organization's chain lock"
    );

    let keys = p.list(MEMBER).await;
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["id"], first["id"]);
    assert!(
        keys[0].get("key").is_none(),
        "the key is shown once and listed never: {}",
        keys[0]
    );

    let (status, second) = p.mint(ADMIN).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_ne!(second["key"], first["key"]);
    let (status, third) = p.mint(OWNER).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{LIVE_MAX} live keys are the most: {third}"
    );
    assert!(
        third["detail"]
            .as_str()
            .is_some_and(|d| d.contains("retire"))
    );

    let first_id = first["id"].as_str().expect("an id").to_owned();
    let (status, _) = p
        .send(p.request("DELETE", &format!("{LANE}/{first_id}"), MEMBER))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member retires no key");
    let (status, _) = p
        .send(p.request("DELETE", &format!("{LANE}/{first_id}"), ADMIN))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let keys = p.list(OWNER).await;
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0]["id"], second["id"], "the live key first");
    assert!(keys[0]["retired_at"].is_null());
    assert_eq!(keys[1]["id"], first["id"]);
    assert!(keys[1]["retired_at"].is_string(), "{}", keys[1]);

    // Retired already: as it is, and nothing recorded.
    let (status, _) = p
        .send(p.request("DELETE", &format!("{LANE}/{first_id}"), OWNER))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for missing in [Uuid::new_v4().to_string(), "not-an-id".to_owned()] {
        let (status, _) = p
            .send(p.request("DELETE", &format!("{LANE}/{missing}"), OWNER))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }

    // With one retired, the project may mint again.
    let (status, _) = p.mint(OWNER).await;
    assert_eq!(status, StatusCode::CREATED);

    assert_eq!(
        recorded(&pool, &p.organization).await,
        vec![
            ("created".to_owned(), OWNER.to_owned()),
            ("created".to_owned(), ADMIN.to_owned()),
            ("deleted".to_owned(), ADMIN.to_owned()),
            ("created".to_owned(), OWNER.to_owned()),
        ]
    );
    assert!(
        notices_waiting(&pool, &p.project).await.is_empty(),
        "a person who retires a key knows it: nobody is told"
    );
}

#[sqlx::test]
async fn a_key_of_another_shape_or_retired_opens_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let (_, minted) = p.mint(OWNER).await;
    let key = minted["key"].as_str().expect("the key").to_owned();

    let (status, body) = p.ping(&format!("/{key}/nightly-report")).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "OK"));
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
    let keys = p.list(OWNER).await;
    assert!(
        keys[0]["last_ping_at"].is_string(),
        "a ping touches its key: {}",
        keys[0]
    );
    // A ping the lane refuses after its key is resolved touches it too, as a
    // request an API key opens and a lane then refuses touches the key: its
    // line carries the key, and the rotation counts its week from the key's
    // last ping.
    let id = minted["id"].as_str().expect("an id");
    aged(&pool, id, 0, None).await;
    let (status, _) = p.ping(&format!("/{key}/Nightly")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        p.list(OWNER).await[0]["last_ping_at"].is_string(),
        "a refused ping left its key untouched"
    );

    // ⚠ A key the table does not hold is a 404, never a 401, and the answer
    // never echoes it.
    let unknown = format!("{PING_KEY_PREFIX}{}", "0".repeat(PING_KEY_RANDOM_LEN));
    for other in [
        format!("/{unknown}/nightly-report"),
        format!("/{}/nightly-report", head(&unknown, unknown.len() - 1)),
        format!(
            "/telmoni_{}/nightly-report",
            "0".repeat(PING_KEY_RANDOM_LEN)
        ),
        "/x/nightly-report".to_owned(),
        format!("/{key}"),
    ] {
        let (status, body) = p.ping(&other).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{other}: {body}");
        assert!(
            !body.contains(&head(&unknown, 16)),
            "{other} was echoed: {body}"
        );
        assert!(
            !body.contains(&head(&key, 16)),
            "{other} echoed the live key: {body}"
        );
    }
    assert_eq!(
        monitors_of(&pool, &p.project).await,
        1,
        "a refused ping makes nothing"
    );

    // Retired, the key opens nothing from then on.
    let id = minted["id"].as_str().expect("an id");
    let (status, _) = p
        .send(p.request("DELETE", &format!("{LANE}/{id}"), OWNER))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = p.ping(&format!("/{key}/nightly-report")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn the_rotation_retires_the_older_key_a_week_after_its_last_ping(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let (_, old) = p.mint(OWNER).await;
    let (_, new) = p.mint(ADMIN).await;
    let old_id = old["id"].as_str().expect("an id");
    let new_id = new["id"].as_str().expect("an id");

    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        retiring(0),
        "both minted today: nothing to retire"
    );

    // The old key pinged this week: the lines are still moving over.
    aged(&pool, old_id, 9, Some(2)).await;
    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        retiring(0)
    );

    // A week past its last ping, with the newer standing: retired.
    aged(&pool, old_id, 9, Some(8)).await;
    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        retiring(1)
    );
    let keys = p.list(OWNER).await;
    assert_eq!(keys[0]["id"], new["id"]);
    assert!(keys[0]["retired_at"].is_null());
    assert_eq!(keys[1]["id"], old["id"]);
    assert!(keys[1]["retired_at"].is_string(), "{}", keys[1]);
    // Told once, in the transaction that retired it, with what finds the
    // line still carrying the key.
    let told = notices_waiting(&pool, &p.project).await;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["kind"], "ping_key_retired");
    assert_eq!(
        told[0]["dedup_key"],
        format!("telemetry:ping-key:{old_id}:retired")
    );
    assert_eq!(told[0]["metadata"]["ping_key_id"], old["id"]);
    assert_eq!(told[0]["metadata"]["ping_key_prefix"], old["prefix"]);
    let body = told[0]["body"].as_str().expect("a body");
    assert!(
        body.contains(old["prefix"].as_str().expect("a prefix")),
        "{body}"
    );
    assert!(body.contains("had not pinged in over a week"), "{body}");
    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        retiring(0),
        "the next tick finds nothing"
    );
    assert_eq!(
        notices_waiting(&pool, &p.project).await.len(),
        1,
        "a tick that retires nothing tells nothing"
    );
    let (status, _) = p
        .ping(&format!(
            "/{}/nightly-report",
            old["key"].as_str().expect("the key")
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a retired key opens nothing");

    // The newest key is never retired by the rotation, however old, since
    // nothing stands after it.
    aged(&pool, new_id, 30, None).await;
    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        retiring(0)
    );

    // One never pinged is retired a week after its minting once a newer one
    // stands: an unused key is not kept for the lines that never carried it.
    let (status, third) = p.mint(OWNER).await;
    assert_eq!(status, StatusCode::CREATED, "{third}");
    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        retiring(1)
    );
    let keys = p.list(OWNER).await;
    assert_eq!(keys[0]["id"], third["id"]);
    assert!(keys[0]["retired_at"].is_null());
    assert_eq!(
        keys.iter()
            .filter(|key| key["retired_at"].is_string())
            .count(),
        2
    );
    let told = notices_waiting(&pool, &p.project).await;
    assert_eq!(told.len(), 2, "{told:?}");
    assert_eq!(told[1]["metadata"]["ping_key_id"], new["id"]);
    assert!(
        told[1]["body"]
            .as_str()
            .is_some_and(|body| body.contains("was minted over a week ago and never pinged")),
        "{}",
        told[1]
    );

    let chain = recorded(&pool, &p.organization).await;
    assert_eq!(
        chain,
        vec![
            ("created".to_owned(), OWNER.to_owned()),
            ("created".to_owned(), ADMIN.to_owned()),
            ("deleted".to_owned(), "service:telemetry".to_owned()),
            ("created".to_owned(), OWNER.to_owned()),
            ("deleted".to_owned(), "service:telemetry".to_owned()),
        ],
        "each retirement by the rotation is the module's own act on the chain"
    );
}

/// A key the rotation would retire, a week past its last ping with a newer
/// one standing.
async fn stale(p: &Project, pool: &PgPool) -> Value {
    let (_, old) = p.mint(OWNER).await;
    p.mint(ADMIN).await;
    let id = old.get("id").and_then(Value::as_str).expect("an id");
    aged(pool, id, 9, Some(8)).await;
    old
}

/// A tick that retired `retired` keys and left none for a later one.
fn retiring(retired: u32) -> Rotated {
    Rotated { retired, left: 0 }
}

/// How many of `organization`'s ping keys its chain records the rotation
/// retiring.
async fn retired_by_the_rotation(pool: &PgPool, organization: &OrganizationId) -> usize {
    recorded(pool, organization)
        .await
        .into_iter()
        .filter(|(action, by)| action == "deleted" && by == "service:telemetry")
        .count()
}

/// Whether every one of the project's keys is live.
async fn all_live(p: &Project) -> bool {
    p.list(OWNER)
        .await
        .iter()
        .all(|key| key["retired_at"].is_null())
}

/// The project's rows handed to `organization` and committed, as a
/// transfer's move hands them, under the project's move lock.
async fn moved(p: &Project, organization: &OrganizationId) {
    let mut mover = maintenance_scope(&p.state.db, TelemetryLane)
        .await
        .expect("the lane");
    project_move::lock_for_move(mover.conn(), &p.project)
        .await
        .expect("the move lock");
    db::move_project(&mut mover, &p.project, organization)
        .await
        .expect("moved");
    mover.commit().await.expect("the move commits");
}

/// The tick's stale key of `p`'s project, as `read` holds it.
fn key_of(read: &[ResolvedPingKey], p: &Project) -> ResolvedPingKey {
    read.iter()
        .find(|key| key.project_id == p.project)
        .cloned()
        .expect("the project's stale key")
}

/// The stale keys as a tick reads them, before it retires any.
async fn stale_keys(state: &AppState) -> Vec<ResolvedPingKey> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane)
        .await
        .expect("the lane");
    let stale = db::stale_ping_keys(&mut tx, REPLACED_AFTER_S, 100)
        .await
        .expect("read the stale keys");
    tx.commit().await.expect("the read commits");
    stale
}

/// The notice is written in the transaction that retires the key and records
/// it: one that cannot be written leaves the key live and the chain as it
/// was, for the next tick to retire, record and tell together.
#[sqlx::test]
async fn a_retirement_whose_notice_cannot_be_written_is_no_retirement(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    stale(&p, &pool).await;

    sqlx::query("REVOKE INSERT ON telemetry.outbox FROM telemetry")
        .execute(&pool)
        .await
        .expect("the outbox closed to the module");
    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        Rotated {
            retired: 0,
            left: 1
        },
        "a key was retired without its notice"
    );
    assert!(all_live(&p).await);
    assert_eq!(recorded(&pool, &p.organization).await.len(), 2);
    assert!(notices_waiting(&pool, &p.project).await.is_empty());

    sqlx::query("GRANT INSERT ON telemetry.outbox TO telemetry")
        .execute(&pool)
        .await
        .expect("the outbox open again");
    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        retiring(1)
    );
    assert_eq!(recorded(&pool, &p.organization).await.len(), 3);
    assert_eq!(notices_waiting(&pool, &p.project).await.len(), 1);
}

/// ⚠ One organization whose chain lock is busy — a transfer's accept holding
/// it past the rotation's wait, say — holds up its own keys alone: the tick
/// retires every other organization's, waits on the busy lock once however
/// many of that organization's keys it found, and leaves them for a later
/// tick, which retires them once the lock is free.
#[sqlx::test]
async fn an_organization_whose_chain_lock_is_busy_holds_up_its_own_keys_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let busy = project(&pool);
    let busy_too = project_of(&pool, busy.organization.clone());
    let free = project(&pool);
    for p in [&busy, &busy_too, &free] {
        stale(p, &pool).await;
    }

    let mut holder = pool.begin().await.expect("a transaction to hold the lock");
    lock_chain(&mut holder, &busy.organization)
        .await
        .expect("the chain lock");
    let (_logging, logs) = captured_logs();
    let rotated = tokio::time::timeout(Duration::from_secs(10), retire_stale_keys(&free.state))
        .await
        .expect("the tick waited on the busy lock past its own wait")
        .expect("a tick");
    holder.rollback().await.expect("let the lock go");
    assert_eq!(
        rotated,
        Rotated {
            retired: 1,
            left: 2
        }
    );
    assert_eq!(
        logs.text().matches("passed over an organization").count(),
        1,
        "the busy lock was waited on once for each of its keys"
    );
    assert_eq!(retired_by_the_rotation(&pool, &free.organization).await, 1);
    assert_eq!(notices_waiting(&pool, &free.project).await.len(), 1);
    for p in [&busy, &busy_too] {
        assert!(all_live(p).await, "a key retired under a lock it never had");
        assert!(notices_waiting(&pool, &p.project).await.is_empty());
    }
    assert_eq!(retired_by_the_rotation(&pool, &busy.organization).await, 0);

    assert_eq!(
        retire_stale_keys(&free.state).await.expect("a tick"),
        retiring(2)
    );
    assert_eq!(retired_by_the_rotation(&pool, &busy.organization).await, 2);
}

/// A key whose project moves to another organization is retired where the
/// project went, never under the one the tick's read named: while a move
/// holds the project the key is left for a later tick, at once and with
/// nothing recorded or told, and a move committed since the read is seen
/// once the key's project is held — its retirement recorded on the chain of
/// the organization holding the project, and its notice filed there.
#[sqlx::test]
async fn a_stale_key_whose_project_moves_is_retired_where_it_went(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let old = stale(&p, &pool).await;
    let read = stale_keys(&p.state).await;
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].organization_id, p.organization);

    let to = OrganizationId::new();
    let mut mover = maintenance_scope(&p.state.db, TelemetryLane)
        .await
        .expect("the lane");
    project_move::lock_for_move(mover.conn(), &p.project)
        .await
        .expect("the move lock");
    let rotated = tokio::time::timeout(Duration::from_secs(10), retire_stale_keys(&p.state))
        .await
        .expect("the tick waited on the move")
        .expect("a tick");
    assert_eq!(
        rotated,
        Rotated {
            retired: 0,
            left: 1
        }
    );
    db::move_project(&mut mover, &p.project, &to)
        .await
        .expect("moved");
    mover.commit().await.expect("the move commits");
    assert!(all_live(&p).await);
    assert!(notices_waiting(&pool, &p.project).await.is_empty());

    assert_eq!(
        retire_each(&p.state, &read).await.expect("a tick"),
        retiring(1)
    );
    let retired: bool = sqlx::query_scalar(
        "SELECT retired_at IS NOT NULL FROM telemetry.ping_keys WHERE id = $1::uuid",
    )
    .bind(old["id"].as_str().expect("an id"))
    .fetch_one(&pool)
    .await
    .expect("read the key");
    assert!(retired);
    assert_eq!(
        recorded(&pool, &to).await,
        vec![("deleted".to_owned(), "service:telemetry".to_owned())]
    );
    assert_eq!(
        recorded(&pool, &p.organization).await.len(),
        2,
        "the two mintings alone"
    );
    let filed: Vec<String> = sqlx::query_scalar(
        "SELECT organization_id FROM telemetry.outbox WHERE project_id = $1 AND kind = 'notice'",
    )
    .bind(&p.project)
    .fetch_all(&pool)
    .await
    .expect("read where the notice is filed");
    assert_eq!(filed, [to.to_string()]);
}

/// The tick passes over an organization whose chain lock it found busy by
/// where each key stands when the tick reaches it, not where its read found
/// it: a key whose project moved into that organization since is left
/// without a second wait on the lock, and one whose project moved out of it
/// is retired where it went.
#[sqlx::test]
async fn a_busy_organization_is_passed_over_by_where_its_keys_stand_now(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let busy = project(&pool);
    let coming = project(&pool);
    let leaving = project_of(&pool, busy.organization.clone());
    for p in [&busy, &coming, &leaving] {
        stale(p, &pool).await;
    }
    let read = stale_keys(&busy.state).await;
    let keys = [
        key_of(&read, &busy),
        key_of(&read, &coming),
        key_of(&read, &leaving),
    ];
    moved(&coming, &busy.organization).await;
    let away = OrganizationId::new();
    moved(&leaving, &away).await;

    let mut holder = pool.begin().await.expect("a transaction to hold the lock");
    lock_chain(&mut holder, &busy.organization)
        .await
        .expect("the chain lock");
    let (_logging, logs) = captured_logs();
    let rotated = tokio::time::timeout(Duration::from_secs(10), retire_each(&busy.state, &keys))
        .await
        .expect("the tick waited on the busy lock past its own wait")
        .expect("a tick");
    holder.rollback().await.expect("let the lock go");
    assert_eq!(
        rotated,
        Rotated {
            retired: 1,
            left: 2
        }
    );
    assert_eq!(
        logs.text().matches("passed over an organization").count(),
        1,
        "the busy lock was waited on again for a key that came under it"
    );
    assert!(all_live(&busy).await);
    assert!(all_live(&coming).await);
    assert_eq!(
        retired_by_the_rotation(&pool, &away).await,
        1,
        "the key that left was retired where it went"
    );
}

/// A database that stops answering ends the tick where it stopped, as a
/// failed read of the keys does, rather than costing every key after it the
/// same wait and a line of its own.
#[sqlx::test]
async fn a_tick_that_loses_its_database_ends_there(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    stale(&p, &pool).await;
    let read = stale_keys(&p.state).await;
    let twice = [read[0].clone(), read[0].clone()];

    p.state.db.close().await;
    let (_logging, logs) = captured_logs();
    let ended = retire_each(&p.state, &twice).await;
    assert!(
        matches!(ended, Err(TelmoniError::Database(sqlx::Error::PoolClosed))),
        "{ended:?}"
    );
    assert!(
        !logs.text().contains("could not retire a key"),
        "a key was tried as though the database had answered"
    );
}

/// A minting or a retirement that cannot have the organization's chain lock
/// within its wait — a transfer's accept holding it past two seconds, say —
/// is answered for the person to try again, mints, retires and records
/// nothing, and never asks auth again without the lock; once the lock is
/// free, both go through.
#[sqlx::test]
async fn a_key_change_that_waits_too_long_on_the_chain_changes_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let (_, minted) = p.mint(OWNER).await;
    let retire = format!("{LANE}/{}", minted["id"].as_str().expect("an id"));

    let mut holder = pool.begin().await.expect("a transaction to hold the lock");
    lock_chain(&mut holder, &p.organization)
        .await
        .expect("the chain lock");
    let (status, body) = tokio::time::timeout(Duration::from_secs(10), p.mint(OWNER))
        .await
        .expect("the minting waited on the lock past its own wait");
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["type"], "/errors/auth/conflict");
    let (status, body) = tokio::time::timeout(
        Duration::from_secs(10),
        p.send(p.request("DELETE", &retire, OWNER)),
    )
    .await
    .expect("the retirement waited on the lock past its own wait");
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    holder.rollback().await.expect("let the lock go");
    assert_eq!(
        p.auth.chain_held(),
        [true],
        "auth was asked again for the first minting alone"
    );
    let keys = p.list(OWNER).await;
    assert_eq!(keys.len(), 1);
    assert!(keys[0]["retired_at"].is_null());
    assert_eq!(recorded(&pool, &p.organization).await.len(), 1);

    let (status, _) = p.mint(OWNER).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = p.send(p.request("DELETE", &retire, OWNER)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(recorded(&pool, &p.organization).await.len(), 3);
}

/// Two replicas whose ticks find one key retire it, record it and tell of it
/// once: the second, let in under the organization's chain lock once the
/// first has committed, finds it retired and does nothing.
#[sqlx::test]
async fn a_key_two_ticks_find_is_retired_recorded_and_told_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    stale(&p, &pool).await;

    let mut holder = pool.begin().await.expect("a transaction to hold the lock");
    lock_chain(&mut holder, &p.organization)
        .await
        .expect("the chain lock");
    let (first, second, ()) = tokio::join!(
        retire_stale_keys(&p.state),
        retire_stale_keys(&p.state),
        async {
            common::eventually("both ticks wait on the chain lock", || async {
                advisory_waits(&pool).await == 2
            })
            .await;
            holder.rollback().await.expect("let the lock go");
        },
    );
    let (first, second) = (first.expect("a tick"), second.expect("a tick"));
    assert_eq!(first.retired + second.retired, 1);
    assert_eq!(
        first.left + second.left,
        0,
        "the second found it retired, and left nothing for a later tick"
    );
    assert_eq!(retired_by_the_rotation(&pool, &p.organization).await, 1);
    assert_eq!(notices_waiting(&pool, &p.project).await.len(), 1);
}

/// A key is retired only if still stale when the rotation retires it: a ping
/// since the tick's read, or its newer key's retirement by a person, keeps it.
#[sqlx::test]
async fn a_key_stale_when_read_but_not_when_retired_stands(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    // Heard from since the tick read it.
    let p = project(&pool);
    let old = stale(&p, &pool).await;
    let read = stale_keys(&p.state).await;
    let key = old["key"].as_str().expect("the key");
    let (status, body) = p.ping(&format!("/{key}/nightly-report")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rotated = retire_each(&p.state, &[key_of(&read, &p)])
        .await
        .expect("a tick");
    assert_eq!(rotated, Rotated::default(), "a key a ping had just used");
    assert!(all_live(&p).await);
    let (status, body) = p.ping(&format!("/{key}/nightly-report")).await;
    assert_eq!(status, StatusCode::OK, "the key a ping kept: {body}");

    // Its newer key retired by a person since the tick read it.
    let q = project(&pool);
    let (_, older) = q.mint(OWNER).await;
    let (_, newer) = q.mint(ADMIN).await;
    aged(&pool, older["id"].as_str().expect("an id"), 9, Some(8)).await;
    let read = stale_keys(&q.state).await;
    let newer_id = newer["id"].as_str().expect("an id");
    let (status, body) = q
        .send(q.request("DELETE", &format!("{LANE}/{newer_id}"), OWNER))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let rotated = retire_each(&q.state, &[key_of(&read, &q)])
        .await
        .expect("a tick");
    assert_eq!(rotated, Rotated::default(), "the project's last live key");
    let keys = q.list(OWNER).await;
    let live: Vec<&Value> = keys
        .iter()
        .filter(|key| key["retired_at"].is_null())
        .collect();
    assert_eq!(live.len(), 1, "{keys:?}");
    assert_eq!(live[0]["id"], older["id"]);

    for (project, organization) in [(&p.project, &p.organization), (&q.project, &q.organization)] {
        assert_eq!(retired_by_the_rotation(&pool, organization).await, 0);
        let told = notices_waiting(&pool, project).await;
        assert!(
            told.iter()
                .all(|notice| notice["kind"] != "ping_key_retired"),
            "{told:?}"
        );
    }
}

/// How many sessions wait on an advisory lock in this database.
async fn advisory_waits(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks
          WHERE locktype = 'advisory' AND NOT granted
            AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_one(pool)
    .await
    .expect("count the waits on an advisory lock")
}
