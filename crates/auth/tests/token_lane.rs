//! The `/v1` lane a `tm_` token opens: who it lets in, and who it refuses.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test scaffolding: asserts and fixture setup"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::test_provider::{ScriptedProvider, as_person, full_router};
use telmoni_auth::{AppState, Config};
use telmoni_shared::seam::Telemetry;
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::types::CONNECTION_BYTES;
use telmoni_shared::{OrganizationId, ProjectId, TelmoniError, TenantError};

const SERVICE_SECRET: &str = "test-service-secret";
const USER: &str = "user_token_lane";
const EMAIL: &str = "lane@example.test";

fn app(pool: PgPool) -> Router {
    app_with(pool, telmoni_auth::Siblings::default())
}

/// The router over `siblings`, as a deployment wires them.
fn app_with(pool: PgPool, siblings: telmoni_auth::Siblings) -> Router {
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
    let state = Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(Arc::new(
            ScriptedProvider::new(),
        ))),
        db,
        config,
        siblings,
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(Arc::new(
            telmoni_shared::mail::NoopSender,
        ))),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
    });
    full_router(state)
}

/// Telemetry as the read API meters through it: a day of `day` bytes, every
/// spend kept, the day refused once spent.
struct Meter {
    day: u64,
    spent: Mutex<HashMap<OrganizationId, u64>>,
}

impl Meter {
    fn of(&self, organization_id: &OrganizationId) -> u64 {
        self.spent
            .lock()
            .unwrap()
            .get(organization_id)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait]
impl Telemetry for Meter {
    async fn purge_organization(&self, _: &OrganizationId) -> Result<u64, TelmoniError> {
        Ok(0)
    }

    async fn purge_project(&self, _: &ProjectId) -> Result<u64, TelmoniError> {
        Ok(0)
    }

    async fn admit_data_out(&self, organization_id: &OrganizationId) -> Result<(), TelmoniError> {
        if self.of(organization_id) < self.day {
            return Ok(());
        }
        Err(TenantError::PastCeiling {
            detail: "the day's data out is spent".into(),
            retry_after_secs: 60,
        }
        .into())
    }

    async fn spend_data_out(&self, organization_id: &OrganizationId, bytes: u64) {
        *self
            .spent
            .lock()
            .unwrap()
            .entry(organization_id.clone())
            .or_default() += bytes;
    }

    async fn move_project(&self, _: &ProjectId, _: &OrganizationId) -> Result<(), TelmoniError> {
        Ok(())
    }
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
    /// The raw `tm_` value, as a customer holds it.
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
                    .uri("/internal/v1/projects")
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

/// Sign in and mint one token that reads.
async fn token(pool: &PgPool) -> Fixture {
    token_with(pool, &["read"]).await
}

/// Sign in and mint one token holding `scopes`.
async fn token_with(pool: &PgPool, scopes: &[&str]) -> Fixture {
    apply_audit_migrations(pool).await;
    seed_identity(pool, USER, EMAIL).await;
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/internal/v1/me")
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
                    .uri("/internal/v1/tokens")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &organization)
                    .header("x-project-id", &project)
                    .header("content-type", "application/json"),
                pool,
                USER,
            )
            .await
            .body(Body::from(
                json!({ "name": "ci", "created_by": USER, "scopes": scopes }).to_string(),
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

/// ⚠ **A key minted to send spans reads nothing.** One baked into an
/// agent's process is the one most likely to leak, so `/v1` refuses it as a
/// role the matrix does not admit is refused — the key itself still live.
#[sqlx::test]
async fn an_ingest_key_opens_no_read_lane(pool: PgPool) {
    let fixture = token_with(&pool, &["ingest"]).await;
    let resp = organization(&pool, &format!("Bearer {}", fixture.raw)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = json_body(resp).await;
    assert_eq!(body["type"], "/errors/authz/forbidden", "{body}");
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
    assert_eq!(
        body["name"], "My organization",
        "named after an owner who gave no name: {body}"
    );
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
    assert_eq!(body["members"][0]["role"], "owner", "{body}");
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
                    .uri(format!("/internal/v1/tokens/{}", fixture.id))
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
    assert_eq!(body["detail"], "a live tm_ API key is required");
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

/// Every shape that is not a live bearer, refused the same way. The `tm_`
#[sqlx::test]
async fn nothing_but_a_tor_bearer_gets_in(pool: PgPool) {
    token(&pool).await;
    for bearer in [
        "",
        "Bearer",
        "Bearer ",
        "Bearer not-a-telmoni-token",
        "Basic dG9yXzE6",
        "tm_bare_without_the_scheme",
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
                    .uri("/internal/v1/tokens")
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

/// The read API holds an organization to its plan's data out a day: each
/// answer is counted with what its connection costs, and once the day is
/// spent the next is refused, a 429 that says when it lifts.
#[sqlx::test]
async fn the_read_api_counts_each_answer_and_refuses_past_the_day(pool: PgPool) {
    let fixture = token(&pool).await;
    let organization = OrganizationId::try_new(fixture.organization.as_str()).unwrap();
    // A day one answer spends, its connection alone.
    let meter = Arc::new(Meter {
        day: CONNECTION_BYTES,
        spent: Mutex::new(HashMap::new()),
    });
    let read = || {
        app_with(
            pool.clone(),
            telmoni_auth::Siblings {
                telemetry: Some(meter.clone()),
                ..telmoni_auth::Siblings::default()
            },
        )
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/organization")
                .header("x-service-secret", SERVICE_SECRET)
                .header(header::AUTHORIZATION, format!("Bearer {}", fixture.raw))
                .body(Body::empty())
                .unwrap(),
        )
    };

    let resp = read().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let answer = resp.into_body().collect().await.unwrap().to_bytes();
    let answered = u64::try_from(answer.len()).unwrap();
    assert_eq!(meter.of(&organization), answered + CONNECTION_BYTES);

    let resp = read().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        resp.headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
        Some("60")
    );
    let body = json_body(resp).await;
    assert_eq!(body["type"], "/errors/tenant/rate-limited", "{body}");
    assert_eq!(
        meter.of(&organization),
        answered + CONNECTION_BYTES,
        "a refusal is not counted"
    );
}
