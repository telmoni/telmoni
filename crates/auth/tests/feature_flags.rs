//! Feature flags: the store, the resolver's order, and the answers carrying a set.
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
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::test_provider::{ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::Flag;
use telmoni_shared::seam::Auth as _;
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};

const SERVICE_SECRET: &str = "test-service-secret";

fn app(pool: PgPool) -> Router {
    router(state(pool))
}

fn state(pool: PgPool) -> Arc<AppState> {
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
    let db = service_pool(&pool, ServiceRole::Auth);
    Arc::new(AppState {
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
    })
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("JSON body")
}

/// Sign `user` in, provisioning on the first call.
async fn me(pool: &PgPool, user: &str) -> (StatusCode, serde_json::Value) {
    seed_identity(pool, user, &format!("{user}@example.test")).await;
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
    let status = resp.status();
    (status, json_body(resp).await)
}

/// Sign `user` in and answer the organization `/me` made them active in — the
/// one provisioned for them on their first call.
async fn sign_in(pool: &PgPool, user: &str) -> String {
    let (status, body) = me(pool, user).await;
    assert_eq!(status, StatusCode::OK, "sign-in for {user}: {body}");
    body.get("activeOrganizationId")
        .and_then(serde_json::Value::as_str)
        .expect("/me names the active organization")
        .to_owned()
}

/// A project in `organization`, made by `owner` as the console would: sign-in
/// makes none.
async fn create_project(pool: &PgPool, owner: &str, organization: &str) -> String {
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/internal/projects")
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
    json_body(resp)
        .await
        .get("id")
        .and_then(serde_json::Value::as_str)
        .expect("the project's id")
        .to_owned()
}

/// What `make flag` writes: one append-only row, global or per organization.
async fn flip(pool: &PgPool, organization: Option<&str>, flag: Flag, on: bool) {
    match organization {
        None => sqlx::query(
            "INSERT INTO auth.feature_flags (key, enabled, note, actor) VALUES ($1, $2, 'test', 'test')",
        )
        .bind(flag.as_str())
        .bind(on)
        .execute(pool)
        .await
        .unwrap(),
        Some(a) => sqlx::query(
            "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
             VALUES ($1, $2, $3, 'test', 'test')",
        )
        .bind(a)
        .bind(flag.as_str())
        .bind(on)
        .execute(pool)
        .await
        .unwrap(),
    };
}

/// With no row anywhere, every catalog key is present on `/me` and on.
#[sqlx::test]
async fn every_flag_rides_me_and_is_on_with_no_rows(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (status, body) = me(&pool, "u_a").await;
    assert_eq!(status, StatusCode::OK);
    let flags = body["flags"].as_object().expect("flags is an object");
    assert_eq!(flags.len(), Flag::all().len(), "every catalog key, no more");
    for f in Flag::all() {
        assert_eq!(flags[f.as_str()], true, "{f} is on with no row");
    }
}

/// The resolver's order: organization row over global row over catalog, latest
/// row per key winning.
#[sqlx::test]
async fn an_organization_row_outranks_a_global_row_which_outranks_the_catalog(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization_a = sign_in(&pool, "u_a").await;
    sign_in(&pool, "u_b").await;

    flip(&pool, None, Flag::ApiTokens, false).await;
    let (_, a) = me(&pool, "u_a").await;
    let (_, b) = me(&pool, "u_b").await;
    assert_eq!(
        a["flags"]["api_tokens"], false,
        "a global off reaches every organization"
    );
    assert_eq!(b["flags"]["api_tokens"], false);
    assert_eq!(a["flags"]["members"], true, "and touches nothing else");

    flip(&pool, Some(&organization_a), Flag::ApiTokens, true).await;
    let (_, a) = me(&pool, "u_a").await;
    let (_, b) = me(&pool, "u_b").await;
    assert_eq!(
        a["flags"]["api_tokens"], true,
        "the organization's own row wins"
    );
    assert_eq!(b["flags"]["api_tokens"], false, "and is nobody else's");

    flip(&pool, None, Flag::ApiTokens, true).await;
    flip(&pool, None, Flag::Members, false).await;
    let (_, b) = me(&pool, "u_b").await;
    assert_eq!(
        b["flags"]["api_tokens"], true,
        "the latest global row is the answer"
    );
    assert_eq!(b["flags"]["members"], false);
}
/// `Flag::Signup` off makes no NEW organization — the newcomer signs in to
/// their account alone, with the global set saying why — and keeps serving
/// existing ones.
#[sqlx::test]
async fn a_closed_signup_refuses_a_new_organization_and_serves_an_existing_one(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (status, _) = me(&pool, "u_existing").await;
    assert_eq!(status, StatusCode::OK);

    flip(&pool, None, Flag::Signup, false).await;

    let (status, body) = me(&pool, "u_newcomer").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activeOrganizationId"], serde_json::Value::Null);
    assert_eq!(body["organizations"], json!([]));
    assert_eq!(body["flags"]["signup"], false, "{body}");
    let memberships: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_members WHERE user_id = 'u_newcomer'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(memberships, 0, "a refused sign-up provisions nothing");
    let organizations: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.organizations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        organizations, 1,
        "the existing organization, and no second one for the newcomer"
    );

    let (status, body) = me(&pool, "u_existing").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["flags"]["signup"], false,
        "and the existing organization still sees the set"
    );
}

/// Token validation carries the organization's set, and `/v1` refuses on
/// `Flag::PublicApi`.
#[sqlx::test]
async fn validation_carries_the_set_and_v1_refuses_when_public_api_is_off(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, "u_owner").await;

    let minted = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/internal/tokens")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &organization)
                    .header(
                        "x-project-id",
                        &create_project(&pool, "u_owner", &organization).await,
                    )
                    .header("content-type", "application/json"),
                &pool,
                "u_owner",
            )
            .await
            .body(Body::from(
                json!({ "name": "ci", "created_by": "u_owner" }).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(minted.status(), StatusCode::CREATED);
    let token = json_body(minted).await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let validate = |pool: PgPool, token: String| async move {
        let resp = app(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/tokens/validate")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "token": token }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        json_body(resp).await
    };
    let v1_organization = |pool: PgPool, token: String| async move {
        app(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/organization")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    };

    let verdict = validate(pool.clone(), token.clone()).await;
    assert_eq!(verdict["organization_id"], organization.as_str());
    assert_eq!(verdict["flags"]["api_tokens"], true);
    assert_eq!(
        v1_organization(pool.clone(), token.clone()).await.status(),
        StatusCode::OK
    );

    flip(&pool, Some(&organization), Flag::ApiTokens, false).await;
    flip(&pool, None, Flag::PublicApi, false).await;

    let verdict = validate(pool.clone(), token.clone()).await;
    assert_eq!(
        verdict["flags"]["api_tokens"], false,
        "the token's ORGANIZATION set"
    );
    assert_eq!(verdict["flags"]["public_api"], false);

    let refused = v1_organization(pool.clone(), token.clone()).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        refused
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("60"),
        "a machine lane says when to come back"
    );
    let body = json_body(refused).await;
    assert_eq!(body["type"], "/errors/tenant/feature-off");
    assert_eq!(body["flag"], "public_api");
}

/// The global set a module beside auth reads in process — the delivery
/// loop's kill switch — carries the global rows and no organization's own.
#[sqlx::test]
async fn the_global_set_a_module_reads_carries_the_global_rows_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, "u_owner").await;

    flip(&pool, None, Flag::Signup, false).await;
    flip(&pool, Some(&organization), Flag::Members, false).await;

    let global = state(pool).global_flags().await.unwrap();
    assert!(!global.is_on(Flag::Signup));
    assert!(
        global.is_on(Flag::Members),
        "an organization row is not global"
    );
}
/// A served read stamps the token's last use, and once its own organization
/// has the API switched off — its row, not the global one — the same token
/// opens no `/v1` lane.
#[sqlx::test]
async fn a_served_read_stamps_last_used_and_an_organizations_own_switch_refuses_its_token(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, "u_owner").await;
    let minted = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/internal/tokens")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &organization)
                    .header(
                        "x-project-id",
                        &create_project(&pool, "u_owner", &organization).await,
                    )
                    .header("content-type", "application/json"),
                &pool,
                "u_owner",
            )
            .await
            .body(Body::from(
                json!({ "name": "ci", "created_by": "u_owner" }).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    let token = json_body(minted).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let v1_organization = |pool: PgPool, token: String| async move {
        app(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/organization")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    };

    let before: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_used_at FROM auth.api_tokens WHERE organization_id = $1")
            .bind(&organization)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(before.is_none(), "never used yet");
    assert_eq!(
        v1_organization(pool.clone(), token.clone()).await.status(),
        StatusCode::OK
    );
    let after: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_used_at FROM auth.api_tokens WHERE organization_id = $1")
            .bind(&organization)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(after.is_some(), "the bump committed before the handler ran");

    flip(&pool, Some(&organization), Flag::PublicApi, false).await;
    let refused = v1_organization(pool, token).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(refused).await["flag"], "public_api");
}

/// A call on the tokens lane, by `u_owner` in `project`.
async fn token_lane(
    pool: &PgPool,
    organization: &str,
    project: &str,
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", organization)
                    .header("x-project-id", project)
                    .header("content-type", "application/json"),
                pool,
                "u_owner",
            )
            .await
            .body(Body::from(body.to_string()))
            .unwrap(),
        )
        .await
        .unwrap()
}

/// Minting and rotating are the switch's own lanes, refused by the server
/// whoever the caller is, not only by the console in front of it. Revoking
/// never is: a leaked key must be revocable whatever the switch says. The row
/// is the organization's own, which only a read under its scope sees.
#[sqlx::test]
async fn a_switched_off_api_tokens_flag_refuses_minting_and_rotating_but_not_revoking(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, "u_owner").await;
    let project = create_project(&pool, "u_owner", &organization).await;

    let minted = token_lane(
        &pool,
        &organization,
        &project,
        "POST",
        "/internal/tokens",
        json!({ "name": "ci", "created_by": "u_owner" }),
    )
    .await;
    assert_eq!(minted.status(), StatusCode::CREATED);
    let id = json_body(minted).await["id"]
        .as_str()
        .expect("the minted token's id")
        .to_owned();

    flip(&pool, Some(&organization), Flag::ApiTokens, false).await;

    let refused = token_lane(
        &pool,
        &organization,
        &project,
        "POST",
        "/internal/tokens",
        json!({ "name": "ci-2", "created_by": "u_owner" }),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = json_body(refused).await;
    assert_eq!(body["type"], "/errors/tenant/feature-off");
    assert_eq!(body["flag"], "api_tokens");

    let rotated = token_lane(
        &pool,
        &organization,
        &project,
        "POST",
        &format!("/internal/tokens/{id}/rotate"),
        json!({}),
    )
    .await;
    assert_eq!(rotated.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(rotated).await["flag"], "api_tokens");

    let revoked = token_lane(
        &pool,
        &organization,
        &project,
        "DELETE",
        &format!("/internal/tokens/{id}"),
        json!({}),
    )
    .await;
    assert_eq!(
        revoked.status(),
        StatusCode::NO_CONTENT,
        "a leaked key is revocable whatever the switch says"
    );
}
