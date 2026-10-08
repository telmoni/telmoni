//! `POST /internal/organizations`: a signed-in person founds another
//! organization. Who owns it, the URL it goes by, what is refused, and what
//! its chain records.
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
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{Flag, UserId};

const SERVICE_SECRET: &str = "test-service-secret";

fn app(pool: PgPool) -> Router {
    app_with(pool, false)
}

/// The router, on a deployment with `VERIFY_EMAIL` on or off.
fn app_with(pool: PgPool, verify_email: bool) -> Router {
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
        issuer: telmoni_auth::test_provider::test_issuer_verifying(db.clone(), verify_email),
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

async fn call(
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
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// `POST /me` as `user`, naming no organization.
async fn me(pool: &PgPool, user: &str) -> (StatusCode, Value) {
    call(pool, "POST", "/me", user, None, None).await
}

/// Sign somebody in for the first time; answers the id of the organization
/// they were provisioned with.
async fn sign_in(pool: &PgPool, user: &str) -> String {
    seed_identity(pool, user, &format!("{user}@example.test")).await;
    let (status, body) = me(pool, user).await;
    assert_eq!(status, StatusCode::OK, "sign-in failed for {user}: {body}");
    body["activeOrganizationId"].as_str().unwrap().to_owned()
}

/// `POST /internal/organizations` as `user`. No `x-organization-id`: the lane
/// names no existing organization.
async fn create(pool: &PgPool, user: &str, body: Value) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        "/internal/organizations",
        user,
        None,
        Some(body),
    )
    .await
}

/// `owner` brings `user` into `organization` as a member: invite, accept.
async fn join(pool: &PgPool, organization: &str, owner: &str, user: &str) {
    let (status, body) = call(
        pool,
        "POST",
        "/internal/organization/invites",
        owner,
        Some(organization),
        Some(json!({ "email": format!("{user}@example.test"), "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let invite = body["id"].as_str().unwrap().to_owned();
    let (status, body) = call(
        pool,
        "POST",
        &format!("/internal/me/invites/{invite}/accept"),
        user,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// How many events `organization`'s chain holds.
async fn chain_length(pool: &PgPool, organization: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit.events WHERE organization_id = $1")
        .bind(organization)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The organizations `user` owns, sorted here rather than by the database's
/// collation, so they compare with ids sorted the same way.
async fn owned_by(pool: &PgPool, user: &str) -> Vec<String> {
    let mut owned: Vec<String> = sqlx::query_scalar(
        "SELECT organization_id FROM auth.organization_members
          WHERE user_id = $1 AND role = 'owner'",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .unwrap();
    owned.sort();
    owned
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

async fn organizations_named(pool: &PgPool, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM auth.organizations WHERE name = $1")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// What `make flag` writes: one append-only global row.
async fn flip(pool: &PgPool, flag: Flag, on: bool) {
    sqlx::query(
        "INSERT INTO auth.feature_flags (key, enabled, note, actor) VALUES ($1, $2, 'test', 'test')",
    )
    .bind(flag.as_str())
    .bind(on)
    .execute(pool)
    .await
    .unwrap();
}

/// Wait until some request in this test's database is queued on an advisory
/// lock, before the test moves the world under it.
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

/// Wait until a request in this test is held up by the transaction on the
/// `blocker` backend, before the test lets that transaction finish.
async fn until_blocked_by(pool: &PgPool, blocker: i32) {
    let mut blocked = false;
    for _ in 0..200 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))",
        )
        .bind(blocker)
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting > 0 {
            blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        blocked,
        "no request waited on the transaction within five seconds"
    );
}

/// The creator owns what they made, `/me` lists it beside the rest, and the
/// organization a sign-in opens in stays the one it was: making an
/// organization is not choosing it.
#[sqlx::test]
async fn a_created_organization_is_its_creators_and_moves_no_default(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_new_org").await;

    let (status, body) = create(&pool, "user_new_org", json!({ "name": "Acme Robotics" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let created = body["id"].as_str().unwrap().to_owned();
    assert!(created.starts_with("org_") && created != own, "{body}");
    assert_eq!(body["name"], "Acme Robotics", "{body}");
    assert_eq!(body["slug"], "acme-robotics", "{body}");
    assert_eq!(
        owned_by(&pool, "user_new_org").await,
        sorted(vec![own.clone(), created.clone()])
    );

    let (status, body) = me(&pool, "user_new_org").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = body["organizations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["organizationId"] == created.as_str())
        .unwrap_or_else(|| panic!("the new organization is not listed: {body}"));
    assert_eq!(listed["role"], "owner", "{body}");
    assert_eq!(listed["name"], "Acme Robotics", "{body}");
    assert_eq!(listed["slug"], "acme-robotics", "{body}");
    assert_eq!(
        body["defaultOrganizationId"],
        own.as_str(),
        "creating an organization moved the default: {body}"
    );
    assert_eq!(body["activeOrganizationId"], own.as_str(), "{body}");
    assert_eq!(body["firstLogin"], false, "{body}");
}

/// With no URL asked for — the field absent, or blank, as a client that sends
/// every field leaves it — the name gives one as it gives a first
/// organization's: the name's own slug, then the next number, a console word
/// passed over, and a placeholder for a name with nothing Latin in it. A URL
/// asked for is taken as given.
#[sqlx::test]
async fn the_url_is_derived_from_the_name_unless_one_is_asked_for(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_url_first").await;
    sign_in(&pool, "user_url_second").await;

    let (status, body) = create(&pool, "user_url_first", json!({ "name": "Zoë's Lab" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["slug"], "zoes-lab", "{body}");
    let (status, body) = create(&pool, "user_url_second", json!({ "name": "Zoë's Lab" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["slug"], "zoes-lab-2", "{body}");

    let (status, body) = create(&pool, "user_url_first", json!({ "name": "Settings" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["slug"], "settings-2", "{body}");

    let (status, body) = create(&pool, "user_url_first", json!({ "name": "株式会社" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["name"], "株式会社", "{body}");
    assert!(body["slug"].as_str().unwrap().starts_with("org-"), "{body}");

    for blank in ["", "   "] {
        let (status, body) = create(
            &pool,
            "user_url_first",
            json!({ "name": "Blank Lab", "slug": blank }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{blank:?}: {body}");
        assert!(
            body["slug"].as_str().unwrap().starts_with("blank-lab"),
            "{blank:?}: {body}"
        );
    }

    let (status, body) = create(
        &pool,
        "user_url_second",
        json!({ "name": "Zoë's Lab", "slug": " zoe-research " }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["slug"], "zoe-research", "{body}");
    assert_eq!(body["name"], "Zoë's Lab", "{body}");
}

/// A URL another organization goes by — one awaiting deletion included — is
/// the 409 a URL change on Settings gives, and nothing is made.
#[sqlx::test]
async fn a_url_another_organization_holds_is_a_conflict(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_url_holder").await;
    let late = sign_in(&pool, "user_url_late").await;
    let (status, body) = create(
        &pool,
        "user_url_holder",
        json!({ "name": "Launchpad", "slug": "launchpad" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = create(
        &pool,
        "user_url_late",
        json!({ "name": "Second Launchpad", "slug": "launchpad" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["type"], "/errors/auth/conflict", "{body}");
    assert_eq!(
        body["detail"], "another organization already has that URL",
        "{body}"
    );
    assert_eq!(owned_by(&pool, "user_url_late").await, vec![late.clone()]);
    assert_eq!(organizations_named(&pool, "Second Launchpad").await, 0);

    let (status, body) = create(
        &pool,
        "user_url_holder",
        json!({ "name": "Doomed", "slug": "doomed" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    sqlx::query(
        "UPDATE auth.organizations
            SET status = 'pending_deletion', deletion_requested_at = now(),
                erase_after = now() + interval '15 minutes', deletion_kind = 'owner'
          WHERE external_id = $1",
    )
    .bind(body["id"].as_str().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = create(
        &pool,
        "user_url_late",
        json!({ "name": "Heir", "slug": "doomed" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(owned_by(&pool, "user_url_late").await, vec![late]);
}

/// The name is held to the rename's rules and the URL to a URL change's, each
/// a 400 that makes nothing: an empty or unprintable name, one past 80
/// characters, a URL of the wrong shape or one of the console's own words,
/// and a body naming anything else.
#[sqlx::test]
async fn a_bad_name_or_url_is_refused_and_makes_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_careless").await;
    let organizations: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.organizations")
        .fetch_one(&pool)
        .await
        .unwrap();

    for refused in [
        json!({ "name": "" }),
        json!({ "name": "   " }),
        json!({ "name": "\u{200B}\u{202E}" }),
        json!({ "name": "x".repeat(81) }),
        json!({ "name": "Acme", "slug": "Acme" }),
        json!({ "name": "Acme", "slug": "acme--labs" }),
        json!({ "name": "Acme", "slug": "a".repeat(49) }),
        json!({ "name": "Acme", "slug": "settings" }),
        json!({ "name": "Acme", "slug": "console" }),
        json!({ "slug": "acme" }),
        json!({ "name": "Acme", "owner": "user_somebody_else" }),
    ] {
        let (status, body) = create(&pool, "user_careless", refused.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }
    assert_eq!(owned_by(&pool, "user_careless").await, vec![own]);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.organizations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        after, organizations,
        "a refused creation made an organization"
    );

    // Counted in characters, not bytes: eighty accented letters are a name.
    let (status, body) = create(&pool, "user_careless", json!({ "name": "é".repeat(80) })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// The global `signup` flag closes this door too: the feature-off 503 naming
/// `signup`, with the flag's own sentence, and nothing made. Open again, it
/// works.
#[sqlx::test]
async fn closed_sign_ups_refuse_a_new_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_closed_out").await;
    flip(&pool, Flag::Signup, false).await;

    let (status, body) = create(&pool, "user_closed_out", json!({ "name": "Acme" })).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["type"], "/errors/tenant/feature-off", "{body}");
    assert_eq!(body["flag"], "signup", "{body}");
    assert_eq!(body["detail"], "Sign-ups are closed right now.", "{body}");
    assert_eq!(owned_by(&pool, "user_closed_out").await, vec![own]);

    flip(&pool, Flag::Signup, true).await;
    let (status, body) = create(&pool, "user_closed_out", json!({ "name": "Acme" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// ⚠ **A creation queues on the person's lock.** Their account's deletion
/// holds it while it reads what they own, so a creation that waited on it is
/// refused once the deletion lands, rather than adding an organization the
/// deletion never saw. Once the deletion is confirmed, the person gate turns
/// their bearer away before the lane is reached.
#[sqlx::test]
async fn a_creation_waits_on_the_persons_lock_and_is_refused_once_their_deletion_lands(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_leaving").await;

    // Stands in for `DELETE /internal/me`, which holds the person's lock for
    // its whole transaction.
    let mut holder = pool.begin().await.unwrap();
    telmoni_auth::db::locks::lock_person(&mut holder, &UserId::try_new("user_leaving").unwrap())
        .await
        .unwrap();
    let creating = {
        let pool = pool.clone();
        tokio::spawn(async move {
            create(&pool, "user_leaving", json!({ "name": "Escape Hatch" })).await
        })
    };
    until_a_request_waits_on_a_lock(&pool).await;
    assert!(
        !creating.is_finished(),
        "the creation did not wait for the person's lock"
    );
    sqlx::query(
        "INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())
         ON CONFLICT (user_id) DO UPDATE SET deletion_requested_at = now()",
    )
    .bind("user_leaving")
    .execute(&mut *holder)
    .await
    .unwrap();
    holder.commit().await.unwrap();

    let (status, body) = creating.await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["detail"], "account deletion in progress", "{body}");
    assert_eq!(
        owned_by(&pool, "user_leaving").await,
        vec![own.clone()],
        "a person being deleted founded an organization"
    );
    assert_eq!(organizations_named(&pool, "Escape Hatch").await, 0);

    let (status, body) = create(&pool, "user_leaving", json!({ "name": "Escape Hatch" })).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(owned_by(&pool, "user_leaving").await, vec![own]);
}

/// The new organization's chain opens with its creation and its owner's
/// seat, both in the creator's name and marked as asked for, where a first
/// organization's say `auto_provision`.
#[sqlx::test]
async fn the_new_chain_records_the_creation_on_request(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_audited").await;
    let (status, body) = create(&pool, "user_audited", json!({ "name": "Ledger" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let created = body["id"].as_str().unwrap().to_owned();

    let chain = |organization: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (String, String, Option<String>, String, Option<Value>)>(
                "SELECT action, resource_kind, resource_id, actor_id, metadata
                   FROM audit.events WHERE organization_id = $1 ORDER BY seq",
            )
            .bind(organization)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        chain(created.clone()).await,
        vec![
            (
                "created".to_owned(),
                "organization".to_owned(),
                Some(created.clone()),
                "user_audited".to_owned(),
                Some(json!({ "kind": "on_request" })),
            ),
            (
                "created".to_owned(),
                "member".to_owned(),
                Some("user_audited".to_owned()),
                "user_audited".to_owned(),
                Some(json!({ "kind": "on_request", "role": "owner" })),
            ),
        ]
    );
    let first = chain(own).await;
    assert_eq!(
        first.first().and_then(|event| event.4.clone()),
        Some(json!({ "kind": "auto_provision" })),
        "the provisioned organization's chain changed: {first:?}"
    );
}

/// Nothing caps how many organizations a person makes, more than the
/// console's per-minute throttle lets through included: each one succeeds,
/// and each is theirs.
#[sqlx::test]
async fn there_is_no_cap_on_how_many_a_person_creates(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_prolific").await;

    let mut owned = vec![own];
    for n in 1..=12 {
        let (status, body) = create(
            &pool,
            "user_prolific",
            json!({ "name": format!("Venture {n}") }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{n}: {body}");
        owned.push(body["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(owned_by(&pool, "user_prolific").await, sorted(owned));
}

/// Creations at once are taken one at a time on the person's lock. Each reads
/// the URL it wants before that lock, so one that read it before another took
/// it is given a placeholder at the write: every one is made, the first under
/// the name's own URL, and no two share one.
#[sqlx::test]
async fn creations_at_once_each_get_their_own_url(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_hasty").await;

    let creations: Vec<_> = (0..4)
        .map(|_| {
            let pool = pool.clone();
            tokio::spawn(
                async move { create(&pool, "user_hasty", json!({ "name": "Twin" })).await },
            )
        })
        .collect();
    let mut slugs = Vec::new();
    for creation in creations {
        let (status, body) = creation.await.unwrap();
        assert_eq!(status, StatusCode::CREATED, "{body}");
        slugs.push(body["slug"].as_str().unwrap().to_owned());
    }
    slugs.sort();
    slugs.dedup();
    assert_eq!(slugs.len(), 4, "two creations share a URL: {slugs:?}");
    assert!(slugs.iter().any(|slug| slug == "twin"), "{slugs:?}");
    assert!(
        slugs
            .iter()
            .all(|slug| slug == "twin" || slug.starts_with("twin-") || slug.starts_with("org-")),
        "{slugs:?}"
    );
}

/// ⚠ **Creating an organization moves nobody's default.** Somebody who owns
/// none opens in the oldest organization they belong to, and the one they
/// found would otherwise take that over as the oldest they own: their current
/// default is kept as chosen. A choice they made is left as it is.
#[sqlx::test]
async fn somebody_who_owns_none_keeps_their_default(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let host = sign_in(&pool, "user_default_host").await;
    flip(&pool, Flag::Signup, false).await;
    seed_identity(
        &pool,
        "user_default_guest",
        "user_default_guest@example.test",
    )
    .await;
    let (status, body) = me(&pool, "user_default_guest").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activeOrganizationId"], Value::Null, "{body}");
    join(&pool, &host, "user_default_host", "user_default_guest").await;
    flip(&pool, Flag::Signup, true).await;
    let (_, body) = me(&pool, "user_default_guest").await;
    assert_eq!(body["defaultOrganizationId"], host.as_str(), "{body}");

    let (status, body) = create(&pool, "user_default_guest", json!({ "name": "Side Co" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let side = body["id"].as_str().unwrap().to_owned();
    let (_, body) = me(&pool, "user_default_guest").await;
    assert_eq!(
        body["defaultOrganizationId"],
        host.as_str(),
        "the organization they founded took their default over: {body}"
    );
    assert!(
        body["organizations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["organizationId"] == side.as_str() && o["role"] == "owner"),
        "{body}"
    );

    let (status, body) = call(
        &pool,
        "PUT",
        "/internal/me/default-organization",
        "user_default_guest",
        None,
        Some(json!({ "organizationId": side })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = create(&pool, "user_default_guest", json!({ "name": "Third Co" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, body) = me(&pool, "user_default_guest").await;
    assert_eq!(
        body["defaultOrganizationId"],
        side.as_str(),
        "a creation overwrote a choice: {body}"
    );
}

/// ⚠ **A seat removed while the default is kept leaves the creation
/// standing.** The default of somebody who owns none is kept by recording
/// their seat; a removal that commits while that write waits on the seat's
/// row fails the write, and the organization is founded all the same — the
/// one they now own being their default, as the removal would have made it.
#[sqlx::test]
async fn a_seat_removed_while_the_default_is_kept_leaves_the_creation_standing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let host = sign_in(&pool, "user_racing_host").await;
    flip(&pool, Flag::Signup, false).await;
    seed_identity(&pool, "user_racing_guest", "user_racing_guest@example.test").await;
    let (status, body) = me(&pool, "user_racing_guest").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    join(&pool, &host, "user_racing_host", "user_racing_guest").await;
    flip(&pool, Flag::Signup, true).await;

    // Stands in for the host removing them: the seat is deleted and the
    // removal not yet committed, so the write that records it waits on it.
    let mut removal = pool.begin().await.unwrap();
    let removal_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *removal)
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM auth.organization_members WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(&host)
    .bind("user_racing_guest")
    .execute(&mut *removal)
    .await
    .unwrap();
    let creating = {
        let pool = pool.clone();
        tokio::spawn(async move {
            create(&pool, "user_racing_guest", json!({ "name": "Lifeboat" })).await
        })
    };
    until_blocked_by(&pool, removal_pid).await;
    removal.commit().await.unwrap();

    let (status, body) = creating.await.unwrap();
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let created = body["id"].as_str().unwrap().to_owned();
    assert_eq!(
        owned_by(&pool, "user_racing_guest").await,
        vec![created.clone()]
    );
    let (_, body) = me(&pool, "user_racing_guest").await;
    assert_eq!(body["defaultOrganizationId"], created.as_str(), "{body}");
}

/// The lane names no existing organization, so an `x-organization-id` riding
/// along — one the caller is not even in — is read by nothing: what is made is
/// theirs, and the named organization's chain is untouched.
#[sqlx::test]
async fn an_organization_header_is_read_by_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_headed").await;
    let foreign = sign_in(&pool, "user_headed_stranger").await;
    let foreign_chain = chain_length(&pool, &foreign).await;

    let (status, body) = call(
        &pool,
        "POST",
        "/internal/organizations",
        "user_headed",
        Some(&foreign),
        Some(json!({ "name": "Own Way" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let created = body["id"].as_str().unwrap().to_owned();
    assert_ne!(created, foreign);
    assert!(
        owned_by(&pool, "user_headed").await.contains(&created),
        "{body}"
    );
    assert_eq!(chain_length(&pool, &foreign).await, foreign_chain);
}

/// ⚠ **Nobody unproved comes to own anything.** While `VERIFY_EMAIL` waits on
/// somebody's address `/me` refuses them, and so does this lane: a bearer from
/// before the switch was turned on still reaches it. Addresses taken at their
/// word, the same person founds one.
#[sqlx::test]
async fn an_unproved_address_founds_nothing_while_addresses_must_be_proved(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_identity(&pool, "user_unproved", "user_unproved@example.test").await;
    sqlx::query("UPDATE auth.identities SET email_verified = false WHERE user_id = $1")
        .bind("user_unproved")
        .execute(&pool)
        .await
        .unwrap();

    let req = as_person(
        Request::builder()
            .method("POST")
            .uri("/internal/organizations")
            .header("x-service-secret", SERVICE_SECRET)
            .header("content-type", "application/json"),
        &pool,
        "user_unproved",
    )
    .await
    .body(Body::from(json!({ "name": "Acme" }).to_string()))
    .unwrap();
    let resp = app_with(pool.clone(), true).oneshot(req).await.unwrap();
    let status = resp.status();
    let body = json_body(resp).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        body["detail"], "verify your email address to continue",
        "{body}"
    );
    assert!(owned_by(&pool, "user_unproved").await.is_empty());

    let (status, body) = create(&pool, "user_unproved", json!({ "name": "Acme" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}
