//! Handing an organization over: the owner offers it to an admin, the admin
//! accepts, and the two swap roles in one transaction. Every guard around that
//! swap — who may offer, what, to whom, what ends an offer, what can never
//! demote or remove the owner — who is mailed each change, and the race it has
//! to win against a removal.
#![allow(
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test scaffolding: asserts and fixture setup"
)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::test_provider::{ScriptedProvider, as_person, bearer};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::mail::{Mail, MailError, MailSender, NoopSender};
use telmoni_shared::seam::Auth as _;
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{OrganizationId, OrganizationRole, UserId};

const SERVICE_SECRET: &str = "test-service-secret";
const OWNER: &str = "user_transfer_owner";
const ADMIN: &str = "user_transfer_admin";
const MEMBER: &str = "user_transfer_member";

/// Every mail auth hands its transport, kept for the tests that read them.
#[derive(Clone, Default)]
struct Outbox(Arc<Mutex<Vec<Mail>>>);

impl Outbox {
    /// What was sent since the last look, and forget it.
    fn take(&self) -> Vec<Mail> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

#[async_trait::async_trait]
impl MailSender for Outbox {
    async fn send(&self, mail: &Mail) -> Result<(), MailError> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

fn app(pool: PgPool) -> Router {
    app_mailing(pool, Arc::new(NoopSender))
}

fn app_mailing(pool: PgPool, sender: Arc<dyn MailSender>) -> Router {
    router(state_mailing(pool, sender))
}

fn state_mailing(pool: PgPool, sender: Arc<dyn MailSender>) -> Arc<AppState> {
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
    Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(Arc::new(
            ScriptedProvider::new(),
        ))),
        db,
        config,
        siblings: telmoni_auth::Siblings::default(),
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(sender)),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
    })
}

/// The organization role a sibling module is answered for `caller` on
/// `organization`, through the seam: the bearer and the context, as the
/// console relayed them.
async fn resolved_role(
    state: &AppState,
    caller: &str,
    organization: &str,
) -> Option<OrganizationRole> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", bearer(&state.db, caller).await)
            .parse()
            .unwrap(),
    );
    headers.insert("x-organization-id", organization.parse().unwrap());
    state
        .resolve(&headers)
        .await
        .expect("a roster row resolves")
        .organization_role
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    if bytes.is_empty() {
        return json!(null);
    }
    serde_json::from_slice(&bytes).expect("JSON body")
}

/// One request as `caller`, acting in `organization` when given.
async fn call(
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    organization: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send(
        app(pool.clone()),
        pool,
        method,
        uri,
        caller,
        organization,
        body,
    )
    .await
}

/// `call`, into an app of the caller's choosing.
async fn send(
    app: Router,
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    organization: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("content-type", "application/json");
    if let Some(organization) = organization {
        req = req.header("x-organization-id", organization);
    }
    let req = as_person(req, pool, caller)
        .await
        .body(Body::from(body.unwrap_or_else(|| json!({})).to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// One request as `caller` on `project`, in `organization`.
async fn call_in_project(
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    organization: &str,
    project: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("x-organization-id", organization)
        .header("x-project-id", project)
        .header("content-type", "application/json");
    let req = as_person(req, pool, caller)
        .await
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// Sign somebody in for the first time; answers the organization they were
/// provisioned with.
async fn sign_in(pool: &PgPool, user: &str) -> String {
    seed_identity(pool, user, &format!("{user}@example.test")).await;
    let (status, body) = call(pool, "POST", "/me", user, None, None).await;
    assert_eq!(status, StatusCode::OK, "sign-in failed for {user}: {body}");
    body["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned()
}

/// Bring `user` into `organization` at `role` the way the product does: the
/// owner invites, they accept.
async fn join(pool: &PgPool, organization: &str, user: &str, role: &str) {
    let (status, body) = call(
        pool,
        "POST",
        "/internal/organization/invites",
        OWNER,
        Some(organization),
        Some(json!({ "email": format!("{user}@example.test"), "role": role })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite {user}: {body}");
    let invite = body["id"].as_str().expect("invite id").to_owned();
    let (status, body) = call(
        pool,
        "POST",
        &format!("/internal/me/invites/{invite}/accept"),
        user,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "accept for {user}: {body}");
}

/// The owner makes a project in `organization`, which sign-in does not;
/// answers its id.
async fn create_project(pool: &PgPool, organization: &str, name: &str) -> String {
    let (status, body) = call(
        pool,
        "POST",
        "/internal/projects",
        OWNER,
        Some(organization),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "creating {name}: {body}");
    body["id"].as_str().expect("the project's id").to_owned()
}

/// The owner renames their organization: the offer's mail calls it by name.
async fn name(pool: &PgPool, organization: &str, name: &str) {
    let (status, body) = call(
        pool,
        "PATCH",
        "/internal/organization",
        OWNER,
        Some(organization),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "naming {organization}: {body}");
}

/// The owner's organization, named Acme, with an admin and a member on it.
async fn organization_with_staff(pool: &PgPool) -> String {
    apply_audit_migrations(pool).await;
    let organization = sign_in(pool, OWNER).await;
    name(pool, &organization, "Acme").await;
    sign_in(pool, ADMIN).await;
    sign_in(pool, MEMBER).await;
    join(pool, &organization, ADMIN, "admin").await;
    join(pool, &organization, MEMBER, "member").await;
    organization
}

/// Who holds which role, straight from the table.
async fn roles(pool: &PgPool, organization: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT user_id, role FROM auth.organization_members
          WHERE organization_id = $1 ORDER BY user_id",
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn role_of(pool: &PgPool, organization: &str, user: &str) -> Option<String> {
    roles(pool, organization)
        .await
        .into_iter()
        .find(|(u, _)| u == user)
        .map(|(_, r)| r)
}

async fn offer(pool: &PgPool, organization: &str, caller: &str, to: &str) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        "/internal/organization/owner-transfer",
        caller,
        Some(organization),
        Some(json!({ "memberId": to })),
    )
    .await
}

async fn accept(pool: &PgPool, organization: &str, caller: &str) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        "/internal/organization/owner-transfer/accept",
        caller,
        Some(organization),
        None,
    )
    .await
}

/// Wait until some request in this test's database is queued on an advisory
/// lock — proof it got past the person gate and into the lane — before the
/// test moves the world under it.
async fn until_a_request_waits_on_a_lock(pool: &PgPool) {
    let mut queued = false;
    for _ in 0..200 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks
              WHERE locktype = 'advisory' AND NOT granted
                AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting > 0 {
            queued = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(queued, "no request queued on a lock within five seconds");
}

/// The ownership rows on the organization's chain, oldest first, by kind.
async fn ownership_audit(pool: &PgPool, organization: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT metadata->>'kind' FROM audit.events
          WHERE organization_id = $1 AND metadata->>'kind' LIKE 'ownership%'
          ORDER BY seq",
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// One ownership request as `caller` in `organization`, with every mail it
/// sends kept in `outbox`.
async fn call_mailing(
    pool: &PgPool,
    outbox: &Outbox,
    method: &str,
    uri: &str,
    caller: &str,
    organization: &str,
    body: Option<Value>,
) -> StatusCode {
    send(
        app_mailing(pool.clone(), Arc::new(outbox.clone())),
        pool,
        method,
        uri,
        caller,
        Some(organization),
        body,
    )
    .await
    .0
}

/// Accept or decline as `caller`, with every mail the answer sends kept in
/// `outbox`.
async fn answer(
    pool: &PgPool,
    outbox: &Outbox,
    organization: &str,
    caller: &str,
    verb: &str,
) -> StatusCode {
    call_mailing(
        pool,
        outbox,
        "POST",
        &format!("/internal/organization/owner-transfer/{verb}"),
        caller,
        organization,
        None,
    )
    .await
}

/// Who each mail went to, and what it was about, in the order sent.
fn addressed(sent: &[Mail]) -> Vec<(String, String)> {
    sent.iter()
        .map(|m| (m.to.clone(), m.subject.clone()))
        .collect()
}

#[sqlx::test]
async fn only_the_owner_offers_and_only_to_an_admin(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;

    let (status, body) = offer(&pool, &organization, ADMIN, MEMBER).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an admin handed it over: {body}"
    );

    let (status, body) = offer(&pool, &organization, OWNER, MEMBER).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a plain member was offered the organization: {body}"
    );

    let (status, body) = offer(&pool, &organization, OWNER, OWNER).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = offer(&pool, &organization, OWNER, "user_nobody_here").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = offer(&pool, &organization, OWNER, ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["offeredTo"], ADMIN);
    // The address the console's live notice goes to comes from the roster,
    // never from the browser that asked.
    assert_eq!(body["offeredToEmail"], format!("{ADMIN}@example.test"));
    assert_eq!(
        role_of(&pool, &organization, OWNER).await.as_deref(),
        Some("owner"),
        "an offer changes nothing until it is accepted"
    );
}

/// ⚠ **A pending account deletion is the person's, not the organization's**:
/// it lives in `auth.accounts`, which has no roster policy, so an offer to an
/// admin who is deleting their account is answered like any other. It can
/// never be taken up
/// (`an_accept_queued_behind_the_acceptors_account_deletion_is_refused`), and
/// their erasure removes the roster row it lives on.
#[sqlx::test]
async fn an_offer_does_not_tell_the_owner_an_admin_is_deleting_their_account(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    sqlx::query("INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())")
        .bind(ADMIN)
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = offer(&pool, &organization, OWNER, ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["offeredTo"], ADMIN);
}

/// A transport that refuses every send, as a provider outage would.
struct Refusing;

#[async_trait::async_trait]
impl MailSender for Refusing {
    async fn send(&self, _mail: &Mail) -> Result<(), MailError> {
        Err(MailError::Rejected {
            status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
        })
    }
}

/// ⚠ **A mail that fails undoes nothing.** Every ownership mail goes after its
/// change commits, so a provider refusing it leaves the offer, the decline,
/// the withdrawal and the transfer standing, each answered and audited.
#[sqlx::test]
async fn a_mail_the_provider_refuses_undoes_nothing(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let transfer = "/internal/organization/owner-transfer";
    let refusing = || app_mailing(pool.clone(), Arc::new(Refusing));

    for (method, uri, caller) in [
        ("POST", format!("{transfer}/decline"), ADMIN),
        ("DELETE", transfer.to_owned(), OWNER),
        ("POST", format!("{transfer}/accept"), ADMIN),
    ] {
        let (status, body) = send(
            refusing(),
            &pool,
            "POST",
            transfer,
            OWNER,
            Some(&organization),
            Some(json!({ "memberId": ADMIN })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "offer: {body}");
        let (status, body) = send(
            refusing(),
            &pool,
            method,
            &uri,
            caller,
            Some(&organization),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{method} {uri}: {body}");
    }

    assert_eq!(
        role_of(&pool, &organization, ADMIN).await.as_deref(),
        Some("owner")
    );
    assert_eq!(
        role_of(&pool, &organization, OWNER).await.as_deref(),
        Some("admin")
    );
    assert_eq!(
        ownership_audit(&pool, &organization).await,
        vec![
            "ownership_offered",
            "ownership_offer_declined",
            "ownership_offered",
            "ownership_offer_cancelled",
            "ownership_offered",
            "ownership_transferred",
        ]
    );
}

/// The owner who made the offer hears its answer with no console open: mailed
/// when the admin declines, and when they accept. A refused answer mails
/// nobody.
#[sqlx::test]
async fn an_answer_is_mailed_to_the_owner_who_made_the_offer(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let outbox = Outbox::default();
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );

    assert_eq!(
        answer(&pool, &outbox, &organization, MEMBER, "accept").await,
        StatusCode::CONFLICT
    );
    assert!(outbox.take().is_empty(), "a refused accept mailed somebody");

    assert_eq!(
        answer(&pool, &outbox, &organization, ADMIN, "decline").await,
        StatusCode::OK
    );
    let sent = addressed(&outbox.take());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, format!("{OWNER}@example.test"));
    assert!(
        sent[0].1.contains("declined") && sent[0].1.contains("Acme"),
        "{sent:?}"
    );

    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    assert_eq!(
        answer(&pool, &outbox, &organization, ADMIN, "accept").await,
        StatusCode::OK
    );
    let sent = addressed(&outbox.take());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0].0,
        format!("{OWNER}@example.test"),
        "the previous owner is not the one told"
    );
    assert!(sent[0].1.contains("is now the owner of Acme"), "{sent:?}");
}

/// An admin mailed an offer is mailed again when it goes: the owner withdrew
/// it, or offered the organization to somebody else. Offering it to the same
/// admin again renews their offer and withdraws nothing.
#[sqlx::test]
async fn a_withdrawn_or_replaced_offer_is_mailed_to_the_admin_who_held_it(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let second = "user_transfer_second_admin";
    sign_in(&pool, second).await;
    join(&pool, &organization, second, "admin").await;
    let outbox = Outbox::default();
    let transfer = "/internal/organization/owner-transfer";
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );

    let to_second = Some(json!({ "memberId": second }));
    assert_eq!(
        call_mailing(
            &pool,
            &outbox,
            "POST",
            transfer,
            OWNER,
            &organization,
            to_second.clone()
        )
        .await,
        StatusCode::OK
    );
    let sent = addressed(&outbox.take());
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(
        sent.iter()
            .any(|(to, subject)| *to == format!("{second}@example.test")
                && subject.contains("wants to make you the owner of Acme")),
        "the new holder was not offered it: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|(to, subject)| *to == format!("{ADMIN}@example.test")
                && subject.contains("withdrew the offer of Acme")),
        "the replaced holder was not told: {sent:?}"
    );

    assert_eq!(
        call_mailing(
            &pool,
            &outbox,
            "POST",
            transfer,
            OWNER,
            &organization,
            to_second
        )
        .await,
        StatusCode::OK
    );
    let sent = addressed(&outbox.take());
    assert_eq!(
        sent.len(),
        1,
        "a renewed offer withdrew something: {sent:?}"
    );
    assert_eq!(sent[0].0, format!("{second}@example.test"));
    assert!(
        sent[0].1.contains("wants to make you the owner"),
        "{sent:?}"
    );

    assert_eq!(
        call_mailing(
            &pool,
            &outbox,
            "DELETE",
            transfer,
            OWNER,
            &organization,
            None
        )
        .await,
        StatusCode::OK
    );
    let sent = addressed(&outbox.take());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, format!("{second}@example.test"));
    assert!(sent[0].1.contains("withdrew the offer of Acme"), "{sent:?}");
}

#[sqlx::test]
async fn accepting_swaps_the_two_roles_and_both_steps_are_audited(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );

    let (status, body) = accept(&pool, &organization, MEMBER).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "somebody who holds no offer accepted it: {body}"
    );

    let (status, body) = accept(&pool, &organization, ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["previousOwner"], OWNER);

    assert_eq!(
        roles(&pool, &organization).await,
        vec![
            (ADMIN.to_owned(), "owner".to_owned()),
            (MEMBER.to_owned(), "member".to_owned()),
            (OWNER.to_owned(), "admin".to_owned()),
        ]
    );
    let offers_left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_members
          WHERE organization_id = $1 AND transfer_offered_at IS NOT NULL",
    )
    .bind(&organization)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(offers_left, 0, "the accepted offer is spent");
    assert_eq!(
        ownership_audit(&pool, &organization).await,
        vec!["ownership_offered", "ownership_transferred"]
    );

    // The roster reads the same thing the table does.
    let (status, body) = call(
        &pool,
        "GET",
        "/internal/organization/members",
        ADMIN,
        Some(&organization),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["members"][0]["member_id"], ADMIN);
    assert_eq!(body["members"][0]["is_owner"], true);
}

#[sqlx::test]
async fn a_declined_or_withdrawn_offer_cannot_be_accepted(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;

    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, body) = call(
        &pool,
        "POST",
        "/internal/organization/owner-transfer/decline",
        ADMIN,
        Some(&organization),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        accept(&pool, &organization, ADMIN).await.0,
        StatusCode::CONFLICT
    );

    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, body) = call(
        &pool,
        "DELETE",
        "/internal/organization/owner-transfer",
        ADMIN,
        Some(&organization),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the recipient withdrew the owner's offer: {body}"
    );
    let (status, body) = call(
        &pool,
        "DELETE",
        "/internal/organization/owner-transfer",
        OWNER,
        Some(&organization),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["offeredTo"], ADMIN);
    assert_eq!(body["offeredToEmail"], format!("{ADMIN}@example.test"));
    assert_eq!(
        accept(&pool, &organization, ADMIN).await.0,
        StatusCode::CONFLICT
    );

    assert_eq!(
        role_of(&pool, &organization, OWNER).await.as_deref(),
        Some("owner")
    );
    assert_eq!(
        ownership_audit(&pool, &organization).await,
        vec![
            "ownership_offered",
            "ownership_offer_declined",
            "ownership_offered",
            "ownership_offer_cancelled",
        ]
    );
}

#[sqlx::test]
async fn demoting_or_removing_the_recipient_ends_the_offer(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;

    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, body) = call(
        &pool,
        "PUT",
        &format!("/internal/organization/members/{ADMIN}/role"),
        OWNER,
        Some(&organization),
        Some(json!({ "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        accept(&pool, &organization, ADMIN).await.0,
        StatusCode::CONFLICT
    );

    let (status, body) = call(
        &pool,
        "PUT",
        &format!("/internal/organization/members/{ADMIN}/role"),
        OWNER,
        Some(&organization),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        accept(&pool, &organization, ADMIN).await.0,
        StatusCode::CONFLICT,
        "promoting them back revived an offer the demotion ended"
    );

    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{ADMIN}"),
        OWNER,
        Some(&organization),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        role_of(&pool, &organization, OWNER).await.as_deref(),
        Some("owner")
    );
    let offers_left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_members
          WHERE organization_id = $1 AND transfer_offered_at IS NOT NULL",
    )
    .bind(&organization)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(offers_left, 0);
}

#[sqlx::test]
async fn an_offer_older_than_seven_days_is_dead(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    sqlx::query(
        "UPDATE auth.organization_members SET transfer_offered_at = now() - interval '8 days'
          WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(&organization)
    .bind(ADMIN)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = accept(&pool, &organization, ADMIN).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        role_of(&pool, &organization, OWNER).await.as_deref(),
        Some("owner"),
        "the refused accept's demotion was rolled back"
    );

    let (_, body) = call(
        &pool,
        "GET",
        "/internal/organization/members",
        OWNER,
        Some(&organization),
        None,
    )
    .await;
    let admin_row = body["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["member_id"] == ADMIN)
        .unwrap()
        .clone();
    assert_eq!(
        admin_row["ownership_offer_expires_at"],
        Value::Null,
        "a lapsed offer is still listed as live"
    );

    // Declined by nobody, as it is withdrawn by nobody: the chain records only
    // the offer the owner made.
    let (status, body) = call(
        &pool,
        "POST",
        "/internal/organization/owner-transfer/decline",
        ADMIN,
        Some(&organization),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        ownership_audit(&pool, &organization).await,
        vec!["ownership_offered"]
    );
}

#[sqlx::test]
async fn a_second_offer_replaces_the_first(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let second = "user_transfer_second_admin";
    sign_in(&pool, second).await;
    join(&pool, &organization, second, "admin").await;

    let (status, body) = offer(&pool, &organization, OWNER, ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["withdrawnFromEmail"],
        Value::Null,
        "nothing was replaced"
    );
    let (status, body) = offer(&pool, &organization, OWNER, second).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Whoever held the replaced offer is named, so their console can be told.
    assert_eq!(body["withdrawnFromEmail"], format!("{ADMIN}@example.test"));
    let replaces: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'replaces' FROM audit.events
          WHERE organization_id = $1 AND metadata->>'kind' = 'ownership_offered'
          ORDER BY seq DESC LIMIT 1",
    )
    .bind(&organization)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(replaces.as_deref(), Some(ADMIN));

    assert_eq!(
        accept(&pool, &organization, ADMIN).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(accept(&pool, &organization, second).await.0, StatusCode::OK);
    assert_eq!(
        role_of(&pool, &organization, second).await.as_deref(),
        Some("owner")
    );
}

/// ⚠ **Nothing but a transfer moves the owner.** Every lane that changes a
/// roster row refuses the owner's, and an invitation accepted by the owner —
/// which once rewrote the role of whoever accepted it — changes nothing.
#[sqlx::test]
async fn the_owner_cannot_leave_be_removed_demoted_or_reseated_by_an_invite(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;

    let (status, body) = call(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{OWNER}"),
        OWNER,
        Some(&organization),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "the owner left: {body}");

    let (status, body) = call(
        &pool,
        "PUT",
        &format!("/internal/organization/members/{OWNER}/role"),
        OWNER,
        Some(&organization),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the owner was demoted: {body}"
    );

    let (status, body) = call(
        &pool,
        "PUT",
        &format!("/internal/organization/members/{ADMIN}/role"),
        OWNER,
        Some(&organization),
        Some(json!({ "role": "owner" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "ownership was granted by a role change: {body}"
    );

    let (status, body) = call(
        &pool,
        "POST",
        "/internal/organization/invites",
        OWNER,
        Some(&organization),
        Some(json!({ "email": format!("{OWNER}@example.test"), "role": "member" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the owner's own address was invited in: {body}"
    );

    // An invitation minted straight into the table, as a stale one sent before
    // somebody became the owner would be: accepting it must not touch them.
    let invite = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO auth.organization_invites
             (id, organization_id, email, role, token_hash, invited_by, expires_at)
         VALUES ($1, $2, $3, 'member', 'stale-hash', $4, now() + interval '1 day')",
    )
    .bind(invite)
    .bind(&organization)
    .bind(format!("{OWNER}@example.test"))
    .bind(OWNER)
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = call(
        &pool,
        "POST",
        &format!("/internal/me/invites/{invite}/accept"),
        OWNER,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    assert_eq!(
        role_of(&pool, &organization, OWNER).await.as_deref(),
        Some("owner"),
        "the owner is no longer the owner"
    );
}

#[sqlx::test]
async fn the_old_owner_stays_as_an_admin_and_may_then_leave(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    assert_eq!(accept(&pool, &organization, ADMIN).await.0, StatusCode::OK);

    let (status, body) = call(
        &pool,
        "DELETE",
        &format!("/internal/organization/members/{OWNER}"),
        OWNER,
        Some(&organization),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(role_of(&pool, &organization, OWNER).await, None);

    // Left with nothing, their next sign-in gives them an organization again.
    let (status, body) = call(&pool, "POST", "/me", OWNER, None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fresh = body["activeOrganizationId"].as_str().unwrap();
    assert_ne!(fresh, organization);
    assert_eq!(body["firstLogin"], true);
    assert_eq!(body["organizations"][0]["role"], "owner");
}

/// A transfer moves one owner row and mints nothing. The admin who accepts
/// keeps the organization they were given at sign-in and now owns both, the
/// older row still their active one; the previous owner, on the roster as an
/// admin, belongs somewhere and so is handed no fresh one. A transfer is one
/// way a person comes to own several organizations; creating one on request
/// is another.
#[sqlx::test]
async fn a_transfer_moves_one_owner_row_and_mints_no_organization(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let (status, body) = call(&pool, "POST", "/me", ADMIN, None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let admins_own = body["activeOrganizationId"].as_str().unwrap().to_owned();
    assert_ne!(admins_own, organization, "{body}");
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM auth.organizations")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let before = count().await;

    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    assert_eq!(accept(&pool, &organization, ADMIN).await.0, StatusCode::OK);

    let held = |body: &Value| -> Vec<(String, String)> {
        let mut held: Vec<_> = body["organizations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| {
                (
                    o["organizationId"].as_str().unwrap().to_owned(),
                    o["role"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        held.sort();
        held
    };

    let (status, body) = call(&pool, "POST", "/me", ADMIN, None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["firstLogin"], false, "{body}");
    let mut both = vec![
        (admins_own.clone(), "owner".to_owned()),
        (organization.clone(), "owner".to_owned()),
    ];
    both.sort();
    assert_eq!(held(&body), both, "{body}");
    assert_eq!(body["activeOrganizationId"], admins_own, "{body}");

    let (status, body) = call(&pool, "POST", "/me", OWNER, None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["firstLogin"], false, "{body}");
    assert_eq!(
        held(&body),
        vec![(organization.clone(), "admin".to_owned())],
        "{body}"
    );
    assert_eq!(body["activeOrganizationId"], organization, "{body}");
    assert_eq!(count().await, before, "a transfer minted an organization");
}

/// What a sibling module enforces on: `require_owner` passes for the
/// person holding the owner row, and stops passing for the one who handed it
/// over.
#[sqlx::test]
async fn resolve_answers_owner_for_whoever_holds_the_row(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let state = state_mailing(pool.clone(), Arc::new(NoopSender));

    assert_eq!(
        resolved_role(&state, OWNER, &organization).await,
        Some(OrganizationRole::Owner)
    );
    assert_eq!(
        resolved_role(&state, ADMIN, &organization).await,
        Some(OrganizationRole::Admin)
    );

    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    assert_eq!(accept(&pool, &organization, ADMIN).await.0, StatusCode::OK);

    assert_eq!(
        resolved_role(&state, ADMIN, &organization).await,
        Some(OrganizationRole::Owner)
    );
    assert_eq!(
        resolved_role(&state, OWNER, &organization).await,
        Some(OrganizationRole::Admin)
    );
}

/// ⚠ **The race the organization lock exists for.** An accept and a removal
/// of the same admin, fired together: whichever wins, the organization ends
/// with exactly one owner, and never with the new owner removed out from
/// under a transfer that already committed.
#[sqlx::test]
async fn accept_racing_a_removal_leaves_exactly_one_owner(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );

    let accepting = {
        let pool = pool.clone();
        let organization = organization.clone();
        tokio::spawn(async move { accept(&pool, &organization, ADMIN).await.0 })
    };
    let removing = {
        let pool = pool.clone();
        let organization = organization.clone();
        tokio::spawn(async move {
            call(
                &pool,
                "DELETE",
                &format!("/internal/organization/members/{ADMIN}"),
                OWNER,
                Some(&organization),
                None,
            )
            .await
            .0
        })
    };
    let (accepted, removed) = (accepting.await.unwrap(), removing.await.unwrap());

    let owners: Vec<String> = roles(&pool, &organization)
        .await
        .into_iter()
        .filter(|(_, role)| role == "owner")
        .map(|(user, _)| user)
        .collect();
    assert_eq!(owners.len(), 1, "owners after the race: {owners:?}");
    match (accepted, removed) {
        (StatusCode::OK, _) => {
            assert_eq!(owners, vec![ADMIN.to_owned()]);
            assert_ne!(
                removed,
                StatusCode::NO_CONTENT,
                "the transfer committed and the removal still went through"
            );
        }
        (_, StatusCode::NO_CONTENT) => {
            assert_eq!(owners, vec![OWNER.to_owned()]);
            assert_eq!(role_of(&pool, &organization, ADMIN).await, None);
        }
        other => panic!("neither the accept nor the removal succeeded: {other:?}"),
    }
}

/// ⚠ **Somebody on their way out cannot take an organization on.** Account
/// deletion reads what the person owns once, under their lock; an accept that
/// landed after that read would leave a shared organization owned by somebody
/// who can never sign in to hand it on, and an erasure that never finishes.
/// The accept queues on the same lock and is refused under it.
#[sqlx::test]
async fn an_accept_queued_behind_the_acceptors_account_deletion_is_refused(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );

    // Stands in for `DELETE /internal/me`, which holds the person's lock for
    // its whole transaction and marks them pending inside it.
    let mut deletion = pool.begin().await.unwrap();
    telmoni_auth::db::locks::lock_person(&mut deletion, &UserId::try_new(ADMIN).unwrap())
        .await
        .unwrap();
    let accepting = {
        let pool = pool.clone();
        let organization = organization.clone();
        tokio::spawn(async move { accept(&pool, &organization, ADMIN).await })
    };
    until_a_request_waits_on_a_lock(&pool).await;
    sqlx::query("INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())")
        .bind(ADMIN)
        .execute(&mut *deletion)
        .await
        .unwrap();
    deletion.commit().await.unwrap();

    let (status, body) = accepting.await.unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        role_of(&pool, &organization, OWNER).await.as_deref(),
        Some("owner")
    );
    assert_eq!(
        role_of(&pool, &organization, ADMIN).await.as_deref(),
        Some("admin")
    );
}

/// ⚠ **A seat the new owner held is folded into the ownership.** Every seat
/// lane refuses to touch the owner, so a seat left behind could be neither
/// changed nor left — and after handing the organization on, it would outrank
/// the admin fallback and cap them at whatever it said.
#[sqlx::test]
async fn seats_the_new_owner_held_are_folded_into_the_ownership(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let project = create_project(&pool, &organization, "Platform").await;
    sqlx::query(
        "INSERT INTO auth.project_members (id, project_id, user_id, role, added_by)
         VALUES (gen_random_uuid(), $1, $2, 'member', $2)",
    )
    .bind(&project)
    .bind(ADMIN)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );

    let (status, body) = accept(&pool, &organization, ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let seats: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth.project_members WHERE user_id = $1")
            .bind(ADMIN)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(seats, 0, "the new owner still holds a seat");
    let folded: Vec<(Option<String>, String)> = sqlx::query_as(
        "SELECT in_project, metadata->>'role' FROM audit.events
          WHERE organization_id = $1 AND metadata->>'kind' = 'seat_folded_into_ownership'",
    )
    .bind(&organization)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(folded, vec![(Some(project), "member".to_owned())]);
}

/// ⚠ **A rotated key is its rotator's.** Only they ever see its secret, and
/// the key it replaces may have been minted by somebody who has since handed
/// the organization on — recording that person would pin the new credential on
/// someone who never held it.
#[sqlx::test]
async fn a_key_rotated_after_a_transfer_is_recorded_as_the_new_owners(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let project = create_project(&pool, &organization, "Platform").await;
    let (status, minted) = call_in_project(
        &pool,
        "POST",
        "/internal/tokens",
        OWNER,
        &organization,
        &project,
        json!({ "name": "ci", "created_by": OWNER }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let old = minted["id"].as_str().unwrap().to_owned();
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
    assert_eq!(accept(&pool, &organization, ADMIN).await.0, StatusCode::OK);

    let (status, rotated) = call_in_project(
        &pool,
        "POST",
        &format!("/internal/tokens/{old}/rotate"),
        ADMIN,
        &organization,
        &project,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rotated}");
    let created_by: String =
        sqlx::query_scalar("SELECT created_by FROM auth.api_tokens WHERE id::text = $1")
            .bind(rotated["id"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(created_by, ADMIN);
}

/// ⚠ **A stranger cannot stand in an organization's queue.** Its id arrives
/// from the caller, and a lock is a queue: membership is checked before the
/// lock is taken, so somebody outside the organization is refused at once
/// while its roster is busy, instead of holding a connection in line.
#[sqlx::test]
async fn a_stranger_is_refused_without_queueing_on_the_organizations_lock(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    let stranger = "user_transfer_stranger";
    sign_in(&pool, stranger).await;

    let mut busy = pool.begin().await.unwrap();
    telmoni_auth::db::locks::lock_organization(
        &mut busy,
        &OrganizationId::try_new(&organization).unwrap(),
    )
    .await
    .unwrap();
    // Every locked lane a caller can name an organization to, member removal
    // included.
    for (method, path) in [
        (
            "POST",
            "/internal/organization/owner-transfer/accept".to_owned(),
        ),
        ("DELETE", format!("/internal/organization/members/{ADMIN}")),
    ] {
        let answered = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            call(&pool, method, &path, stranger, Some(&organization), None),
        )
        .await;
        let (status, body) = answered.expect("the stranger queued on the organization's lock");
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
    busy.rollback().await.unwrap();
}

/// An organization on its way out cannot change hands: nothing acts in one
/// once its deletion is requested, and this lane says so in the same words.
#[sqlx::test]
async fn an_organization_being_deleted_cannot_be_offered_or_accepted(pool: PgPool) {
    let organization = organization_with_staff(&pool).await;
    assert_eq!(
        offer(&pool, &organization, OWNER, ADMIN).await.0,
        StatusCode::OK
    );
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

    for (status, body) in [
        accept(&pool, &organization, ADMIN).await,
        offer(&pool, &organization, OWNER, ADMIN).await,
    ] {
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(
            body["detail"], "this organization is being deleted",
            "{body}"
        );
    }
    assert_eq!(
        role_of(&pool, &organization, OWNER).await.as_deref(),
        Some("owner")
    );
}
