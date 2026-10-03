//! `PATCH /internal/organization` — who may name an organization, and what
//! names and URLs it takes.
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

/// `PATCH /internal/organization` as `caller`, with `body` as the console
/// would send it: a name, a slug, or both.
async fn update(
    pool: &PgPool,
    caller: &str,
    organization: &str,
    body: Value,
) -> (StatusCode, Value) {
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("PATCH")
                    .uri("/internal/organization")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", organization)
                    .header("content-type", "application/json"),
                pool,
                caller,
            )
            .await
            .body(Body::from(body.to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// The name alone, as the Settings page's Name form sends it.
async fn rename(
    pool: &PgPool,
    caller: &str,
    organization: &str,
    name: &str,
) -> (StatusCode, Value) {
    update(pool, caller, organization, json!({ "name": name })).await
}

/// A lane as `caller`, with `x-organization-id` and a JSON body: the invite
/// lanes, for the organization that has no name yet.
async fn post(
    pool: &PgPool,
    caller: &str,
    organization: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", organization)
                    .header("content-type", "application/json"),
                pool,
                caller,
            )
            .await
            .body(Body::from(body.to_string()))
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
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("Acme Robotics")
    );
}

/// The slug on the row, straight from the table.
async fn stored_slug(pool: &PgPool, organization: &str) -> String {
    sqlx::query_scalar::<_, String>("SELECT slug FROM auth.organizations WHERE external_id = $1")
        .bind(organization)
        .fetch_one(pool)
        .await
        .expect("read the slug back")
}

/// The first name takes the organization off its placeholder: the first free
/// slug the name gives, never another organization's, never a word the
/// console's own paths use. A name that gives none leaves the placeholder.
#[sqlx::test]
async fn the_first_name_gives_the_organization_its_url(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let first = sign_in(&pool, "usr_slug_first", "first@example.test").await;
    let second = sign_in(&pool, "usr_slug_second", "second@example.test").await;
    let third = sign_in(&pool, "usr_slug_third", "third@example.test").await;
    let fourth = sign_in(&pool, "usr_slug_fourth", "fourth@example.test").await;
    assert!(stored_slug(&pool, &first).await.starts_with("org-"));

    let (status, body) = rename(&pool, "usr_slug_first", &first, "Acme Robotics").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], "acme-robotics");
    assert_eq!(stored_slug(&pool, &first).await, "acme-robotics");

    let (_, body) = rename(&pool, "usr_slug_second", &second, "ACME robotics!").await;
    assert_eq!(
        body["slug"], "acme-robotics-2",
        "took the first organization's slug"
    );

    let (_, body) = rename(&pool, "usr_slug_third", &third, "Account").await;
    assert_eq!(
        body["slug"], "account-2",
        "took a word the console's paths use"
    );

    let placeholder = stored_slug(&pool, &fourth).await;
    let (status, body) = rename(&pool, "usr_slug_fourth", &fourth, "株式会社").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "株式会社");
    assert_eq!(
        body["slug"], placeholder,
        "a name with no slug in it left the placeholder, for the owner to replace on Settings"
    );
}

/// ⚠ **A rename moves no URL.** The name and the URL are two settings: once
/// an organization has its URL, every link to its pages stands through any
/// number of renames, as on Vercel.
#[sqlx::test]
async fn a_rename_keeps_the_url(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_rename_keeps";
    let organization = sign_in(&pool, owner, "keeps@example.test").await;
    rename(&pool, owner, &organization, "Acme Robotics").await;
    assert_eq!(stored_slug(&pool, &organization).await, "acme-robotics");

    let (status, body) = rename(&pool, owner, &organization, "Acme Robotics Ltd").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Acme Robotics Ltd");
    assert_eq!(body["slug"], "acme-robotics", "a rename moved the URL");
    assert_eq!(stored_slug(&pool, &organization).await, "acme-robotics");
}

/// The URL is an owner's or admin's to choose: a slug, free, and not one of the
/// console's own words. The answer carries the name too, as the console's
/// Settings show both.
#[sqlx::test]
async fn the_url_is_a_setting_of_its_own(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_url_owner";
    let other = "usr_url_other";
    let organization = sign_in(&pool, owner, "url-owner@example.test").await;
    let others = sign_in(&pool, other, "url-other@example.test").await;
    rename(&pool, owner, &organization, "Acme Robotics").await;
    rename(&pool, other, &others, "Globex").await;

    let (status, body) = update(&pool, owner, &organization, json!({ "slug": "acme" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["slug"], "acme");
    assert_eq!(body["name"], "Acme Robotics");
    assert_eq!(stored_slug(&pool, &organization).await, "acme");

    let (status, body) = update(&pool, owner, &organization, json!({ "slug": "globex" })).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "another organization's URL: {body}"
    );
    assert_eq!(stored_slug(&pool, &organization).await, "acme");

    let (status, _) = update(&pool, owner, &organization, json!({ "slug": "acme" })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the URL it already has is not taken"
    );

    for bad in [
        "Acme",
        "acme_robotics",
        "-acme",
        "acme--robotics",
        "a".repeat(49).as_str(),
    ] {
        let (status, body) = update(&pool, owner, &organization, json!({ "slug": bad })).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "`{bad}` is not a slug: {body}"
        );
    }
    let (status, body) = update(&pool, owner, &organization, json!({ "slug": "account" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a reserved word: {body}");
    assert_eq!(stored_slug(&pool, &organization).await, "acme");

    let (status, _) = update(&pool, owner, &organization, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "nothing to set");

    let (status, body) = update(
        &pool,
        owner,
        &organization,
        json!({ "name": "Acme Robotics Ltd", "slug": "acme-ltd" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Acme Robotics Ltd");
    assert_eq!(body["slug"], "acme-ltd");
}

/// ⚠ **An unnamed organization has only its owner.** The console asks for the
/// name before it opens; the lanes hold the same line, so no invitation ever
/// shows somebody an organization with nothing to call it by.
#[sqlx::test]
async fn an_unnamed_organization_invites_nobody(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_unnamed_owner";
    let organization = sign_in(&pool, owner, "unnamed@example.test").await;
    // A project is the owner's to make before the organization has a name; an
    // invitation to it is not.
    let (status, created) = post(
        &pool,
        owner,
        &organization,
        "/internal/projects",
        json!({ "name": "Platform" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let project = created["id"].as_str().expect("the project's id").to_owned();

    let invite = json!({ "email": "colleague@example.test", "role": "member" });
    let (status, body) = post(
        &pool,
        owner,
        &organization,
        "/internal/organization/invites",
        invite.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = post(
        &pool,
        owner,
        &organization,
        &format!("/internal/projects/{project}/invites"),
        invite.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    rename(&pool, owner, &organization, "Acme Robotics").await;
    let (status, body) = post(
        &pool,
        owner,
        &organization,
        "/internal/organization/invites",
        invite,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "named, it invites: {body}");
}

/// ⚠️ **A name is printed in mail to any address an owner or admin invites**,
/// so it is held to what a display name is: control characters and invisible
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
    assert_eq!(status, StatusCode::OK);
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
    assert_eq!(status, StatusCode::OK);
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
    assert_eq!(status, StatusCode::OK);
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
    assert_ne!(status, StatusCode::OK);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("Before")
    );
}
