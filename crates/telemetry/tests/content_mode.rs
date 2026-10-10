//! The content switch through the module's own router: every role on a
//! project reads its mode, the organization's owner alone changes it, to a
//! mode the switch offers, under the organization that holds the project when
//! the change is written, and every change is recorded on that organization's
//! chain in the same transaction.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, SECRET};
use telmoni_shared::acting::{Acting, ActingProject};
use telmoni_shared::audit::lock_chain;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::{apply_audit_migrations, chain_lock_held};
use telmoni_shared::{
    AuthError, ContentMode, OrganizationId, OrganizationRole, ProjectId, Role, UserId,
};
use telmoni_telemetry::router;

const LANE: &str = "/internal/telemetry/content-mode";
const OWNER: &str = "user_owner";
const ADMIN: &str = "user_admin";
const MEMBER: &str = "user_member";

/// The module's router, auth as it holds it, and the project the roster
/// seats its three people on.
struct Project {
    app: Router,
    auth: Arc<AuthStub>,
    organization: OrganizationId,
    project: ProjectId,
}

/// A project, its organization's chain where its changes are recorded.
async fn project(pool: &PgPool) -> Project {
    apply_audit_migrations(pool).await;
    unaudited(pool)
}

/// A project in a database with no audit chain, where any record fails.
fn unaudited(pool: &PgPool) -> Project {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::probing(pool);
    for (user, role, seat) in [
        (OWNER, OrganizationRole::Owner, None),
        (ADMIN, OrganizationRole::Admin, None),
        (MEMBER, OrganizationRole::Member, Some(Role::Member)),
    ] {
        auth.seat(user, &organization, &project, role, seat);
    }
    Project {
        app: router(common::state(pool, auth.clone())),
        auth,
        organization,
        project,
    }
}

impl Project {
    /// A request as the console sends one: the service secret, the person's
    /// bearer, their organization and the project.
    fn request(&self, method: &str, user: &str, body: Option<Value>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(LANE)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{user}"))
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

    async fn set(&self, user: &str, mode: &str) -> (StatusCode, Value) {
        self.send(self.request("PUT", user, Some(json!({ "content_mode": mode }))))
            .await
    }

    /// What auth answers the owner when it reads them again: the same person
    /// on the same project, in `organization` at `organization_role`, seated
    /// as `role`.
    fn owner_read_again_as(
        &self,
        organization: &OrganizationId,
        organization_role: OrganizationRole,
        role: Role,
    ) -> Acting {
        Acting {
            user_id: UserId::try_new(OWNER).expect("a user id"),
            organization_id: organization.clone(),
            organization_role: Some(organization_role),
            project: Some(ActingProject {
                project_id: self.project.clone(),
                role,
            }),
            session_id: None,
            expires_at: chrono::Utc::now().timestamp() + 3600,
        }
    }
}

/// `organization`'s chain lock, held from a session of `pool`'s until the
/// transaction is dropped, as an accept or a hand-over in flight holds it.
async fn hold_chain(
    pool: &PgPool,
    organization: &OrganizationId,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut holder = pool.begin().await.expect("a holder's transaction");
    lock_chain(&mut holder, organization)
        .await
        .expect("take the organization's chain lock");
    holder
}

/// A project's row as a mode left it, written as the owner.
async fn seed(
    pool: &PgPool,
    project: &ProjectId,
    organization: &OrganizationId,
    mode: ContentMode,
) {
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id, content_mode) \
         VALUES ($1, $2, $3)",
    )
    .bind(project)
    .bind(organization)
    .bind(mode)
    .execute(pool)
    .await
    .expect("seed a project's settings");
}

/// The project's row, as the owner reads it: its organization and its mode.
async fn row(pool: &PgPool, project: &ProjectId) -> Option<(String, String)> {
    sqlx::query_as(
        "SELECT organization_id, content_mode FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await
    .expect("read the row as the owner")
}

/// One row of an organization's chain, as far as a change to a project's
/// mode writes it.
#[derive(Debug, PartialEq)]
struct Recorded {
    actor_id: String,
    action: String,
    resource_kind: String,
    resource_id: String,
    in_project: String,
    metadata: Value,
}

/// Every row of `organization`'s chain, oldest first.
async fn audit(pool: &PgPool, organization: &OrganizationId) -> Vec<Recorded> {
    let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT actor_id, action, resource_kind, resource_id, in_project, metadata::text
           FROM audit.events WHERE organization_id = $1 ORDER BY seq",
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .expect("read the chain as the owner");
    rows.into_iter()
        .map(
            |(actor_id, action, resource_kind, resource_id, in_project, metadata)| Recorded {
                actor_id,
                action,
                resource_kind,
                resource_id,
                in_project,
                metadata: serde_json::from_str(&metadata).expect("details are JSON"),
            },
        )
        .collect()
}

/// The record of the owner switching `project` from one mode to another.
fn switched(project: &ProjectId, from: &str, to: &str) -> Recorded {
    Recorded {
        actor_id: OWNER.to_string(),
        action: "updated".to_string(),
        resource_kind: "project".to_string(),
        resource_id: project.to_string(),
        in_project: project.to_string(),
        metadata: json!({ "content_mode": { "from": from, "to": to } }),
    }
}

#[sqlx::test]
async fn every_role_reads_the_mode_and_the_modes_offered(pool: PgPool) {
    let p = project(&pool).await;
    for user in [OWNER, ADMIN, MEMBER] {
        let (status, body) = p.send(p.request("GET", user, None)).await;
        assert_eq!(status, StatusCode::OK, "{user} could not read the mode");
        assert_eq!(body, json!({ "content_mode": "off", "offered": ["off"] }));
    }

    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;
    let (_, body) = p.send(p.request("GET", MEMBER, None)).await;
    assert_eq!(
        body,
        json!({ "content_mode": "sealed", "offered": ["off"] }),
        "the read answered the default over the project's own row"
    );
}

/// ⚠ The owner's change lands under the project's organization, and its
/// chain records who made it, on which project, and from what to what.
#[sqlx::test]
async fn the_owner_switches_it_and_the_change_is_recorded(pool: PgPool) {
    let p = project(&pool).await;
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;

    let (status, body) = p.set(OWNER, "off").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "content_mode": "off", "offered": ["off"] }));
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "off".to_string()))
    );
    assert_eq!(
        audit(&pool, &p.organization).await,
        [switched(&p.project, "sealed", "off")]
    );
    assert_eq!(
        p.auth.chain_held(),
        [true],
        "auth was asked again without the organization's chain lock held"
    );
}

/// A save of the mode a project already has — `off` on a project with no
/// row reads as one — writes nothing and records nothing: every row on the
/// chain is a switch.
#[sqlx::test]
async fn a_save_that_changes_nothing_writes_and_records_nothing(pool: PgPool) {
    let p = project(&pool).await;
    let (status, body) = p.set(OWNER, "off").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "content_mode": "off", "offered": ["off"] }));
    assert_eq!(row(&pool, &p.project).await, None);

    seed(&pool, &p.project, &p.organization, ContentMode::Off).await;
    assert_eq!(p.set(OWNER, "off").await.0, StatusCode::OK);
    assert!(audit(&pool, &p.organization).await.is_empty());
}

/// ⚠ **Nobody below the owner changes it**, though the matrix lets an admin
/// update a project: the refusal names the role that would, and nothing is
/// written or recorded.
#[sqlx::test]
async fn nobody_below_the_owner_changes_it(pool: PgPool) {
    let p = project(&pool).await;
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;
    for (user, held) in [(ADMIN, "admin"), (MEMBER, "member")] {
        let (status, body) = p.set(user, "off").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{user} changed the mode");
        assert_eq!(body["type"], "/errors/authz/insufficient-role");
        assert_eq!(body["detail"], format!("required owner, have {held}"));
    }
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "sealed".to_string()))
    );
    assert!(audit(&pool, &p.organization).await.is_empty());
    assert_eq!(
        p.auth.read_again(),
        0,
        "somebody below the owner waited on the organization's chain to be refused"
    );
}

/// ⚠ `off` is the one mode offered until a table keeps content and our SDK
/// seals it: the others are refused, to the owner too, and nothing moves.
#[sqlx::test]
async fn a_mode_not_offered_is_refused(pool: PgPool) {
    let p = project(&pool).await;
    for mode in ["on", "sealed"] {
        let (status, body) = p.set(OWNER, mode).await;
        assert_eq!(status, StatusCode::CONFLICT, "{mode} was taken");
        assert_eq!(body["type"], "/errors/auth/conflict");
    }
    assert_eq!(row(&pool, &p.project).await, None);
    assert!(audit(&pool, &p.organization).await.is_empty());
    assert_eq!(
        p.auth.read_again(),
        0,
        "a mode not offered waited on a lock"
    );
}

#[sqlx::test]
async fn a_body_that_is_not_a_change_is_refused(pool: PgPool) {
    let p = project(&pool).await;
    for body in [
        json!({ "content_mode": "plain" }),
        json!({ "content_mode": "OFF" }),
        json!({ "content_mode": "off", "organization_id": "org_other" }),
        json!({}),
    ] {
        let (status, _) = p.send(p.request("PUT", OWNER, Some(body.clone()))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} was taken");
    }
    assert_eq!(row(&pool, &p.project).await, None);
}

/// The organization is the one auth answers for the project, never the
/// header's: a request naming another lands under the project's own.
#[sqlx::test]
async fn the_change_lands_under_the_projects_organization_whatever_the_headers_name(pool: PgPool) {
    let p = project(&pool).await;
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;
    let elsewhere = OrganizationId::new();
    let mut request = p.request("PUT", OWNER, Some(json!({ "content_mode": "off" })));
    request.headers_mut().insert(
        "x-organization-id",
        elsewhere.as_str().parse().expect("a header value"),
    );

    assert_eq!(p.send(request).await.0, StatusCode::OK);
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "off".to_string()))
    );
    assert_eq!(audit(&pool, &p.organization).await.len(), 1);
    assert!(audit(&pool, &elsewhere).await.is_empty());
}

/// ⚠ **A change resolved before the project moved is refused.** A transfer's
/// accept holds the organization's chain lock from its records to its commit,
/// so a change waiting on that lock reads auth again under it; one that finds
/// the project gone writes nothing under the organization it left.
#[sqlx::test]
async fn a_change_resolved_before_the_project_moved_is_refused(pool: PgPool) {
    let p = project(&pool).await;
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;
    // As auth leaves a former owner: a member of the new organization,
    // seated on the project as an admin.
    p.auth.answer_again(Ok(p.owner_read_again_as(
        &OrganizationId::new(),
        OrganizationRole::Member,
        Role::Admin,
    )));

    let (status, body) = p.set(OWNER, "off").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["type"], "/errors/auth/conflict");
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "sealed".to_string()))
    );
    assert!(audit(&pool, &p.organization).await.is_empty());
    assert_eq!(
        p.auth.chain_held(),
        [true],
        "auth was asked again without the organization's chain lock held"
    );
}

/// And one resolved before its organization's ownership changed hands is
/// refused as anyone below the owner is.
#[sqlx::test]
async fn a_change_resolved_before_the_ownership_moved_is_refused(pool: PgPool) {
    let p = project(&pool).await;
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;
    p.auth.answer_again(Ok(p.owner_read_again_as(
        &p.organization,
        OrganizationRole::Admin,
        Role::Admin,
    )));

    let (status, body) = p.set(OWNER, "off").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["type"], "/errors/authz/insufficient-role");
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "sealed".to_string()))
    );
    assert_eq!(p.auth.chain_held(), [true]);
}

/// And one whose project was deleted, or whose session ended, before auth
/// was asked again is refused as auth refuses it, with nothing written.
#[sqlx::test]
async fn a_change_auth_refuses_when_asked_again_writes_nothing(pool: PgPool) {
    let p = project(&pool).await;
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;
    p.auth
        .answer_again(Err(AuthError::BadRequest("project not found".into()).into()));

    assert_eq!(p.set(OWNER, "off").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "sealed".to_string()))
    );
    assert!(audit(&pool, &p.organization).await.is_empty());
}

/// ⚠ The change and its record commit together: where the record cannot be
/// written, neither is the change.
#[sqlx::test]
async fn a_change_that_cannot_be_recorded_is_not_made(pool: PgPool) {
    let p = unaudited(&pool);
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;

    let (status, body) = p.set(OWNER, "off").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["type"], "/errors/internal/database");
    assert_eq!(
        p.auth.chain_held(),
        [true],
        "the change failed before it reached its record"
    );
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "sealed".to_string()))
    );
}

/// Somebody below the owner, and a mode not offered, are refused before the
/// organization's chain lock is taken: refused at once, with the lock held
/// elsewhere the whole time.
#[sqlx::test]
async fn refusals_come_before_the_chain_lock(pool: PgPool) {
    let p = project(&pool).await;
    let holder = hold_chain(&pool, &p.organization).await;
    for (user, mode, refused) in [
        (ADMIN, "off", StatusCode::FORBIDDEN),
        (MEMBER, "off", StatusCode::FORBIDDEN),
        (OWNER, "on", StatusCode::CONFLICT),
    ] {
        let answer = tokio::time::timeout(Duration::from_secs(1), p.set(user, mode))
            .await
            .unwrap_or_else(|_| panic!("{user} setting {mode} waited on the chain lock"));
        assert_eq!(answer.0, refused, "{user} setting {mode}");
    }
    drop(holder);
    assert_eq!(p.auth.read_again(), 0);
}

/// A change that cannot have the organization's chain lock within its wait —
/// a transfer's accept, say, holding it past two seconds — is answered for
/// the owner to try again, and writes nothing.
#[sqlx::test]
async fn a_change_that_waits_too_long_on_the_chain_writes_nothing(pool: PgPool) {
    let p = project(&pool).await;
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;
    let holder = hold_chain(&pool, &p.organization).await;

    let (status, body) = tokio::time::timeout(Duration::from_secs(10), p.set(OWNER, "off"))
        .await
        .expect("the wait on the chain lock gave up on its own");
    drop(holder);
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["type"], "/errors/auth/conflict");
    assert_eq!(
        p.auth.read_again(),
        0,
        "auth was asked again without the lock"
    );
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "sealed".to_string()))
    );
    assert!(audit(&pool, &p.organization).await.is_empty());
}

/// Auth answering again past the change's budget leaves the change refused,
/// nothing written, and the organization's chain lock let go before the
/// answer.
#[sqlx::test]
async fn a_change_auth_answers_too_late_writes_nothing(pool: PgPool) {
    let p = project(&pool).await;
    seed(&pool, &p.project, &p.organization, ContentMode::Sealed).await;
    p.auth.delaying_next_read_again(Duration::from_secs(5));

    let (status, body) = p.set(OWNER, "off").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["type"], "/errors/auth/identity-unavailable");
    assert!(
        !chain_lock_held(&pool, &p.organization).await,
        "the refused change kept the organization's chain lock after it answered"
    );
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "sealed".to_string()))
    );
    assert!(audit(&pool, &p.organization).await.is_empty());
}

/// A row a transfer's failed settling left under the organization the
/// project left is filed under the one that holds it by the owner's next
/// save: a switch recorded on that organization's chain alone, and a save of
/// the mode it has re-filed and recorded nowhere, as the settling is.
#[sqlx::test]
async fn the_owners_save_heals_a_row_left_under_another_organization(pool: PgPool) {
    let p = project(&pool).await;
    let left = OrganizationId::new();
    seed(&pool, &p.project, &left, ContentMode::On).await;

    assert_eq!(p.set(OWNER, "off").await.0, StatusCode::OK);
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "off".to_string()))
    );
    assert_eq!(
        audit(&pool, &p.organization).await,
        [switched(&p.project, "on", "off")]
    );
    assert!(audit(&pool, &left).await.is_empty());

    sqlx::query("UPDATE telemetry.project_settings SET organization_id = $2 WHERE project_id = $1")
        .bind(&p.project)
        .bind(&left)
        .execute(&pool)
        .await
        .expect("leave the row under the other organization again, as the owner");
    assert_eq!(p.set(OWNER, "off").await.0, StatusCode::OK);
    assert_eq!(
        row(&pool, &p.project).await,
        Some((p.organization.to_string(), "off".to_string()))
    );
    assert_eq!(audit(&pool, &p.organization).await.len(), 1);
}

/// The lanes are the console's: without the service secret, or without a
/// project, nothing is read or written.
#[sqlx::test]
async fn the_lanes_need_the_secret_and_a_project(pool: PgPool) {
    let p = project(&pool).await;
    for method in ["GET", "PUT"] {
        let body = (method == "PUT").then(|| json!({ "content_mode": "off" }));
        let mut request = p.request(method, OWNER, body.clone());
        request.headers_mut().remove(SERVICE_SECRET_HEADER);
        assert_eq!(p.send(request).await.0, StatusCode::UNAUTHORIZED);

        let mut request = p.request(method, OWNER, body);
        request.headers_mut().remove("x-project-id");
        assert_eq!(p.send(request).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(row(&pool, &p.project).await, None);
}
