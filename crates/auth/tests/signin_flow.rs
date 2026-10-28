//! Sign-in flow: the `/internal/v1/auth/*` endpoints through the real router,
//! with an external identity provider scripted in-process beside the login
//! form, and the issuer opening every session.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test scaffolding")]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::default_organization::DefaultOrganization;
use telmoni_auth::issuer::Issuer;
use telmoni_auth::password::PasswordProvider;
use telmoni_auth::test_provider::{
    Call, ScriptedProvider, TEST_SUBJECT, bearer, bearer_in, full_router,
};
use telmoni_auth::{AppState, Config};
use telmoni_shared::test_util::{ServiceRole, seed_identity, service_pool};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};

const SERVICE_SECRET: &str = "test-service-secret";
const REDIRECT_URI: &str = "http://localhost:3000/auth/callback";
const APP_URL: &str = "http://localhost:3000";

/// RFC 7636's own example pair, and a nonce, as the console's doors seal them.
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const NONCE: &str = "nonce_0123456789abcdef";

/// How the router under test is put together.
struct Setup {
    /// Whether the test door is open (`ALLOW_TEST_SESSION`); a deployed
    /// auth's is shut.
    test_door: bool,
    /// Whether the login form is on.
    login_form: bool,
    /// The external provider's policy.
    allow_sign_up: bool,
    link_by_email: bool,
    /// Where every person created starts (`TELMONI_DEFAULT_ORG_ID`).
    default_organization: Option<DefaultOrganization>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            test_door: false,
            login_form: true,
            allow_sign_up: true,
            link_by_email: false,
            default_organization: None,
        }
    }
}

/// Real auth router around a scripted provider, handed back beside it so a
/// test can script its answers and read what the lanes asked of it.
fn app(pool: PgPool) -> (Router, Arc<ScriptedProvider>) {
    app_configured(pool, Setup::default())
}

fn app_configured(pool: PgPool, setup: Setup) -> (Router, Arc<ScriptedProvider>) {
    let config = Config {
        database_url: String::new(), // pool already built by #[sqlx::test]
        service_secret: SERVICE_SECRET.into(),
        service_secret_next: None,
        allow_test_session: setup.test_door,
        redirect_uri: REDIRECT_URI.into(),
        app_url: APP_URL.into(),
        mail_from: "Telmoni <test@example.com>".into(),
        support_email: None,
        deletion_tail_budget_ms: 8_000,
    };
    let provider = Arc::new(ScriptedProvider::new());
    let db = service_pool(&pool, ServiceRole::Auth);
    // As `test_issuer` builds it, address verification on.
    let issuer = Arc::new(
        Issuer::new(db.clone(), APP_URL, true)
            .with_default_organization(setup.default_organization),
    );
    let mailer: Arc<dyn telmoni_auth::mailer::Mailer> = Arc::new(
        telmoni_auth::mailer::ComposingMailer::new(Arc::new(telmoni_shared::mail::NoopSender)),
    );
    let password = setup.login_form.then(|| {
        Arc::new(PasswordProvider::new(
            db.clone(),
            Arc::clone(&issuer),
            Arc::clone(&mailer),
            true,
            Vec::new(),
        ))
    });
    let state = Arc::new(AppState {
        issuer,
        password,
        external: Some(telmoni_auth::external::External {
            provider: provider.clone(),
            allow_sign_up: setup.allow_sign_up,
            link_by_email: setup.link_by_email,
        }),
        db,
        config,
        mailer,
        exchange_cache: telmoni_auth::ExchangeCache::new(),
        siblings: telmoni_auth::Siblings::default(),
    });
    (full_router(state), provider)
}

fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .header("x-service-secret", SERVICE_SECRET)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .body(Body::empty())
        .unwrap()
}

/// `POST /internal/v1/me` as the console makes it: the person is who the
/// bearer names, and what describes them is the identity an exchange
/// recorded — the body carries nothing about them.
async fn post_me(pool: &PgPool, user: &str) -> Request<Body> {
    post_me_under(&bearer(pool, user).await)
}

/// The same, under a bearer of session `sid`, whose row `/me` records.
async fn post_me_with_session(pool: &PgPool, user: &str, sid: &str) -> Request<Body> {
    post_me_under(&bearer_in(pool, user, sid).await)
}

/// `POST /internal/v1/me` under any bearer.
fn post_me_under(bearer: &str) -> Request<Body> {
    Request::post("/internal/v1/me")
        .header("content-type", "application/json")
        .header("x-service-secret", SERVICE_SECRET)
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::from("{}"))
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("JSON body")
}

/// An external sign-in: the scripted provider names the person, the issuer
/// opens the session. Answers the exchange's body.
async fn exchange_external(app: &Router, code: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/v1/auth/exchange",
            serde_json::json!({ "code": code, "codeVerifier": VERIFIER, "nonce": NONCE, "provider": "external" }),
        ))
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn linked_person(
    pool: &PgPool,
    provider: &ScriptedProvider,
    subject: &str,
) -> Option<String> {
    sqlx::query_scalar(
        "SELECT user_id FROM auth.external_identities WHERE provider = $1 AND subject = $2",
    )
    .bind(&provider.base)
    .bind(subject)
    .fetch_optional(pool)
    .await
    .unwrap()
}

#[sqlx::test]
async fn start_sends_the_browser_to_the_console_page_while_the_form_is_on(pool: PgPool) {
    let (app, provider) = app(pool);

    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/v1/auth/start",
            serde_json::json!({ "state": "csrf-abc-123", "codeChallenge": CHALLENGE, "nonce": NONCE }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["authorizeUrl"],
        format!("{APP_URL}/auth/sign-in?state=csrf-abc-123")
    );

    let resp = app
        .oneshot(post_json(
            "/internal/v1/auth/start",
            serde_json::json!({
                "state": "csrf-abc-123",
                "codeChallenge": CHALLENGE,
                "nonce": NONCE,
                "signUp": true,
                "loginHint": "ada@example.com",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["authorizeUrl"],
        format!("{APP_URL}/auth/sign-up?state=csrf-abc-123&email=ada%40example.com")
    );
    assert!(provider.calls().is_empty(), "{:?}", provider.calls());
}

/// The sign-in page's "Continue with…" names the provider; with the form
/// off, the door goes straight there.
#[sqlx::test]
async fn start_names_the_external_provider_when_asked_or_when_the_form_is_off(pool: PgPool) {
    let (app, provider) = app(pool.clone());
    let resp = app
        .oneshot(post_json(
            "/internal/v1/auth/start",
            serde_json::json!({ "state": "csrf-abc-123", "codeChallenge": CHALLENGE, "nonce": NONCE, "provider": "external" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["authorizeUrl"],
        format!(
            "{}/authorize?redirect_uri={REDIRECT_URI}&state=csrf-abc-123",
            provider.base
        )
    );
    assert_eq!(
        provider.calls(),
        vec![Call::AuthorizeUrl {
            redirect_uri: REDIRECT_URI.into(),
            state: "csrf-abc-123".into(),
            code_challenge: CHALLENGE.into(),
            nonce: NONCE.into(),
            sign_up: false,
            login_hint: None,
        }]
    );

    let (app, provider) = app_configured(
        pool,
        Setup {
            login_form: false,
            ..Setup::default()
        },
    );
    let resp = app
        .oneshot(post_json(
            "/internal/v1/auth/start",
            serde_json::json!({
                "state": "csrf-abc-123",
                "codeChallenge": CHALLENGE,
                "nonce": NONCE,
                "signUp": true,
                "loginHint": "ada@example.com",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let url = json_body(resp).await["authorizeUrl"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(url.starts_with(&provider.base), "{url}");
    assert!(url.contains("&prompt=create"), "{url}");
    assert!(url.contains("&login_hint=ada@example.com"), "{url}");
}

#[sqlx::test]
async fn start_refuses_a_provider_it_does_not_have(pool: PgPool) {
    let (app, provider) = app(pool);
    let resp = app
        .oneshot(post_json(
            "/internal/v1/auth/start",
            serde_json::json!({ "state": "csrf-abc-123", "codeChallenge": CHALLENGE, "nonce": NONCE, "provider": "google" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(provider.calls().is_empty());
}

/// The browser's sign-out names the provider's page for a session that
/// began there, and lands straight back for one that began here.
#[sqlx::test]
async fn logout_url_names_the_providers_page_only_for_a_session_that_began_there(pool: PgPool) {
    let (app, provider) = app(pool);

    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/v1/auth/logout-url",
            serde_json::json!({
                "sessionId": "session_42",
                "idToken": "id.token.x",
                "returnTo": "http://localhost:3000/",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["logoutUrl"],
        format!(
            "{}/logout?id_token_hint=id.token.x&return_to=http://localhost:3000/",
            provider.base
        )
    );
    assert_eq!(
        provider.calls(),
        vec![Call::LogoutUrl {
            id_token: Some("id.token.x".into()),
            return_to: "http://localhost:3000/".into(),
        }]
    );

    let resp = app
        .oneshot(post_json(
            "/internal/v1/auth/logout-url",
            serde_json::json!({
                "sessionId": "session_43",
                "returnTo": "http://localhost:3000/",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["logoutUrl"], "http://localhost:3000/");
    assert_eq!(provider.calls().len(), 1, "{:?}", provider.calls());
}

/// The exchange asks the provider who the person is, makes them a person
/// here under an id of our own, links the provider's subject to it, and
/// opens a session the issuer minted.
#[sqlx::test]
async fn an_external_exchange_creates_the_person_and_opens_a_session(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, provider) = app(pool.clone());

    let (status, body) = exchange_external(&app, "code_xyz").await;
    assert_eq!(status, StatusCode::OK);
    let user_id = body["userId"].as_str().unwrap().to_owned();
    assert!(user_id.starts_with("user_"), "{user_id}");
    assert_ne!(
        user_id, TEST_SUBJECT,
        "the provider's subject is not our id"
    );
    assert_eq!(body["email"], "ada@example.com");
    assert_eq!(body["emailVerified"], true);
    assert_eq!(body["firstName"], "Ada");
    assert_eq!(body["idToken"], format!("id_token_for_{TEST_SUBJECT}"));
    assert_eq!(body["authMethod"], serde_json::Value::Null);
    assert!(body["refreshToken"].is_string());
    assert!(body["expiresIn"].as_u64().unwrap() > 0);
    assert_eq!(
        provider.calls(),
        vec![Call::Exchange {
            code: "code_xyz".into(),
            code_verifier: VERIFIER.into(),
            nonce: NONCE.into(),
        }]
    );
    assert_eq!(
        linked_person(&pool, &provider, TEST_SUBJECT)
            .await
            .as_deref(),
        Some(user_id.as_str())
    );

    // The bearer is the issuer's own opaque secret, and `/me` takes it.
    let bearer = body["accessToken"].as_str().unwrap();
    assert!(
        bearer.len() == 43 && !bearer.contains('.'),
        "nothing of the provider's is relayed"
    );
    assert!(body["sessionId"].is_string(), "{body}");
    let resp = app.oneshot(post_me_under(bearer)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let me = json_body(resp).await;
    assert_eq!(me["person"]["email"], "ada@example.com");
    assert_eq!(me["person"]["displayName"], "Ada Lovelace");
    assert_eq!(me["firstLogin"], true, "{me}");
    assert_eq!(me["organizations"][0]["role"], "owner", "{me}");
    assert!(me["sessionRowId"].is_string(), "{me}");
}

#[sqlx::test]
async fn a_second_external_sign_in_finds_the_same_person(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, _provider) = app(pool.clone());

    let (_, first) = exchange_external(&app, "code_1").await;
    let (status, second) = exchange_external(&app, "code_2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["userId"], second["userId"]);
    assert_ne!(
        first["accessToken"], second["accessToken"],
        "a session each"
    );

    let people: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.identities")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(people, 1);
}

#[sqlx::test]
async fn exchange_duplicate_code_returns_cached_result_without_calling_the_provider_again(
    pool: PgPool,
) {
    let (app, provider) = app(pool);

    let (status, body1) = exchange_external(&app, "code_dup").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body2) = exchange_external(&app, "code_dup").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body1, body2);
    assert_eq!(
        provider.count(|c| matches!(c, Call::Exchange { .. })),
        1,
        "the duplicate code was served from the cache"
    );
}

/// With the provider's sign-ups closed, only a subject already linked signs
/// in through it.
#[sqlx::test]
async fn a_stranger_is_refused_while_the_providers_sign_ups_are_closed(pool: PgPool) {
    let (app, provider) = app_configured(
        pool.clone(),
        Setup {
            allow_sign_up: false,
            ..Setup::default()
        },
    );
    let (status, body) = exchange_external(&app, "code_1").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["type"], "/errors/auth/sign-up-closed", "{body}");
    assert_eq!(linked_person(&pool, &provider, TEST_SUBJECT).await, None);
    let people: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.identities")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(people, 0, "a refused sign-in left a person behind");
}

/// ⚠ A subject the provider names for the first time whose address an
/// account here already holds is refused, unless the deployment chose to
/// trust the provider's addresses: linking by address is how a password
/// account moves onto a provider, and how a provider that never checks an
/// address could take one over.
#[sqlx::test]
async fn an_address_an_account_holds_links_only_when_the_deployment_allows(pool: PgPool) {
    seed_identity(&pool, "user_local_1", "ada@example.com").await;

    let (app, provider) = app(pool.clone());
    let (status, _) = exchange_external(&app, "code_1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(linked_person(&pool, &provider, TEST_SUBJECT).await, None);

    let (app, provider) = app_configured(
        pool.clone(),
        Setup {
            link_by_email: true,
            ..Setup::default()
        },
    );
    let (status, body) = exchange_external(&app, "code_2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["userId"], "user_local_1");
    assert_eq!(
        linked_person(&pool, &provider, TEST_SUBJECT)
            .await
            .as_deref(),
        Some("user_local_1")
    );
}

#[sqlx::test]
async fn refresh_exchanges_the_refresh_token_for_a_fresh_pair(pool: PgPool) {
    let (app, provider) = app(pool);
    let (_, signed_in) = exchange_external(&app, "code_1").await;
    let refresh_token = signed_in["refreshToken"].as_str().unwrap();

    let resp = app
        .oneshot(post_json(
            "/internal/v1/auth/refresh",
            serde_json::json!({ "refreshToken": refresh_token }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["userId"], signed_in["userId"]);
    assert_ne!(body["accessToken"], signed_in["accessToken"]);
    assert_ne!(body["refreshToken"], signed_in["refreshToken"]);
    assert_eq!(
        body["idToken"],
        serde_json::Value::Null,
        "a refresh carries no id token; the console keeps the one it sealed"
    );
    assert_eq!(
        provider.count(|c| matches!(c, Call::Exchange { .. })),
        1,
        "a refresh asks nothing of the provider: {:?}",
        provider.calls()
    );
}

#[sqlx::test]
async fn session_endpoints_require_the_service_secret(pool: PgPool) {
    let (app, provider) = app(pool);

    let resp = app
        .oneshot(
            Request::post("/internal/v1/auth/exchange")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "code": "code_xyz", "codeVerifier": VERIFIER, "nonce": NONCE, "provider": "external" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(provider.calls().is_empty(), "{:?}", provider.calls());
}

/// The CLI's start: the codes the person will see, and the one the CLI polls
/// with, minted here for the console's device page.
#[sqlx::test]
async fn a_device_start_hands_the_cli_the_codes(pool: PgPool) {
    let (app, provider) = app(pool);

    let resp = app
        .oneshot(post_json("/cli/v1/auth/device", serde_json::json!({})))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["userCode"].as_str().unwrap().len(), 9, "{body}");
    assert!(body["deviceCode"].is_string());
    assert_eq!(body["verificationUri"], format!("{APP_URL}/auth/device"));
    assert_eq!(body["expiresIn"], 600);
    assert_eq!(body["interval"], 5);
    assert!(provider.calls().is_empty(), "{:?}", provider.calls());
}

/// A blank device code is refused before anything is looked up.
#[sqlx::test]
async fn a_device_poll_refuses_a_blank_code(pool: PgPool) {
    let (app, provider) = app(pool);

    let resp = app
        .oneshot(post_json(
            "/cli/v1/auth/device/poll",
            serde_json::json!({ "deviceCode": "  " }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(provider.calls().is_empty(), "{:?}", provider.calls());
}

/// What the console's sign-in pages show, as this deployment is configured.
#[sqlx::test]
async fn the_config_lane_describes_the_sign_in(pool: PgPool) {
    let (app, _) = app(pool.clone());
    let resp = app.oneshot(get("/internal/v1/auth/config")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({
            "passwordSignIn": true,
            "allowSignUp": true,
            "verifyEmail": true,
            "external": { "name": "the test provider" },
        })
    );

    let (app, _) = app_configured(
        pool,
        Setup {
            login_form: false,
            ..Setup::default()
        },
    );
    let resp = app.oneshot(get("/internal/v1/auth/config")).await.unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["passwordSignIn"], false);
    assert_eq!(body["allowSignUp"], false);
}

/// The login form's lanes exist only while the form is on.
#[sqlx::test]
async fn the_password_lanes_are_off_with_the_form(pool: PgPool) {
    let (app, _) = app_configured(
        pool,
        Setup {
            login_form: false,
            ..Setup::default()
        },
    );
    for lane in [
        "/internal/v1/auth/password/sign-up",
        "/internal/v1/auth/password/sign-in",
        "/internal/v1/auth/password/forgot",
    ] {
        let resp = app
            .clone()
            .oneshot(post_json(
                lane,
                serde_json::json!({ "email": "a@example.com" }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{lane}");
    }
}

/// A sign-in with sign-ups closed provisions nothing, and is still a sign-in:
/// the console seals a cookie for the person's account, so the session is
/// recorded like any other — listed on their sessions page, and ended by
/// their sign-out.
#[sqlx::test]
async fn a_sign_in_with_sign_ups_closed_records_its_session(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    sqlx::query(
        "INSERT INTO auth.feature_flags (key, enabled, note, actor) VALUES ($1, false, 'test', 'test')",
    )
    .bind(telmoni_shared::Flag::Signup.as_str())
    .execute(&pool)
    .await
    .unwrap();
    seed_identity(&pool, "user_unprovisioned_123", "nobody@example.com").await;
    let (app, _provider) = app(pool.clone());

    let resp = app
        .oneshot(post_me_with_session(&pool, "user_unprovisioned_123", "ses_123").await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["activeOrganizationId"], serde_json::Value::Null);

    let row: String = sqlx::query_scalar(
        "SELECT id::text FROM auth.sessions
          WHERE user_id = 'user_unprovisioned_123' AND sid = 'ses_123'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(body["sessionRowId"], row.as_str());
    let organizations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_members WHERE user_id = 'user_unprovisioned_123'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        organizations, 0,
        "a closed sign-up provisioned an organization"
    );
}

/// A sign-in records the session its bearer belongs to, keyed on the person
/// the bearer names, and hands the row id back for the cookie.
#[sqlx::test]
async fn a_sign_in_records_the_session_its_bearer_names(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_provisioned_456", "test@example.com").await;
    let (app, _provider) = app(pool.clone());

    let resp = app
        .oneshot(post_me_with_session(&pool, "user_provisioned_456", "ses_456").await)
        .await
        .unwrap();
    let status = resp.status();
    let body = json_body(resp).await;
    assert_eq!(status, StatusCode::OK, "response body: {body:?}");
    let row_id = body["sessionRowId"]
        .as_str()
        .expect("the session row id rides the sign-in answer")
        .parse::<uuid::Uuid>()
        .expect("the row id is a uuid");

    let (person, sid): (String, String) =
        sqlx::query_as("SELECT user_id, sid FROM auth.sessions WHERE id = $1")
            .bind(row_id)
            .fetch_one(&pool)
            .await
            .expect("the session row");
    assert_eq!(person, "user_provisioned_456");
    assert_eq!(sid, "ses_456");
}

/// ⚠ AN ORGANIZATION IS BORN NAMED AFTER ITS HOLDER, at the slug that name
/// reads as, so the console opens
/// straight onto it. Nothing is read off the address, which travels beside it
/// for the console to name whom to ask: read from the person, never copied
/// onto the organization.
#[sqlx::test]
async fn provisioning_names_the_organization_after_its_holder(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (application, _provider) = app_configured(
        pool.clone(),
        Setup {
            test_door: true,
            ..Setup::default()
        },
    );

    for (user, names, expected_name, expected_slug) in [
        (
            "user_named_at_birth",
            serde_json::json!({ "firstName": "Ada", "lastName": "Lovelace" }),
            "Ada's organization",
            "adas-organization",
        ),
        (
            "user_with_no_name",
            serde_json::json!({}),
            "My organization",
            "my-organization",
        ),
    ] {
        // Through the writer the exchange uses, so the lowercasing is the
        // service's own rather than the fixture's.
        let mut session = serde_json::json!({ "userId": user, "email": "Ada@Example.com" });
        if let (Some(session), Some(names)) = (session.as_object_mut(), names.as_object()) {
            session.extend(names.clone());
        }
        let resp = application
            .clone()
            .oneshot(post_json("/internal/v1/test/session", session))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bearer = json_body(resp).await["accessToken"]
            .as_str()
            .unwrap()
            .to_owned();

        let resp = application
            .clone()
            .oneshot(post_me_under(&bearer))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        let organization = body["activeOrganizationId"]
            .as_str()
            .expect("/me names the organization it provisioned");

        let (name, slug): (String, String) =
            sqlx::query_as("SELECT name, slug FROM auth.organizations WHERE external_id = $1")
                .bind(organization)
                .fetch_one(&pool)
                .await
                .expect("the organization row");

        assert_eq!(name, expected_name, "{user}");
        assert_eq!(slug, expected_slug, "{user}");
        assert_eq!(body["organizations"][0]["name"], expected_name, "{user}");
        assert_eq!(body["organizations"][0]["ownerEmail"], "ada@example.com");
    }
}

/// ⚠ **One address, two people, two organizations.** An external provider owns
/// address uniqueness for the people it names, so each person is provisioned
/// their own organization regardless of email overlap.
#[sqlx::test]
async fn two_people_on_one_address_are_each_provisioned_an_organization(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_collision_1", "shared@example.com").await;
    seed_identity(&pool, "user_collision_2", "shared@example.com").await;
    let (application, _provider) = app(pool.clone());

    let mut organizations = std::collections::BTreeSet::new();
    for user in ["user_collision_1", "user_collision_2"] {
        let resp = application
            .clone()
            .oneshot(post_me(&pool, user).await)
            .await
            .unwrap();
        let status = resp.status();
        let body = json_body(resp).await;
        assert_eq!(status, StatusCode::OK, "{user}: {body}");
        assert_eq!(body["firstLogin"], true, "{user}: {body}");
        assert_eq!(body["organizations"][0]["role"], "owner", "{user}: {body}");
        organizations.insert(
            body["activeOrganizationId"]
                .as_str()
                .expect("/me names the organization it provisioned")
                .to_owned(),
        );
    }
    assert_eq!(
        organizations.len(),
        2,
        "one organization each, not one shared: {organizations:?}"
    );
}

/// The same on every later sign-in: an address the provider moves somebody
/// onto is followed even when another person here holds it.
#[sqlx::test]
async fn an_address_moved_onto_one_in_use_is_followed_on_the_next_sign_in(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_refresh_1", "user1@example.com").await;
    seed_identity(&pool, "user_refresh_2", "user2@example.com").await;
    let (application, _provider) = app(pool.clone());

    let resp1 = application
        .clone()
        .oneshot(post_me(&pool, "user_refresh_1").await)
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    let resp2 = application
        .clone()
        .oneshot(post_me(&pool, "user_refresh_2").await)
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);

    // The provider now says user 2 holds user 1's address.
    seed_identity(&pool, "user_refresh_2", "user1@example.com").await;
    let resp3 = application
        .oneshot(post_me(&pool, "user_refresh_2").await)
        .await
        .unwrap();

    let status = resp3.status();
    let body = json_body(resp3).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["firstLogin"], false, "the same person, not a new one");
    assert_eq!(body["person"]["email"], "user1@example.com");
    assert_eq!(
        body["organizations"][0]["ownerEmail"], "user1@example.com",
        "and rides beside the organization they own, for the console to name whom to ask"
    );
}

/// ⚠ **`/me` is described by the exchange, never by the console.** The exchange
/// records what the provider answered, and that is who `/me` provisions the
/// organization for.
#[sqlx::test]
async fn me_refuses_a_body_that_describes_the_person(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (application, _provider) = app(pool.clone());
    let (_, signed_in) = exchange_external(&application, "code_e2e").await;
    let bearer = signed_in["accessToken"].as_str().unwrap();

    let resp = application
        .oneshot(
            Request::post("/internal/v1/me")
                .header("content-type", "application/json")
                .header("x-service-secret", SERVICE_SECRET)
                .header("authorization", format!("Bearer {bearer}"))
                .body(Body::from(
                    serde_json::json!({ "email": "someone-else@example.com" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a body that describes the person is refused, not trusted"
    );
}

/// A bearer no sign-in minted names nobody, and cannot provision anything or
/// record a session.
#[sqlx::test]
async fn me_refuses_a_bearer_no_sign_in_minted(pool: PgPool) {
    let (app, _provider) = app(pool.clone());
    let resp = app
        .oneshot(post_me_under("never_minted_here"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let organizations: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.organizations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(organizations, 0, "a bearer nobody minted provisioned one");
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "a refused /me recorded a session row");
}

/// ⚠ **The test door exists only where it was opened.** A deployed auth has
/// `ALLOW_TEST_SESSION` off (and refuses it in a pod), so it has no route
/// that writes an identity from a body.
#[sqlx::test]
async fn the_test_session_route_is_mounted_only_when_the_door_is_open(pool: PgPool) {
    let request = || {
        post_json(
            "/internal/v1/test/session",
            serde_json::json!({ "userId": "user_test_door", "email": "door@example.com" }),
        )
    };

    let (deployed, _provider) = app(pool.clone());
    let resp = deployed.oneshot(request()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let (opened, _provider) = app_configured(
        pool.clone(),
        Setup {
            test_door: true,
            ..Setup::default()
        },
    );
    let resp = opened.oneshot(request()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let email: String = sqlx::query_scalar("SELECT email FROM auth.identities WHERE user_id = $1")
        .bind("user_test_door")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(email, "door@example.com");
}

/// Ada's address, as the scripted provider answers it, verified or not.
fn ada(verified: bool) -> telmoni_auth::provider::Authenticated {
    let mut answer = telmoni_auth::test_provider::authenticated(TEST_SUBJECT);
    answer.subject.email_verified = verified;
    answer
}

/// A live invitation for `email`, from an organization made for it.
async fn invite(pool: &PgPool, email: &str) {
    sqlx::query(
        "INSERT INTO auth.organizations (id, external_id, slug, name, shard_key)
         VALUES ($1, $2, 'org-' || md5($2), 'Acme', $3)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind("org_invites_1")
    .bind(uuid::Uuid::new_v4())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth.organization_invites
             (id, organization_id, email, role, token_hash, invited_by, expires_at, shard_key)
         VALUES ($1, $2, $3, 'member', $4, 'user_owner_1', now() + interval '7 days', $5)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind("org_invites_1")
    .bind(email)
    .bind(telmoni_shared::digest::sha256_hex(b"invite_token_1"))
    .bind(uuid::Uuid::new_v4())
    .execute(pool)
    .await
    .unwrap();
}

/// With sign-ups closed, an invitation is the operator's own yes at a
/// provider as at the form — for an address the provider verified, never
/// for one it only typed.
#[sqlx::test]
async fn with_sign_ups_closed_an_invited_verified_address_signs_up(pool: PgPool) {
    invite(&pool, "ada@example.com").await;
    let (app, provider) = app_configured(
        pool.clone(),
        Setup {
            allow_sign_up: false,
            ..Setup::default()
        },
    );
    provider.on_exchange(Ok(ada(false)));
    let (status, _) = exchange_external(&app, "code_1").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an unverified address");
    assert_eq!(linked_person(&pool, &provider, TEST_SUBJECT).await, None);

    provider.on_exchange(Ok(ada(true)));
    let (status, body) = exchange_external(&app, "code_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        linked_person(&pool, &provider, TEST_SUBJECT)
            .await
            .is_some()
    );
}

/// ⚠ Linking by address takes the deployment's switch and an address this
/// sign-in's provider verified: with the switch on, an unverified address
/// is refused as if it were off.
#[sqlx::test]
async fn an_address_links_only_where_the_provider_verified_it(pool: PgPool) {
    seed_identity(&pool, "user_local_1", "ada@example.com").await;
    let (app, provider) = app_configured(
        pool.clone(),
        Setup {
            link_by_email: true,
            ..Setup::default()
        },
    );
    provider.on_exchange(Ok(ada(false)));
    let (status, _) = exchange_external(&app, "code_1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(linked_person(&pool, &provider, TEST_SUBJECT).await, None);
}

/// A linked person's unverified new address never takes one another person
/// holds; one nobody holds is recorded as before.
#[sqlx::test]
async fn an_unverified_address_never_takes_one_another_person_holds(pool: PgPool) {
    seed_identity(&pool, "user_grace_1", "grace@example.com").await;
    let (app, provider) = app(pool.clone());
    provider.on_exchange(Ok(ada(true)));
    let (status, body) = exchange_external(&app, "code_1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ada_id = linked_person(&pool, &provider, TEST_SUBJECT).await.unwrap();
    let address = |user: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT email FROM auth.identities WHERE user_id = $1")
                .bind(user)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    let mut squat = ada(false);
    squat.subject.email = Some("grace@example.com".into());
    provider.on_exchange(Ok(squat));
    let (status, _) = exchange_external(&app, "code_2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(address(ada_id.clone()).await, "ada@example.com");
    assert_eq!(address("user_grace_1".into()).await, "grace@example.com");

    let mut moved = ada(false);
    moved.subject.email = Some("ada.new@example.com".into());
    provider.on_exchange(Ok(moved));
    let (status, _) = exchange_external(&app, "code_3").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(address(ada_id).await, "ada.new@example.com");
}

/// ⚠ An account made at the form under an address it never proved is not
/// linked, or whoever made it would keep a password into the person who
/// signs in at the provider; once it proved the address, it is.
#[sqlx::test]
async fn an_address_links_only_to_an_account_that_proved_it(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, provider) = app_configured(
        pool.clone(),
        Setup {
            link_by_email: true,
            ..Setup::default()
        },
    );
    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/v1/auth/password/sign-up",
            serde_json::json!({
                "email": "ada@example.com",
                "password": "correct horse battery staple",
                "codeChallenge": CHALLENGE,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    provider.on_exchange(Ok(ada(true)));
    let (status, _) = exchange_external(&app, "code_1").await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an address the account never proved"
    );
    assert_eq!(linked_person(&pool, &provider, TEST_SUBJECT).await, None);

    sqlx::query("UPDATE auth.identities SET email_verified = true WHERE email = $1")
        .bind("ada@example.com")
        .execute(&pool)
        .await
        .unwrap();
    provider.on_exchange(Ok(ada(true)));
    let (status, body) = exchange_external(&app, "code_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let holder: String = sqlx::query_scalar("SELECT user_id FROM auth.identities WHERE email = $1")
        .bind("ada@example.com")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        linked_person(&pool, &provider, TEST_SUBJECT).await,
        Some(holder)
    );
}

/// The deployment's default organization, `org_default_1`, with its project,
/// and another organization's project beside them.
async fn default_home(pool: &PgPool) {
    for (organization, project) in [
        ("org_default_1", "project_default_1"),
        ("org_elsewhere_1", "project_elsewhere_1"),
    ] {
        sqlx::query(
            "INSERT INTO auth.organizations (id, external_id, slug, name, shard_key)
             VALUES ($1, $2, 'org-' || md5($2), 'Acme', $3)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(organization)
        .bind(uuid::Uuid::new_v4())
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO auth.projects (external_id, organization_id, name, slug)
             VALUES ($1, $2, 'Agents', 'agents')",
        )
        .bind(project)
        .bind(organization)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Everyone created joins `organization` as an admin, seated on `project`
/// as a member.
fn landing(organization: &str, project: &str) -> DefaultOrganization {
    DefaultOrganization {
        organization_id: OrganizationId::try_new(organization).unwrap(),
        role: OrganizationRole::Admin,
        project: Some((ProjectId::try_new(project).unwrap(), Role::Member)),
    }
}

/// `user`'s role on the default organization's roster, and the seat they
/// hold, wherever it is.
async fn default_roles(pool: &PgPool, user: &str) -> (Option<String>, Option<String>) {
    let roster = sqlx::query_scalar(
        "SELECT role FROM auth.organization_members
          WHERE organization_id = 'org_default_1' AND user_id = $1",
    )
    .bind(user)
    .fetch_optional(pool)
    .await
    .unwrap();
    let seat = sqlx::query_scalar("SELECT role FROM auth.project_members WHERE user_id = $1")
        .bind(user)
        .fetch_optional(pool)
        .await
        .unwrap();
    (roster, seat)
}

/// A person the provider creates joins the default organization and its
/// project as they are created, audited as the deployment's act, and owns
/// nothing of their own; removed, they are not put back at their next
/// sign-in.
#[sqlx::test]
async fn a_person_a_provider_creates_joins_the_default_organization_once(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    default_home(&pool).await;
    let (app, _provider) = app_configured(
        pool.clone(),
        Setup {
            default_organization: Some(landing("org_default_1", "project_default_1")),
            ..Setup::default()
        },
    );

    let (status, body) = exchange_external(&app, "code_1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let user_id = body["userId"].as_str().unwrap().to_owned();
    assert_eq!(
        default_roles(&pool, &user_id).await,
        (Some("admin".into()), Some("member".into()))
    );
    let recorded: Vec<(String, Option<String>, serde_json::Value)> = sqlx::query_as(
        "SELECT actor_id, in_project, metadata FROM audit.events
          WHERE organization_id = 'org_default_1' AND resource_id = $1 ORDER BY seq",
    )
    .bind(&user_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        recorded,
        [
            (
                "service:default-organization".to_owned(),
                None,
                serde_json::json!({ "kind": "default_organization", "role": "admin" }),
            ),
            (
                "service:default-organization".to_owned(),
                Some("project_default_1".to_owned()),
                serde_json::json!({ "kind": "default_organization", "role": "member" }),
            ),
        ]
    );

    let resp = app
        .clone()
        .oneshot(post_me_under(body["accessToken"].as_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let me = json_body(resp).await;
    assert_eq!(me["firstLogin"], false, "{me}");
    assert_eq!(me["activeOrganizationId"], "org_default_1", "{me}");
    assert_eq!(
        me["organizations"].as_array().map(Vec::len),
        Some(1),
        "{me}"
    );

    for table in ["auth.project_members", "auth.organization_members"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE user_id = $1"
        )))
        .bind(&user_id)
        .execute(&pool)
        .await
        .unwrap();
    }
    let (status, again) = exchange_external(&app, "code_2").await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["userId"], body["userId"]);
    assert_eq!(default_roles(&pool, &user_id).await, (None, None));
}

/// ⚠ An organization gone since boot, or a project no longer in it, is
/// passed over at the joining: the sign-in stands.
#[sqlx::test]
async fn a_default_organization_gone_since_boot_never_refuses_a_sign_in(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    default_home(&pool).await;
    let (app, _provider) = app_configured(
        pool.clone(),
        Setup {
            default_organization: Some(landing("org_default_1", "project_elsewhere_1")),
            ..Setup::default()
        },
    );
    let (status, body) = exchange_external(&app, "code_1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let user_id = body["userId"].as_str().unwrap().to_owned();
    assert_eq!(
        default_roles(&pool, &user_id).await,
        (Some("admin".into()), None),
        "the roster row, and no seat on another organization's project"
    );

    let (app, provider) = app_configured(
        pool.clone(),
        Setup {
            default_organization: Some(landing("org_gone_1", "project_default_1")),
            ..Setup::default()
        },
    );
    let mut grace = telmoni_auth::test_provider::authenticated("subject_grace");
    grace.subject.email = Some("grace@example.com".into());
    provider.on_exchange(Ok(grace));
    let (status, body) = exchange_external(&app, "code_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM auth.organization_members WHERE user_id = $1)
              + (SELECT count(*) FROM auth.project_members WHERE user_id = $1)",
    )
    .bind(body["userId"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 0);
}

/// The test door joins whom it creates, as the exchange it stands in for
/// would, and nobody it only records again.
#[sqlx::test]
async fn the_test_door_joins_only_whom_it_creates(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    default_home(&pool).await;
    seed_identity(&pool, "user_already_here", "here@example.com").await;
    let (app, _provider) = app_configured(
        pool.clone(),
        Setup {
            test_door: true,
            default_organization: Some(landing("org_default_1", "project_default_1")),
            ..Setup::default()
        },
    );
    for (user, email) in [
        ("user_new_here", "new@example.com"),
        ("user_already_here", "here@example.com"),
    ] {
        let resp = app
            .clone()
            .oneshot(post_json(
                "/internal/v1/test/session",
                serde_json::json!({ "userId": user, "email": email }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{user}");
    }
    assert_eq!(
        default_roles(&pool, "user_new_here").await,
        (Some("admin".into()), Some("member".into()))
    );
    assert_eq!(
        default_roles(&pool, "user_already_here").await,
        (None, None)
    );
}

/// ⚠ Boot refuses an organization that is not active, and a project that is
/// not an active one of it.
#[sqlx::test]
async fn boot_refuses_a_default_it_cannot_land_anybody_in(pool: PgPool) {
    default_home(&pool).await;
    let db = service_pool(&pool, ServiceRole::Auth);
    landing("org_default_1", "project_default_1")
        .check(&db)
        .await
        .expect("an active organization and its project");
    DefaultOrganization {
        project: None,
        ..landing("org_default_1", "project_default_1")
    }
    .check(&db)
    .await
    .expect("an organization alone");

    sqlx::query(
        "UPDATE auth.projects SET status = 'pending_deletion', deletion_requested_at = now()
          WHERE external_id = 'project_default_1'",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (default, variable) in [
        (
            landing("org_gone_1", "project_default_1"),
            "TELMONI_DEFAULT_ORG_ID",
        ),
        (
            landing("org_default_1", "project_elsewhere_1"),
            "TELMONI_DEFAULT_PROJECT_ID",
        ),
        (
            landing("org_default_1", "project_default_1"),
            "TELMONI_DEFAULT_PROJECT_ID",
        ),
    ] {
        let refused = default.check(&db).await.expect_err(variable).to_string();
        assert!(refused.starts_with(variable), "{refused}");
    }
}
