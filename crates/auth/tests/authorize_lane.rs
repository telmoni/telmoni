//! Token relay end to end on auth's side: the in-process `Auth` seam answers
//! a module beside auth with the tuple derived from auth's own tables, a
//! refresh is refused for a session ended here, and a sign-out ends the
//! session the bearer names.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test scaffolding")]
#![expect(
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::test_provider::{Call, ScriptedProvider, as_person, bearer, bearer_in};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::seam::Auth as _;
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{OrganizationRole, Role};

const SERVICE_SECRET: &str = "test-service-secret";

/// Real auth state around a scripted provider, handed back beside it so a
/// test can script its answers and read what the lanes asked of it.
fn state(pool: PgPool) -> (Arc<AppState>, Arc<ScriptedProvider>) {
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
    let provider = Arc::new(ScriptedProvider::new());
    let db = service_pool(&pool, ServiceRole::Auth);
    let state = Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(provider.clone())),
        db,
        config,
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(Arc::new(
            telmoni_shared::mail::NoopSender,
        ))),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
        siblings: telmoni_auth::Siblings::default(),
    });
    (state, provider)
}

fn app(pool: PgPool) -> (Router, Arc<AppState>, Arc<ScriptedProvider>) {
    let (state, provider) = state(pool);
    (router(state.clone()), state, provider)
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    if bytes.is_empty() {
        return json!(null);
    }
    serde_json::from_slice(&bytes).expect("JSON body")
}

/// `POST /me` for `user`, under a bearer of session `sid` (one of its own
/// when `None`); provisions on first call. Answers the organization `/me`
/// made them active in — on a first sign-in, the one it provisioned — and the
/// whole `/me` body.
async fn sign_in(app: &Router, pool: &PgPool, user: &str, sid: Option<&str>) -> (String, Value) {
    // What the exchange would have recorded.
    seed_identity(pool, user, &format!("{user}@example.test")).await;
    let bearer = match sid {
        Some(sid) => bearer_in(pool, user, sid).await,
        None => bearer(pool, user).await,
    };
    let resp = app
        .clone()
        .oneshot(
            Request::post("/me")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {bearer}"))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "sign-in failed for {user}");
    let me = json_body(resp).await;
    let organization = me["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned();
    (organization, me)
}

/// A project in `organization`, made by `owner` as the console would: sign-in
/// makes none.
async fn create_project(app: &Router, pool: &PgPool, owner: &str, organization: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            as_person(
                Request::post("/internal/projects")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", organization)
                    .header("content-type", "application/json"),
                pool,
                owner,
            )
            .await
            .body(Body::from(json!({ "name": "Platform" }).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "the fixture project");
    json_body(resp).await["id"]
        .as_str()
        .expect("the project's id")
        .to_owned()
}

/// The headers a module beside auth hands the seam, as the console relayed
/// them: the person's bearer and the context.
fn context(bearer: Option<&str>, organization: &str, project: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(bearer) = bearer {
        headers.insert("authorization", format!("Bearer {bearer}").parse().unwrap());
    }
    headers.insert("x-organization-id", organization.parse().unwrap());
    if let Some(project) = project {
        headers.insert("x-project-id", project.parse().unwrap());
    }
    headers
}

/// The module's question: who is `user`, acting on `organization` and
/// `project`?
async fn resolve(
    state: &AppState,
    user: &str,
    organization: &str,
    project: Option<&str>,
) -> Result<telmoni_shared::acting::Acting, telmoni_shared::TelmoniError> {
    state
        .resolve(&context(
            Some(&bearer(&state.db, user).await),
            organization,
            project,
        ))
        .await
}

/// Seat `member` on `project`, in `organization`, at `role`, through the
/// invite lane: `owner` sends it and `member` accepts.
async fn seat(
    app: &Router,
    pool: &PgPool,
    owner: &str,
    organization: &str,
    project: &str,
    member: &str,
    role: &str,
) {
    let resp = app
        .clone()
        .oneshot(
            as_person(
                Request::post(format!("/internal/projects/{project}/invites"))
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", organization)
                    .header("content-type", "application/json"),
                pool,
                owner,
            )
            .await
            .body(Body::from(
                json!({ "email": format!("{member}@example.test"), "role": role }).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let link = json_body(resp).await["link"].as_str().unwrap().to_owned();
    let secret = link.rsplit('/').next().unwrap().to_owned();
    let resp = app
        .clone()
        .oneshot(
            as_person(
                Request::post("/internal/invites/accept")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("content-type", "application/json"),
                pool,
                member,
            )
            .await
            .body(Body::from(json!({ "token": secret }).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "accept failed");
}

#[sqlx::test]
async fn the_seam_answers_owner_seat_and_non_member_from_auths_own_tables(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, state, _provider) = app(pool.clone());
    let (organization, _) = sign_in(&app, &pool, "user_owner", None).await;
    sign_in(&app, &pool, "user_member", None).await;
    sign_in(&app, &pool, "user_stranger", None).await;
    let project = create_project(&app, &pool, "user_owner", &organization).await;
    seat(
        &app,
        &pool,
        "user_owner",
        &organization,
        &project,
        "user_member",
        "member",
    )
    .await;

    let acting = resolve(&state, "user_owner", &organization, Some(&project))
        .await
        .unwrap();
    assert_eq!(acting.user_id.as_str(), "user_owner");
    assert_eq!(acting.organization_id.as_str(), organization.as_str());
    assert_eq!(acting.organization_role, Some(OrganizationRole::Owner));
    let acting_project = acting.project.expect("the project named");
    assert_eq!(acting_project.project_id.as_str(), project);
    assert_eq!(acting_project.role, Role::Owner);
    assert!(acting.expires_at > chrono::Utc::now().timestamp());

    let acting = resolve(&state, "user_member", &organization, Some(&project))
        .await
        .unwrap();
    assert_eq!(
        acting.project.as_ref().map(|p| p.role),
        Some(Role::Member),
        "the project role is the seat"
    );
    assert_eq!(
        acting.organization_role,
        Some(OrganizationRole::Member),
        "the organization role is their row on its roster, which accepting the seat wrote"
    );

    let refused = resolve(&state, "user_stranger", &organization, Some(&project))
        .await
        .unwrap_err();
    assert_eq!(refused.to_problem_details().status, 403);

    // The organization context: the owner is the roster's owner row, everyone
    // else a row or a 403.
    let acting = resolve(&state, "user_owner", &organization, None)
        .await
        .unwrap();
    assert_eq!(acting.organization_role, Some(OrganizationRole::Owner));
    assert!(acting.project.is_none());
    let acting = resolve(&state, "user_member", &organization, None)
        .await
        .unwrap();
    assert_eq!(acting.organization_role, Some(OrganizationRole::Member));
    let refused = resolve(&state, "user_stranger", &organization, None)
        .await
        .unwrap_err();
    assert_eq!(refused.to_problem_details().status, 403);
}

/// ⚠ The seam resolves the bearer itself: no bearer, or one auth never
/// minted, is a 401 before any organization is read, and a missing context
/// is a 400 — the module asserts nothing about who is asking.
#[sqlx::test]
async fn the_seam_is_a_person_lane(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (state, _provider) = state(pool.clone());

    let refused = state
        .resolve(&context(None, "org_owner", None))
        .await
        .unwrap_err();
    assert_eq!(refused.to_problem_details().status, 401);

    let refused = state
        .resolve(&context(Some("never_minted_here"), "org_owner", None))
        .await
        .unwrap_err();
    assert_eq!(refused.to_problem_details().status, 401);

    seed_identity(&pool, "user_owner", "user_owner@example.test").await;
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", bearer(&pool, "user_owner").await)
            .parse()
            .unwrap(),
    );
    let refused = state.resolve(&headers).await.unwrap_err();
    assert_eq!(refused.to_problem_details().status, 400);
}

async fn refresh_with(app: &Router, refresh_token: &str, row: &Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post("/internal/auth/refresh")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "refresh_token": refresh_token, "session_row_id": row }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// `POST /me` under a bearer the issuer minted.
async fn me_under(app: &Router, bearer: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/me")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {bearer}"))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// The session an external sign-in opens is the issuer's: a refresh rotates
/// its tokens and touches its row, and a sign-out ends the row and the
/// tokens together — with nothing asked of the provider after the exchange.
#[sqlx::test]
async fn refresh_touches_the_row_and_a_sign_out_ends_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, _state, provider) = app(pool.clone());

    let resp = app
        .clone()
        .oneshot(
            Request::post("/internal/auth/exchange")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "code": "code_1", "provider": "external" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let tokens = json_body(resp).await;
    let bearer = tokens["accessToken"].as_str().unwrap().to_owned();
    let first = tokens["refreshToken"].as_str().unwrap().to_owned();
    let sid = tokens["sessionId"].as_str().expect("a sid").to_owned();

    let (status, me) = me_under(&app, &bearer).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    let row = me["sessionRowId"].clone();
    assert!(row.is_string(), "{me}");
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_seen_at FROM auth.sessions WHERE provider_sid = $1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();

    let resp = refresh_with(&app, &first, &row).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let rotated = json_body(resp).await;
    let second = rotated["refreshToken"].as_str().unwrap().to_owned();
    assert_ne!(first, second, "the refresh token rotates");
    let rotated_bearer = rotated["accessToken"].as_str().unwrap().to_owned();
    assert_ne!(rotated_bearer, bearer, "the bearer rotates too");
    assert_eq!(
        rotated["sessionId"].as_str(),
        Some(sid.as_str()),
        "the session is the same one"
    );
    let (row_sid, after): (Option<String>, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT provider_sid, last_seen_at FROM auth.sessions WHERE id = $1::uuid")
            .bind(row.as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row_sid.as_deref(), Some(sid.as_str()));
    assert!(after >= before);

    // Sign out ends the row by the sid the cookie holds, and every bearer
    // naming that session is refused from then on.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/internal/auth/logout")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .body(Body::from(json!({ "session_id": sid }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let (status, _) = me_under(&app, &rotated_bearer).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a revoked session's bearer"
    );
    assert_eq!(
        refresh_with(&app, &second, &row).await.status(),
        StatusCode::UNAUTHORIZED,
        "a refresh on a revoked row is refused before the token is spent"
    );
    assert_eq!(
        provider.count(|c| !matches!(c, Call::Exchange { .. })),
        0,
        "the provider was asked for the exchange and nothing else: {:?}",
        provider.calls()
    );
}

/// ⚠ **Sign-out ends the session here whichever way the browser leaves.** The
/// console skipped this call whenever it sent the browser through the
/// provider's logout page, so every normal sign-out left the row live and the
/// outstanding access token accepted until it expired. The session is the
/// issuer's, so the provider has nothing to end.
#[sqlx::test]
async fn logout_revokes_the_row_and_asks_nothing_of_the_provider(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, _state, provider) = app(pool.clone());

    sign_in(&app, &pool, "user_logout", Some("sid_logout")).await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/internal/auth/logout")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "session_id": "sid_logout" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM auth.sessions WHERE provider_sid = $1",
    )
    .bind("sid_logout")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(revoked, "the row outlived the sign-out");
    assert!(provider.calls().is_empty(), "{:?}", provider.calls());
}
