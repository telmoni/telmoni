//! Accounts auth holds itself, through the real router: sign-up under each
//! policy (open or by invitation, with the address confirmed from its mail
//! or taken at its word), sign-in and the code the callback spends, the
//! bearer and its refresh, the reset, the device grant, the address change
//! and the seeded admin — with the login form wired in and every mail
//! captured.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::map_unwrap_or,
    reason = "test scaffolding: a panicking helper is a failing test"
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::db::{AuthLane, refresh_tokens};
use telmoni_auth::mailer::{ComposingMailer, Mailer};
use telmoni_auth::password::PasswordProvider;
use telmoni_auth::test_provider::test_issuer_verifying;
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::digest::sha256_hex;
use telmoni_shared::mail::{Mail, MailError, MailSender};
use telmoni_shared::test_util::service_pool;

const SERVICE_SECRET: &str = "test-service-secret";
const APP_URL: &str = "http://localhost:3000";
const PASSWORD: &str = "correct horse battery staple";

/// Every mail the provider and the lanes composed, in order.
#[derive(Default)]
struct Outbox(Mutex<Vec<Mail>>);

#[async_trait::async_trait]
impl MailSender for Outbox {
    async fn send(&self, mail: &Mail) -> Result<(), MailError> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

impl Outbox {
    fn all(&self) -> Vec<Mail> {
        self.0.lock().unwrap().clone()
    }

    /// The newest mail to `to` whose text carries `marker`, waiting a little
    /// for one that is sent off the request (the reset link).
    async fn wait_for(&self, to: &str, marker: &str) -> Mail {
        for _ in 0..40 {
            if let Some(mail) = self
                .all()
                .into_iter()
                .rev()
                .find(|m| m.to == to && m.text.contains(marker))
            {
                return mail;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no mail to {to} carrying {marker:?}; got {:?}", self.all());
    }

    /// The `uid` and `token` of the link in `mail` that starts with `path`.
    fn link(mail: &Mail, path: &str) -> (String, String) {
        let start = mail
            .text
            .find(&format!("{APP_URL}{path}?"))
            .unwrap_or_else(|| panic!("no {path} link in {:?}", mail.text));
        let end = mail.text[start..]
            .find(char::is_whitespace)
            .map_or(mail.text.len(), |e| start + e);
        let url = reqwest::Url::parse(&mail.text[start..end]).unwrap();
        let get = |k: &str| {
            url.query_pairs()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
                .unwrap_or_else(|| panic!("no {k} in {url}"))
        };
        (get("uid"), get("token"))
    }

    /// The six digits after "code is " in `mail`.
    fn code(mail: &Mail) -> String {
        let at = mail.text.find("code is ").expect("a code") + "code is ".len();
        mail.text[at..at + 6].to_owned()
    }
}

/// The deployment's sign-up policy.
#[derive(Clone, Copy)]
struct Policy {
    allow_sign_up: bool,
    verify_email: bool,
}

/// Sign-ups open, addresses confirmed from a mail: what most tests assume.
const OPEN_AND_VERIFIED: Policy = Policy {
    allow_sign_up: true,
    verify_email: true,
};

fn config() -> Config {
    Config {
        database_url: String::new(),
        service_secret: SERVICE_SECRET.into(),
        service_secret_next: None,
        allow_test_session: false,
        redirect_uri: format!("{APP_URL}/auth/callback"),
        app_url: APP_URL.into(),
        mail_from: "Telmoni <test@example.com>".into(),
        support_email: None,
        deletion_tail_budget_ms: 8_000,
    }
}

/// The login form over `pool`, mailing to `sender`, under `policy`.
fn password_provider(
    pool: &PgPool,
    policy: Policy,
    sender: Arc<dyn MailSender>,
) -> (Arc<PasswordProvider>, Arc<telmoni_auth::issuer::Issuer>) {
    let db = service_pool(pool, "auth");
    let issuer = test_issuer_verifying(db.clone(), policy.verify_email);
    let mailer: Arc<dyn Mailer> = Arc::new(ComposingMailer::new(sender));
    let provider = Arc::new(PasswordProvider::new(
        db,
        Arc::clone(&issuer),
        mailer,
        policy.allow_sign_up,
    ));
    (provider, issuer)
}

/// The real router with the login form on, minting and resolving its own
/// sessions as a deployment does.
fn app(pool: PgPool) -> (Router, Arc<Outbox>) {
    app_under(pool, OPEN_AND_VERIFIED)
}

fn app_under(pool: PgPool, policy: Policy) -> (Router, Arc<Outbox>) {
    let outbox = Arc::new(Outbox::default());
    let sender: Arc<dyn MailSender> = outbox.clone();
    let (password, issuer) = password_provider(&pool, policy, sender.clone());
    let state = Arc::new(AppState {
        db: service_pool(&pool, "auth"),
        config: config(),
        issuer,
        password: Some(password),
        external: None,
        mailer: Arc::new(ComposingMailer::new(sender)),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
        siblings: telmoni_auth::Siblings::default(),
    });
    (router(state), outbox)
}

fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .header("x-service-secret", SERVICE_SECRET)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_as(uri: &str, bearer: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .header("x-service-secret", SERVICE_SECRET)
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// The sign-up request every test sends.
fn sign_up_request(email: &str) -> Request<Body> {
    post_json(
        "/internal/auth/password/sign-up",
        serde_json::json!({ "email": email, "password": PASSWORD, "givenName": "Ada" }),
    )
}

/// Sign up under a policy that mails a verification link, and read the link.
/// The account keeps the address lowercased, and the mail goes to that.
async fn sign_up(app: &Router, outbox: &Outbox, email: &str) -> (String, String) {
    let resp = app.clone().oneshot(sign_up_request(email)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    Outbox::link(
        &outbox
            .wait_for(&email.to_ascii_lowercase(), "/auth/verify")
            .await,
        "/auth/verify",
    )
}

async fn verify(app: &Router, uid: &str, token: &str) {
    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/auth/password/verify",
            serde_json::json!({ "userId": uid, "token": token }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

async fn sign_in(app: &Router, email: &str, password: &str) -> axum::response::Response {
    app.clone()
        .oneshot(post_json(
            "/internal/auth/password/sign-in",
            serde_json::json!({ "email": email, "password": password }),
        ))
        .await
        .unwrap()
}

/// Spend a code at the exchange lane, as the console's callback does.
async fn exchange(app: &Router, code: &str) -> axum::response::Response {
    app.clone()
        .oneshot(post_json(
            "/internal/auth/exchange",
            serde_json::json!({ "code": code }),
        ))
        .await
        .unwrap()
}

/// Sign up, verify, sign in and exchange: the tokens a session starts with.
async fn signed_in(app: &Router, outbox: &Outbox, email: &str) -> serde_json::Value {
    let (uid, token) = sign_up(app, outbox, email).await;
    verify(app, &uid, &token).await;
    let resp = sign_in(app, email, PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let code = json_body(resp).await["code"].as_str().unwrap().to_owned();
    let resp = exchange(app, &code).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let tokens = json_body(resp).await;
    assert_eq!(tokens["userId"], uid);
    tokens
}

async fn me(app: &Router, bearer: &str) -> StatusCode {
    app.clone()
        .oneshot(post_as("/me", bearer, serde_json::json!({})))
        .await
        .unwrap()
        .status()
}

#[sqlx::test]
async fn a_sign_up_is_verified_from_its_mail_and_then_signs_in(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app(pool);

    let (uid, token) = sign_up(&app, &outbox, "Ada@Example.com").await;
    assert!(uid.starts_with("user_"), "{uid}");

    // Before the link is spent the password is right and the sign-in is
    // still refused, and a fresh link goes out.
    let resp = sign_in(&app, "ada@example.com", PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let links = outbox
        .all()
        .iter()
        .filter(|m| m.text.contains("/auth/verify"))
        .count();
    assert_eq!(links, 2, "a refused sign-in re-sends the link");

    // One link is live at a time: the re-sent one retired the first.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/auth/password/verify",
            serde_json::json!({ "userId": uid, "token": token }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a superseded link still works"
    );
    let (resent_uid, token) = Outbox::link(
        &outbox.wait_for("ada@example.com", "/auth/verify").await,
        "/auth/verify",
    );
    assert_eq!(resent_uid, uid);

    verify(&app, &uid, &token).await;
    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/auth/password/verify",
            serde_json::json!({ "userId": uid, "token": token }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "a link works once");

    let resp = sign_in(&app, "ada@example.com", PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let code = json_body(resp).await["code"].as_str().unwrap().to_owned();

    let resp = exchange(&app, &code).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let tokens = json_body(resp).await;
    assert_eq!(tokens["userId"], uid);
    assert_eq!(tokens["email"], "ada@example.com");
    assert_eq!(tokens["emailVerified"], true);
    assert_eq!(tokens["firstName"], "Ada");
    assert_eq!(tokens["authMethod"], "password");
    assert_eq!(tokens["idToken"], serde_json::Value::Null);
    let bearer = tokens["accessToken"].as_str().unwrap();
    assert!(
        bearer.len() == 43 && !bearer.contains('.'),
        "the bearer is an opaque 256-bit secret, nothing to read out of it"
    );
    assert!(tokens["refreshToken"].is_string());

    // The bearer resolves to the session the exchange named, and `/me`
    // records it.
    assert_eq!(me(&app, bearer).await, StatusCode::OK);
    let sid = tokens["sessionId"].as_str().expect("a session id");
    assert!(sid.starts_with("ses_"), "{sid}");
    assert_eq!(
        exchange(&app, &code).await.status(),
        StatusCode::OK,
        "a retry within the cache"
    );
}

/// With `VERIFY_EMAIL` off, an address is taken at its word: the sign-up
/// answers the code that opens the session, and mails nothing.
#[sqlx::test]
async fn with_verification_off_a_sign_up_signs_in_at_once(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app_under(
        pool,
        Policy {
            allow_sign_up: true,
            verify_email: false,
        },
    );

    let resp = app
        .clone()
        .oneshot(sign_up_request("ada@example.com"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let code = json_body(resp).await["code"].as_str().unwrap().to_owned();
    assert!(outbox.all().is_empty(), "{:?}", outbox.all());

    let resp = exchange(&app, &code).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let tokens = json_body(resp).await;
    assert_eq!(tokens["emailVerified"], true);
    assert_eq!(tokens["authMethod"], "password");
    assert_eq!(
        me(&app, tokens["accessToken"].as_str().unwrap()).await,
        StatusCode::OK
    );
    assert_eq!(
        sign_in(&app, "ada@example.com", PASSWORD).await.status(),
        StatusCode::OK,
        "and the password signs in from then on"
    );
}

/// With `ALLOW_SIGN_UP` off, an invitation is the way in: a sign-up for an
/// address nobody invited is refused, and one an organization invited goes
/// through.
#[sqlx::test]
async fn sign_ups_closed_admit_the_invited_alone(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app_under(
        pool.clone(),
        Policy {
            allow_sign_up: false,
            verify_email: true,
        },
    );

    let resp = app
        .clone()
        .oneshot(sign_up_request("stranger@example.com"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(outbox.all().is_empty());

    sqlx::query("INSERT INTO auth.organizations (id, external_id, shard_key) VALUES ($1, $2, $3)")
        .bind(uuid::Uuid::now_v7())
        .bind("org_invites_1")
        .bind(uuid::Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO auth.organization_invites
             (id, organization_id, email, role, token_hash, invited_by, expires_at, shard_key)
         VALUES ($1, $2, $3, 'member', $4, 'user_owner_1', now() + interval '7 days', $5)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind("org_invites_1")
    .bind("grace@example.com")
    .bind(sha256_hex(b"invite_token_1"))
    .bind(uuid::Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();

    let (uid, token) = sign_up(&app, &outbox, "Grace@Example.com").await;
    verify(&app, &uid, &token).await;
    assert_eq!(
        sign_in(&app, "grace@example.com", PASSWORD).await.status(),
        StatusCode::OK
    );
}

/// `ADMIN_EMAIL` and `ADMIN_PASSWORD`: created once, verified, and left
/// alone when the address already has an account.
#[sqlx::test]
async fn the_admin_account_is_seeded_once_and_signs_in(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app_under(
        pool.clone(),
        Policy {
            allow_sign_up: false,
            verify_email: true,
        },
    );
    let sender: Arc<dyn MailSender> = outbox.clone();
    let (admin, _) = password_provider(
        &pool,
        Policy {
            allow_sign_up: false,
            verify_email: true,
        },
        sender,
    );

    assert!(
        admin
            .seed_admin("Admin@Example.com", PASSWORD)
            .await
            .unwrap()
    );
    assert!(
        !admin
            .seed_admin("admin@example.com", "another password")
            .await
            .unwrap(),
        "a second boot leaves the account as it was"
    );
    assert!(
        admin
            .seed_admin("admin@example.com", "short")
            .await
            .is_err(),
        "the password floor holds for the admin too"
    );
    assert!(outbox.all().is_empty(), "{:?}", outbox.all());

    let resp = sign_in(&app, "admin@example.com", PASSWORD).await;
    assert_eq!(resp.status(), StatusCode::OK, "seeded verified");
    assert_eq!(
        sign_in(&app, "admin@example.com", "another password")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test]
async fn a_refresh_rotates_the_token_and_a_reuse_ends_the_session(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let db = service_pool(&pool, "auth");
    let (app, outbox) = app(pool);
    let tokens = signed_in(&app, &outbox, "ada@example.com").await;
    let first = tokens["refreshToken"].as_str().unwrap().to_owned();

    let refresh = |token: String| {
        let app = app.clone();
        async move {
            app.oneshot(post_json(
                "/internal/auth/refresh",
                serde_json::json!({ "refresh_token": token }),
            ))
            .await
            .unwrap()
        }
    };

    let resp = refresh(first.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let rotated = json_body(resp).await;
    let second = rotated["refreshToken"].as_str().unwrap().to_owned();
    assert_ne!(first, second);
    assert_eq!(
        me(&app, rotated["accessToken"].as_str().unwrap()).await,
        StatusCode::OK
    );

    // Presented again at once — a second tab refreshing from the same
    // cookie — the spent token earns another rather than ending the session.
    let resp = refresh(first.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK, "inside the reuse grace");
    let third = json_body(resp).await["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(second, third);

    // Past the grace it is a copy somebody kept: refused, and it takes every
    // token of the session with it. The clock is the spend's argument, so
    // the test need not wait the grace out.
    let later = chrono::Utc::now() + chrono::Duration::minutes(5);
    let mut tx = maintenance_scope(&db, AuthLane).await.unwrap();
    let spent = refresh_tokens::spend(&mut tx, &sha256_hex(first.as_bytes()), later)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        matches!(spent, Some(refresh_tokens::Spent::Reused)),
        "{spent:?}"
    );
    assert_eq!(refresh(second).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(refresh(third).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        me(&app, rotated["accessToken"].as_str().unwrap()).await,
        StatusCode::UNAUTHORIZED,
        "the session's bearers went with it"
    );
}

#[sqlx::test]
async fn a_wrong_password_and_an_unknown_address_answer_alike_and_guesses_lock(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app(pool);
    let (uid, token) = sign_up(&app, &outbox, "ada@example.com").await;
    verify(&app, &uid, &token).await;

    assert_eq!(
        sign_in(&app, "nobody@example.com", PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..telmoni_auth::db::credentials::LOCKOUT_AFTER {
        assert_eq!(
            sign_in(&app, "ada@example.com", "not the password")
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let resp = sign_in(&app, "ada@example.com", PASSWORD).await;
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the right password is refused while the lock holds"
    );
    assert!(resp.headers().get("retry-after").is_some());
}

/// A second sign-up for an address is refused outright: the address is
/// somebody's, and the console says to sign in instead.
#[sqlx::test]
async fn a_second_sign_up_for_an_address_is_refused(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app(pool);
    sign_up(&app, &outbox, "ada@example.com").await;

    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/auth/password/sign-up",
            serde_json::json!({ "email": "ada@example.com", "password": "another password" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(outbox.all().len(), 1, "no second mail");
}

#[sqlx::test]
async fn a_reset_from_the_inbox_sets_the_password_and_ends_every_session(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app(pool);
    let tokens = signed_in(&app, &outbox, "ada@example.com").await;
    let bearer = tokens["accessToken"].as_str().unwrap().to_owned();
    assert_eq!(me(&app, &bearer).await, StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/auth/password/forgot",
            serde_json::json!({ "email": "ada@example.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let (uid, token) = Outbox::link(
        &outbox.wait_for("ada@example.com", "/auth/reset").await,
        "/auth/reset",
    );
    assert_eq!(uid, tokens["userId"]);

    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/auth/password/reset",
            serde_json::json!({ "userId": uid, "token": token, "password": "a brand new password" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        sign_in(&app, "ada@example.com", PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        sign_in(&app, "ada@example.com", "a brand new password")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        me(&app, &bearer).await,
        StatusCode::UNAUTHORIZED,
        "the session from before the reset is over"
    );

    // An address with no account: the same answer, and no mail.
    let before = outbox.all().len();
    let resp = app
        .clone()
        .oneshot(post_json(
            "/internal/auth/password/forgot",
            serde_json::json!({ "email": "nobody@example.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(outbox.all().len(), before);
}

#[sqlx::test]
async fn a_device_is_approved_or_denied_from_the_console(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app(pool);
    let tokens = signed_in(&app, &outbox, "ada@example.com").await;
    let bearer = tokens["accessToken"].as_str().unwrap().to_owned();

    let start = || {
        let app = app.clone();
        async move {
            let resp = app
                .oneshot(post_json(
                    "/internal/auth/device/start",
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            json_body(resp).await
        }
    };
    let poll = |device_code: String| {
        let app = app.clone();
        async move {
            app.oneshot(post_json(
                "/internal/auth/device/poll",
                serde_json::json!({ "deviceCode": device_code }),
            ))
            .await
            .unwrap()
        }
    };

    let started = start().await;
    let device_code = started["deviceCode"].as_str().unwrap().to_owned();
    let user_code = started["userCode"].as_str().unwrap().to_owned();
    assert_eq!(user_code.len(), 9, "{user_code}");
    assert_eq!(started["verificationUri"], format!("{APP_URL}/auth/device"));
    assert_eq!(started["interval"], 5);

    let resp = poll(device_code.clone()).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["status"], "authorization_pending");
    let resp = poll(device_code.clone()).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(
        json_body(resp).await["status"],
        "slow_down",
        "a second poll inside the interval"
    );

    // Approved by the signed-in person, typing the code as people do.
    let resp = app
        .clone()
        .oneshot(post_as(
            "/internal/auth/device/approve",
            &bearer,
            serde_json::json!({ "userCode": user_code.to_lowercase() }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut granted = None;
    for _ in 0..3 {
        let resp = poll(device_code.clone()).await;
        if resp.status() == StatusCode::OK {
            granted = Some(json_body(resp).await);
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let granted = granted.expect("the poll after approval is granted");
    assert_eq!(granted["userId"], tokens["userId"]);
    assert_eq!(
        me(&app, granted["accessToken"].as_str().unwrap()).await,
        StatusCode::OK
    );
    assert_eq!(
        poll(device_code).await.status(),
        StatusCode::UNAUTHORIZED,
        "a granted code is finished"
    );

    let started = start().await;
    let resp = app
        .clone()
        .oneshot(post_as(
            "/internal/auth/device/deny",
            &bearer,
            serde_json::json!({ "userCode": started["userCode"] }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        poll(started["deviceCode"].as_str().unwrap().to_owned())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let resp = app
        .clone()
        .oneshot(post_as(
            "/internal/auth/device/approve",
            &bearer,
            serde_json::json!({ "userCode": "ZZZZ-ZZZZ" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_address_change_takes_a_code_from_each_inbox(pool: PgPool) {
    telmoni_shared::test_util::apply_audit_migrations(&pool).await;
    let (app, outbox) = app(pool);
    let tokens = signed_in(&app, &outbox, "ada@example.com").await;
    let bearer = tokens["accessToken"].as_str().unwrap().to_owned();

    let resp = app
        .clone()
        .oneshot(post_as(
            "/internal/me/email-change",
            &bearer,
            serde_json::json!({ "newEmail": "ada.new@example.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let current = Outbox::code(&outbox.wait_for("ada@example.com", "code is").await);
    let new = Outbox::code(&outbox.wait_for("ada.new@example.com", "code is").await);
    assert_ne!(current, new);

    let resp = app
        .clone()
        .oneshot(post_as(
            "/internal/me/email-change/confirm",
            &bearer,
            serde_json::json!({ "currentCode": current, "newCode": "000000" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "the new inbox's code is checked"
    );

    let resp = app
        .clone()
        .oneshot(post_as(
            "/internal/me/email-change/confirm",
            &bearer,
            serde_json::json!({ "currentCode": current, "newCode": new }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["email"], "ada.new@example.com");

    assert_eq!(
        sign_in(&app, "ada.new@example.com", PASSWORD)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        sign_in(&app, "ada@example.com", PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );

    // An address another account holds is refused before any code goes out.
    sign_up(&app, &outbox, "grace@example.com").await;
    let resp = sign_in(&app, "ada.new@example.com", PASSWORD).await;
    let code = json_body(resp).await["code"].as_str().unwrap().to_owned();
    let resp = exchange(&app, &code).await;
    let bearer = json_body(resp).await["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = app
        .clone()
        .oneshot(post_as(
            "/internal/me/email-change",
            &bearer,
            serde_json::json!({ "newEmail": "grace@example.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

/// What the console's sign-in pages show for a deployment with the form on
/// and no external provider.
#[sqlx::test]
async fn the_config_lane_describes_the_form(pool: PgPool) {
    let (app, _) = app(pool);
    let resp = app
        .oneshot(
            Request::get("/internal/auth/config")
                .header("x-service-secret", SERVICE_SECRET)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({
            "passwordSignIn": true,
            "allowSignUp": true,
            "verifyEmail": true,
            "external": null,
        })
    );
}
