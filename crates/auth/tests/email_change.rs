//! `POST /internal/me/email-change` and its `/confirm` — the one lane that
//! moves `auth.identities.email` on a person's say-so. Two codes, one per
//! inbox, and the pair is the whole design.
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

use telmoni_auth::db::confirmation_codes::{self, Act};
use telmoni_auth::provider::ConfirmedEmail;
use telmoni_auth::test_provider::{Call, ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{AuthError, OrganizationId, TelmoniError, UserId};

const SERVICE_SECRET: &str = "test-service-secret";
const USER: &str = "user_change_1";
const EMAIL: &str = "ada@example.com";
const NEW_EMAIL: &str = "ada.new@example.com";
const OTHER: &str = "user_change_2";
const OTHER_EMAIL: &str = "grace@example.com";

/// The provider's code, typed into the NEW-address field.
const NEW_CODE: &str = "222222";

/// Records every mail handed to the transport, and can be told to refuse.
#[derive(Default)]
struct Recorder {
    sent: std::sync::Mutex<Vec<telmoni_shared::mail::Mail>>,
    fail: bool,
}

impl Recorder {
    fn failing() -> Self {
        Self {
            sent: std::sync::Mutex::new(Vec::new()),
            fail: true,
        }
    }
    fn mails(&self) -> Vec<telmoni_shared::mail::Mail> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl telmoni_shared::mail::MailSender for Recorder {
    async fn send(
        &self,
        mail: &telmoni_shared::mail::Mail,
    ) -> Result<(), telmoni_shared::mail::MailError> {
        if self.fail {
            return Err(telmoni_shared::mail::MailError::Rejected {
                status: reqwest::StatusCode::FORBIDDEN,
            });
        }
        self.sent.lock().unwrap().push(mail.clone());
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
    let db = service_pool(&pool, "auth");
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

/// What a code exchange would have recorded about the person. Opening a change
/// needs nothing more: it is the person's, not any organization's.
async fn seed(pool: &PgPool, user: &str, email: &str) {
    apply_audit_migrations(pool).await;
    seed_identity(pool, user, email).await;
}

/// `/me` for `user`, acting in `organization` when one is named.
async fn me(pool: &PgPool, user: &str, organization: Option<&str>) -> Value {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/me")
        .header("x-service-secret", SERVICE_SECRET)
        .header("content-type", "application/json");
    if let Some(organization) = organization {
        builder = builder.header("x-organization-id", organization);
    }
    let (app, _) = app(pool.clone(), Arc::new(Recorder::default()));
    let resp = app
        .oneshot(
            as_person(builder, pool, user)
                .await
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "/me failed for {user}");
    serde_json::from_str(&body_of(resp).await).unwrap()
}

/// Seed the person and sign them in, which provisions the organization the
/// confirm is audited on. Answers its id.
async fn seed_signed_in(pool: &PgPool, user: &str, email: &str) -> String {
    seed(pool, user, email).await;
    me(pool, user, None)
        .await
        .get("activeOrganizationId")
        .and_then(Value::as_str)
        .expect("/me names the active organization")
        .to_owned()
}

fn uid(user: &str) -> UserId {
    UserId::try_new(user).unwrap()
}

fn oid(organization: &str) -> OrganizationId {
    OrganizationId::try_new(organization).unwrap()
}

/// Step one: the person's bearer and the new address in the body — the only
/// address a caller may supply on this lane.
async fn open(pool: &PgPool, actor: &str, new_email: &str) -> Request<Body> {
    open_naming(pool, actor, None, new_email).await
}

/// Step one, with whatever organization the console happens to have active.
async fn open_naming(
    pool: &PgPool,
    actor: &str,
    organization: Option<&str>,
    new_email: &str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/internal/me/email-change")
        .header("x-service-secret", SERVICE_SECRET)
        .header("content-type", "application/json");
    if let Some(organization) = organization {
        builder = builder.header("x-organization-id", organization);
    }
    as_person(builder, pool, actor)
        .await
        .body(Body::from(
            serde_json::json!({ "newEmail": new_email }).to_string(),
        ))
        .unwrap()
}

/// Step two: one code from each inbox, recorded on the chain of the
/// organization the console names.
async fn confirm(
    pool: &PgPool,
    actor: &str,
    organization: &str,
    current_code: &str,
    new_code: &str,
) -> Request<Body> {
    as_person(
        Request::builder()
            .method("POST")
            .uri("/internal/me/email-change/confirm")
            .header("x-service-secret", SERVICE_SECRET)
            .header("x-organization-id", organization)
            .header("content-type", "application/json"),
        pool,
        actor,
    )
    .await
    .body(Body::from(
        serde_json::json!({ "currentCode": current_code, "newCode": new_code }).to_string(),
    ))
    .unwrap()
}

/// The send the lane makes for `user`: the provider is asked about the PERSON,
/// never about an address alone.
fn send_for(user: &str, new_email: &str) -> Call {
    Call::SendEmailChange {
        subject: user.to_owned(),
        new_email: new_email.to_owned(),
    }
}

/// The send the lane makes for the account under test.
fn send(new_email: &str) -> Call {
    send_for(USER, new_email)
}

/// The confirm the lane makes for the account under test, with the code it
/// was typed.
fn confirm_with(code: &str) -> Call {
    Call::ConfirmEmailChange {
        subject: USER.to_owned(),
        code: code.to_owned(),
    }
}

/// The provider's answer to a spent code: the address it now holds. A confirm
/// with no send before it has nothing to default to, so every confirm-only
/// test scripts this.
fn confirmed(email: &str, verified: bool) -> ConfirmedEmail {
    ConfirmedEmail {
        email: email.to_owned(),
        email_verified: verified,
    }
}

/// The provider's "wrong code" refusal.
fn wrong_provider_code() -> Result<ConfirmedEmail, TelmoniError> {
    Err(AuthError::BadRequest("invalid or expired confirmation code".into()).into())
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

/// Read back the address the person's identity holds.
async fn stored_email(pool: &PgPool, user: &str) -> String {
    sqlx::query_scalar::<_, String>("SELECT email FROM auth.identities WHERE user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The organization's own name — the column this lane must never touch.
async fn stored_name(pool: &PgPool, organization: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT name FROM auth.organizations WHERE external_id = $1",
    )
    .bind(organization)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Mint a live code straight into the table, so a confirm test skips step one.
async fn plant_code(pool: &PgPool, user: &str, act: Act<'_>, code: &str) {
    let mut tx = telmoni_shared::db::tenant_session::person_scope(pool, &uid(user))
        .await
        .unwrap();
    confirmation_codes::create(
        &mut tx,
        &uid(user),
        act,
        &confirmation_codes::hash_code(code),
        chrono::Utc::now() + chrono::Duration::minutes(15),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Every code row the person holds, of any purpose.
async fn code_rows(pool: &PgPool, user: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM auth.confirmation_codes WHERE user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The six digits our mail carried.
fn code_in(text: &str) -> String {
    let (_, rest) = text
        .split_once("code is ")
        .expect("the mail states its code");
    rest.chars().take_while(char::is_ascii_digit).collect()
}

/// ⚠ **THE ASSERTION THE WHOLE SECOND FACTOR RESTS ON.** Our code goes to the
/// address the identity holds; sent to the body's address, the asker would be
/// mailing themselves.
#[sqlx::test]
async fn our_code_goes_to_the_stored_address_never_the_one_in_the_body(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let recorder = Arc::new(Recorder::default());
    let (app, _) = app(pool.clone(), recorder.clone());

    let resp = app
        .oneshot(open(&pool, USER, NEW_EMAIL).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let mails = recorder.mails();
    assert_eq!(mails.len(), 1, "{mails:?}");
    assert_eq!(
        mails[0].to, EMAIL,
        "our code went to the address in the body — the old-inbox factor proves \
         nothing if the asker chooses its destination"
    );
    assert!(mails[0].text.contains(NEW_EMAIL), "{}", mails[0].text);
    assert_eq!(stored_email(&pool, USER).await, EMAIL);
}

/// The provider is asked for the normalised address, so the handler cannot
/// check one string and send another.
#[sqlx::test]
async fn the_provider_is_asked_for_the_normalised_address(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));

    let resp = app
        .oneshot(open(&pool, USER, "  Ada.New@Example.COM  ").await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(provider.calls(), vec![send(NEW_EMAIL)]);
}

/// ⚠ **THE REFUSAL THAT PREVENTS A LOCKOUT**: a taken address is refused with
/// zero mails and no code of ours minted. The provider owns which addresses
/// are taken — nothing here is UNIQUE — so it is asked first, and its refusal
/// is the check.
#[sqlx::test]
async fn an_address_the_provider_says_is_taken_is_refused_before_anything_is_mailed(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    seed(&pool, OTHER, OTHER_EMAIL).await;
    let recorder = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), recorder.clone());
    provider.on_send_email_change(Err(AuthError::Conflict(
        "that email address is not available".into(),
    )
    .into()));

    let resp = app
        .oneshot(open(&pool, USER, OTHER_EMAIL).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(
        recorder.mails().is_empty(),
        "a refused change mailed something"
    );
    assert_eq!(
        code_rows(&pool, USER).await,
        0,
        "a refused change minted a code of ours"
    );
    assert_eq!(
        provider.calls(),
        vec![send(OTHER_EMAIL)],
        "the provider's refusal is the only check, and it is asked once"
    );
}

#[sqlx::test]
async fn the_address_it_already_has_is_a_bad_request(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let recorder = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), recorder.clone());

    let resp = app.oneshot(open(&pool, USER, EMAIL).await).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(recorder.mails().is_empty());
    assert!(provider.calls().is_empty());
}

#[sqlx::test]
async fn a_malformed_address_never_reaches_the_provider(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));

    for bad in ["", "nope", "two words@example.com"] {
        let resp = app
            .clone()
            .oneshot(open(&pool, USER, bad).await)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    assert!(provider.calls().is_empty());
}

/// The confused-deputy guard: the service secret authenticates the caller, not
/// the subject, and the bearer is the only subject either step has. Naming the
/// victim's organization, or typing the code mailed to them, changes nothing of
/// theirs.
#[sqlx::test]
async fn one_person_cannot_change_anothers_address(pool: PgPool) {
    let victims = seed_signed_in(&pool, USER, EMAIL).await;
    let attackers = seed_signed_in(&pool, OTHER, OTHER_EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let recorder = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), recorder.clone());

    let resp = app
        .clone()
        .oneshot(confirm(&pool, OTHER, &victims, "111111", NEW_CODE).await)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "the victim's organization is not the caller's to act in"
    );

    let resp = app
        .clone()
        .oneshot(confirm(&pool, OTHER, &attackers, "111111", NEW_CODE).await)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "the victim's code opened the caller's own change"
    );

    let resp = app
        .oneshot(open_naming(&pool, OTHER, Some(&victims), NEW_EMAIL).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let mails = recorder.mails();
    assert_eq!(mails.len(), 1, "{mails:?}");
    assert_eq!(
        mails[0].to, OTHER_EMAIL,
        "the change opened was not the bearer's"
    );

    assert_eq!(
        provider.calls(),
        vec![send_for(OTHER, NEW_EMAIL)],
        "the provider was asked about somebody other than the bearer"
    );
    assert_eq!(stored_email(&pool, USER).await, EMAIL);
    let (live, attempts): (bool, i32) = sqlx::query_as(
        "SELECT consumed_at IS NULL, attempts FROM auth.confirmation_codes
          WHERE user_id = $1 AND purpose = 'email_change'",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(live, "somebody else's guesses burned the victim's code");
    assert_eq!(
        attempts, 0,
        "somebody else's guesses counted against the victim's code"
    );
}

/// The confirm is recorded on the chain the console names, so the right person
/// naming an organization they are not in is refused before anything is
/// checked, spent or counted.
#[sqlx::test]
async fn confirming_under_an_organization_the_person_is_not_in_is_refused(pool: PgPool) {
    seed_signed_in(&pool, USER, EMAIL).await;
    let someone_elses = seed_signed_in(&pool, OTHER, OTHER_EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &someone_elses, "111111", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(provider.calls().is_empty());
    assert_eq!(stored_email(&pool, USER).await, EMAIL);
    let (live, attempts): (bool, i32) = sqlx::query_as(
        "SELECT consumed_at IS NULL, attempts FROM auth.confirmation_codes WHERE user_id = $1",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        live && attempts == 0,
        "the refusal spent or counted the code"
    );
}

/// A person on their way out gets no credential — the password-reset lane's
/// rule. Once they have confirmed deleting their account the person gate
/// refuses their bearer outright.
#[sqlx::test]
async fn a_person_being_deleted_is_refused(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    sqlx::query("INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())")
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    let recorder = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), recorder.clone());

    let resp = app
        .oneshot(open(&pool, USER, NEW_EMAIL).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(recorder.mails().is_empty());
    assert!(provider.calls().is_empty());
}

/// A bearer auth never minted names nobody, so there is no current address
/// to mail our code to.
#[sqlx::test]
async fn a_bearer_auth_never_minted_never_reaches_the_provider(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/me/email-change")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .header("authorization", "Bearer never_minted_here")
                .body(Body::from(
                    serde_json::json!({ "newEmail": NEW_EMAIL }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(provider.calls().is_empty());
}

/// Our code is useless undelivered, so a failed send is reported as a failure.
#[sqlx::test]
async fn a_code_this_service_could_not_mail_is_an_error_not_a_false_sent(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let (app, _) = app(pool.clone(), Arc::new(Recorder::failing()));

    let resp = app
        .oneshot(open(&pool, USER, NEW_EMAIL).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = body_of(resp).await;
    assert!(!body.contains("\"sent\""), "{body}");
    assert!(body.contains("/errors/mail/delivery-failed"), "{body}");
}

/// The relay budget: this lane makes the provider mail an address the caller typed.
#[sqlx::test]
async fn a_caller_cannot_mail_the_world_from_one_account(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    for i in 0..5 {
        let addr = format!("target{i}@example.com");
        let resp = app
            .clone()
            .oneshot(open(&pool, USER, &addr).await)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED, "press {i}");
    }

    let resp = app
        .oneshot(open(&pool, USER, "target9@example.com").await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        provider.count(|c| matches!(c, Call::SendEmailChange { .. })),
        5,
        "the press over budget still made the provider mail somebody"
    );
}

/// Both codes move the address, and `/me` — which reads the identity and
/// nothing else — shows the new one, as the label of the organization they own
/// too.
#[sqlx::test]
async fn both_codes_move_the_address(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_of(resp).await;
    assert!(body.contains(NEW_EMAIL), "{body}");
    assert_eq!(stored_email(&pool, USER).await, NEW_EMAIL);
    assert_eq!(provider.calls(), vec![confirm_with(NEW_CODE)]);

    let me = me(&pool, USER, Some(&organization)).await;
    assert_eq!(
        me["person"]["email"], NEW_EMAIL,
        "the next `/me` still reads the old address"
    );
    assert_eq!(
        me["organizations"][0]["ownerEmail"], NEW_EMAIL,
        "the organization the person owns is still labelled by the old address"
    );
}

/// ⚠ **THE ADDRESS MOVES AND THE NAME DOES NOT**: the change is the person's,
/// and the organization's name is its owner's choice, which it never rewrites.
#[sqlx::test]
async fn the_address_moves_without_disturbing_the_name(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    assert_eq!(
        stored_name(&pool, &organization).await,
        None,
        "a freshly provisioned organization holds a name"
    );
    sqlx::query("UPDATE auth.organizations SET name = $2 WHERE external_id = $1")
        .bind(&organization)
        .bind("Acme Robotics")
        .execute(&pool)
        .await
        .unwrap();
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stored_email(&pool, USER).await, NEW_EMAIL);
    assert_eq!(
        stored_name(&pool, &organization).await.as_deref(),
        Some("Acme Robotics"),
        "the confirmed change rewrote a name the owner chose"
    );
}

/// ⚠ **OUR FACTOR IS CHECKED FIRST**: a session thief without the old inbox
/// cannot reach the provider through this lane at all.
#[sqlx::test]
async fn a_wrong_current_code_never_reaches_the_provider(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "999999", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        provider.calls().is_empty(),
        "a caller without our code reached the provider"
    );
    assert_eq!(stored_email(&pool, USER).await, EMAIL);
}

/// ⚠ **SIX DIGITS IS A MILLION, AND THIS IS WHAT BOUNDS IT**: anything holding
/// the service secret reaches the handler without the console's budget.
#[sqlx::test]
async fn a_code_dies_after_a_handful_of_wrong_guesses(pool: PgPool) {
    use telmoni_auth::db::confirmation_codes::MAX_ATTEMPTS;
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    for attempt in 1..=MAX_ATTEMPTS {
        let resp = app
            .clone()
            .oneshot(confirm(&pool, USER, &organization, "999999", NEW_CODE).await)
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "guess {attempt} was not refused"
        );
    }

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "the correct code still worked after {MAX_ATTEMPTS} wrong ones"
    );
    assert!(
        provider.calls().is_empty(),
        "a burned code still reached the provider"
    );
    assert_eq!(stored_email(&pool, USER).await, EMAIL);

    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.confirmation_codes
          WHERE user_id = $1 AND consumed_at IS NULL",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 0, "a burned code is still live");
    assert_eq!(
        code_rows(&pool, USER).await,
        1,
        "burning erased the record that a code was issued"
    );
}

/// ⚠ **ONE LIVE CODE PER PURPOSE**, or asking again would reset the attempt
/// budget forever.
#[sqlx::test]
async fn asking_again_supersedes_the_previous_code(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    plant_code(&pool, USER, Act::EmailChange, "222333").await;

    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.confirmation_codes
          WHERE user_id = $1 AND consumed_at IS NULL",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 1, "two codes of one purpose are live at once");
    assert_eq!(
        code_rows(&pool, USER).await,
        2,
        "superseding a code erased the issue it records"
    );

    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));
    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "the superseded code still opened the door"
    );
}

/// ⚠ **VERIFY, DON'T CONSUME**: a typo in the provider's field must not burn
/// the old-inbox factor.
#[sqlx::test]
async fn a_wrong_new_code_leaves_our_code_live(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider
        .on_confirm_email_change(wrong_provider_code())
        .on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .clone()
        .oneshot(confirm(&pool, USER, &organization, "111111", "000000").await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(stored_email(&pool, USER).await, EMAIL);

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stored_email(&pool, USER).await, NEW_EMAIL);
    assert_eq!(
        provider.calls(),
        vec![confirm_with("000000"), confirm_with(NEW_CODE)],
        "the provider was handed something other than the code typed"
    );
}

/// The provider's answer is authoritative, never the request's value.
#[sqlx::test]
async fn the_stored_address_comes_from_the_provider_not_the_request(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed("Provider.Says@Example.COM", true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        stored_email(&pool, USER).await,
        "provider.says@example.com",
        "the row followed the request instead of the provider"
    );
}

/// An unverified answer is worth logging, never a reason to leave our record
/// disagreeing with the provider — on the address or on whether it is
/// verified, which the next refresh would write anyway.
#[sqlx::test]
async fn an_unverified_answer_still_converges(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, false)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stored_email(&pool, USER).await, NEW_EMAIL);
    let verified = sqlx::query_scalar::<_, bool>(
        "SELECT email_verified FROM auth.identities WHERE user_id = $1",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !verified,
        "the record claims a verification the provider denied"
    );
}

/// ⚠ Both DELETION codes, mailed to the inbox being left, die with the change:
/// the one that would delete an organization the person owns, and the one that
/// would delete the account.
#[sqlx::test]
async fn every_code_is_cleared_including_a_live_deletion_code(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    plant_code(
        &pool,
        USER,
        Act::OrganizationDeletion(&oid(&organization)),
        "333333",
    )
    .await;
    plant_code(&pool, USER, Act::AccountDeletion, "444444").await;
    assert_eq!(code_rows(&pool, USER).await, 3);

    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        code_rows(&pool, USER).await,
        0,
        "a code mailed to the address they just left is still spendable"
    );
}

#[sqlx::test]
async fn every_session_is_revoked_and_nothing_of_it_reaches_the_provider(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;

    let mut tx = telmoni_shared::db::tenant_session::person_scope(&pool, &uid(USER))
        .await
        .unwrap();
    for sid in [Some("sess_a"), Some("sess_b"), None] {
        telmoni_auth::db::sessions::create(&mut tx, &uid(USER), sid, Some("agent"))
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    // The three above, and the session the sign-in's own `/me` recorded.
    let live_before = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM auth.sessions WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live_before, 4);

    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_of(resp).await;
    assert!(body.contains("\"sessionsRevoked\":4"), "{body}");

    let live = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM auth.sessions WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 0);

    // The sessions are the issuer's, and it ended them: the provider was
    // asked to spend its code and nothing else.
    assert_eq!(
        provider.count(|c| !matches!(c, Call::ConfirmEmailChange { .. })),
        0,
        "{:?}",
        provider.calls()
    );
}

/// Audited once, as an update to the PERSON's membership, on the chain of the
/// organization the console named.
#[sqlx::test]
async fn the_change_is_audited_once_as_an_update_to_the_member(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(&pool, USER, Act::EmailChange, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    type Row = (String, String, String, Option<String>, Value);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT action, resource_kind, actor_id, resource_id, metadata
           FROM audit.events
          WHERE organization_id = $1
            AND metadata->>'kind' IS DISTINCT FROM 'auto_provision'",
    )
    .bind(&organization)
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 1, "{rows:?}");
    let (action, kind, actor, resource, metadata) = &rows[0];
    assert_eq!(action, "updated");
    assert_eq!(kind, "member");
    assert_eq!(actor, USER);
    assert_eq!(resource.as_deref(), Some(USER));
    assert_eq!(metadata["kind"], "email_change");
    assert_eq!(metadata["email"], NEW_EMAIL);
    assert_eq!(
        metadata["sessions_revoked"], 1,
        "the session the sign-in's own `/me` recorded"
    );
}

/// Audited once, on the confirm: nothing has changed when step one returns,
/// so no chain anywhere gains a row.
#[sqlx::test]
async fn asking_for_a_change_is_not_audited(pool: PgPool) {
    seed(&pool, USER, EMAIL).await;
    let (app, _) = app(pool.clone(), Arc::new(Recorder::default()));

    let resp = app
        .oneshot(open(&pool, USER, NEW_EMAIL).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit.events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// Neither code comes back in a response, on the path a person actually takes:
/// ours arrives by mail, and both are typed into the confirm. Nothing is
/// scripted: the provider confirms the address the send opened, as a real one
/// would.
#[sqlx::test]
async fn neither_code_appears_in_any_response_body(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    let recorder = Arc::new(Recorder::default());
    let (app, provider) = app(pool.clone(), recorder.clone());

    let opened = app
        .clone()
        .oneshot(open(&pool, USER, NEW_EMAIL).await)
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::ACCEPTED);
    let opened = body_of(opened).await;
    let ours = code_in(&recorder.mails()[0].text);
    assert_eq!(ours.len(), 6, "{ours}");

    let confirmed = app
        .oneshot(confirm(&pool, USER, &organization, &ours, NEW_CODE).await)
        .await
        .unwrap();
    assert_eq!(
        confirmed.status(),
        StatusCode::OK,
        "the mailed code did not confirm, so its body proves nothing"
    );
    let confirmed = body_of(confirmed).await;

    for body in [&opened, &confirmed] {
        assert!(!body.contains(&ours), "{body}");
        assert!(!body.contains(NEW_CODE), "{body}");
    }
    assert_eq!(
        provider.calls(),
        vec![send(NEW_EMAIL), confirm_with(NEW_CODE)]
    );
    assert_eq!(stored_email(&pool, USER).await, NEW_EMAIL);
}

/// A code minted for one purpose must not be spendable as another — neither
/// deletion code opens an email change.
#[sqlx::test]
async fn a_deletion_code_cannot_authorize_an_email_change(pool: PgPool) {
    let organization = seed_signed_in(&pool, USER, EMAIL).await;
    plant_code(
        &pool,
        USER,
        Act::OrganizationDeletion(&oid(&organization)),
        "111111",
    )
    .await;
    plant_code(&pool, USER, Act::AccountDeletion, "111111").await;
    let (app, provider) = app(pool.clone(), Arc::new(Recorder::default()));
    provider.on_confirm_email_change(Ok(confirmed(NEW_EMAIL, true)));

    let resp = app
        .oneshot(confirm(&pool, USER, &organization, "111111", NEW_CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(provider.calls().is_empty());
    assert_eq!(stored_email(&pool, USER).await, EMAIL);
}
