//! Members: who else may see an organization's runs, and at what role.
#![expect(
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
use telmoni_auth::{AppState, Config, db::members, router};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{ProjectId, Role, UserId};

const SERVICE_SECRET: &str = "test-service-secret";

/// Records every mail handed to the transport.
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

fn app(pool: PgPool) -> Router {
    app_with_mail(pool, Arc::new(telmoni_shared::mail::NoopSender))
}

fn app_recording_mail(pool: PgPool) -> (Router, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    (app_with_mail(pool, recorder.clone()), recorder)
}

fn app_with_mail(pool: PgPool, sender: Arc<dyn telmoni_shared::mail::MailSender>) -> Router {
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
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(sender)),
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

/// The organization a person owns: the one their first sign-in provisioned,
/// which `/me` makes active until they switch.
async fn organization_of(pool: &PgPool, user: &str) -> String {
    sqlx::query_scalar(
        "SELECT organization_id FROM auth.organization_members
          WHERE user_id = $1 AND role = 'owner'",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .expect("sign-in provisions an organization its person owns")
}

/// The project a person's own organization was provisioned with.
async fn project_of(pool: &PgPool, user: &str) -> String {
    sqlx::query_scalar("SELECT external_id FROM auth.projects WHERE organization_id = $1")
        .bind(organization_of(pool, user).await)
        .fetch_one(pool)
        .await
        .expect("sign-in provisions exactly one project")
}

/// Sign somebody in, provisioning their organization. Returns the `/me` body.
async fn sign_in(pool: &PgPool, user: &str) -> Value {
    seed_identity(pool, user, &format!("{user}@example.test")).await;
    let body = json!({});
    let resp = app(pool.clone())
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
            .body(Body::from(body.to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "sign-in failed for {user}");
    json_body(resp).await
}

/// One request on the internal lane, as the BFF sends it: in the caller's own
/// organization, the one they are active in until they switch.
async fn call(
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call_in(pool, method, uri, caller, None, body).await
}

/// The same, naming the PROJECT being acted on.
async fn call_in(
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    project: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("x-organization-id", organization_of(pool, caller).await);
    if let Some(project) = project {
        req = req.header("x-project-id", project);
    }
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = as_person(req, pool, caller)
        .await
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// The address a signed-in test user holds, which invitations are sent to.
fn address_of(user: &str) -> String {
    format!("{user}@example.test")
}

/// The secret out of an invitation link — the ONE place it is ever returned.
fn secret_of(link: &str) -> String {
    link.rsplit('/')
        .next()
        .expect("a link with a segment")
        .into()
}

/// Offer `email` a role on `owner`'s project. Returns the create body.
async fn invite(pool: &PgPool, owner: &str, email: &str, role: &str) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        &format!(
            "/internal/projects/{}/invites",
            project_of(pool, owner).await
        ),
        owner,
        Some(json!({ "email": email, "role": role })),
    )
    .await
}

/// Spend a link as `caller`.
async fn accept(pool: &PgPool, caller: &str, secret: &str) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        "/internal/invites/accept",
        caller,
        Some(json!({ "token": secret })),
    )
    .await
}

/// Two people with an organization each, the second seated at `role` on the
/// first's project by an accepted invite.
async fn owner_and_member(pool: &PgPool, role: &str) -> (String, String) {
    apply_audit_migrations(pool).await;
    let owner = "user_owner";
    let member = "user_member";
    sign_in(pool, owner).await;
    sign_in(pool, member).await;

    let (status, offer) = invite(pool, owner, &address_of(member), role).await;
    assert_eq!(status, StatusCode::CREATED, "invite failed: {offer}");
    let secret = secret_of(offer["link"].as_str().expect("the link"));

    let (status, body) = accept(pool, member, &secret).await;
    assert_eq!(status, StatusCode::CREATED, "accept failed: {body}");
    assert_eq!(body["projectId"], project_of(pool, owner).await);
    assert_eq!(body["inviteId"], offer["id"]);
    assert_eq!(body["inviterEmail"], address_of(owner));
    assert_eq!(
        body["ownerOrganizationId"],
        organization_of(pool, owner).await
    );
    (owner.to_string(), member.to_string())
}

/// The whole loop: invite by address, accept, see it on both sides, remove it.
#[sqlx::test]
async fn an_invitation_accepted_seats_a_member_and_both_sides_see_it(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "admin").await;

    let (status, body) = call(
        &pool,
        "GET",
        &format!(
            "/internal/projects/{}/members",
            project_of(&pool, &owner).await
        ),
        &owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = body["members"].as_array().expect("members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["member_id"], json!(owner));
    assert_eq!(members[0]["email"], json!(address_of(&owner)));
    assert_eq!(members[0]["role"], json!("owner"));
    assert_eq!(members[0]["is_owner"], json!(true));
    assert_eq!(members[1]["member_id"], json!(member));
    assert_eq!(members[1]["email"], json!(address_of(&member)));
    assert_eq!(members[1]["role"], json!("admin"));
    assert_eq!(members[1]["is_owner"], json!(false));

    let (status, body) = call(
        &pool,
        "GET",
        &format!(
            "/internal/projects/{}/invites",
            project_of(&pool, &owner).await
        ),
        &owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["invites"].as_array().expect("invites").is_empty());

    let me = sign_in(&pool, &member).await;
    let memberships = me["memberships"].as_array().expect("memberships");
    assert_eq!(memberships.len(), 1);
    assert_eq!(
        memberships[0]["projectId"],
        json!(project_of(&pool, &owner).await)
    );
    assert_eq!(
        memberships[0]["organizationId"],
        json!(organization_of(&pool, &owner).await),
        "the seat names the organization that holds the project"
    );
    assert_eq!(memberships[0]["role"], json!("admin"));

    let owner_me = sign_in(&pool, &owner).await;
    assert!(
        owner_me["memberships"]
            .as_array()
            .expect("array")
            .is_empty()
    );

    let (status, _) = call(
        &pool,
        "DELETE",
        &format!(
            "/internal/projects/{}/members/{member}",
            project_of(&pool, &owner).await
        ),
        &owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let me = sign_in(&pool, &member).await;
    assert!(me["memberships"].as_array().expect("array").is_empty());
}

/// **The property the whole design exists for.**
#[sqlx::test]
async fn an_address_with_an_organization_is_answered_exactly_like_one_without(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner";
    sign_in(&pool, owner).await;
    sign_in(&pool, "user_known").await;

    let (registered, a) = invite(&pool, owner, &address_of("user_known"), "member").await;
    let (stranger, b) = invite(&pool, owner, "nobody-at-all@example.test", "member").await;

    assert_eq!(registered, StatusCode::CREATED);
    assert_eq!(stranger, registered, "the statuses differ");

    let keys = |v: &Value| {
        let mut k: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(keys(&a), keys(&b), "the bodies have different shapes");

    let (status, body) = call(
        &pool,
        "GET",
        &format!(
            "/internal/projects/{}/invites",
            project_of(&pool, owner).await
        ),
        owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["invites"].as_array().expect("invites").len(), 2);
}

/// The link is the credential, and it is returned exactly once.
#[sqlx::test]
async fn the_link_is_returned_at_create_and_never_again(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner";
    sign_in(&pool, owner).await;

    let (status, created) = invite(&pool, owner, "someone@example.test", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        created["link"]
            .as_str()
            .expect("a link")
            .contains("/invite/"),
        "{created}"
    );

    let (_, listed) = call(
        &pool,
        "GET",
        &format!(
            "/internal/projects/{}/invites",
            project_of(&pool, owner).await
        ),
        owner,
        None,
    )
    .await;
    let row = &listed["invites"][0];
    let printed = row.to_string();
    assert!(
        row["link"].is_null(),
        "the list returns the link: {printed}"
    );
    assert!(
        row["token"].is_null(),
        "the list returns a token: {printed}"
    );
    assert!(
        !printed.contains("token_hash") && !printed.contains("tokenHash"),
        "the list returns the hash: {printed}"
    );
}

/// **A forwarded link seats nobody**: holding the ADDRESS is the proof, not
/// holding the link.
#[sqlx::test]
async fn a_link_only_seats_the_address_it_was_sent_to(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner";
    sign_in(&pool, owner).await;
    sign_in(&pool, "user_invited").await;
    sign_in(&pool, "user_forwarded").await;

    let (_, body) = invite(&pool, owner, &address_of("user_invited"), "admin").await;
    let secret = secret_of(body["link"].as_str().expect("the link"));

    let (status, body) = accept(&pool, "user_forwarded", &secret).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains(&address_of("user_invited")),
        "the refusal must name the address to sign in with, got: {detail}"
    );

    let (status, _) = accept(&pool, "user_invited", &secret).await;
    assert_eq!(status, StatusCode::CREATED);
}

/// Spent, revoked and fictional invitations all answer the same 404.
#[sqlx::test]
async fn a_spent_a_revoked_and_a_fictional_link_answer_identically(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner";
    sign_in(&pool, owner).await;
    sign_in(&pool, "user_member").await;
    sign_in(&pool, "user_other").await;

    let (_, body) = invite(&pool, owner, &address_of("user_member"), "member").await;
    let spent = secret_of(body["link"].as_str().unwrap());
    accept(&pool, "user_member", &spent).await;

    let (_, body) = invite(&pool, owner, &address_of("user_other"), "member").await;
    let revoked = secret_of(body["link"].as_str().unwrap());
    let id = body["id"].as_str().expect("the invite id");
    let (status, withdrawn) = call(
        &pool,
        "DELETE",
        &format!(
            "/internal/projects/{}/invites/{id}",
            project_of(&pool, owner).await
        ),
        owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(withdrawn["email"], address_of("user_other"));

    let fictional = uuid::Uuid::new_v4().to_string();

    let mut seen = Vec::new();
    for secret in [&spent, &revoked, &fictional] {
        let (status, body) = call(
            &pool,
            "POST",
            "/internal/invites/look",
            "user_other",
            Some(json!({ "token": secret })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{secret} answered {status}");
        seen.push(body["detail"].as_str().unwrap_or_default().to_owned());
    }
    assert_eq!(seen[0], seen[1], "a spent link reads unlike a revoked one");
    assert_eq!(
        seen[1], seen[2],
        "a revoked link reads unlike a fictional one"
    );
}

/// Re-inviting an address replaces the offer: never two live links for one seat.
#[sqlx::test]
async fn re_inviting_an_address_kills_the_link_it_replaces(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner";
    sign_in(&pool, owner).await;
    sign_in(&pool, "user_member").await;

    let (_, first) = invite(&pool, owner, &address_of("user_member"), "member").await;
    let (_, second) = invite(&pool, owner, &address_of("user_member"), "admin").await;
    let old = secret_of(first["link"].as_str().unwrap());
    let new = secret_of(second["link"].as_str().unwrap());
    assert_ne!(old, new);

    let (status, _) = accept(&pool, "user_member", &old).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the replaced link still works"
    );

    let (status, _) = accept(&pool, "user_member", &new).await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, body) = call(
        &pool,
        "GET",
        &format!(
            "/internal/projects/{}/members",
            project_of(&pool, owner).await
        ),
        owner,
        None,
    )
    .await;
    let members = body["members"].as_array().expect("members");
    assert_eq!(members.len(), 2);
    let admin = members
        .iter()
        .find(|m| m["member_id"] == "user_member")
        .expect("member");
    assert_eq!(admin["role"], json!("admin"), "the newer offer");
}

/// `owner` cannot be granted: a project's owner is its organization's owner.
#[sqlx::test]
async fn nobody_can_be_invited_as_an_owner(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;

    let (status, _) = invite(&pool, "user_owner", "other@example.test", "owner").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Inviting somebody already seated is a 409, and inviting yourself is a 400.
#[sqlx::test]
async fn inviting_an_existing_member_or_yourself_is_refused(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "member").await;

    let (status, _) = invite(&pool, &owner, &address_of(&member), "admin").await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = invite(&pool, &owner, &address_of(&owner), "admin").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A string that cannot be an address is a 400, never an unacceptable row.
#[sqlx::test]
async fn a_malformed_address_is_refused_before_anything_is_written(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;

    for bad in ["dana", "dana@", "@example.test", "da na@example.test"] {
        let (status, _) = invite(&pool, "user_owner", bad, "member").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "`{bad}` was accepted");
    }
    let (_, body) = call(
        &pool,
        "GET",
        &format!(
            "/internal/projects/{}/invites",
            project_of(&pool, "user_owner").await
        ),
        "user_owner",
        None,
    )
    .await;
    assert!(body["invites"].as_array().expect("invites").is_empty());
}

/// The role changes, and the member sees the new one on their next sign-in.
#[sqlx::test]
async fn an_owner_changes_a_role(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "member").await;

    let (status, _) = call(
        &pool,
        "PUT",
        &format!(
            "/internal/projects/{}/members/{member}/role",
            project_of(&pool, &owner).await
        ),
        &owner,
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let me = sign_in(&pool, &member).await;
    assert_eq!(me["memberships"][0]["role"], json!("admin"));
}

/// A member leaves by their own hand, audited on the OWNER's chain.
#[sqlx::test]
async fn a_member_leaves_and_the_owners_chain_records_it(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "admin").await;

    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/memberships/{}", project_of(&pool, &owner).await),
        &member,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let me = sign_in(&pool, &member).await;
    assert!(me["memberships"].as_array().expect("array").is_empty());

    let on_owner: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'member' AND action = 'deleted'",
    )
    .bind(organization_of(&pool, &owner).await)
    .fetch_one(&pool)
    .await
    .expect("count the owner's rows");
    assert_eq!(on_owner, 1, "the leave belongs to the owner's chain");

    // The leaver's own organization holds one member row of its own — the
    // owner row its provisioning wrote for them — and the leave adds nothing.
    let on_member: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT metadata ->> 'kind' FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'member'",
    )
    .bind(organization_of(&pool, &member).await)
    .fetch_all(&pool)
    .await
    .expect("read the member's rows");
    assert_eq!(
        on_member,
        vec![Some("auto_provision".to_owned())],
        "nothing lands on the leaver's own chain"
    );
}

/// The organization's owner cannot leave its project; that is deletion.
#[sqlx::test]
async fn an_owner_cannot_leave_their_own_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    let project = project_of(&pool, "user_owner").await;
    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/memberships/{project}"),
        "user_owner",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// **A member seat reads the roster and changes none of it.**
#[sqlx::test]
async fn a_member_may_not_manage_the_project_they_were_let_into(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "member").await;
    sign_in(&pool, "user_third").await;
    let project = project_of(&pool, &owner).await;

    for surface in ["members", "invites"] {
        let (status, _) = call(
            &pool,
            "GET",
            &format!("/internal/projects/{project}/{surface}"),
            &member,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "a member reads {surface}");
    }

    let (status, _) = call(
        &pool,
        "POST",
        &format!("/internal/projects/{project}/invites"),
        &member,
        Some(json!({ "email": address_of("user_third"), "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member may not invite");

    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/projects/{project}/members/{member}"),
        &member,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member may not remove");
}

/// An admin seat runs the roster: it invites, and it removes a seat it did
/// not grant.
#[sqlx::test]
async fn an_admin_seat_invites_and_removes(pool: PgPool) {
    let (owner, admin) = owner_and_member(&pool, "admin").await;
    let third = "user_third";
    sign_in(&pool, third).await;
    let project = project_of(&pool, &owner).await;

    let (status, offer) = call(
        &pool,
        "POST",
        &format!("/internal/projects/{project}/invites"),
        &admin,
        Some(json!({ "email": address_of(third), "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "an admin invites: {offer}");
    let secret = secret_of(offer["link"].as_str().expect("the link"));
    let (status, body) = accept(&pool, third, &secret).await;
    assert_eq!(status, StatusCode::CREATED, "accept failed: {body}");

    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/projects/{project}/members/{third}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "an admin removes a seat");
}

/// A stranger reaches nothing, with the same 403 a denied member gets.
#[sqlx::test]
async fn a_stranger_reaches_nothing(pool: PgPool) {
    let (owner, _) = owner_and_member(&pool, "admin").await;
    sign_in(&pool, "user_stranger").await;

    for surface in ["members", "invites"] {
        let (status, _) = call(
            &pool,
            "GET",
            &format!(
                "/internal/projects/{}/{surface}",
                project_of(&pool, &owner).await
            ),
            "user_stranger",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}

/// Token validation names the one organization the token may act on, in the
/// documented snake_case shape, and carries no memberships: a token acts AS its
/// organization, which holds no seats — not even the seat its minter holds on
/// somebody else's project.
#[sqlx::test]
async fn a_validated_token_carries_the_organizations_it_may_act_on(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "admin").await;

    let own_project = project_of(&pool, &member).await;
    let (status, minted) = call_in(
        &pool,
        "POST",
        "/internal/tokens",
        &member,
        Some(&own_project),
        Some(json!({ "name": "ci", "created_by": member })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "mint failed: {minted}");
    let token = minted["token"].as_str().expect("the raw token").to_owned();

    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/tokens/validate")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .body(Body::from(json!({ "token": token }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let verdict = json_body(resp).await;

    assert_eq!(
        verdict["organization_id"],
        organization_of(&pool, &member).await,
        "the token's own organization"
    );
    assert_eq!(
        verdict["name"],
        address_of(&member),
        "an unnamed organization is printed as its owner's address"
    );
    assert_eq!(verdict["token_id"], minted["id"]);
    assert!(
        verdict.get("memberships").is_none(),
        "the minter's seat on another organization's project rode along on their token: {verdict}"
    );

    let (status, minted) = call_in(
        &pool,
        "POST",
        "/internal/tokens",
        &owner,
        Some(&project_of(&pool, &owner).await),
        Some(json!({ "name": "ci", "created_by": owner })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "mint failed: {minted}");
    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/tokens/validate")
                .header("x-service-secret", SERVICE_SECRET)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "token": minted["token"].as_str().unwrap() }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let verdict = json_body(resp).await;
    assert_eq!(
        verdict["organization_id"],
        organization_of(&pool, &owner).await
    );
    assert!(verdict.get("memberships").is_none(), "{verdict}");
}

/// One organization cannot make this platform mail an unbounded number of addresses.
#[sqlx::test]
async fn one_organization_cannot_mail_the_world(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner";
    sign_in(&pool, owner).await;

    let max = telmoni_auth::handler::invite::INVITE_DAILY_MAX;
    for i in 0..max {
        let (status, body) = invite(&pool, owner, &format!("p{i}@example.test"), "member").await;
        assert_eq!(status, StatusCode::CREATED, "invitation {i} failed: {body}");
    }
    let (status, body) = invite(&pool, owner, "one-too-many@example.test", "member").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("tomorrow"),
        "the refusal must say when it lifts, got: {detail}"
    );
}

/// Every membership change is on the owner's chain, with the member named.
#[sqlx::test]
async fn every_membership_change_is_audited_on_the_owners_chain(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "member").await;
    call(
        &pool,
        "PUT",
        &format!(
            "/internal/projects/{}/members/{member}/role",
            project_of(&pool, &owner).await
        ),
        &owner,
        Some(json!({ "role": "admin" })),
    )
    .await;
    call(
        &pool,
        "DELETE",
        &format!(
            "/internal/projects/{}/members/{member}",
            project_of(&pool, &owner).await
        ),
        &owner,
        None,
    )
    .await;

    let changes: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT action, metadata ->> 'kind' FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'member' AND resource_id = $2
          ORDER BY seq",
    )
    .bind(organization_of(&pool, &owner).await)
    .bind(&member)
    .fetch_all(&pool)
    .await
    .expect("read the chain");
    assert_eq!(
        changes,
        vec![
            (
                "created".to_owned(),
                Some("enrolled_with_project_invite".to_owned())
            ),
            ("created".to_owned(), Some("invite_accepted".to_owned())),
            ("updated".to_owned(), None),
            ("deleted".to_owned(), Some("removed_by_owner".to_owned())),
        ],
        "the accept enrols them on the roster and seats them; then the role moves and the seat goes"
    );
}

/// **The send is on the chain too, and the secret is not.**
#[sqlx::test]
async fn the_invitation_is_audited_without_its_secret(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner";
    sign_in(&pool, owner).await;

    let (_, body) = invite(&pool, owner, "dana@example.test", "member").await;
    let secret = secret_of(body["link"].as_str().unwrap());
    let organization = organization_of(&pool, owner).await;

    // Inside the project: provisioning wrote the owner's own member row on
    // this chain too, outside any project.
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit.events
          WHERE organization_id = $1 AND in_project = $2
            AND resource_kind = 'member' AND action = 'created'",
    )
    .bind(&organization)
    .bind(project_of(&pool, owner).await)
    .fetch_all(&pool)
    .await
    .expect("read the chain");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], json!("invited"));
    assert_eq!(rows[0]["email"], json!("dana@example.test"));

    let whole: String = sqlx::query_scalar(
        "SELECT string_agg(metadata::text, ' ') FROM audit.events WHERE organization_id = $1",
    )
    .bind(&organization)
    .fetch_one(&pool)
    .await
    .expect("read the chain");
    assert!(
        !whole.contains(&secret),
        "the chain carries a working invitation link"
    );
}

/// P9's mail half: an invitation reaches an address the owner typed.
#[sqlx::test]
async fn an_invitation_mails_the_address_the_owner_typed(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, mail) = app_recording_mail(pool.clone());
    seed_identity(&pool, "user_owner", "user_owner@example.test").await;

    let resp = app
        .clone()
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/me")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("content-type", "application/json"),
                &pool,
                "user_owner",
            )
            .await
            .body(Body::from(json!({}).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let organization = json_body(resp).await["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned();

    assert!(
        mail.0.lock().unwrap().is_empty(),
        "sign-in sent mail: {:?}",
        mail.0.lock().unwrap()
    );

    let resp = app
        .clone()
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/internal/projects/{}/invites",
                        project_of(&pool, "user_owner").await
                    ))
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", &organization)
                    .header("content-type", "application/json"),
                &pool,
                "user_owner",
            )
            .await
            .body(Body::from(
                json!({ "email": "stranger@example.test", "role": "member" }).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let link = json_body(resp).await["link"].as_str().unwrap().to_owned();

    let sent = mail.0.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "one invitation, one mail");
    assert_eq!(
        sent[0].to, "stranger@example.test",
        "the invitation goes to the address that was typed"
    );
    assert!(
        sent[0].text.contains(&link),
        "the mail carries a different link from the one the owner was shown"
    );
    assert!(
        sent[0].text.contains("user_owner@example.test"),
        "it does not say who is asking: {}",
        sent[0].text
    );
    assert!(
        sent[0].text.contains("do nothing"),
        "it does not tell an unexpecting reader they can ignore it: {}",
        sent[0].text
    );
}

#[sqlx::test]
async fn a_duplicate_seat_is_recognised_by_its_constraint(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_owner_dup";
    let member = "usr_member_dup";
    sign_in(&pool, owner).await;
    sign_in(&pool, member).await;

    let owner_proj = project_of(&pool, owner).await;
    let owner_tid = ProjectId::try_new(&owner_proj).expect("valid owner project id");
    let member_uid = UserId::try_new(member).expect("valid member user id");
    let added_by = UserId::try_new(owner).expect("valid user id");

    let mut tx = project_scope(&pool, &owner_tid)
        .await
        .expect("open project scope");

    members::insert(&mut tx, &owner_tid, &member_uid, Role::Admin, &added_by)
        .await
        .expect("first insert succeeds");

    let err = members::insert(&mut tx, &owner_tid, &member_uid, Role::Admin, &added_by)
        .await
        .expect_err("second insert fails with duplicate constraint");

    assert!(
        members::already_a_member(&err),
        "expected already_a_member to recognize duplicate constraint, but got: {err:?}"
    );
}

#[sqlx::test]
async fn incoming_invites_are_listed_and_can_be_accepted_in_app(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_inc_owner";
    let invitee = "usr_inc_invitee";
    sign_in(&pool, owner).await;
    let invitee_me = sign_in(&pool, invitee).await;
    assert!(
        invitee_me["incomingInvites"]
            .as_array()
            .expect("incomingInvites")
            .is_empty()
    );

    let project = project_of(&pool, owner).await;

    let (status, invite_res) = call(
        &pool,
        "POST",
        &format!("/internal/projects/{project}/invites"),
        owner,
        Some(json!({ "email": format!("{invitee}@example.test"), "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invite_id = invite_res["id"].as_str().expect("invite id");

    let invitee_me_after = sign_in(&pool, invitee).await;
    let me_invites = invitee_me_after["incomingInvites"]
        .as_array()
        .expect("incomingInvites array");
    assert_eq!(me_invites.len(), 1);
    assert_eq!(me_invites[0]["id"], invite_id);
    assert_eq!(me_invites[0]["scope"], "project");
    assert_eq!(me_invites[0]["role"], "admin");

    let (status, list_body) = call(&pool, "GET", "/internal/me/invites", invitee, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list_body["invites"]
            .as_array()
            .expect("invites array")
            .len(),
        1
    );

    let (status, accepted) = call(
        &pool,
        "POST",
        &format!("/internal/me/invites/{invite_id}/accept"),
        invitee,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["scope"], "project");
    assert_eq!(accepted["targetId"], project.as_str());
    assert_eq!(accepted["inviteId"], invite_id);
    assert_eq!(accepted["inviterEmail"], address_of(owner));
    assert_eq!(
        accepted["ownerOrganizationId"],
        organization_of(&pool, owner).await
    );

    let (status, list_after_body) = call(&pool, "GET", "/internal/me/invites", invitee, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        list_after_body["invites"]
            .as_array()
            .expect("invites array")
            .is_empty()
    );

    let (status, members_body) = call(
        &pool,
        "GET",
        &format!("/internal/projects/{project}/members"),
        owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = members_body["members"].as_array().expect("members");
    assert!(members.iter().any(|m| m["member_id"] == invitee));
}

#[sqlx::test]
async fn incoming_invites_can_be_declined_in_app(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_dec_owner";
    let invitee = "usr_dec_invitee";
    sign_in(&pool, owner).await;
    sign_in(&pool, invitee).await;

    let project = project_of(&pool, owner).await;

    let (status, invite_res) = call(
        &pool,
        "POST",
        &format!("/internal/projects/{project}/invites"),
        owner,
        Some(json!({ "email": format!("{invitee}@example.test"), "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invite_id = invite_res["id"].as_str().expect("invite id");

    let (status, declined) = call(
        &pool,
        "POST",
        &format!("/internal/me/invites/{invite_id}/decline"),
        invitee,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(declined["inviteId"], invite_id);
    assert_eq!(declined["inviterEmail"], address_of(owner));
    assert_eq!(
        declined["ownerOrganizationId"],
        organization_of(&pool, owner).await
    );

    let (status, list_body) = call(&pool, "GET", "/internal/me/invites", invitee, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        list_body["invites"]
            .as_array()
            .expect("invites array")
            .is_empty()
    );

    let (status, owner_invites_body) = call(
        &pool,
        "GET",
        &format!("/internal/projects/{project}/invites"),
        owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        owner_invites_body["invites"]
            .as_array()
            .expect("invites")
            .is_empty()
    );
}

#[sqlx::test]
async fn project_members_list_includes_project_owner(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner_only";
    sign_in(&pool, owner).await;

    let (status, body) = call(
        &pool,
        "GET",
        &format!(
            "/internal/projects/{}/members",
            project_of(&pool, owner).await
        ),
        owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = body["members"].as_array().expect("members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["member_id"], json!(owner));
    assert_eq!(members[0]["role"], json!("owner"));
    assert_eq!(members[0]["is_owner"], json!(true));
}

#[sqlx::test]
async fn cannot_modify_or_remove_project_owner_from_members(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owner = "user_owner_fixed";
    sign_in(&pool, owner).await;
    let project = project_of(&pool, owner).await;

    let (status, _) = call(
        &pool,
        "PUT",
        &format!("/internal/projects/{project}/members/{owner}/role"),
        owner,
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/projects/{project}/members/{owner}"),
        owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
