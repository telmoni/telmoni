//! The `/v1` lane a `telmoni_` token opens: who it lets in, and who it refuses.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test scaffolding: asserts and fixture setup"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::test_provider::{ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};

const SERVICE_SECRET: &str = "test-service-secret";
const USER: &str = "user_token_lane";
const EMAIL: &str = "lane@example.test";

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

/// One provisioned organization and the token minted on it.
struct Fixture {
    /// The organization `/me` provisioned for [`USER`] — minted, never their id.
    organization: String,
    /// The slug `/me` answered for it.
    slug: String,
    /// The project the token was minted on.
    project: String,
    /// The raw `telmoni_` value, as a customer holds it.
    raw: String,
    id: String,
}

/// A project in `organization`, made by [`USER`] as the console would: sign-in
/// makes none, and a token hangs off a project.
async fn create_project(pool: &PgPool, organization: &str, name: &str) -> String {
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
                USER,
            )
            .await
            .body(Body::from(json!({ "name": name }).to_string()))
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

/// Sign in and mint one token.
async fn token(pool: &PgPool) -> Fixture {
    apply_audit_migrations(pool).await;
    seed_identity(pool, USER, EMAIL).await;
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/me")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("content-type", "application/json"),
                pool,
                USER,
            )
            .await
            .body(Body::from(json!({}).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let me = json_body(resp).await;
    let organization = me["activeOrganizationId"]
        .as_str()
        .expect("/me names the organization it provisioned")
        .to_owned();
    let slug = me["organizations"][0]["slug"]
        .as_str()
        .expect("/me answers the organization's slug")
        .to_owned();

    let project = create_project(pool, &organization, "Platform").await;

    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/internal/tokens")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &organization)
                    .header("x-project-id", &project)
                    .header("content-type", "application/json"),
                pool,
                USER,
            )
            .await
            .body(Body::from(
                json!({ "name": "ci", "created_by": USER }).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = json_body(resp).await;
    Fixture {
        organization,
        slug,
        project,
        raw: body["token"].as_str().expect("the raw token").to_owned(),
        id: body["id"].as_str().expect("the token id").to_owned(),
    }
}

/// `GET /v1/organization` as the BFF forwards it, bearer untouched.
async fn organization(pool: &PgPool, bearer: &str) -> axum::response::Response {
    app(pool.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/organization")
                .header("x-service-secret", SERVICE_SECRET)
                .header(header::AUTHORIZATION, bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[sqlx::test]
async fn a_live_token_reads_the_organization_it_belongs_to(pool: PgPool) {
    let fixture = token(&pool).await;
    let resp = organization(&pool, &format!("Bearer {}", fixture.raw)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["organization_id"], fixture.organization.as_str());
    // The slug the console's URL shows, as `/me` answers it: a script holding
    // only a key can spell a console link, or name the organization to the CLI.
    assert!(telmoni_shared::slug::is_slug(&fixture.slug), "{body}");
    assert_eq!(body["slug"], fixture.slug.as_str(), "{body}");
    assert!(body["name"].is_null(), "nobody has named it yet: {body}");
    assert_eq!(
        body["owner"]["email"], EMAIL,
        "the owner rides beside the organization, as its contact: {body}"
    );
}

/// **A key reads its own project's roster and no further.** A project admin
/// may mint one, and the console refuses that seat the organization's roster,
/// so a key that listed it would read past its minter: the organization's
/// other project seats somebody the key's project never did, and `/v1/members`
/// names the owner and the key's project's seat alone.
#[sqlx::test]
async fn a_token_lists_its_projects_roster_and_not_the_organizations(pool: PgPool) {
    let fixture = token(&pool).await;
    let elsewhere = create_project(&pool, &fixture.organization, "Elsewhere").await;
    seed_identity(&pool, "user_seated", "seated@example.test").await;
    seed_identity(&pool, "user_elsewhere", "elsewhere@example.test").await;
    for (user, project) in [
        ("user_seated", fixture.project.as_str()),
        ("user_elsewhere", elsewhere.as_str()),
    ] {
        sqlx::query(
            "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
             VALUES ($1, $2, 'member', $3)",
        )
        .bind(&fixture.organization)
        .bind(user)
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO auth.project_members (project_id, user_id, role, added_by)
             VALUES ($1, $2, 'member', $3)",
        )
        .bind(project)
        .bind(user)
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    }

    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/members")
                .header("x-service-secret", SERVICE_SECRET)
                .header(header::AUTHORIZATION, format!("Bearer {}", fixture.raw))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let members: Vec<&str> = body["members"]
        .as_array()
        .expect("a members array")
        .iter()
        .filter_map(|m| m["member_id"].as_str())
        .collect();
    assert_eq!(members, [USER, "user_seated"], "{body}");
    assert_eq!(body["members"][0]["is_owner"], true, "{body}");
    assert_eq!(body["members"][1]["role"], "member", "{body}");
}

/// **The audit chain is internal, and a live token opens no door to it.**
#[sqlx::test]
async fn no_lane_reads_a_customer_their_audit_chain(pool: PgPool) {
    let fixture = token(&pool).await;
    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/audit")
                .header("x-service-secret", SERVICE_SECRET)
                .header(header::AUTHORIZATION, format!("Bearer {}", fixture.raw))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a token that reads /v1/organization must still find no audit lane"
    );
    // As a problem document, like every other `/v1` answer.
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );
}

#[sqlx::test]
async fn a_revoked_token_is_401(pool: PgPool) {
    let fixture = token(&pool).await;
    assert_eq!(
        organization(&pool, &format!("Bearer {}", fixture.raw))
            .await
            .status(),
        StatusCode::OK,
        "it read before it was revoked"
    );

    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/internal/tokens/{}", fixture.id))
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &fixture.organization)
                    .header("x-project-id", &fixture.project),
                &pool,
                USER,
            )
            .await
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success(), "revoke failed");

    let refused = organization(&pool, &format!("Bearer {}", fixture.raw)).await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        refused
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer"),
        "a refused credential says what one looks like"
    );
    // A problem document, like every other `/v1` answer, under the type a bad
    // credential gets everywhere else.
    assert_eq!(
        refused
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );
    let body = json_body(refused).await;
    assert_eq!(body["type"], "/errors/auth/invalid-token");
    assert_eq!(body["detail"], "a live telmoni_ API key is required");
}

#[sqlx::test]
async fn an_expired_token_is_401(pool: PgPool) {
    let fixture = token(&pool).await;
    sqlx::query("UPDATE auth.api_tokens SET expires_at = now() - interval '1 hour' WHERE id = $1")
        .bind(fixture.id.parse::<uuid::Uuid>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        organization(&pool, &format!("Bearer {}", fixture.raw))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

/// Every shape that is not a live bearer, refused the same way. The `telmoni_`
#[sqlx::test]
async fn nothing_but_a_tor_bearer_gets_in(pool: PgPool) {
    token(&pool).await;
    for bearer in [
        "",
        "Bearer",
        "Bearer ",
        "Bearer not-a-telmoni-token",
        "Basic dG9yXzE6",
        "telmoni_bare_without_the_scheme",
    ] {
        assert_eq!(
            organization(&pool, bearer).await.status(),
            StatusCode::UNAUTHORIZED,
            "{bearer:?} got in"
        );
    }
}

/// The scheme is case-insensitive (RFC 9110 § 11.1): `bearer` is a credential.
#[sqlx::test]
async fn the_scheme_is_case_insensitive(pool: PgPool) {
    let fixture = token(&pool).await;
    assert_eq!(
        organization(&pool, &format!("bearer {}", fixture.raw))
            .await
            .status(),
        StatusCode::OK
    );
}

/// The project is named by `x-project-id` and nothing else; no header stands in.
#[sqlx::test]
async fn a_token_route_without_the_project_header_is_refused(pool: PgPool) {
    let fixture = token(&pool).await;
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("GET")
                    .uri("/internal/tokens")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &fixture.organization),
                &pool,
                USER,
            )
            .await
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
