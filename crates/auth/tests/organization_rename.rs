//! `PUT /internal/organization/name` — who may name an organization.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test scaffolding: asserts and fixture setup"
)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::test_provider::{ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};

const SERVICE_SECRET: &str = "test-service-secret";

fn app(pool: PgPool) -> Router {
    let config = Config {
        database_url: String::new(),
        service_secret: SERVICE_SECRET.into(),
        service_secret_next: None,
        allow_test_session: false,
        redirect_uri: "http://localhost:3000/auth/callback".into(),
        app_url: "http://localhost:3000".into(),
        mail_from: "Telmoni <test@example.com>".into(),
        support_email: None,
        deletion_tail_budget_ms: 8_000,
    };
    let db = service_pool(&pool, "auth");
    let state = Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(Arc::new(
            ScriptedProvider::new(),
        ))),
        db,
        config,
        siblings: telmoni_auth::Siblings::default(),
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(Arc::new(
            telmoni_shared::mail::NoopSender,
        ))),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
    });
    router(state)
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    if bytes.is_empty() {
        return json!(null);
    }
    serde_json::from_slice(&bytes).expect("JSON body")
}

/// Sign somebody in, provisioning the organization they own. Returns its id,
/// as `/me` names it active.
async fn sign_in(pool: &PgPool, user: &str, email: &str) -> String {
    seed_identity(pool, user, email).await;
    let body = json!({});
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/me")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("content-type", "application/json"),
                pool,
                user,
            )
            .await
            .body(Body::from(body.to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "sign-in failed for {user}");
    json_body(resp)
        .await
        .get("activeOrganizationId")
        .and_then(Value::as_str)
        .expect("/me names the active organization")
        .to_owned()
}

/// Seat `member` on `organization` at `role`, as its owner `owner` would.
async fn seat(pool: &PgPool, organization: &str, owner: &str, member: &str, role: &str) {
    sqlx::query(
        "INSERT INTO auth.organization_members (id, organization_id, user_id, role, added_by, shard_key)
         VALUES (gen_random_uuid(), $1, $2, $3, $4, gen_random_uuid())",
    )
    .bind(organization)
    .bind(member)
    .bind(role)
    .bind(owner)
    .execute(pool)
    .await
    .expect("seat the member");
}

/// The name on the row, straight from the table — never from a response body.
async fn stored_name(pool: &PgPool, organization: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT name FROM auth.organizations WHERE external_id = $1",
    )
    .bind(organization)
    .fetch_one(pool)
    .await
    .expect("read the organization back")
}

async fn rename(
    pool: &PgPool,
    caller: &str,
    organization: &str,
    name: &str,
) -> (StatusCode, Value) {
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("PUT")
                    .uri("/internal/organization/name")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", organization)
                    .header("content-type", "application/json"),
                pool,
                caller,
            )
            .await
            .body(Body::from(json!({ "name": name }).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

#[sqlx::test]
async fn an_owner_names_their_own_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_rename_owner";
    let organization = sign_in(&pool, owner, "owner@example.test").await;

    let (status, _) = rename(&pool, owner, &organization, "Acme Robotics").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("Acme Robotics")
    );
}

/// ⚠️ **A name is printed in mail to any address the owner invites**, so it is
/// held to what a display name is: control characters and invisible
/// formatting — the line breaks, bidi overrides and zero-width characters that
/// make one name read as another — are dropped, and a name of nothing else is
/// no name.
#[sqlx::test]
async fn a_name_keeps_nothing_invisible(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_rename_sanitised";
    let organization = sign_in(&pool, owner, "sanitised@example.test").await;

    let (status, _) = rename(
        &pool,
        owner,
        &organization,
        "Acme\u{202E}Corp\r\nBcc: x@example.test\u{200B}",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("AcmeCorpBcc: x@example.test")
    );

    let (status, _) = rename(&pool, owner, &organization, "\u{202E}\u{200B}\n").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("AcmeCorpBcc: x@example.test"),
        "a name of nothing visible replaced a real one"
    );
}

/// ⚠️ **THE REFUSAL THE CONSOLE BUG WOULD HAVE HIT**: a member in somebody
/// else's organization cannot rename it.
#[sqlx::test]
async fn a_member_of_another_organization_cannot_rename_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_rename_victim";
    let member = "usr_rename_member";
    let organization = sign_in(&pool, owner, "victim@example.test").await;
    sign_in(&pool, member, "member@example.test").await;
    seat(&pool, &organization, owner, member, "member").await;
    rename(&pool, owner, &organization, "Untouched").await;

    let (status, _) = rename(&pool, member, &organization, "Renamed By A Member").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("Untouched"),
        "a refused rename still moved the name",
    );
}

/// An ADMIN of the organization can rename it.
#[sqlx::test]
async fn an_admin_of_the_organization_can_rename_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_rename_victim2";
    let admin = "usr_rename_admin";
    let organization = sign_in(&pool, owner, "victim2@example.test").await;
    sign_in(&pool, admin, "admin@example.test").await;
    seat(&pool, &organization, owner, admin, "admin").await;
    rename(&pool, owner, &organization, "Untouched").await;

    let (status, _) = rename(&pool, admin, &organization, "Renamed By An Admin").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("Renamed By An Admin")
    );
}

/// ⚠️ **A COMPLETE STRANGER**, as a forged header produces, is refused.
#[sqlx::test]
async fn a_stranger_cannot_rename_an_organization_they_have_no_row_in(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_rename_victim3";
    let stranger = "usr_rename_stranger";
    let organization = sign_in(&pool, owner, "victim3@example.test").await;
    let strangers_own = sign_in(&pool, stranger, "stranger@example.test").await;
    rename(&pool, owner, &organization, "Untouched").await;

    let (status, _) = rename(&pool, stranger, &organization, "Renamed By A Stranger").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("Untouched"),
        "a stranger's forged x-organization-id renamed somebody else's organization",
    );
    assert_eq!(stored_name(&pool, &strangers_own).await, None);
}

/// The header pair decides: a stranger naming their OWN organization renames
/// only their own row.
#[sqlx::test]
async fn the_refusals_are_the_membership_check_and_not_a_dead_fixture(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_rename_ctl_owner";
    let other = "usr_rename_ctl_other";
    let owners = sign_in(&pool, owner, "ctl-owner@example.test").await;
    let others = sign_in(&pool, other, "ctl-other@example.test").await;

    let (status, _) = rename(&pool, other, &others, "Their Own").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        stored_name(&pool, &others).await.as_deref(),
        Some("Their Own")
    );
    assert_eq!(stored_name(&pool, &owners).await, None);
}

/// An organization on its way out takes no new name: a 404, not a silent no-op.
#[sqlx::test]
async fn an_organization_being_deleted_is_not_renamed(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_rename_pending";
    let organization = sign_in(&pool, owner, "pending@example.test").await;
    rename(&pool, owner, &organization, "Before").await;
    sqlx::query(
        "UPDATE auth.organizations
            SET status = 'pending_deletion', deletion_requested_at = now(),
                erase_after = now() + interval '14 days', deletion_kind = 'owner'
          WHERE external_id = $1",
    )
    .bind(&organization)
    .execute(&pool)
    .await
    .expect("mark pending deletion");

    let (status, _) = rename(&pool, owner, &organization, "After").await;
    assert_ne!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("Before")
    );
}
