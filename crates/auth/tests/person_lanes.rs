//! The bearer gate on every lane that acts for a person. The service secret
//! proves the hop began inside the platform and nothing about WHO is asking,
//! so a request with the secret and no bearer must be refused on each of them.
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

use telmoni_auth::test_provider::{ScriptedProvider, as_person, bearer, bearer_in};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};

const SERVICE_SECRET: &str = "test-service-secret";
const USER: &str = "user_person_lanes";
const EMAIL: &str = "lanes@example.test";

fn app(pool: PgPool) -> Router {
    app_with(pool, false)
}

/// The router, with the test door open when `allow_test_session`.
fn app_with(pool: PgPool, allow_test_session: bool) -> Router {
    let config = Config {
        database_url: String::new(),
        service_secret: SERVICE_SECRET.into(),
        service_secret_next: None,
        allow_test_session,
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

/// `POST /me` with `authorization` as given, or absent.
async fn me(pool: &PgPool, authorization: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/me")
        .header("x-service-secret", SERVICE_SECRET)
        .header("content-type", "application/json");
    if let Some(authorization) = authorization {
        req = req.header("authorization", authorization);
    }
    let resp = app(pool.clone())
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// The organization `/me` made the caller active in — the one they were
/// provisioned with, on a first sign-in.
fn active_organization(me: &Value) -> String {
    me["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned()
}

/// Every route in the router's `person` group and `/me`. A route added to
/// the group belongs here too.
const PERSON_ROUTES: &[(&str, &str)] = &[
    ("GET", "/internal/auth/sessions"),
    ("POST", "/internal/auth/sessions/{id}/revoke"),
    ("GET", "/internal/projects"),
    ("POST", "/internal/projects"),
    ("GET", "/internal/projects/everywhere"),
    ("PATCH", "/internal/projects/{project_id}"),
    ("DELETE", "/internal/projects/{project_id}"),
    ("GET", "/internal/projects/{project_id}/members"),
    (
        "PUT",
        "/internal/projects/{project_id}/members/{member_id}/role",
    ),
    (
        "DELETE",
        "/internal/projects/{project_id}/members/{member_id}",
    ),
    ("POST", "/internal/projects/{project_id}/transfer"),
    ("DELETE", "/internal/projects/{project_id}/transfer"),
    ("POST", "/internal/projects/{project_id}/transfer/accept"),
    ("POST", "/internal/projects/{project_id}/transfer/decline"),
    ("POST", "/internal/organizations"),
    ("DELETE", "/internal/organization"),
    ("POST", "/internal/organization/deletion-code"),
    ("PATCH", "/internal/organization"),
    ("POST", "/internal/organization/owner-transfer"),
    ("DELETE", "/internal/organization/owner-transfer"),
    ("POST", "/internal/organization/owner-transfer/accept"),
    ("POST", "/internal/organization/owner-transfer/decline"),
    ("GET", "/internal/organization/export"),
    ("GET", "/internal/organization/members"),
    ("PUT", "/internal/organization/members/{member_id}/role"),
    ("DELETE", "/internal/organization/members/{member_id}"),
    ("GET", "/internal/organization/invites"),
    ("POST", "/internal/organization/invites"),
    ("DELETE", "/internal/organization/invites/{invite_id}"),
    ("GET", "/internal/projects/{project_id}/invites"),
    ("POST", "/internal/projects/{project_id}/invites"),
    (
        "DELETE",
        "/internal/projects/{project_id}/invites/{invite_id}",
    ),
    ("POST", "/internal/invites/accept"),
    ("DELETE", "/internal/me"),
    ("POST", "/internal/me/deletion-code"),
    ("POST", "/internal/me/password-reset"),
    ("POST", "/internal/me/email-change"),
    ("POST", "/internal/me/email-change/confirm"),
    ("PUT", "/internal/me/analytics"),
    ("PUT", "/internal/me/default-organization"),
    ("GET", "/internal/me/invites"),
    ("POST", "/internal/me/invites/{invite_id}/accept"),
    ("POST", "/internal/me/invites/{invite_id}/decline"),
    ("DELETE", "/internal/memberships/{project_id}"),
    ("POST", "/internal/tokens"),
    ("GET", "/internal/tokens"),
    ("DELETE", "/internal/tokens/{token_id}"),
    ("POST", "/internal/tokens/{token_id}/rotate"),
    ("GET", "/internal/audit/projects/{project_id}"),
    ("GET", "/internal/audit/organizations/{organization_id}"),
    ("POST", "/me"),
];

/// A route pattern with every parameter filled in from the caller's own
/// organization, so a served request could only be a served one.
fn fill(pattern: &str, organization: &str, project: &str) -> String {
    pattern
        .replace("{project_id}", project)
        .replace("{organization_id}", organization)
        .replace("{member_id}", "user_somebody_else")
        .replace("{invite_id}", "00000000-0000-0000-0000-000000000000")
        .replace("{token_id}", "00000000-0000-0000-0000-000000000000")
        .replace("{id}", "00000000-0000-0000-0000-000000000000")
}

/// ⚠ **THE GATE EVERY OTHER SUITE ASSUMES.** Each of them carries a bearer, so
/// each would pass with the gate unmounted; this is the test that mounts it.
#[sqlx::test]
async fn every_person_lane_refuses_a_request_with_no_bearer(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, USER, EMAIL).await;
    let (status, body) = me(
        &pool,
        Some(&format!("Bearer {}", bearer(&pool, USER).await)),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "sign-in");
    let organization = active_organization(&body);
    // Sign-in makes no project; the routes below are filled in with a real
    // one, so a refusal below is the lane's and never a missing row's.
    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/projects")
                .header("x-service-secret", SERVICE_SECRET)
                .header("x-organization-id", &organization)
                .header(
                    "authorization",
                    format!("Bearer {}", bearer(&pool, USER).await),
                )
                .header("content-type", "application/json")
                .body(Body::from(json!({ "name": "Platform" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "the fixture project");
    let project = json_body(resp).await["id"]
        .as_str()
        .expect("the project's id")
        .to_owned();

    for (method, pattern) in PERSON_ROUTES {
        let uri = fill(pattern, &organization, &project);
        let resp = app(pool.clone())
            .oneshot(
                Request::builder()
                    .method(*method)
                    .uri(&uri)
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &organization)
                    .header("x-project-id", &project)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri} was served with the service secret and no bearer"
        );
    }
}

/// The subject is the bearer and nothing else: a body that still carries
/// `userId` — or an `email` describing them — is refused outright rather than
/// read and ignored, because a caller relying on it would be naming someone
/// the bearer does not.
#[sqlx::test]
async fn me_refuses_a_body_that_names_a_user(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, USER, EMAIL).await;
    let bearer = format!("Bearer {}", bearer(&pool, USER).await);

    for refused in [
        json!({ "userId": "user_somebody_else" }),
        json!({ "email": "someone-else@example.test" }),
    ] {
        let (status, body) = me(&pool, Some(&bearer), refused.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.organizations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "a refused sign-in provisioned an organization");

    let (status, body) = me(&pool, Some(&bearer), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["person"]["userId"], USER);
    assert_eq!(body["person"]["email"], EMAIL);
    let organization = active_organization(&body);
    assert!(
        organization.starts_with("org_") && organization != USER,
        "the provisioned organization is minted, never the person's id: {organization}"
    );
    assert_eq!(
        body["organizations"][0]["organizationId"],
        organization.as_str()
    );
    assert_eq!(body["organizations"][0]["role"], "owner");
    assert_eq!(body["organizations"][0]["ownerEmail"], EMAIL);
}

/// A bearer's session is recorded once: the same session on a second render
/// answers with the same row, never a duplicate.
#[sqlx::test]
async fn me_records_the_bearers_session_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, USER, EMAIL).await;
    let bearer = format!(
        "Bearer {}",
        bearer_in(&pool, USER, "sid_person_lanes").await
    );

    let (status, first) = me(&pool, Some(&bearer), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let row_id = first["sessionRowId"]
        .as_str()
        .expect("a bearer's session answers with the row it recorded")
        .to_owned();

    let (status, second) = me(&pool, Some(&bearer), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["sessionRowId"].as_str(),
        Some(row_id.as_str()),
        "the same session came back as a different row"
    );

    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.sessions WHERE user_id = $1 AND provider_sid = $2",
    )
    .bind(USER)
    .bind("sid_person_lanes")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1, "one session, one row");
}

/// A `telmoni_` API token rides the same header on `/v1`, and it is not a
/// person: on this lane it is refused without being looked up.
#[sqlx::test]
async fn a_telmoni_api_token_is_not_a_person(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let (status, body) = me(&pool, Some("Bearer telmoni_not_a_person_at_all"), json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.organizations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "an API token provisioned an organization");
}

/// The context headers stay, and they are still context: a bearer for one
/// person with another's organization named is the membership refusal, not a
/// served request.
#[sqlx::test]
async fn the_bearer_names_the_person_and_the_header_only_the_context(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mut organizations = Vec::new();
    for (user, email) in [(USER, EMAIL), ("user_person_other", "other@example.test")] {
        seed_identity(&pool, user, email).await;
        let (status, body) = me(
            &pool,
            Some(&format!("Bearer {}", bearer(&pool, user).await)),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "sign-in for {user}");
        organizations.push(active_organization(&body));
    }

    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("GET")
                    .uri("/internal/organization/members")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &organizations[0]),
                &pool,
                "user_person_other",
            )
            .await
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// A bearer is a secret auth minted, and nothing else resolves: one it never
/// minted, and one whose session was signed out, are both refused.
#[sqlx::test]
async fn only_a_bearer_auth_minted_and_still_holds_resolves(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, USER, EMAIL).await;

    let (status, body) = me(&pool, Some("Bearer never_minted_here"), json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let bearer = format!("Bearer {}", bearer_in(&pool, USER, "sid_signed_out").await);
    let (status, body) = me(&pool, Some(&bearer), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let resp = app(pool.clone())
        .oneshot(
            Request::post("/internal/auth/logout")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "session_id": "sid_signed_out" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let (status, body) = me(&pool, Some(&bearer), json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.access_tokens WHERE sid = $1")
        .bind("sid_signed_out")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "the sign-out deleted the session's bearers");
}

/// ⚠ **The test door is shut unless the deployment opened it.** It records an
/// identity from a body and signs it in, so a router built without
/// `ALLOW_TEST_SESSION` does not route it at all, and an open one still asks
/// for the service secret. What it mints is a session like any other.
#[sqlx::test]
async fn the_test_door_opens_only_when_asked_and_mints_a_real_session(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let request = |secret: Option<&str>| {
        let mut builder = Request::post("/test/session").header("content-type", "application/json");
        if let Some(secret) = secret {
            builder = builder.header("x-service-secret", secret);
        }
        builder
            .body(Body::from(
                json!({ "userId": USER, "email": EMAIL, "firstName": "Ada" }).to_string(),
            ))
            .unwrap()
    };

    let shut = app(pool.clone())
        .oneshot(request(Some(SERVICE_SECRET)))
        .await
        .unwrap();
    assert_eq!(shut.status(), StatusCode::NOT_FOUND);
    let identities: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.identities")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(identities, 0, "a shut door recorded somebody");

    let unsigned = app_with(pool.clone(), true)
        .oneshot(request(None))
        .await
        .unwrap();
    assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);

    let opened = app_with(pool.clone(), true)
        .oneshot(request(Some(SERVICE_SECRET)))
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    let tokens = json_body(opened).await;
    assert_eq!(tokens["userId"], USER);
    assert!(tokens["refreshToken"].is_string(), "{tokens}");
    let session = tokens["sessionId"].as_str().expect("a session id");

    let (status, me_body) = me(
        &pool,
        Some(&format!(
            "Bearer {}",
            tokens["accessToken"].as_str().unwrap()
        )),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{me_body}");
    assert_eq!(me_body["person"]["userId"], USER);
    assert_eq!(me_body["person"]["email"], EMAIL);
    let recorded: String =
        sqlx::query_scalar("SELECT provider_sid FROM auth.sessions WHERE id = $1::uuid")
            .bind(me_body["sessionRowId"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        recorded, session,
        "the door's session is the one `/me` recorded"
    );
}
