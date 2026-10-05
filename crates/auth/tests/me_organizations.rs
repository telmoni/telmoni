//! `/me` once organizations stopped being people: which organization a person
//! lands in, when one is made for them, and what may be written onto an
//! organization's chain in their name.
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
use telmoni_shared::UserId;
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};

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

/// `POST /me` as `user`, asking to act in `requested` when given.
async fn me(pool: &PgPool, user: &str, requested: Option<&str>) -> (StatusCode, Value) {
    call(pool, "POST", "/me", user, requested, None).await
}

/// Sign somebody in for the first time; answers the id of the organization
/// they were provisioned with.
async fn sign_in(pool: &PgPool, user: &str) -> String {
    seed_identity(pool, user, &format!("{user}@example.test")).await;
    let (status, body) = me(pool, user, None).await;
    assert_eq!(status, StatusCode::OK, "sign-in failed for {user}: {body}");
    body["activeOrganizationId"].as_str().unwrap().to_owned()
}

/// `owner` makes a project in `organization`, which sign-in does not;
/// answers its id.
async fn create_project(pool: &PgPool, owner: &str, organization: &str, name: &str) -> String {
    let (status, body) = call(
        pool,
        "POST",
        "/internal/projects",
        owner,
        Some(organization),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "creating {name}: {body}");
    body["id"].as_str().unwrap().to_owned()
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

async fn owned_by(pool: &PgPool, user: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT organization_id FROM auth.organization_members
          WHERE user_id = $1 AND role = 'owner' ORDER BY organization_id",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// ⚠ **Two first renders at once make one organization.** Nothing unique
/// stands between them any more — an organization's id is minted — so the
/// person's lock and the re-check under it are the whole guarantee.
#[sqlx::test]
async fn concurrent_first_sign_ins_provision_exactly_one_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let user = "user_me_concurrent";
    seed_identity(&pool, user, "concurrent@example.test").await;

    let renders: Vec<_> = (0..6)
        .map(|_| {
            let pool = pool.clone();
            tokio::spawn(async move { me(&pool, user, None).await })
        })
        .collect();
    let mut firsts = 0;
    let mut actives = std::collections::BTreeSet::new();
    for render in renders {
        let (status, body) = render.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["firstLogin"] == true {
            firsts += 1;
        }
        actives.insert(body["activeOrganizationId"].as_str().unwrap().to_owned());
    }

    assert_eq!(
        owned_by(&pool, user).await.len(),
        1,
        "one person, one organization"
    );
    assert_eq!(
        firsts, 1,
        "firstLogin is true on exactly the render that provisioned"
    );
    assert_eq!(
        actives.len(),
        1,
        "every render landed in the same organization"
    );
}

/// The organization a request acts in: the one asked for when the person is
/// in it, otherwise the oldest they own — and a request naming somewhere they
/// are not, or something that is not an organization id at all, is not an
/// error. A stale cookie must never cost somebody the console.
#[sqlx::test]
async fn the_active_organization_is_the_requested_one_only_when_the_person_is_in_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_active").await;
    let other = sign_in(&pool, "user_me_other_owner").await;
    let stranger = sign_in(&pool, "user_me_stranger").await;
    join(&pool, &other, "user_me_other_owner", "user_me_active").await;

    let (status, body) = me(&pool, "user_me_active", Some(&other)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activeOrganizationId"], other.as_str());
    let listed: Vec<(String, String)> = body["organizations"]
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
    assert_eq!(
        listed,
        vec![
            (own.clone(), "owner".to_owned()),
            (other.clone(), "member".to_owned())
        ]
    );
    assert_eq!(
        body["organizations"][1]["ownerEmail"], "user_me_other_owner@example.test",
        "each organization carries its owner, for the console to name whom to ask"
    );

    for requested in [stranger.as_str(), "user_me_active", "not an id at all"] {
        let (status, body) = me(&pool, "user_me_active", Some(requested)).await;
        assert_eq!(status, StatusCode::OK, "{requested}: {body}");
        assert_eq!(
            body["activeOrganizationId"],
            own.as_str(),
            "{requested} did not fall back to the organization they own"
        );
    }
}

/// The console names the organization it is rendering by the slug in its
/// path; the CLI names one by id, and the id wins when both are sent. A slug
/// the person is in nowhere falls back like an id does.
#[sqlx::test]
async fn the_console_names_the_organization_by_its_slug(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_slug").await;
    let other = sign_in(&pool, "user_me_slug_owner").await;
    join(&pool, &other, "user_me_slug_owner", "user_me_slug").await;

    let (_, body) = me(&pool, "user_me_slug", None).await;
    let slug_of = |id: &str| -> String {
        body["organizations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["organizationId"] == id)
            .and_then(|o| o["slug"].as_str())
            .unwrap()
            .to_owned()
    };
    let (own_slug, other_slug) = (slug_of(&own), slug_of(&other));

    let body = me_by_slug(&pool, "user_me_slug", &other_slug, None).await;
    assert_eq!(body["activeOrganizationId"], other.as_str(), "{body}");

    let body = me_by_slug(&pool, "user_me_slug", &other_slug, Some(&own)).await;
    assert_eq!(
        body["activeOrganizationId"],
        own.as_str(),
        "the id lost to the slug"
    );

    for requested in ["org-nobody-holds-this", "Not A Slug", own_slug.as_str()] {
        let body = me_by_slug(&pool, "user_me_slug", requested, None).await;
        assert_eq!(
            body["activeOrganizationId"],
            own.as_str(),
            "{requested}: {body}"
        );
    }
}

/// `POST /me` as `user` with the console's `x-organization-slug`, and the
/// CLI's `x-organization-id` beside it when given.
async fn me_by_slug(pool: &PgPool, user: &str, slug: &str, id: Option<&str>) -> Value {
    let mut req = Request::builder()
        .method("POST")
        .uri("/me")
        .header("x-service-secret", SERVICE_SECRET)
        .header("content-type", "application/json")
        .header("x-organization-slug", slug);
    if let Some(id) = id {
        req = req.header("x-organization-id", id);
    }
    let req = as_person(req, pool, user)
        .await
        .body(Body::from(json!({}).to_string()))
        .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    json_body(resp).await
}

/// Put `organization` into its deletion window, or take it back out, as an
/// owner's deletion and a restore do.
async fn set_pending_deletion(pool: &PgPool, organization: &str, pending: bool) {
    let sql = if pending {
        "UPDATE auth.organizations
            SET status = 'pending_deletion', deletion_requested_at = now(),
                erase_after = now() + interval '14 days', deletion_kind = 'owner'
          WHERE external_id = $1"
    } else {
        "UPDATE auth.organizations
            SET status = 'active', deletion_requested_at = NULL,
                erase_after = NULL, deletion_kind = NULL
          WHERE external_id = $1"
    };
    sqlx::query(sql)
        .bind(organization)
        .execute(pool)
        .await
        .unwrap();
}

/// `PUT /internal/me/default-organization` as `user`.
async fn choose_default(pool: &PgPool, user: &str, organization: &str) -> (StatusCode, Value) {
    call(
        pool,
        "PUT",
        "/internal/me/default-organization",
        user,
        None,
        Some(json!({ "organizationId": organization })),
    )
    .await
}

/// The organization a person chose is the one a sign-in opens: it answers a
/// request that names none — the console's after a sign-in forgets its cookie,
/// the CLI's at login — while a request naming another still acts there.
/// Recorded on the chosen organization's chain.
#[sqlx::test]
async fn a_person_opens_in_the_organization_they_chose(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_chooser").await;
    let other = sign_in(&pool, "user_me_chosen_owner").await;
    join(&pool, &other, "user_me_chosen_owner", "user_me_chooser").await;

    let (_, body) = me(&pool, "user_me_chooser", None).await;
    assert_eq!(body["defaultOrganizationId"], own.as_str(), "{body}");
    assert_eq!(body["activeOrganizationId"], own.as_str(), "{body}");

    let (status, body) = choose_default(&pool, "user_me_chooser", &other).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["defaultOrganizationId"], other.as_str(), "{body}");

    let (_, body) = me(&pool, "user_me_chooser", None).await;
    assert_eq!(body["defaultOrganizationId"], other.as_str(), "{body}");
    assert_eq!(body["activeOrganizationId"], other.as_str(), "{body}");

    let (_, body) = me(&pool, "user_me_chooser", Some(&own)).await;
    assert_eq!(body["activeOrganizationId"], own.as_str(), "{body}");
    assert_eq!(
        body["defaultOrganizationId"],
        other.as_str(),
        "acting somewhere else moved the default: {body}"
    );

    let recorded: Vec<String> = sqlx::query_scalar(
        "SELECT organization_id FROM audit.events
          WHERE resource_kind = 'member' AND action = 'updated'
            AND metadata->>'default_organization' = 'true'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(recorded, vec![other.clone()]);
}

/// ⚠ **The choice is the seat's.** Leaving the chosen organization, or being
/// removed from it, hands the person back to the oldest they own, as Vercel
/// picks a new default team for whoever leaves theirs — and coming back does
/// not bring the old choice with it. One being deleted is passed over, and
/// back once it is restored.
#[sqlx::test]
async fn the_default_goes_with_the_seat_it_was_chosen_on(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_mover").await;
    let other = sign_in(&pool, "user_me_mover_host").await;
    join(&pool, &other, "user_me_mover_host", "user_me_mover").await;
    let (status, body) = choose_default(&pool, "user_me_mover", &other).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    set_pending_deletion(&pool, &other, true).await;
    let (_, body) = me(&pool, "user_me_mover", None).await;
    assert_eq!(body["defaultOrganizationId"], own.as_str(), "{body}");
    set_pending_deletion(&pool, &other, false).await;
    let (_, body) = me(&pool, "user_me_mover", None).await;
    assert_eq!(body["defaultOrganizationId"], other.as_str(), "{body}");

    let (status, body) = call(
        &pool,
        "DELETE",
        "/internal/organization/members/user_me_mover",
        "user_me_mover",
        Some(&other),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (_, body) = me(&pool, "user_me_mover", None).await;
    assert_eq!(body["defaultOrganizationId"], own.as_str(), "{body}");

    join(&pool, &other, "user_me_mover_host", "user_me_mover").await;
    let (_, body) = me(&pool, "user_me_mover", None).await;
    assert_eq!(
        body["defaultOrganizationId"],
        own.as_str(),
        "a new seat revived the choice made on the old one: {body}"
    );

    // ⚠ A removal is the host's act, bound to the host, and the account it
    // clears is the removed person's, which no binding of the host's reaches:
    // the foreign key clears it whoever deleted the seat.
    let (status, body) = choose_default(&pool, "user_me_mover", &other).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &pool,
        "DELETE",
        "/internal/organization/members/user_me_mover",
        "user_me_mover_host",
        Some(&other),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let chosen: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT default_membership_id FROM auth.accounts WHERE user_id = 'user_me_mover'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        chosen, None,
        "the removal left the choice naming a seat that is gone"
    );
    let (_, body) = me(&pool, "user_me_mover", None).await;
    assert_eq!(body["defaultOrganizationId"], own.as_str(), "{body}");
}

/// Only an organization the person holds a seat in, and that is not being
/// deleted, can be chosen; a refusal changes nothing.
#[sqlx::test]
async fn a_default_is_chosen_only_among_the_persons_own_organizations(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_picky").await;
    let stranger = sign_in(&pool, "user_me_picky_stranger").await;

    let (status, body) = choose_default(&pool, "user_me_picky", &stranger).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = choose_default(&pool, "user_me_picky", "not an id at all").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (_, body) = me(&pool, "user_me_picky", None).await;
    assert_eq!(body["defaultOrganizationId"], own.as_str(), "{body}");
    let chosen: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT default_membership_id FROM auth.accounts WHERE user_id = 'user_me_picky'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(chosen, None, "a refusal recorded a choice");
}

/// Somebody whose account deletion is under way chooses nothing. Once they have
/// confirmed it the person gate refuses their bearer outright; the lane's own
/// refusal is for a deletion confirmed while the choice was on its way.
#[sqlx::test]
async fn somebody_on_their_way_out_chooses_no_default(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_me_going").await;
    let other = sign_in(&pool, "user_me_going_host").await;
    join(&pool, &other, "user_me_going_host", "user_me_going").await;
    sqlx::query(
        "INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())
         ON CONFLICT (user_id) DO UPDATE SET deletion_requested_at = now()",
    )
    .bind("user_me_going")
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = choose_default(&pool, "user_me_going", &other).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let chosen: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT default_membership_id FROM auth.accounts WHERE user_id = 'user_me_going'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(chosen, None, "a person on their way out recorded a choice");
}

/// A person who owns nothing active lands in the organization they joined,
/// and is NOT given a new one while they belong somewhere.
#[sqlx::test]
async fn somebody_who_owns_nothing_lands_where_they_belong(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_member_only").await;
    let other = sign_in(&pool, "user_me_their_owner").await;
    join(&pool, &other, "user_me_their_owner", "user_me_member_only").await;
    sqlx::query(
        "UPDATE auth.organizations
            SET status = 'pending_deletion', deletion_requested_at = now(),
                erase_after = now() + interval '14 days', deletion_kind = 'owner'
          WHERE external_id = $1",
    )
    .bind(&own)
    .execute(&pool)
    .await
    .unwrap();

    // A seat in a third organization, which is then deleted by its owner: the
    // seat must go the way of the organization in the answer.
    let third = sign_in(&pool, "user_me_third_owner").await;
    join(&pool, &third, "user_me_third_owner", "user_me_member_only").await;
    create_project(&pool, "user_me_third_owner", &third, "Platform").await;
    sqlx::query(
        "INSERT INTO auth.project_members (project_id, user_id, role, added_by)
         SELECT external_id, 'user_me_member_only', 'member', 'user_me_third_owner'
           FROM auth.projects WHERE organization_id = $1",
    )
    .bind(&third)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE auth.organizations
            SET status = 'pending_deletion', deletion_requested_at = now(),
                erase_after = now() + interval '14 days', deletion_kind = 'owner'
          WHERE external_id = $1",
    )
    .bind(&third)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = me(&pool, "user_me_member_only", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activeOrganizationId"], other.as_str());
    assert_eq!(body["firstLogin"], false);
    assert_eq!(
        body["organizations"].as_array().unwrap().len(),
        1,
        "an organization being deleted is not offered"
    );
    assert!(
        body["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .all(|seat| seat["organizationId"] != third.as_str()),
        "a seat in an organization being deleted is still listed: {body}"
    );
    assert_eq!(
        body["deletedOrganizations"][0]["organizationId"],
        own.as_str(),
        "the organization they own and deleted is offered back: {body}"
    );
    assert_eq!(
        body["deletedOrganizations"][0]["restorable"], true,
        "{body}"
    );

    // What they merely belong to is theirs to leave, not to restore.
    let (_, body) = me(&pool, "user_me_their_owner", None).await;
    assert_eq!(body["deletedOrganizations"], json!([]), "{body}");
}

/// Leaving everything is not a way to be locked out. With their own
/// organization deleted and the one they joined left, the next `/me` makes
/// them a fresh one — and never hands back either of the others.
#[sqlx::test]
async fn somebody_who_has_left_everything_gets_a_fresh_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_leaver").await;
    let other = sign_in(&pool, "user_me_host").await;
    join(&pool, &other, "user_me_host", "user_me_leaver").await;
    sqlx::query("DELETE FROM auth.organizations WHERE external_id = $1")
        .bind(&own)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = call(
        &pool,
        "DELETE",
        "/internal/organization/members/user_me_leaver",
        "user_me_leaver",
        Some(&other),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = me(&pool, "user_me_leaver", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fresh = body["activeOrganizationId"].as_str().unwrap();
    assert!(fresh.starts_with("org_"), "{body}");
    assert_ne!(fresh, own);
    assert_ne!(fresh, other);
    assert_eq!(body["firstLogin"], true);
    let organizations = body["organizations"].as_array().unwrap();
    assert_eq!(organizations.len(), 1, "{body}");
    assert_eq!(organizations[0]["role"], "owner");
    assert_eq!(
        owned_by(&pool, "user_me_leaver").await,
        vec![fresh.to_owned()]
    );
}

/// Being removed is the same as leaving, to `/me`: with their own organization
/// gone and the host's owner having removed them, the next `/me` makes them a
/// fresh one. A replacement, never a second: while they still hold the host's
/// seat, nothing is minted for them.
#[sqlx::test]
async fn somebody_removed_from_their_last_organization_gets_a_fresh_one(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_removed").await;
    let other = sign_in(&pool, "user_me_remover").await;
    join(&pool, &other, "user_me_remover", "user_me_removed").await;
    sqlx::query("DELETE FROM auth.organizations WHERE external_id = $1")
        .bind(&own)
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = me(&pool, "user_me_removed", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["firstLogin"], false, "{body}");
    assert_eq!(body["activeOrganizationId"], other.as_str(), "{body}");
    assert!(
        owned_by(&pool, "user_me_removed").await.is_empty(),
        "somebody still seated somewhere was given an organization"
    );

    let (status, body) = call(
        &pool,
        "DELETE",
        "/internal/organization/members/user_me_removed",
        "user_me_remover",
        Some(&other),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, body) = me(&pool, "user_me_removed", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fresh = body["activeOrganizationId"].as_str().unwrap();
    assert!(fresh.starts_with("org_"), "{body}");
    assert_ne!(fresh, own);
    assert_ne!(fresh, other);
    assert_eq!(body["firstLogin"], true, "{body}");
    let organizations = body["organizations"].as_array().unwrap();
    assert_eq!(organizations.len(), 1, "{body}");
    assert_eq!(organizations[0]["role"], "owner", "{body}");
    assert_eq!(
        owned_by(&pool, "user_me_removed").await,
        vec![fresh.to_owned()]
    );
}

/// Sign-up closed stops a NEW organization, not an existing account.
#[sqlx::test]
async fn closed_sign_up_refuses_a_first_organization_and_nobody_else(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_me_existing").await;
    sqlx::query(
        "INSERT INTO auth.feature_flags (key, enabled, note, actor)
         VALUES ('signup', false, 'closed for the test', 'test')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = me(&pool, "user_me_existing", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an existing account was shut out: {body}"
    );

    seed_identity(&pool, "user_me_newcomer", "newcomer@example.test").await;
    let (status, body) = me(&pool, "user_me_newcomer", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        owned_by(&pool, "user_me_newcomer").await.is_empty(),
        "a closed sign-up provisioned an organization"
    );
}

/// ⚠ **Closed sign-ups make no organization, and turn nobody away.** A person
/// in none signs in to their account alone: no active organization, the
/// global flags saying why, and their invitations — which is how they get in,
/// and the account's deletion, which they are owed whatever gate is up.
#[sqlx::test]
async fn somebody_in_no_organization_signs_in_to_their_account_while_sign_ups_are_closed(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let theirs = sign_in(&pool, "user_me_inviter").await;
    sqlx::query(
        "INSERT INTO auth.feature_flags (key, enabled, note, actor)
         VALUES ('signup', false, 'closed for the test', 'test')",
    )
    .execute(&pool)
    .await
    .unwrap();
    seed_identity(&pool, "user_me_invitee", "user_me_invitee@example.test").await;
    let (status, body) = call(
        &pool,
        "POST",
        "/internal/organization/invites",
        "user_me_inviter",
        Some(&theirs),
        Some(json!({ "email": "user_me_invitee@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let invite = body["id"].as_str().unwrap().to_owned();

    let (status, body) = me(&pool, "user_me_invitee", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activeOrganizationId"], Value::Null, "{body}");
    assert_eq!(body["organizations"], json!([]), "{body}");
    assert_eq!(body["firstLogin"], false);
    assert_eq!(body["flags"]["signup"], false, "{body}");
    assert_eq!(body["incomingInvites"][0]["id"], invite.as_str(), "{body}");
    assert!(owned_by(&pool, "user_me_invitee").await.is_empty());

    // The invitation is how they get in: accepting it needs no organization.
    let (status, body) = call(
        &pool,
        "POST",
        &format!("/internal/me/invites/{invite}/accept"),
        "user_me_invitee",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = me(&pool, "user_me_invitee", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activeOrganizationId"], theirs.as_str(), "{body}");
    assert!(
        owned_by(&pool, "user_me_invitee").await.is_empty(),
        "joining made them an organization of their own"
    );
}

/// ⚠ **Beta access is the person's.** An invitee's own new organization lacks
/// it while the one that let them in has it; walling them in their own would
/// hide the switcher they need to reach the other.
#[sqlx::test]
async fn beta_access_follows_any_organization_the_person_is_in(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_invitee").await;
    let beta = sign_in(&pool, "user_me_beta_owner").await;
    sqlx::query(
        "INSERT INTO auth.feature_flags (key, enabled, note, actor)
         VALUES ('beta_access', false, 'closed beta', 'test')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
         VALUES ($1, 'beta_access', true, 'in the beta', 'test')",
    )
    .bind(&beta)
    .execute(&pool)
    .await
    .unwrap();

    let (_, body) = me(&pool, "user_me_invitee", Some(&own)).await;
    assert_eq!(body["flags"]["beta_access"], false, "{body}");

    join(&pool, &beta, "user_me_beta_owner", "user_me_invitee").await;
    let (_, body) = me(&pool, "user_me_invitee", Some(&own)).await;
    assert_eq!(body["activeOrganizationId"], own.as_str());
    assert_eq!(
        body["flags"]["beta_access"], true,
        "a beta member is walled in their own organization: {body}"
    );
}

/// ⚠ **A person event lands only on a chain they belong to.** Analytics
/// consent, session revocation and email changes are audited on the
/// organization the console names; naming somebody else's must be refused
/// before anything is written.
#[sqlx::test]
async fn a_person_event_is_refused_on_an_organization_the_person_is_not_in(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let own = sign_in(&pool, "user_me_consent").await;
    let foreign = sign_in(&pool, "user_me_foreign_owner").await;
    let chain_len = |organization: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM audit.events WHERE organization_id = $1",
            )
            .bind(organization)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let foreign_before = chain_len(foreign.clone()).await;

    let (status, body) = call(
        &pool,
        "PUT",
        "/internal/me/analytics",
        "user_me_consent",
        Some(&foreign),
        Some(json!({ "opt_in": true })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(chain_len(foreign.clone()).await, foreign_before);

    let own_before = chain_len(own.clone()).await;
    let (status, body) = call(
        &pool,
        "PUT",
        "/internal/me/analytics",
        "user_me_consent",
        Some(&own),
        Some(json!({ "opt_in": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(chain_len(own).await, own_before + 1);

    let (_, body) = me(&pool, "user_me_consent", None).await;
    assert_eq!(body["person"]["analyticsOptIn"], true);
}

/// ⚠ **Somebody in no organization still runs their own account.** With
/// sign-ups closed they sign in to it alone, and revoking a session is how
/// they cut off a stolen one. There is no chain for these acts to land on, so
/// they take no `x-organization-id` and write no audit row anywhere — and a
/// person who IS in an organization cannot drop the header to act unrecorded.
#[sqlx::test]
async fn somebody_in_no_organization_manages_their_account_without_naming_one(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let theirs = sign_in(&pool, "user_me_member").await;
    sqlx::query(
        "INSERT INTO auth.feature_flags (key, enabled, note, actor)
         VALUES ('signup', false, 'closed for the test', 'test')",
    )
    .execute(&pool)
    .await
    .unwrap();
    seed_identity(&pool, "user_me_alone", "user_me_alone@example.test").await;
    let (status, body) = me(&pool, "user_me_alone", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activeOrganizationId"], Value::Null, "{body}");
    // A browser they are signed in on elsewhere, and want to cut off.
    let session: String = sqlx::query_scalar(
        "INSERT INTO auth.sessions (user_id, user_agent) VALUES ('user_me_alone', 'stolen')
         RETURNING id::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let chain_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.events")
        .fetch_one(&pool)
        .await
        .unwrap();

    let (status, body) = call(
        &pool,
        "PUT",
        "/internal/me/analytics",
        "user_me_alone",
        None,
        Some(json!({ "opt_in": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = call(
        &pool,
        "GET",
        "/internal/auth/sessions",
        "user_me_alone",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == session.as_str()),
        "the list is theirs to see without an organization: {body}"
    );
    let (status, body) = call(
        &pool,
        "POST",
        &format!("/internal/auth/sessions/{session}/revoke"),
        "user_me_alone",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let chain_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        chain_after, chain_before,
        "no organization's chain took the acts"
    );
    let revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM auth.sessions WHERE id = $1::uuid")
            .bind(&session)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(revoked);

    // A member of an organization names it, or is refused.
    let (status, body) = call(
        &pool,
        "PUT",
        "/internal/me/analytics",
        "user_me_member",
        None,
        Some(json!({ "opt_in": true })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = call(
        &pool,
        "PUT",
        "/internal/me/analytics",
        "user_me_member",
        Some(&theirs),
        Some(json!({ "opt_in": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
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

/// ⚠ **An accept queues on the person's lock.** The self-service lanes decide
/// under it whether somebody is in any organization — and, if not, act with no
/// chain to record on — so an accept that skipped it could seat them between
/// that check and the act. Under the same lock, an account whose deletion was
/// confirmed while the accept waited takes no seat: its erasure would trip
/// over it.
#[sqlx::test]
async fn an_accept_waits_on_the_persons_lock_and_is_refused_once_their_deletion_lands(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let theirs = sign_in(&pool, "user_me_inviter").await;
    seed_identity(&pool, "user_me_invitee", "user_me_invitee@example.test").await;
    let (status, body) = call(
        &pool,
        "POST",
        "/internal/organization/invites",
        "user_me_inviter",
        Some(&theirs),
        Some(json!({ "email": "user_me_invitee@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let invite = body["id"].as_str().unwrap().to_owned();

    // Stands in for a self-service lane or `DELETE /internal/me`, either of
    // which holds the person's lock for its whole transaction.
    let mut holder = pool.begin().await.unwrap();
    telmoni_auth::db::locks::lock_person(&mut holder, &UserId::try_new("user_me_invitee").unwrap())
        .await
        .unwrap();
    let accepting = {
        let pool = pool.clone();
        tokio::spawn(async move {
            call(
                &pool,
                "POST",
                &format!("/internal/me/invites/{invite}/accept"),
                "user_me_invitee",
                None,
                None,
            )
            .await
        })
    };
    until_a_request_waits_on_a_lock(&pool).await;
    assert!(
        !accepting.is_finished(),
        "the accept did not wait for the person's lock"
    );
    sqlx::query("INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())")
        .bind("user_me_invitee")
        .execute(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();

    let (status, body) = accepting.await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let seated: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM auth.organization_members
                         WHERE organization_id = $1 AND user_id = 'user_me_invitee')",
    )
    .bind(&theirs)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!seated, "a person being deleted took a seat");
}
