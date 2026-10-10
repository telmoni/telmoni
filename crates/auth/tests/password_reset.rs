//! `POST /internal/me/password-reset` — the lane that asks the provider to mail
//! a one-time link.
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
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::provider::PasswordResetLink;
use telmoni_auth::test_provider::{Call, ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::AuthError;
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};

const SERVICE_SECRET: &str = "test-service-secret";
const USER: &str = "user_reset_1";
const EMAIL: &str = "ada@example.com";
const OTHER: &str = "user_reset_2";
const OTHER_EMAIL: &str = "grace@example.com";

/// The hosted page with the token in its query, which must only reach the mail.
const RESET_URL: &str = "https://idp.telmoni.invalid/password-reset/tok_live_abc123";

/// Records every mail the router hands to the transport.
#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<telmoni_shared::mail::Mail>>);

#[async_trait::async_trait]
impl telmoni_shared::mail::MailSender for Recorder {
    async fn send(
        &self,
        mail: &telmoni_shared::mail::Mail,
    ) -> Result<(), telmoni_shared::mail::MailError> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

/// The router around a scripted provider, and the provider itself, so a test
/// can script its answers and read back what the lane asked of it.
fn app(
    pool: PgPool,
    sender: Arc<dyn telmoni_shared::mail::MailSender>,
) -> (Router, Arc<ScriptedProvider>) {
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
    let router = router(Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(provider.clone())),
        db,
        config,
        siblings: telmoni_auth::Siblings::default(),
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(sender)),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
    }));
    (router, provider)
}

/// What a code exchange would have recorded about the person.
async fn seed(pool: &PgPool, user: &str, email: &str) {
    apply_audit_migrations(pool).await;
    seed_identity(pool, user, email).await;
}

/// Sign `user` in, which provisions their organization, and answer its id.
async fn sign_in(pool: &PgPool, user: &str) -> String {
    let (app, _) = app(pool.clone(), Arc::new(Recorder::default()));
    let resp = app
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
            .body(Body::from("{}"))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "sign-in failed for {user}");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let me: Value = serde_json::from_slice(&bytes).unwrap();
    me.get("activeOrganizationId")
        .and_then(Value::as_str)
        .expect("/me names the active organization")
        .to_owned()
}

/// The request the BFF makes, without its bearer: whichever organization the
/// console happens to have active, which this lane does not read.
fn reset_request(organization: Option<&str>) -> axum::http::request::Builder {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/internal/me/password-reset")
        .header("x-service-secret", SERVICE_SECRET);
    if let Some(organization) = organization {
        builder = builder.header("x-organization-id", organization);
    }
    builder
}

/// The request the BFF makes: the person's bearer, and the organization.
async fn reset(pool: &PgPool, actor: &str, organization: Option<&str>) -> Request<Body> {
    as_person(reset_request(organization), pool, actor)
        .await
        .body(Body::empty())
        .unwrap()
}

/// A link the provider minted and mails itself; the URL carries the token.
fn minted() -> PasswordResetLink {
    PasswordResetLink {
        url: RESET_URL.into(),
        expires_at: "2026-09-18T13:45:00.000Z".into(),
    }
}

/// The provider's "no such user": its record of the person is gone.
fn gone() -> Result<PasswordResetLink, telmoni_shared::TelmoniError> {
    Err(AuthError::NotFound("no user at the identity provider for that address".into()).into())
}

async fn body_of(resp: axum::response::Response) -> String {
    String::from_utf8(
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

/// The addresses the provider was asked to mint a link for, in order.
fn addresses_asked_for(provider: &ScriptedProvider) -> Vec<String> {
    provider
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::PasswordReset { email } => Some(email),
            _ => None,
        })
        .collect()
}

/// The whole lane, on the path the console actually calls.
#[sqlx::test]
async fn a_link_is_minted_for_the_stored_address_and_this_service_mails_nothing(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let mail = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), mail.clone());
    provider.on_password_reset(Ok(minted()));

    let resp = app.oneshot(reset(&pool, USER, None).await).await.unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_of(resp).await;
    assert!(body.contains("\"sent\":true"), "{body}");
    assert!(
        !body.contains("idp.telmoni.invalid") && !body.contains("tok_live"),
        "the response carried the credential: {body}"
    );
    assert_eq!(addresses_asked_for(&provider), vec![EMAIL.to_owned()]);

    let sent = mail.0.lock().unwrap();
    assert!(
        sent.is_empty(),
        "auth mailed the reset link itself — the provider already does: {sent:?}"
    );
}

/// ⚠ ASKING FOR SOMEBODY ELSE'S RECOVERY IS THE ATTACK. The bearer is the only
/// subject this lane has: a caller naming the victim's organization still gets
/// a link for their own address, and the victim's never reaches the provider.
#[sqlx::test]
async fn one_person_cannot_ask_for_anothers_reset(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    seed(&pool, OTHER, OTHER_EMAIL).await;
    let victims = sign_in(&pool, USER).await;
    let mail = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), mail.clone());

    let resp = app
        .oneshot(reset(&pool, OTHER, Some(&victims)).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(
        addresses_asked_for(&provider),
        vec![OTHER_EMAIL.to_owned()],
        "the provider was asked for somebody other than the bearer"
    );
    assert!(mail.0.lock().unwrap().is_empty(), "a mail went out anyway");
}

/// The organization header is context this lane never reads: the reset is the
/// person's, not any organization's, so naming one they are not in neither
/// refuses them nor points the link anywhere else.
#[sqlx::test]
async fn the_organization_header_neither_refuses_nor_redirects_the_reset(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    seed(&pool, OTHER, OTHER_EMAIL).await;
    let someone_elses = sign_in(&pool, OTHER).await;
    let mail = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), mail.clone());

    let resp = app
        .oneshot(reset(&pool, USER, Some(&someone_elses)).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(addresses_asked_for(&provider), vec![EMAIL.to_owned()]);
    assert!(mail.0.lock().unwrap().is_empty());
}

/// A bearer auth never minted names nobody: there is no address on record to
/// take, and the lane has nowhere else to take one from.
#[sqlx::test]
async fn a_bearer_auth_never_minted_never_reaches_the_provider(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mail = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), mail.clone());

    let resp = app
        .oneshot(
            reset_request(None)
                .header("authorization", "Bearer never_minted_here")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        provider.calls().is_empty(),
        "an unrecorded person reached the provider"
    );
    assert!(mail.0.lock().unwrap().is_empty());
}

/// ⚠ **A PERSON ON THEIR WAY OUT GETS NO CREDENTIAL**: once they have
/// confirmed deleting their account, the person gate refuses their bearer
/// before this lane can ask for a link.
#[sqlx::test]
async fn a_person_being_deleted_is_refused_a_live_credential(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    sqlx::query("INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())")
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    let mail = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), mail.clone());

    let resp = app.oneshot(reset(&pool, USER, None).await).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(mail.0.lock().unwrap().is_empty());
    assert!(provider.calls().is_empty());
}

/// Deleting an organization touches nobody's account, its owner's included:
/// an owner whose organization is on its way out still recovers their sign-in.
#[sqlx::test]
async fn an_organization_being_deleted_does_not_hold_up_its_owners_reset(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let organization = sign_in(&pool, USER).await;
    sqlx::query(
        "UPDATE auth.organizations
            SET status = 'pending_deletion', deletion_requested_at = now(),
                erase_after = now() + interval '15 minutes', deletion_kind = 'owner'
          WHERE external_id = $1",
    )
    .bind(&organization)
    .execute(&pool)
    .await
    .unwrap();
    let mail = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), mail.clone());

    let resp = app
        .oneshot(reset(&pool, USER, Some(&organization)).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(addresses_asked_for(&provider), vec![EMAIL.to_owned()]);
    assert!(mail.0.lock().unwrap().is_empty());
}

/// ⚠ **THE PROVIDER'S "NO SUCH USER" IS DRIFT, NOT AN OUTAGE**: the provider
/// erased the person and our row outlived it. The lane surfaces that verdict
/// as it is, and mails nothing for a link that was never minted.
#[sqlx::test]
async fn a_provider_with_no_user_for_our_address_is_not_an_internal_error(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let mail = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), mail.clone());
    provider.on_password_reset(gone());

    let resp = app.oneshot(reset(&pool, USER, None).await).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(addresses_asked_for(&provider), vec![EMAIL.to_owned()]);
    assert!(
        mail.0.lock().unwrap().is_empty(),
        "a mail went out for a link that was never minted"
    );
}
