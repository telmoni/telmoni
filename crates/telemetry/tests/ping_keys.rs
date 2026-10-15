//! A project's ping keys through the module's own router: owners and admins
//! mint and retire them and members read them, each minting and retiring
//! recorded on the chain under the organization's lock; two may be live at
//! once and never three; a key of another shape, one the table does not
//! hold and one retired open nothing on the ping lane; the rotation retires
//! the older of two a week after its last ping once a newer one stands, and
//! records it as the module's own act; and a ping touches its key.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::types::{PING_KEY_PREFIX, PING_KEY_RANDOM_LEN};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::handler::ping_keys::{LIVE_MAX, retire_stale_keys};
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
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
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
        0,
        "both minted today: nothing to retire"
    );

    // The old key pinged this week: the lines are still moving over.
    aged(&pool, old_id, 9, Some(2)).await;
    assert_eq!(retire_stale_keys(&p.state).await.expect("a tick"), 0);

    // A week past its last ping, with the newer standing: retired.
    aged(&pool, old_id, 9, Some(8)).await;
    assert_eq!(retire_stale_keys(&p.state).await.expect("a tick"), 1);
    let keys = p.list(OWNER).await;
    assert_eq!(keys[0]["id"], new["id"]);
    assert!(keys[0]["retired_at"].is_null());
    assert_eq!(keys[1]["id"], old["id"]);
    assert!(keys[1]["retired_at"].is_string(), "{}", keys[1]);
    assert_eq!(
        retire_stale_keys(&p.state).await.expect("a tick"),
        0,
        "the next tick finds nothing"
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
    assert_eq!(retire_stale_keys(&p.state).await.expect("a tick"), 0);

    // One never pinged is retired a week after its minting once a newer one
    // stands: an unused key is not kept for the lines that never carried it.
    let (status, third) = p.mint(OWNER).await;
    assert_eq!(status, StatusCode::CREATED, "{third}");
    assert_eq!(retire_stale_keys(&p.state).await.expect("a tick"), 1);
    let keys = p.list(OWNER).await;
    assert_eq!(keys[0]["id"], third["id"]);
    assert!(keys[0]["retired_at"].is_null());
    assert_eq!(
        keys.iter()
            .filter(|key| key["retired_at"].is_string())
            .count(),
        2
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
