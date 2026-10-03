//! Organization-level members integration tests.
#![allow(
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]
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

/// `POST /me` as `user`, whose identity is already recorded.
async fn me(pool: &PgPool, user: &str) -> Value {
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
    assert_eq!(resp.status(), StatusCode::OK, "/me failed for {user}");
    json_body(resp).await
}

/// Sign somebody in, provisioning the organization they own, then name it, as
/// the console has every owner do before anything else, and make the one
/// project these tests act on. Returns its id, as `/me` names it active.
async fn sign_in(pool: &PgPool, user: &str, email: &str) -> String {
    seed_identity(pool, user, email).await;
    let organization = me(pool, user).await["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned();
    let (status, body) = call_organization(
        pool,
        "PATCH",
        "/internal/organization",
        user,
        &organization,
        Some(json!({ "name": "Acme" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "naming {organization}: {body}");
    let (status, body) = call_organization(
        pool,
        "POST",
        "/internal/projects",
        user,
        &organization,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{user}'s first project: {body}"
    );
    organization
}

async fn call_organization(
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    organization: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("x-organization-id", organization);
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = as_person(req, pool, caller)
        .await
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// ⚠ **THE PROOF THAT THE BEARER GATE IS ACTUALLY MOUNTED**: every other test
/// carries one, so they would all pass without it.
#[sqlx::test]
async fn a_request_without_a_bearer_is_refused_even_with_a_valid_service_secret(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "usr_unsigned_01";
    let organization = sign_in(&pool, owner, "unsigned-owner@example.test").await;

    let req = Request::builder()
        .method("GET")
        .uri("/internal/organization/members")
        .header("x-service-secret", SERVICE_SECRET)
        .header("x-organization-id", &organization)
        .body(Body::empty())
        .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a lane that acts for a person served a request with nobody on it"
    );

    let (status, _) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn owner_lists_roster_and_starts_with_owner_row(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_01";
    let organization = sign_in(&pool, owner, "owner@example.test").await;

    let (status, body) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let members = body["members"].as_array().expect("members array");
    assert_eq!(members.len(), 1);
    assert_eq!(
        members[0]["member_id"], owner,
        "the owner is a row naming the person, not the organization"
    );
    assert_eq!(members[0]["role"], "owner");
    assert_eq!(members[0]["is_owner"], true);
    assert_eq!(members[0]["email"], "owner@example.test");
}

#[sqlx::test]
async fn cannot_invite_with_owner_role(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_02";
    let organization = sign_in(&pool, owner, "owner02@example.test").await;

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &organization,
        Some(json!({ "email": "other@example.test", "role": "owner" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn owner_invites_admin_and_admin_accepts(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_03";
    let admin = "usr_admin_03";
    let admin_email = "admin03@example.test";
    let owner_org = sign_in(&pool, owner, "owner03@example.test").await;
    let admin_org = sign_in(&pool, admin, admin_email).await;

    let (status, body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &owner_org,
        Some(json!({ "email": admin_email, "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let link = body["link"].as_str().expect("invite link");
    let token = link.split("/invite/").nth(1).expect("token");

    let (status, look) = call_organization(
        &pool,
        "POST",
        "/internal/invites/look",
        admin,
        &admin_org,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(look["role"], "admin");
    assert_eq!(look["email"], admin_email);

    let (status, accepted) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        admin,
        &admin_org,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(accepted["organizationId"], owner_org);
    assert_eq!(accepted["inviteId"], body["id"]);
    assert_eq!(accepted["inviterEmail"], "owner03@example.test");
    assert_eq!(accepted["ownerOrganizationId"], owner_org);

    let (status, members_body) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        admin,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = members_body["members"].as_array().expect("members list");
    assert_eq!(members.len(), 2);

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        admin,
        &owner_org,
        Some(json!({ "email": "third@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = call_organization(
        &pool,
        "PUT",
        &format!("/internal/organization/members/{admin}/role"),
        admin,
        &owner_org,
        Some(json!({ "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{admin}"),
        admin,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// **Accepting an ORGANIZATION invitation lands on the chain.**
#[sqlx::test]
async fn accepting_an_organization_invitation_is_on_the_owners_chain(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_chain";
    let admin = "usr_admin_chain";
    let admin_email = "admin_chain@example.test";
    let owner_org = sign_in(&pool, owner, "owner_chain@example.test").await;
    let admin_org = sign_in(&pool, admin, admin_email).await;

    let (status, offer) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &owner_org,
        Some(json!({ "email": admin_email, "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let link = offer["link"].as_str().expect("invite link");
    let token = link.split("/invite/").nth(1).expect("token");

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        admin,
        &admin_org,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (action, kind, scope): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT action, metadata ->> 'kind', metadata ->> 'scope'
           FROM audit.events
          WHERE organization_id = $1
            AND resource_kind = 'member'
            AND resource_id = $2
          ORDER BY seq DESC
          LIMIT 1",
    )
    .bind(&owner_org)
    .bind(admin)
    .fetch_one(&pool)
    .await
    .expect("the accept is on the chain");

    assert_eq!(action, "created");
    assert_eq!(kind.as_deref(), Some("organization_invite_accepted"));
    assert_eq!(scope.as_deref(), Some("organization"));
}

#[sqlx::test]
async fn member_role_has_no_organization_wide_powers(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_04";
    let member = "usr_member_04";
    let member_email = "member04@example.test";
    let owner_org = sign_in(&pool, owner, "owner04@example.test").await;
    let member_org = sign_in(&pool, member, member_email).await;

    let (status, body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &owner_org,
        Some(json!({ "email": member_email, "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let link = body["link"].as_str().expect("invite link");
    let token = link.split("/invite/").nth(1).expect("token");

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        member,
        &member_org,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        member,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = call_organization(
        &pool,
        "GET",
        "/internal/organization/invites",
        member,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        member,
        &owner_org,
        Some(json!({ "email": "other@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn owner_cannot_modify_or_remove_self(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_05";
    let organization = sign_in(&pool, owner, "owner05@example.test").await;

    let (status, body) = call_organization(
        &pool,
        "PUT",
        &format!("/internal/organization/members/{owner}/role"),
        owner,
        &organization,
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("handing the organization over"),
        "refused as the owner's row, not for some other reason: {body}"
    );

    let (status, body) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{owner}"),
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("hand it to an admin before you leave"),
        "an owner leaving is told how to go: {body}"
    );

    let (status, roster) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = roster["members"].as_array().expect("members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["member_id"], owner);
    assert_eq!(
        members[0]["role"], "owner",
        "the owner's row moved: {roster}"
    );
}

#[sqlx::test]
async fn owner_updates_role_and_removes_member(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_06";
    let user = "usr_user_06";
    let user_email = "user06@example.test";
    let owner_org = sign_in(&pool, owner, "owner06@example.test").await;
    let user_org = sign_in(&pool, user, user_email).await;

    let (_, body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &owner_org,
        Some(json!({ "email": user_email, "role": "admin" })),
    )
    .await;
    let link = body["link"].as_str().expect("link");
    let token = link.split("/invite/").nth(1).expect("token");

    call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        user,
        &user_org,
        Some(json!({ "token": token })),
    )
    .await;

    let (status, _) = call_organization(
        &pool,
        "PUT",
        &format!("/internal/organization/members/{user}/role"),
        owner,
        &owner_org,
        Some(json!({ "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, roster) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &owner_org,
        None,
    )
    .await;
    let member_row = roster["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["member_id"] == user)
        .expect("found user in roster");
    assert_eq!(member_row["role"], "member");

    let (status, _) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{user}"),
        owner,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, roster_after) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &owner_org,
        None,
    )
    .await;
    let found = roster_after["members"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["member_id"] == user);
    assert!(!found);
}

#[sqlx::test]
async fn me_returns_organization_memberships(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_me";
    let admin = "usr_admin_me";
    let admin_email = "admin_me@example.test";
    let owner_org = sign_in(&pool, owner, "owner_me@example.test").await;
    let admin_org = sign_in(&pool, admin, admin_email).await;
    let admin_me_initial = me(&pool, admin).await;
    let initial = admin_me_initial["organizations"]
        .as_array()
        .expect("organizations array");
    assert_eq!(
        initial.len(),
        1,
        "only the organization their sign-in provisioned: {admin_me_initial}"
    );
    assert_eq!(initial[0]["organizationId"], admin_org);
    assert_eq!(initial[0]["role"], "owner");

    let (_, body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &owner_org,
        Some(json!({ "email": admin_email, "role": "admin" })),
    )
    .await;
    let link = body["link"].as_str().expect("link");
    let token = link.split("/invite/").nth(1).expect("token");

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        admin,
        &admin_org,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let admin_me_after = me(&pool, admin).await;
    let organizations = admin_me_after["organizations"]
        .as_array()
        .expect("organizations array");
    assert_eq!(organizations.len(), 2, "{admin_me_after}");
    assert_eq!(organizations[0]["organizationId"], admin_org);
    assert_eq!(organizations[0]["role"], "owner");
    assert_eq!(organizations[1]["organizationId"], owner_org);
    assert_eq!(organizations[1]["role"], "admin");
    assert_eq!(organizations[1]["ownerEmail"], "owner_me@example.test");
}

#[sqlx::test]
async fn project_invite_accept_enrolls_organization_member_under_project_owner_only(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_proj";
    let invitee = "usr_invitee_proj";
    let invitee_email = "invitee_proj@example.test";
    let owner_org = sign_in(&pool, owner, "owner_proj@example.test").await;
    let invitee_org = sign_in(&pool, invitee, invitee_email).await;

    let (status, projects_body) =
        call_organization(&pool, "GET", "/internal/projects", owner, &owner_org, None).await;
    assert_eq!(status, StatusCode::OK);
    let projects = projects_body["projects"].as_array().expect("projects");
    let project_id = projects[0]["id"].as_str().expect("project id");

    let req = as_person(
        Request::builder()
            .method("POST")
            .uri(format!("/internal/projects/{project_id}/invites"))
            .header("x-service-secret", SERVICE_SECRET)
            .header("x-organization-id", &owner_org)
            .header("content-type", "application/json"),
        &pool,
        owner,
    )
    .await
    .body(Body::from(
        json!({ "email": invitee_email, "role": "admin" }).to_string(),
    ))
    .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = json_body(resp).await;
    assert_eq!(body["ownerOrganizationId"], owner_org);
    let link = body["link"].as_str().expect("link");
    let token = link.split("/invite/").nth(1).expect("token");

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        invitee,
        &invitee_org,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let invitee_me = me(&pool, invitee).await;
    let organizations = invitee_me["organizations"]
        .as_array()
        .expect("organizations array");
    assert_eq!(organizations.len(), 2, "{invitee_me}");
    assert_eq!(organizations[0]["organizationId"], invitee_org);
    assert_eq!(organizations[0]["role"], "owner");
    assert_eq!(organizations[1]["organizationId"], owner_org);
    assert_eq!(organizations[1]["role"], "member");

    let (status, acct_body) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let acct_members = acct_body["members"]
        .as_array()
        .expect("organization members");
    assert_eq!(acct_members.len(), 2);
    assert_eq!(acct_members[0]["role"], "owner");
    assert_eq!(acct_members[1]["member_id"], invitee);
    assert_eq!(acct_members[1]["role"], "member");

    let (status, inv_acct_body) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        invitee,
        &invitee_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let inv_acct_members = inv_acct_body["members"]
        .as_array()
        .expect("invitee organization members");
    assert_eq!(inv_acct_members.len(), 1);
    assert_eq!(inv_acct_members[0]["member_id"], invitee);
    assert_eq!(inv_acct_members[0]["role"], "owner");

    let (status, inv_projects_body) = call_organization(
        &pool,
        "GET",
        "/internal/projects",
        invitee,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let inv_projects = inv_projects_body["projects"]
        .as_array()
        .expect("inv projects");
    assert_eq!(inv_projects.len(), 1);
    assert_eq!(inv_projects[0]["id"], project_id);
    assert_eq!(inv_projects[0]["role"], "admin");

    let (status, own_projects_body) = call_organization(
        &pool,
        "GET",
        "/internal/projects",
        invitee,
        &invitee_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let own_projects = own_projects_body["projects"]
        .as_array()
        .expect("own projects");
    for p in own_projects {
        assert_ne!(
            p["id"], project_id,
            "owner's project must not leak into invitee's personal organization context"
        );
    }
}

#[sqlx::test]
async fn removing_an_organization_member_ends_their_project_access(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_cascade";
    let invitee = "usr_invitee_cascade";
    let invitee_email = "invitee_cascade@example.test";
    let owner_org = sign_in(&pool, owner, "owner_cascade@example.test").await;
    let invitee_org = sign_in(&pool, invitee, invitee_email).await;

    let (status, projects_body) =
        call_organization(&pool, "GET", "/internal/projects", owner, &owner_org, None).await;
    assert_eq!(status, StatusCode::OK);
    let projects = projects_body["projects"].as_array().expect("projects");
    let project_id = projects[0]["id"].as_str().expect("project id");

    let (_, acct_body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &owner_org,
        Some(json!({ "email": invitee_email, "role": "member" })),
    )
    .await;
    let acct_link = acct_body["link"].as_str().expect("acct link");
    let acct_token = acct_link.split("/invite/").nth(1).expect("token");

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        invitee,
        &invitee_org,
        Some(json!({ "token": acct_token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let req = as_person(
        Request::builder()
            .method("POST")
            .uri(format!("/internal/projects/{project_id}/invites"))
            .header("x-service-secret", SERVICE_SECRET)
            .header("x-organization-id", &owner_org)
            .header("content-type", "application/json"),
        &pool,
        owner,
    )
    .await
    .body(Body::from(
        json!({ "email": invitee_email, "role": "admin" }).to_string(),
    ))
    .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = json_body(resp).await;
    let link = body["link"].as_str().expect("link");
    let token = link.split("/invite/").nth(1).expect("token");

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        invitee,
        &invitee_org,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = call_organization(
        &pool,
        "GET",
        &format!("/internal/projects/{project_id}/members"),
        invitee,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, inv_projects_body) = call_organization(
        &pool,
        "GET",
        "/internal/projects",
        invitee,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let inv_projects = inv_projects_body["projects"].as_array().expect("projects");
    assert_eq!(inv_projects.len(), 1);

    let (status, _) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{invitee}"),
        owner,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = call_organization(
        &pool,
        "GET",
        &format!("/internal/projects/{project_id}/members"),
        invitee,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, inv_projects_after) = call_organization(
        &pool,
        "GET",
        "/internal/projects",
        invitee,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after_projects = inv_projects_after["projects"].as_array().expect("projects");
    assert!(after_projects.is_empty());
}

/// ⚠ **THE DOOR OUT.**
#[sqlx::test]
async fn a_member_leaves_an_organization_and_its_projects_with_them(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_leave";
    let member = "usr_member_leave";
    let member_email = "member_leave@example.test";
    let owner_org = sign_in(&pool, owner, "owner_leave@example.test").await;
    let member_org = sign_in(&pool, member, member_email).await;

    let (status, projects_body) =
        call_organization(&pool, "GET", "/internal/projects", owner, &owner_org, None).await;
    assert_eq!(status, StatusCode::OK);
    let project_id = projects_body["projects"][0]["id"]
        .as_str()
        .expect("the owner's project")
        .to_owned();

    let req = as_person(
        Request::builder()
            .method("POST")
            .uri(format!("/internal/projects/{project_id}/invites"))
            .header("x-service-secret", SERVICE_SECRET)
            .header("x-organization-id", &owner_org)
            .header("content-type", "application/json"),
        &pool,
        owner,
    )
    .await
    .body(Body::from(
        json!({ "email": member_email, "role": "admin" }).to_string(),
    ))
    .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = json_body(resp).await;
    let token = body["link"]
        .as_str()
        .expect("link")
        .split("/invite/")
        .nth(1)
        .expect("token")
        .to_owned();

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        member,
        &member_org,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let seats = |who: &'static str| {
        let pool = pool.clone();
        let organization = owner_org.clone();
        async move {
            let roster: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM auth.organization_members
                  WHERE organization_id = $1 AND user_id = $2",
            )
            .bind(&organization)
            .bind(who)
            .fetch_one(&pool)
            .await
            .expect("roster read");
            let projects: i64 =
                sqlx::query_scalar("SELECT count(*) FROM auth.project_members WHERE user_id = $1")
                    .bind(who)
                    .fetch_one(&pool)
                    .await
                    .expect("project read");
            (roster, projects)
        }
    };

    assert_eq!(
        seats(member).await,
        (1, 1),
        "the accept should have seated them on the project and the roster both"
    );

    let (status, _) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{member}"),
        member,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(
        seats(member).await,
        (0, 0),
        "leaving took the roster row and must take the project seats with it"
    );

    let kind: Option<String> = sqlx::query_scalar(
        "SELECT metadata ->> 'kind' FROM audit.events
          WHERE organization_id = $1 AND resource_id = $2
            AND metadata ->> 'scope' = 'organization'
          ORDER BY seq DESC LIMIT 1",
    )
    .bind(&owner_org)
    .bind(member)
    .fetch_one(&pool)
    .await
    .expect("the leave is on the owner's chain");
    assert_eq!(kind.as_deref(), Some("left"));
}

/// ⚠ **THE INVARIANT THAT MAKES THE DOOR SAFE, and it is structural.**
#[sqlx::test]
async fn an_organization_keeps_its_owner_when_everybody_else_leaves(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_last";
    let admin = "usr_admin_last";
    let member = "usr_member_last";
    let owner_org = sign_in(&pool, owner, "owner_last@example.test").await;
    let admin_org = sign_in(&pool, admin, "admin_last@example.test").await;
    let member_org = sign_in(&pool, member, "member_last@example.test").await;

    for (who, who_org, email, role) in [
        (admin, &admin_org, "admin_last@example.test", "admin"),
        (member, &member_org, "member_last@example.test", "member"),
    ] {
        let (_, body) = call_organization(
            &pool,
            "POST",
            "/internal/organization/invites",
            owner,
            &owner_org,
            Some(json!({ "email": email, "role": role })),
        )
        .await;
        let token = body["link"]
            .as_str()
            .expect("link")
            .split("/invite/")
            .nth(1)
            .expect("token")
            .to_owned();
        let (status, _) = call_organization(
            &pool,
            "POST",
            "/internal/invites/accept",
            who,
            who_org,
            Some(json!({ "token": token })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    for who in [admin, member] {
        let (status, body) = call_organization(
            &pool,
            "DELETE",
            &format!("/internal/organization/members/{who}"),
            who,
            &owner_org,
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{who} could not leave: {body}"
        );
    }

    let roster: Vec<(String, String)> = sqlx::query_as(
        "SELECT user_id, role FROM auth.organization_members WHERE organization_id = $1",
    )
    .bind(&owner_org)
    .fetch_all(&pool)
    .await
    .expect("roster read");
    assert_eq!(
        roster,
        vec![(owner.to_owned(), "owner".to_owned())],
        "everybody else left, so the owner's row is all the roster holds"
    );

    let (status, listing) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the owner lost the roster read: {listing}"
    );

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &owner_org,
        Some(json!({ "email": "again@example.test", "role": "admin" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the owner could not invite anybody back into their own organization"
    );

    let (status, _) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{owner}"),
        owner,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The door out is your own: a member who may leave cannot unseat a sibling.
#[sqlx::test]
async fn leaving_is_the_only_removal_a_member_may_make(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_solo";
    let one = "usr_member_one";
    let two = "usr_member_two";
    let owner_org = sign_in(&pool, owner, "owner_solo@example.test").await;
    let one_org = sign_in(&pool, one, "one_solo@example.test").await;
    let two_org = sign_in(&pool, two, "two_solo@example.test").await;

    for (who, who_org, email, role) in [
        (one, &one_org, "one_solo@example.test", "member"),
        (two, &two_org, "two_solo@example.test", "member"),
    ] {
        let (_, body) = call_organization(
            &pool,
            "POST",
            "/internal/organization/invites",
            owner,
            &owner_org,
            Some(json!({ "email": email, "role": role })),
        )
        .await;
        let token = body["link"]
            .as_str()
            .expect("link")
            .split("/invite/")
            .nth(1)
            .expect("token")
            .to_owned();
        let (status, _) = call_organization(
            &pool,
            "POST",
            "/internal/invites/accept",
            who,
            who_org,
            Some(json!({ "token": token })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    for (caller, target) in [(one, two), (two, one)] {
        let (status, body) = call_organization(
            &pool,
            "DELETE",
            &format!("/internal/organization/members/{target}"),
            caller,
            &owner_org,
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{caller} unseated {target}: {body}"
        );
    }

    let still_there: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_members
          WHERE organization_id = $1 AND user_id IN ($2, $3)",
    )
    .bind(&owner_org)
    .bind(one)
    .bind(two)
    .fetch_one(&pool)
    .await
    .expect("roster read");
    assert_eq!(still_there, 2, "both are still on the roster");
}

#[sqlx::test]
async fn organization_removal_leaves_other_organizations_untouched(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner1 = "usr_owner1_iso";
    let owner2 = "usr_owner2_iso";
    let invitee = "usr_invitee_iso";
    let invitee_email = "invitee_iso@example.test";
    let owner1_org = sign_in(&pool, owner1, "owner1_iso@example.test").await;
    let owner2_org = sign_in(&pool, owner2, "owner2_iso@example.test").await;
    let invitee_org = sign_in(&pool, invitee, invitee_email).await;

    let (status, p1_body) = call_organization(
        &pool,
        "GET",
        "/internal/projects",
        owner1,
        &owner1_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let project_id1 = p1_body["projects"][0]["id"].as_str().expect("p1 id");

    let (status, p2_body) = call_organization(
        &pool,
        "GET",
        "/internal/projects",
        owner2,
        &owner2_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let project_id2 = p2_body["projects"][0]["id"].as_str().expect("p2 id");

    let (_, acct1_body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner1,
        &owner1_org,
        Some(json!({ "email": invitee_email, "role": "member" })),
    )
    .await;
    let acct1_token = acct1_body["link"]
        .as_str()
        .unwrap()
        .split("/invite/")
        .nth(1)
        .unwrap()
        .to_string();

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        invitee,
        &invitee_org,
        Some(json!({ "token": acct1_token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, acct2_body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner2,
        &owner2_org,
        Some(json!({ "email": invitee_email, "role": "member" })),
    )
    .await;
    let acct2_token = acct2_body["link"]
        .as_str()
        .unwrap()
        .split("/invite/")
        .nth(1)
        .unwrap()
        .to_string();

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        invitee,
        &invitee_org,
        Some(json!({ "token": acct2_token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let req1 = as_person(
        Request::builder()
            .method("POST")
            .uri(format!("/internal/projects/{project_id1}/invites"))
            .header("x-service-secret", SERVICE_SECRET)
            .header("x-organization-id", &owner1_org)
            .header("content-type", "application/json"),
        &pool,
        owner1,
    )
    .await
    .body(Body::from(
        json!({ "email": invitee_email, "role": "admin" }).to_string(),
    ))
    .unwrap();
    let resp1 = app(pool.clone()).oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::CREATED);
    let token1 = json_body(resp1).await["link"]
        .as_str()
        .unwrap()
        .split("/invite/")
        .nth(1)
        .unwrap()
        .to_string();

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        invitee,
        &invitee_org,
        Some(json!({ "token": token1 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let req2 = as_person(
        Request::builder()
            .method("POST")
            .uri(format!("/internal/projects/{project_id2}/invites"))
            .header("x-service-secret", SERVICE_SECRET)
            .header("x-organization-id", &owner2_org)
            .header("content-type", "application/json"),
        &pool,
        owner2,
    )
    .await
    .body(Body::from(
        json!({ "email": invitee_email, "role": "admin" }).to_string(),
    ))
    .unwrap();
    let resp2 = app(pool.clone()).oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::CREATED);
    let token2 = json_body(resp2).await["link"]
        .as_str()
        .unwrap()
        .split("/invite/")
        .nth(1)
        .unwrap()
        .to_string();

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/invites/accept",
        invitee,
        &invitee_org,
        Some(json!({ "token": token2 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = call_organization(
        &pool,
        "GET",
        &format!("/internal/projects/{project_id1}/members"),
        invitee,
        &owner1_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call_organization(
        &pool,
        "GET",
        &format!("/internal/projects/{project_id2}/members"),
        invitee,
        &owner2_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{invitee}"),
        owner1,
        &owner1_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = call_organization(
        &pool,
        "GET",
        &format!("/internal/projects/{project_id1}/members"),
        invitee,
        &owner1_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, p1_after) = call_organization(
        &pool,
        "GET",
        "/internal/projects",
        invitee,
        &owner1_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(p1_after["projects"].as_array().unwrap().is_empty());

    let (status, _) = call_organization(
        &pool,
        "GET",
        &format!("/internal/projects/{project_id2}/members"),
        invitee,
        &owner2_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, p2_after) = call_organization(
        &pool,
        "GET",
        "/internal/projects",
        invitee,
        &owner2_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let p2_projects = p2_after["projects"].as_array().unwrap();
    assert_eq!(p2_projects.len(), 1);
    assert_eq!(p2_projects[0]["id"], project_id2);
}

/// An organization admin works every project at admin, from one organization
/// row and no project row.
#[sqlx::test]
async fn organization_admin_holds_admin_on_every_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_adm";
    let admin = "usr_admin_adm";
    let member = "usr_member_adm";
    let owner_org = sign_in(&pool, owner, "owner_adm@example.test").await;
    let admin_org = sign_in(&pool, admin, "admin_adm@example.test").await;
    let member_org = sign_in(&pool, member, "member_adm@example.test").await;

    let (status, body) =
        call_organization(&pool, "GET", "/internal/projects", owner, &owner_org, None).await;
    assert_eq!(status, StatusCode::OK);
    let project_id = body["projects"][0]["id"]
        .as_str()
        .expect("project id")
        .to_string();

    for (user, user_org, email, role) in [
        (admin, &admin_org, "admin_adm@example.test", "admin"),
        (member, &member_org, "member_adm@example.test", "member"),
    ] {
        let (status, body) = call_organization(
            &pool,
            "POST",
            "/internal/organization/invites",
            owner,
            &owner_org,
            Some(json!({ "email": email, "role": role })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let token = body["link"]
            .as_str()
            .expect("link")
            .split("/invite/")
            .nth(1)
            .expect("token")
            .to_string();
        let (status, _) = call_organization(
            &pool,
            "POST",
            "/internal/invites/accept",
            user,
            user_org,
            Some(json!({ "token": token })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, body) =
        call_organization(&pool, "GET", "/internal/projects", admin, &owner_org, None).await;
    assert_eq!(status, StatusCode::OK);
    let listed = body["projects"].as_array().expect("projects");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], project_id.as_str());
    assert_eq!(listed[0]["role"], "admin");

    let (status, _) = call_organization(
        &pool,
        "GET",
        &format!("/internal/projects/{project_id}/members"),
        admin,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call_organization(
        &pool,
        "POST",
        &format!("/internal/projects/{project_id}/invites"),
        admin,
        &owner_org,
        Some(json!({ "email": "third_adm@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = call_organization(
        &pool,
        "POST",
        &format!("/internal/projects/{project_id}/invites"),
        member,
        &owner_org,
        Some(json!({ "email": "fourth@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) =
        call_organization(&pool, "GET", "/internal/projects", member, &owner_org, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["projects"].as_array().expect("projects").is_empty());
    let (status, _) = call_organization(
        &pool,
        "GET",
        &format!("/internal/projects/{project_id}/members"),
        member,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) =
        call_organization(&pool, "GET", "/internal/projects", admin, &admin_org, None).await;
    assert_eq!(status, StatusCode::OK);
    for p in body["projects"].as_array().expect("projects") {
        assert_ne!(p["id"], project_id.as_str());
    }
}

/// Revoking answers the offer's address, so the BFF can tell that person's tabs.
#[sqlx::test]
async fn revoking_an_organization_invite_answers_who_it_was_sent_to(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_rev";
    let organization = sign_in(&pool, owner, "owner_rev@example.test").await;

    let (status, body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &organization,
        Some(json!({ "email": "invitee_rev@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invite_id = body["id"].as_str().expect("invite id").to_string();
    assert!(
        body["expiresAt"].is_string(),
        "the expiry the invitee's list will show, so the BFF need not invent one"
    );
    assert_eq!(
        body["ownerOrganizationId"], organization,
        "the organization whose roster hears an offer left it — auth's record, not the caller's context"
    );

    let (status, revoked) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/invites/{invite_id}"),
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked["inviteId"], invite_id.as_str());
    assert_eq!(revoked["email"], "invitee_rev@example.test");
    assert_eq!(revoked["ownerOrganizationId"], organization);

    let (status, _) = call_organization(
        &pool,
        "DELETE",
        &format!("/internal/organization/invites/{invite_id}"),
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// ⚠ **An invitation goes to an address, so only a verified one takes it.**
/// The recorded address is whatever the provider last asserted, verified or
/// not — a refresh writes both — and matching an unverified one would hand the
/// seat to whoever typed it into their account.
#[sqlx::test]
async fn an_unverified_address_cannot_take_an_invitation_sent_to_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_unverified";
    let taker = "usr_taker_unverified";
    let owner_org = sign_in(&pool, owner, "owner_unverified@example.test").await;
    let taker_org = sign_in(&pool, taker, "taker_unverified@example.test").await;
    let (status, body) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &owner_org,
        Some(json!({ "email": "taker_unverified@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let invite = body["id"].as_str().expect("invite id").to_string();
    sqlx::query("UPDATE auth.identities SET email_verified = false WHERE user_id = $1")
        .bind(taker)
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = call_organization(
        &pool,
        "POST",
        &format!("/internal/me/invites/{invite}/accept"),
        taker,
        &taker_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let joined: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_members WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(&owner_org)
    .bind(taker)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(joined, 0, "an unverified address took the seat");
}

/// The in-app lane for an organization offer, accepted and declined.
#[sqlx::test]
async fn incoming_organization_invites_are_accepted_and_declined_in_app(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_inapp";
    let taker = "usr_taker_inapp";
    let refuser = "usr_refuser_inapp";
    let owner_org = sign_in(&pool, owner, "owner_inapp@example.test").await;
    let taker_org = sign_in(&pool, taker, "taker_inapp@example.test").await;
    let refuser_org = sign_in(&pool, refuser, "refuser_inapp@example.test").await;

    let mut ids = Vec::new();
    for email in ["taker_inapp@example.test", "refuser_inapp@example.test"] {
        let (status, body) = call_organization(
            &pool,
            "POST",
            "/internal/organization/invites",
            owner,
            &owner_org,
            Some(json!({ "email": email, "role": "member" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        ids.push(body["id"].as_str().expect("invite id").to_string());
    }

    let taker_me = me(&pool, taker).await;
    let offers = taker_me["incomingInvites"]
        .as_array()
        .expect("incomingInvites");
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0]["id"], ids[0].as_str());
    assert_eq!(offers[0]["scope"], "organization");
    assert_eq!(offers[0]["targetId"], owner_org);

    let (status, accepted) = call_organization(
        &pool,
        "POST",
        &format!("/internal/me/invites/{}/accept", ids[0]),
        taker,
        &taker_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["targetId"], owner_org);
    assert_eq!(accepted["inviteId"], ids[0].as_str());
    assert_eq!(accepted["inviterEmail"], "owner_inapp@example.test");
    assert_eq!(accepted["ownerOrganizationId"], owner_org);

    let (status, declined) = call_organization(
        &pool,
        "POST",
        &format!("/internal/me/invites/{}/decline", ids[1]),
        refuser,
        &refuser_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(declined["status"], "declined");
    assert_eq!(declined["targetId"], owner_org);
    assert_eq!(declined["inviteId"], ids[1].as_str());
    assert_eq!(declined["inviterEmail"], "owner_inapp@example.test");
    assert_eq!(declined["ownerOrganizationId"], owner_org);

    let (status, roster) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &owner_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = roster["members"].as_array().expect("members");
    assert!(
        members
            .iter()
            .any(|m| m["member_id"] == taker && m["role"] == "member")
    );
    assert!(members.iter().all(|m| m["member_id"] != refuser));
    for (user, user_org) in [(taker, &taker_org), (refuser, &refuser_org)] {
        let (status, list) =
            call_organization(&pool, "GET", "/internal/me/invites", user, user_org, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(list["invites"].as_array().expect("invites").is_empty());
    }
}

/// ⚠ **THE REFUSAL EVERY ENDPOINT IN THIS LANE SHARES, walked end to end.**
#[sqlx::test]
async fn a_stranger_is_refused_by_every_endpoint_in_this_lane(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_orgstranger_owner";
    let stranger = "usr_orgstranger_nobody";
    let organization = sign_in(&pool, owner, "orgstranger-owner@example.test").await;
    sign_in(&pool, stranger, "orgstranger-nobody@example.test").await;

    let (status, _) = call_organization(
        &pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &organization,
        Some(json!({ "email": "guest@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "fixture invite was not created"
    );

    let attempts: &[(&str, &str, Option<Value>)] = &[
        ("GET", "/internal/organization/members", None),
        (
            "PUT",
            "/internal/organization/members/usr_orgstranger_victim/role",
            Some(json!({ "role": "admin" })),
        ),
        (
            "DELETE",
            "/internal/organization/members/usr_orgstranger_victim",
            None,
        ),
        ("GET", "/internal/organization/invites", None),
        (
            "POST",
            "/internal/organization/invites",
            Some(json!({ "email": "intruder@example.test", "role": "admin" })),
        ),
        (
            "DELETE",
            "/internal/organization/invites/00000000-0000-0000-0000-000000000000",
            None,
        ),
    ];

    for (method, uri, body) in attempts {
        let (status, _) =
            call_organization(&pool, method, uri, stranger, &organization, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} let a stranger in with a forged x-organization-id",
        );
    }

    let (status, body) = call_organization(
        &pool,
        "GET",
        "/internal/organization/members",
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["members"].as_array().expect("members").len(), 1);

    let (status, body) = call_organization(
        &pool,
        "GET",
        "/internal/organization/invites",
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let invites = body["invites"].as_array().expect("invites");
    assert_eq!(invites.len(), 1, "the stranger's invite reached the roster");
    assert_eq!(invites[0]["email"], "guest@example.test");
}
