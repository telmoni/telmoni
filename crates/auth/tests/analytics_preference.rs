//! `PUT /internal/me/analytics` — the person's own privacy preference.
#![allow(
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]
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

use telmoni_auth::test_provider::{ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};

const SERVICE_SECRET: &str = "test-service-secret";

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
    let db = service_pool(&pool, ServiceRole::Auth);
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
    let resp = app(pool.clone())
        .oneshot(
            as_person(builder, pool, user)
                .await
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "/me failed for {user}");
    json_body(resp).await
}

/// Record the person, then sign them in, which provisions their organization.
async fn sign_in(pool: &PgPool, user: &str, email: &str) -> Value {
    seed_identity(pool, user, email).await;
    me(pool, user, None).await
}

/// The organization `/me` made the caller active in.
fn active_organization(me: &Value) -> String {
    me["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned()
}

/// The preference as `/me` serves it to the console.
fn opted_in(me: &Value) -> &Value {
    &me["person"]["analyticsOptIn"]
}

async fn set_preference(
    pool: &PgPool,
    caller: &str,
    organization: &str,
    opt_in: bool,
) -> (StatusCode, Value) {
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("PUT")
                    .uri("/internal/me/analytics")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", organization)
                    .header("content-type", "application/json"),
                pool,
                caller,
            )
            .await
            .body(Body::from(json!({ "opt_in": opt_in }).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// Wait until some request in this test's database is queued on an advisory
/// lock; fails when none ever is.
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

/// ⚠ **The lane queues on the person's lock**, which account deletion holds for
/// its whole transaction. It writes the person's row and then an
/// organization's chain; deletion writes the chains of what they own and then
/// the person's row. Interleaved without the lock, the two deadlock.
#[sqlx::test]
async fn the_preference_waits_for_the_persons_lock(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let user = "usr_analytics_locked";
    let organization = active_organization(&sign_in(&pool, user, "locked@example.test").await);

    let mut held = pool.begin().await.unwrap();
    telmoni_auth::db::locks::lock_person(
        &mut held,
        &telmoni_shared::UserId::try_new(user).unwrap(),
    )
    .await
    .unwrap();
    let setting = {
        let pool = pool.clone();
        tokio::spawn(async move { set_preference(&pool, user, &organization, true).await })
    };
    until_a_request_waits_on_a_lock(&pool).await;
    held.rollback().await.unwrap();

    let (status, body) = setting.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// ⚠ **FALSE ON A NEW ROW, AND NOTHING IS SENT UNTIL IT IS TRUE.**
#[sqlx::test]
async fn a_new_person_is_not_counted(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let me = sign_in(&pool, "usr_new_01", "new@example.test").await;
    assert_eq!(opted_in(&me), &json!(false));
}

/// It round-trips through `/me`, the only way the console reads it, and it is
/// stored on the person rather than on any organization.
#[sqlx::test]
async fn the_preference_is_stored_and_comes_back_on_me(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let user = "usr_optin_01";
    let organization = active_organization(&sign_in(&pool, user, "optin@example.test").await);

    let (status, body) = set_preference(&pool, user, &organization, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["analytics_opt_in"], json!(true));

    assert_eq!(opted_in(&me(&pool, user, None).await), &json!(true));
    let stored: bool =
        sqlx::query_scalar("SELECT analytics_opt_in FROM auth.accounts WHERE user_id = $1")
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(stored, "the answer is the person's, on their identity");

    let (status, _) = set_preference(&pool, user, &organization, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(opted_in(&me(&pool, user, None).await), &json!(false));
}

/// ⚠ **THE REFUSAL THIS LANE EXISTS FOR**: nobody answers it for someone
/// else, and naming their organization does not make its chain yours to write.
#[sqlx::test]
async fn one_person_cannot_answer_for_another(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let attacker = "usr_attacker_01";
    let victim = "usr_victim_01";
    sign_in(&pool, attacker, "attacker@example.test").await;
    let victims = active_organization(&sign_in(&pool, victim, "victim@example.test").await);

    let (status, _) = set_preference(&pool, attacker, &victims, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    assert_eq!(
        opted_in(&me(&pool, victim, None).await),
        &json!(false),
        "the refusal still wrote the victim's row"
    );
    assert_eq!(
        opted_in(&me(&pool, attacker, None).await),
        &json!(false),
        "the refusal still wrote the attacker's row"
    );
    let written: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events WHERE organization_id = $1 AND action = 'updated'",
    )
    .bind(&victims)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(written, 0, "the refusal wrote onto the victim's chain");
}

/// ⚠ AND MEMBERSHIP DOES NOT HELP: a real member acting in the owner's
/// organization is let in, and answers for themselves alone. The preference
/// is the person's, so it follows them into every organization they are in.
#[sqlx::test]
async fn a_member_of_that_organization_answers_only_for_themselves(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_02";
    let member = "usr_member_02";
    let owners = active_organization(&sign_in(&pool, owner, "owner2@example.test").await);
    let members_own = active_organization(&sign_in(&pool, member, "member2@example.test").await);

    sqlx::query(
        "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
         VALUES ($1, $2, 'member', $3)",
    )
    .bind(&owners)
    .bind(member)
    .bind(owner)
    .execute(&pool)
    .await
    .expect("seat the member");

    let (status, body) = set_preference(&pool, member, &owners, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        opted_in(&me(&pool, owner, None).await),
        &json!(false),
        "a member answered for the owner"
    );
    assert_eq!(
        opted_in(&me(&pool, member, Some(&owners)).await),
        &json!(true)
    );
    assert_eq!(
        opted_in(&me(&pool, member, Some(&members_own)).await),
        &json!(true),
        "the answer stayed behind in the organization it was given in"
    );

    let subject: String = sqlx::query_scalar(
        "SELECT resource_id FROM audit.events
          WHERE organization_id = $1 AND action = 'updated' AND resource_kind = 'member'",
    )
    .bind(&owners)
    .fetch_one(&pool)
    .await
    .expect("the member's answer is recorded on the chain they named");
    assert_eq!(subject, member);
}

/// A privacy choice may need proving later, so the audit row carries the VALUE,
/// on the chain of the organization the console named, about the person.
#[sqlx::test]
async fn setting_it_writes_an_audit_row_carrying_the_value(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let user = "usr_audit_01";
    let organization = active_organization(&sign_in(&pool, user, "audit@example.test").await);
    let (status, _) = set_preference(&pool, user, &organization, true).await;
    assert_eq!(status, StatusCode::OK);

    let (action, kind, actor, resource, metadata): (String, String, String, Option<String>, Value) =
        sqlx::query_as(
            "SELECT action::text, resource_kind::text, actor_id, resource_id, metadata
               FROM audit.events
              WHERE organization_id = $1 AND resource_kind = 'member' AND action = 'updated'
              ORDER BY created_at DESC
              LIMIT 1",
        )
        .bind(&organization)
        .fetch_one(&pool)
        .await
        .expect("an audit row");

    assert_eq!(action, "updated");
    assert_eq!(kind, "member");
    assert_eq!(actor, user);
    assert_eq!(resource.as_deref(), Some(user));
    assert_eq!(metadata["analytics_opt_in"], json!(true));
}
