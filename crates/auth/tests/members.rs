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

use telmoni_auth::test_provider::{
    RecordingNotifications, ScriptedProvider, SiblingCall, SiblingCalls, as_person,
};
use telmoni_auth::{AppState, Config, db::members, router};
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::test_util::{
    ServiceRole, apply_audit_migrations, holding_chain, seed_identity, service_pool,
    until_queued_on_advisory_locks,
};
use telmoni_shared::{NotificationKind, OrganizationId, ProjectId, Role, UserId};

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
    app_with(pool, sender, telmoni_auth::Siblings::default())
}

/// An app recording both the mail it sends and the notices it hands notifications.
fn app_recording(pool: PgPool) -> (Router, Arc<Recorder>, SiblingCalls) {
    let calls = SiblingCalls::default();
    let siblings = telmoni_auth::Siblings {
        notifications: Some(Arc::new(RecordingNotifications::new(calls.clone()))),
        ..telmoni_auth::Siblings::default()
    };
    let recorder = Arc::new(Recorder::default());
    (app_with(pool, recorder.clone(), siblings), recorder, calls)
}

fn app_with(
    pool: PgPool,
    sender: Arc<dyn telmoni_shared::mail::MailSender>,
    siblings: telmoni_auth::Siblings,
) -> Router {
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

/// The one project [`sign_in`] made in a person's own organization.
async fn project_of(pool: &PgPool, user: &str) -> String {
    sqlx::query_scalar("SELECT external_id FROM auth.projects WHERE organization_id = $1")
        .bind(organization_of(pool, user).await)
        .fetch_one(pool)
        .await
        .expect("sign_in makes exactly one project")
}

/// Sign somebody in, provisioning their organization. The first time, it then
/// renames the organization Acme, the name the verdicts below are read for,
/// and makes the one project these tests act on. Returns the `/me` body.
async fn sign_in(pool: &PgPool, user: &str) -> Value {
    seed_identity(pool, user, &format!("{user}@example.test")).await;
    let body = json!({});
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/internal/v1/me")
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
    let me = json_body(resp).await;
    if me["firstLogin"] == true {
        let (status, body) = call(
            pool,
            "PATCH",
            "/internal/v1/organization",
            user,
            Some(json!({ "name": "Acme" })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "naming {user}'s organization: {body}"
        );
        let (status, body) = call(
            pool,
            "POST",
            "/internal/v1/projects",
            user,
            Some(json!({ "name": "Platform" })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{user}'s first project: {body}"
        );
    }
    me
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
            "/internal/v1/projects/{}/invites",
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
        "/internal/v1/invites/accept",
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

/// Put `member` on `owner`'s organization roster as a Member, by an
/// organization invitation accepted.
async fn enrol(pool: &PgPool, owner: &str, member: &str) {
    let (status, offer) = call(
        pool,
        "POST",
        "/internal/v1/organization/invites",
        owner,
        Some(json!({ "email": address_of(member), "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "organization invite: {offer}");
    let secret = secret_of(offer["link"].as_str().expect("the link"));
    let (status, body) = accept(pool, member, &secret).await;
    assert!(status.is_success(), "organization accept: {status} {body}");
}

/// Give `member` a seat on `project` as `giver`, from the organization's roster.
async fn give(
    pool: &PgPool,
    giver: &str,
    project: &str,
    member: &str,
    role: &str,
) -> (StatusCode, Value) {
    call_in(
        pool,
        "POST",
        &format!("/internal/v1/projects/{project}/members"),
        giver,
        Some(project),
        Some(json!({ "memberId": member, "role": role })),
    )
    .await
}

/// [`give`], through `app`.
async fn give_through(
    app: Router,
    pool: &PgPool,
    giver: &str,
    project: &str,
    member: &str,
) -> (StatusCode, Value) {
    let req = as_person(
        Request::builder()
            .method("POST")
            .uri(format!("/internal/v1/projects/{project}/members"))
            .header("x-service-secret", SERVICE_SECRET)
            .header("x-organization-id", organization_of(pool, giver).await)
            .header("x-project-id", project)
            .header("content-type", "application/json"),
        pool,
        giver,
    )
    .await
    .body(Body::from(
        json!({ "memberId": member, "role": "member" }).to_string(),
    ))
    .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

/// The seats nobody has seen yet, as (project, holder).
async fn unseen(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT project_id, user_id FROM auth.project_members
          WHERE seen_at IS NULL ORDER BY user_id, project_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Make `member` a `role` of `owner`'s organization.
async fn make(pool: &PgPool, owner: &str, member: &str, role: &str) {
    let (status, body) = call(
        pool,
        "PUT",
        &format!("/internal/v1/organization/members/{member}/role"),
        owner,
        Some(json!({ "role": role })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{member} made {role}: {body}");
}

/// A member of the organization is seated on one of its projects by its owner
/// with nothing to accept: listed at once, told by mail, shown the seat in
/// `/me` until they have seen it, and recorded on the owner's chain.
#[sqlx::test]
async fn an_owner_seats_a_member_of_the_organization_with_nothing_to_accept(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, mail) = app_recording_mail(pool.clone());
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let project = project_of(&pool, "user_owner").await;
    let organization = organization_of(&pool, "user_owner").await;
    assert!(
        mail.0.lock().unwrap().is_empty(),
        "nothing above mails through this app"
    );

    let req = as_person(
        Request::builder()
            .method("POST")
            .uri(format!("/internal/v1/projects/{project}/members"))
            .header("x-service-secret", SERVICE_SECRET)
            .header("x-organization-id", &organization)
            .header("x-project-id", &project)
            .header("content-type", "application/json"),
        &pool,
        "user_owner",
    )
    .await
    .body(Body::from(
        json!({ "memberId": "user_member", "role": "member" }).to_string(),
    ))
    .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = json_body(resp).await;
    assert_eq!(body["memberId"], "user_member");
    assert_eq!(body["role"], "member");
    assert_eq!(body["email"], address_of("user_member"));
    assert_eq!(body["ownerOrganizationId"], organization);

    let (status, roster) = call_in(
        &pool,
        "GET",
        &format!("/internal/v1/projects/{project}/members"),
        "user_owner",
        Some(&project),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{roster}");
    assert!(
        roster["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["member_id"] == "user_member" && m["role"] == "member"),
        "the seat is on the roster at once: {roster}"
    );

    let me = sign_in(&pool, "user_member").await;
    assert!(
        me["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["projectId"] == project && m["role"] == "member"),
        "the member holds the seat: {me}"
    );
    let seats = me["givenSeats"].as_array().unwrap();
    assert_eq!(seats.len(), 1, "the seat waits to be seen: {me}");
    assert_eq!(seats[0]["projectId"], project);
    assert_eq!(seats[0]["projectName"], "Platform");
    assert_eq!(seats[0]["projectSlug"], "platform");
    assert_eq!(seats[0]["organizationId"], organization);
    assert_eq!(seats[0]["organizationName"], "Acme");
    // The slug the owner's first sign-in minted, which the rename to Acme keeps.
    let home_slug = me["organizations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["organizationId"] == organization)
        .and_then(|o| o["slug"].as_str())
        .expect("the member lists Acme")
        .to_owned();
    assert_eq!(seats[0]["organizationSlug"], home_slug);
    assert_eq!(seats[0]["role"], "member");
    assert_eq!(seats[0]["giverEmail"], address_of("user_owner"));

    let sent = mail.0.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "one mail, to the member: {sent:?}");
    assert_eq!(sent[0].to, address_of("user_member"));
    assert!(
        sent[0].subject.contains("added you to Platform"),
        "{}",
        sent[0].subject
    );
    assert!(
        sent[0]
            .text
            .contains(&format!("http://localhost:3000/{organization}/{project}")),
        "the mail leads to the project, by ids a rename or a move leaves alone: {}",
        sent[0].text
    );
    assert!(
        sent[0].text.contains("nothing to accept"),
        "{}",
        sent[0].text
    );

    let changes: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT action, metadata ->> 'kind' FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'member' AND resource_id = $2
            AND in_project = $3
          ORDER BY seq",
    )
    .bind(&organization)
    .bind("user_member")
    .bind(&project)
    .fetch_all(&pool)
    .await
    .expect("read the chain");
    assert_eq!(
        changes,
        vec![("created".to_owned(), Some("seat_given".to_owned()))],
        "the seat is on the owner's chain, in the project"
    );

    // Seen: the seat leaves /me, its organization named for the person's
    // other consoles; seeing it again is nothing, and names none.
    for expected in [json!(organization), Value::Null] {
        let (status, body) = call(
            &pool,
            "POST",
            &format!("/internal/v1/me/seats/{project}/seen"),
            "user_member",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["organizationId"], expected);
    }
    let me = sign_in(&pool, "user_member").await;
    assert!(
        me["givenSeats"].as_array().unwrap().is_empty(),
        "seen, the seat is no longer the bell's: {me}"
    );
    assert!(
        me["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["projectId"] == project),
        "and still held: {me}"
    );
}

/// Who gives a seat, and to whom: the organization's owner and admins, to
/// its members — never a project-seat admin who is only a member of the
/// organization, nor to its owner or admins, who hold every project, nor to
/// a stranger, nor twice.
#[sqlx::test]
async fn a_seat_is_given_by_the_organizations_owner_or_admins_to_its_members_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    for user in [
        "user_owner",
        "user_member",
        "user_cat",
        "user_dan",
        "user_eve",
    ] {
        sign_in(&pool, user).await;
    }
    enrol(&pool, "user_owner", "user_member").await;
    let project = project_of(&pool, "user_owner").await;

    // Cat is an admin of the organization; Dan an admin seat on the project
    // alone, which enrols him as a member of the organization.
    let (status, offer) = call(
        &pool,
        "POST",
        "/internal/v1/organization/invites",
        "user_owner",
        Some(json!({ "email": address_of("user_cat"), "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    let (status, body) = accept(
        &pool,
        "user_cat",
        &secret_of(offer["link"].as_str().unwrap()),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    let (status, offer) = invite(&pool, "user_owner", &address_of("user_dan"), "admin").await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    let (status, body) = accept(
        &pool,
        "user_dan",
        &secret_of(offer["link"].as_str().unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = give(&pool, "user_dan", &project, "user_member", "member").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a project admin alone: {body}"
    );
    assert_eq!(
        body["detail"],
        "only the organization's owner or an admin can add a member from its roster"
    );
    let (status, _) = give(&pool, "user_member", &project, "user_eve", "member").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member off the project");

    let (status, body) = give(&pool, "user_owner", &project, "user_eve", "member").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a stranger to the organization: {body}"
    );
    let (status, body) = give(&pool, "user_owner", &project, "user_cat", "member").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an organization admin holds every project: {body}"
    );
    let (status, body) = give(&pool, "user_cat", &project, "user_owner", "member").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "so does the owner: {body}");
    let (status, body) = give(&pool, "user_owner", &project, "user_member", "owner").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no seat makes an owner: {body}"
    );

    let (status, body) = give(&pool, "user_cat", &project, "user_member", "admin").await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "an organization admin gives the seat: {body}"
    );
    assert_eq!(body["role"], "admin");
    let (status, body) = give(&pool, "user_owner", &project, "user_member", "member").await;
    assert_eq!(status, StatusCode::CONFLICT, "a second seat: {body}");
    assert_eq!(body["detail"], "that person is already a member");

    // Nothing of the refusals wrote a seat, and the one given stands as given.
    let roster: Vec<(String, String)> = sqlx::query_as(
        "SELECT user_id, role FROM auth.project_members WHERE project_id = $1 ORDER BY user_id",
    )
    .bind(&project)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        roster,
        vec![
            ("user_dan".to_owned(), "admin".to_owned()),
            ("user_member".to_owned(), "admin".to_owned()),
        ]
    );
}

/// A seat given as its person is removed from the organization waits for the
/// removal, and then finds nobody on the roster to seat.
#[sqlx::test]
async fn a_seat_given_as_its_person_is_removed_goes_in_turn(pool: PgPool) {
    let (owner, _) = owner_and_member(&pool, "admin").await;
    let third = "user_third";
    sign_in(&pool, third).await;
    enrol(&pool, &owner, third).await;
    let organization = OrganizationId::try_new(organization_of(&pool, &owner).await).unwrap();
    let project = project_of(&pool, &owner).await;

    let pin = holding_chain(&pool, &organization).await;
    let removing = {
        let (pool, owner) = (pool.clone(), owner.clone());
        tokio::spawn(async move {
            call(
                &pool,
                "DELETE",
                &format!("/internal/v1/organization/members/{third}"),
                &owner,
                None,
            )
            .await
        })
    };
    until_queued_on_advisory_locks(&pool, 1).await;
    let giving = {
        let (pool, owner, project) = (pool.clone(), owner.clone(), project.clone());
        tokio::spawn(async move { give(&pool, &owner, &project, third, "member").await })
    };
    until_queued_on_advisory_locks(&pool, 2).await;
    pin.rollback().await.unwrap();

    let (removed, body) = removing.await.unwrap();
    assert_eq!(removed, StatusCode::NO_CONTENT, "{body}");
    let (given, body) = giving.await.unwrap();
    assert_eq!(given, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["detail"], "that person is not in this organization");
    let seats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(&project)
    .bind(third)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seats, 0, "a seat was written for somebody off the roster");
}

/// With the `members` feature switched off for the organization, no seat is
/// given, as no invitation is sent.
#[sqlx::test]
async fn a_switched_off_members_feature_gives_no_seat(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let project = project_of(&pool, "user_owner").await;
    sqlx::query(
        "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor)
         VALUES ($1, 'members', false, 'test', 'test')",
    )
    .bind(organization_of(&pool, "user_owner").await)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = give(&pool, "user_owner", &project, "user_member", "member").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["type"], "/errors/tenant/feature-off");
    let seats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.project_members WHERE project_id = $1 AND user_id = 'user_member'",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seats, 0);
}

/// An invitation to the project sent before the seat is spent by it: the seat
/// withdraws it, and its link then answers as any withdrawn one, never a 500.
#[sqlx::test]
async fn a_seat_given_withdraws_the_persons_invitation_to_the_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let project = project_of(&pool, "user_owner").await;
    let (status, offer) = invite(&pool, "user_owner", &address_of("user_member"), "admin").await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    let secret = secret_of(offer["link"].as_str().expect("the link"));

    let (status, body) = give(&pool, "user_owner", &project, "user_member", "member").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.member_invites
          WHERE project_id = $1 AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 0, "the invitation outlived the seat");
    let withdrawn: i64 = sqlx::query_scalar(
        "SELECT (metadata ->> 'invitations_withdrawn')::bigint FROM audit.events
          WHERE in_project = $1 AND metadata ->> 'kind' = 'seat_given'",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(withdrawn, 1);

    let (status, body) = accept(&pool, "user_member", &secret).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let role: String = sqlx::query_scalar(
        "SELECT role FROM auth.project_members WHERE project_id = $1 AND user_id = 'user_member'",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(role, "member", "the link rewrote the seat given");
}

/// An accept that finds the seat already held — taken since the link was read
/// — is refused whole with a 409, its invitation left as it was, never a 500
/// from a transaction the duplicate aborted.
#[sqlx::test]
async fn an_accept_finding_the_seat_already_held_is_refused_whole(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    let project = project_of(&pool, "user_owner").await;
    let (status, offer) = invite(&pool, "user_owner", &address_of("user_member"), "admin").await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    let secret = secret_of(offer["link"].as_str().expect("the link"));
    sqlx::query(
        "INSERT INTO auth.project_members (project_id, user_id, role, added_by)
         VALUES ($1, 'user_member', 'member', 'user_owner')",
    )
    .bind(&project)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = accept(&pool, "user_member", &secret).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["detail"], "you are already a member of this project");
    let accepted: bool = sqlx::query_scalar(
        "SELECT accepted_at IS NOT NULL FROM auth.member_invites WHERE project_id = $1",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!accepted, "a refused accept spent the invitation");
}

/// An accept queued behind a seat given, which withdrew its invitation, is
/// told the seat is held rather than that its link was used.
#[sqlx::test]
async fn an_accept_overtaken_by_a_seat_given_says_the_seat_is_held(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let organization = OrganizationId::try_new(organization_of(&pool, "user_owner").await).unwrap();
    let project = project_of(&pool, "user_owner").await;
    let (status, offer) = invite(&pool, "user_owner", &address_of("user_member"), "member").await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    let secret = secret_of(offer["link"].as_str().expect("the link"));

    // The seat waits on the chain holding the organization's lock; the accept,
    // its invitation read live, queues on that lock behind it.
    let pin = holding_chain(&pool, &organization).await;
    let giving = {
        let (pool, project) = (pool.clone(), project.clone());
        tokio::spawn(
            async move { give(&pool, "user_owner", &project, "user_member", "member").await },
        )
    };
    until_queued_on_advisory_locks(&pool, 1).await;
    let accepting = {
        let pool = pool.clone();
        tokio::spawn(async move { accept(&pool, "user_member", &secret).await })
    };
    until_queued_on_advisory_locks(&pool, 2).await;
    pin.rollback().await.unwrap();

    let (given, body) = giving.await.unwrap();
    assert_eq!(given, StatusCode::CREATED, "{body}");
    let (accepted, body) = accepting.await.unwrap();
    assert_eq!(accepted, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["detail"], "you are already a member of this project");
    let (seats, withdrawn): (i64, bool) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM auth.project_members
                  WHERE project_id = $1 AND user_id = 'user_member'),
                (SELECT revoked_at IS NOT NULL AND accepted_at IS NULL
                   FROM auth.member_invites WHERE project_id = $1)",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seats, 1);
    assert!(withdrawn, "the invitation was spent rather than withdrawn");
}

/// The project's feed is told of a seat given, naming the member as the
/// notice's subject, which is what lets an erasure unname them.
#[sqlx::test]
async fn a_seat_given_tells_the_projects_feed_naming_its_member(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, _, calls) = app_recording(pool.clone());
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    sqlx::query(
        "UPDATE auth.identities SET display_name = 'Dana Scully' WHERE user_id = 'user_member'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let organization = organization_of(&pool, "user_owner").await;
    let project = project_of(&pool, "user_owner").await;

    let (status, body) = give_through(app, &pool, "user_owner", &project, "user_member").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        calls.calls(),
        [SiblingCall::Emit {
            organization_id: organization,
            project_id: Some(project),
            kind: NotificationKind::MemberAdded,
            subject_user_id: Some("user_member".into()),
            title: "Dana Scully was added to the project".into(),
        }]
    );
}

/// A seat that waited while its person's account deletion was confirmed is
/// left to the erasure: nobody is mailed, and the project's feed names nobody
/// its last pass could miss.
#[sqlx::test]
async fn a_seat_given_as_its_persons_account_goes_tells_nobody(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let (app, mail, calls) = app_recording(pool.clone());
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let organization = OrganizationId::try_new(organization_of(&pool, "user_owner").await).unwrap();
    let project = project_of(&pool, "user_owner").await;

    // The seat, its person read as staying, waits on the chain; the deletion
    // is confirmed meanwhile.
    let pin = holding_chain(&pool, &organization).await;
    let giving = {
        let (pool, project) = (pool.clone(), project.clone());
        tokio::spawn(async move {
            give_through(app, &pool, "user_owner", &project, "user_member").await
        })
    };
    until_queued_on_advisory_locks(&pool, 1).await;
    sqlx::query(
        "INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ('user_member', now())
         ON CONFLICT (user_id) DO UPDATE SET deletion_requested_at = now()",
    )
    .execute(&pool)
    .await
    .unwrap();
    pin.rollback().await.unwrap();

    let (status, body) = giving.await.unwrap();
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(
        mail.0.lock().unwrap().is_empty(),
        "a person whose account is going was mailed"
    );
    assert!(
        calls.calls().is_empty(),
        "the feed was told of a person whose account is going: {:?}",
        calls.calls()
    );
}

/// Seeing a seat marks that one alone: not the person's seat on another
/// project, nor another person's on the same one.
#[sqlx::test]
async fn seeing_a_seat_marks_that_seat_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    for member in ["user_member", "user_third"] {
        sign_in(&pool, member).await;
        enrol(&pool, "user_owner", member).await;
    }
    let organization = organization_of(&pool, "user_owner").await;
    let first = project_of(&pool, "user_owner").await;
    let (status, created) = call(
        &pool,
        "POST",
        "/internal/v1/projects",
        "user_owner",
        Some(json!({ "name": "Billing" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let second = created["id"].as_str().unwrap().to_owned();
    for (project, member) in [
        (&first, "user_member"),
        (&second, "user_member"),
        (&first, "user_third"),
    ] {
        let (status, body) = give(&pool, "user_owner", project, member, "member").await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let (status, body) = call(
        &pool,
        "POST",
        &format!("/internal/v1/me/seats/{first}/seen"),
        "user_member",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["organizationId"], organization);
    assert_eq!(
        unseen(&pool).await,
        [
            (second.clone(), "user_member".to_owned()),
            (first.clone(), "user_third".to_owned()),
        ]
    );
}

/// A seat seen while a move holds its row past the role's `lock_timeout` is a
/// 409 to try again, not a 500; the move gone, it goes through.
#[sqlx::test]
async fn a_seat_seen_kept_waiting_on_its_row_is_a_conflict(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let organization = organization_of(&pool, "user_owner").await;
    let project = project_of(&pool, "user_owner").await;
    let (status, body) = give(&pool, "user_owner", &project, "user_member", "member").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // A tier sets `lock_timeout` on the role, which the suite's `SET ROLE`
    // skips: the database's own reaches every new connection.
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "ALTER DATABASE \"{database}\" SET lock_timeout = '1s'"
    )))
    .execute(&pool)
    .await
    .unwrap();
    let mut moving = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT 1 FROM auth.project_members
          WHERE project_id = $1 AND user_id = 'user_member' FOR UPDATE",
    )
    .bind(&project)
    .execute(&mut *moving)
    .await
    .unwrap();

    let uri = format!("/internal/v1/me/seats/{project}/seen");
    let seen = || call(&pool, "POST", &uri, "user_member", None);
    let (status, body) = tokio::time::timeout(std::time::Duration::from_secs(10), seen())
        .await
        .expect("the seen lane waited past its lock_timeout");
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("busy")),
        "{body}"
    );
    moving.rollback().await.unwrap();
    let (status, body) = seen().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["organizationId"], organization);
}

/// A member made an admin while a seat for them waits is given none: their
/// role is read under the organization's lock, and an admin holds every
/// project already.
#[sqlx::test]
async fn a_member_made_an_admin_as_a_seat_waits_is_given_none(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let organization = OrganizationId::try_new(organization_of(&pool, "user_owner").await).unwrap();
    let project = project_of(&pool, "user_owner").await;

    let pin = holding_chain(&pool, &organization).await;
    let promoting = {
        let pool = pool.clone();
        tokio::spawn(async move {
            call(
                &pool,
                "PUT",
                "/internal/v1/organization/members/user_member/role",
                "user_owner",
                Some(json!({ "role": "admin" })),
            )
            .await
        })
    };
    until_queued_on_advisory_locks(&pool, 1).await;
    let giving = {
        let (pool, project) = (pool.clone(), project.clone());
        tokio::spawn(
            async move { give(&pool, "user_owner", &project, "user_member", "member").await },
        )
    };
    until_queued_on_advisory_locks(&pool, 2).await;
    pin.rollback().await.unwrap();

    let (promoted, body) = promoting.await.unwrap();
    assert_eq!(promoted, StatusCode::OK, "{body}");
    let (given, body) = giving.await.unwrap();
    assert_eq!(given, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["detail"],
        "the organization's owner and admins are on every project already"
    );
    assert!(unseen(&pool).await.is_empty(), "an admin was given a seat");
}

/// An admin demoted as they give a seat gives none, though an admin seat of
/// their own on the project still lets them manage its members: the roster is
/// its owner's and admins' alone.
#[sqlx::test]
async fn an_admin_demoted_as_they_give_a_seat_gives_none(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    for member in ["user_admin", "user_member"] {
        sign_in(&pool, member).await;
        enrol(&pool, "user_owner", member).await;
    }
    let organization = OrganizationId::try_new(organization_of(&pool, "user_owner").await).unwrap();
    let project = project_of(&pool, "user_owner").await;
    let (status, body) = give(&pool, "user_owner", &project, "user_admin", "admin").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    make(&pool, "user_owner", "user_admin", "admin").await;

    let pin = holding_chain(&pool, &organization).await;
    let demoting = {
        let pool = pool.clone();
        tokio::spawn(async move {
            call(
                &pool,
                "PUT",
                "/internal/v1/organization/members/user_admin/role",
                "user_owner",
                Some(json!({ "role": "member" })),
            )
            .await
        })
    };
    until_queued_on_advisory_locks(&pool, 1).await;
    let giving = {
        let (pool, project) = (pool.clone(), project.clone());
        tokio::spawn(
            async move { give(&pool, "user_admin", &project, "user_member", "member").await },
        )
    };
    until_queued_on_advisory_locks(&pool, 2).await;
    pin.rollback().await.unwrap();

    let (demoted, body) = demoting.await.unwrap();
    assert_eq!(demoted, StatusCode::OK, "{body}");
    let (given, body) = giving.await.unwrap();
    assert_eq!(given, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        body["detail"],
        "only the organization's owner or an admin can add a member from its roster"
    );
    let seats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.project_members WHERE project_id = $1 AND user_id = 'user_member'",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seats, 0, "a demoted admin gave a seat");
}

/// The bell shows a seat given while its person is a Member of an active
/// organization alone: made an admin, they hold every project; once its
/// deletion starts, the organization is closed. An erased giver leaves it
/// named by its organization.
#[sqlx::test]
async fn the_bell_shows_a_seat_given_to_a_member_of_an_active_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    for member in ["user_admin", "user_member"] {
        sign_in(&pool, member).await;
        enrol(&pool, "user_owner", member).await;
    }
    make(&pool, "user_owner", "user_admin", "admin").await;
    let organization = organization_of(&pool, "user_owner").await;
    let project = project_of(&pool, "user_owner").await;
    let (status, body) = give(&pool, "user_admin", &project, "user_member", "member").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let bell = |me: Value| me["givenSeats"].as_array().unwrap().clone();

    let seats = bell(sign_in(&pool, "user_member").await);
    assert_eq!(seats.len(), 1);
    assert_eq!(seats[0]["giverEmail"], address_of("user_admin"));

    make(&pool, "user_owner", "user_member", "admin").await;
    assert!(
        bell(sign_in(&pool, "user_member").await).is_empty(),
        "an admin is shown a seat they need not hold"
    );
    make(&pool, "user_owner", "user_member", "member").await;
    assert_eq!(bell(sign_in(&pool, "user_member").await).len(), 1);

    // What an erasure leaves of the giver: an id nobody answers to.
    sqlx::query("UPDATE auth.project_members SET added_by = 'user_erased' WHERE project_id = $1")
        .bind(&project)
        .execute(&pool)
        .await
        .unwrap();
    let seats = bell(sign_in(&pool, "user_member").await);
    assert_eq!(
        seats.len(),
        1,
        "an erased giver took the seat from the bell"
    );
    assert_eq!(seats[0]["giverEmail"], Value::Null);
    assert_eq!(seats[0]["giverDisplayName"], Value::Null);

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
    assert!(
        bell(sign_in(&pool, "user_member").await).is_empty(),
        "a closed organization's seat is in the bell"
    );
}

/// A member of the organization whose account deletion is confirmed is given
/// no seat, and the refusal reads as a stranger's.
#[sqlx::test]
async fn a_person_whose_account_is_going_is_given_no_seat(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let project = project_of(&pool, "user_owner").await;
    sqlx::query(
        "INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ('user_member', now())
         ON CONFLICT (user_id) DO UPDATE SET deletion_requested_at = now()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = give(&pool, "user_owner", &project, "user_member", "member").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["detail"], "that person is not in this organization");
    let seats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.project_members WHERE project_id = $1 AND user_id = 'user_member'",
    )
    .bind(&project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seats, 0);
}

/// Each seat given mails its person, so a project gives at most as many a day
/// as it sends invitations, counted off the chain: a removal does not reset it.
#[sqlx::test]
async fn the_seats_a_project_gives_are_capped_a_day(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let project = project_of(&pool, "user_owner").await;
    for _ in 0..telmoni_auth::handler::invite::INVITE_DAILY_MAX {
        let (status, body) = give(&pool, "user_owner", &project, "user_member", "member").await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let (status, body) = call_in(
            &pool,
            "DELETE",
            &format!("/internal/v1/projects/{project}/members/user_member"),
            "user_owner",
            Some(&project),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }

    let (status, body) = give(&pool, "user_owner", &project, "user_member", "member").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["detail"],
        "you have added 20 members to this project today — the rest can be added tomorrow"
    );
}

/// The whole loop: invite by address, accept, see it on both sides, remove it.
#[sqlx::test]
async fn an_invitation_accepted_seats_a_member_and_both_sides_see_it(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "admin").await;
    // A seat its holder took is seen as it is written: the bell lists none.
    let me = sign_in(&pool, &member).await;
    assert_eq!(me["givenSeats"], json!([]), "{me}");

    let (status, body) = call(
        &pool,
        "GET",
        &format!(
            "/internal/v1/projects/{}/members",
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
    assert_eq!(members[1]["member_id"], json!(member));
    assert_eq!(members[1]["email"], json!(address_of(&member)));
    assert_eq!(members[1]["role"], json!("admin"));

    let (status, body) = call(
        &pool,
        "GET",
        &format!(
            "/internal/v1/projects/{}/invites",
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
            "/internal/v1/projects/{}/members/{member}",
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
            "/internal/v1/projects/{}/invites",
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
            "/internal/v1/projects/{}/invites",
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
/// holding the link. The refusal says to sign in with the address the mail
/// went to without repeating it: an address never travels in a problem
/// `detail`, and the invitation page already shows it to whoever holds the
/// link.
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
        detail.contains("sign in with the one it was mailed to"),
        "the refusal must point at the mailed-to address, got: {detail}"
    );
    assert!(
        !detail.contains(&address_of("user_invited")),
        "the refusal repeats the invited address: {detail}"
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
            "/internal/v1/projects/{}/invites/{id}",
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
            "/internal/v1/invites/look",
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
            "/internal/v1/projects/{}/members",
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
            "/internal/v1/projects/{}/invites",
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
            "/internal/v1/projects/{}/members/{member}/role",
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
        &format!(
            "/internal/v1/memberships/{}",
            project_of(&pool, &owner).await
        ),
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
        &format!("/internal/v1/memberships/{project}"),
        "user_owner",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// A seat given from the roster is given up as an accepted one is, and the bell forgets it.
#[sqlx::test]
async fn a_seat_given_is_left_as_any_seat_is(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    sign_in(&pool, "user_owner").await;
    sign_in(&pool, "user_member").await;
    enrol(&pool, "user_owner", "user_member").await;
    let project = project_of(&pool, "user_owner").await;
    let (status, body) = give(&pool, "user_owner", &project, "user_member", "member").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        unseen(&pool).await,
        vec![(project.clone(), "user_member".to_owned())],
        "the seat waits in the bell"
    );

    let (status, body) = call(
        &pool,
        "DELETE",
        &format!("/internal/v1/memberships/{project}"),
        "user_member",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let me = sign_in(&pool, "user_member").await;
    assert_eq!(me["memberships"], json!([]), "the seat is still held: {me}");
    assert_eq!(
        me["givenSeats"],
        json!([]),
        "the bell still shows the seat: {me}"
    );
    assert!(
        unseen(&pool).await.is_empty(),
        "a seat given up still waits to be seen"
    );

    let (status, roster) = call(
        &pool,
        "GET",
        "/internal/v1/organization/members",
        "user_owner",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{roster}");
    assert!(
        roster["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["member_id"] == "user_member"),
        "leaving the project took the person off the organization's roster: {roster}"
    );

    let kinds: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT metadata ->> 'kind' FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'member' AND action = 'deleted'",
    )
    .bind(organization_of(&pool, "user_owner").await)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        kinds,
        vec![Some("left".to_owned())],
        "the leave is on the owner's chain"
    );
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
            &format!("/internal/v1/projects/{project}/{surface}"),
            &member,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "a member reads {surface}");
    }

    let (status, _) = call(
        &pool,
        "POST",
        &format!("/internal/v1/projects/{project}/invites"),
        &member,
        Some(json!({ "email": address_of("user_third"), "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member may not invite");

    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/v1/projects/{project}/members/{member}"),
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
        &format!("/internal/v1/projects/{project}/invites"),
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
        &format!("/internal/v1/projects/{project}/members/{third}"),
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
                "/internal/v1/projects/{}/{surface}",
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
        "/internal/v1/tokens",
        &member,
        Some(&own_project),
        Some(json!({ "name": "ci", "created_by": member, "scopes": ["read"] })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "mint failed: {minted}");
    let token = minted["token"].as_str().expect("the raw token").to_owned();

    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/v1/tokens/validate")
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
        verdict["name"], "Acme",
        "the token names its organization by the name its owner gave it"
    );
    assert_eq!(verdict["token_id"], minted["id"]);
    assert!(
        verdict.get("memberships").is_none(),
        "the minter's seat on another organization's project rode along on their token: {verdict}"
    );

    let (status, minted) = call_in(
        &pool,
        "POST",
        "/internal/v1/tokens",
        &owner,
        Some(&project_of(&pool, &owner).await),
        Some(json!({ "name": "ci", "created_by": owner, "scopes": ["read"] })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "mint failed: {minted}");
    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/v1/tokens/validate")
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
            "/internal/v1/projects/{}/members/{member}/role",
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
            "/internal/v1/projects/{}/members/{member}",
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
                    .uri("/internal/v1/me")
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
    // Signed in by hand above, so what `sign_in` does next is done here: the
    // invitation needs a named organization and a project to be invited to.
    let (status, body) = call(
        &pool,
        "PATCH",
        "/internal/v1/organization",
        "user_owner",
        Some(json!({ "name": "Acme" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &pool,
        "POST",
        "/internal/v1/projects",
        "user_owner",
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let resp = app
        .clone()
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/internal/v1/projects/{}/invites",
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
        &format!("/internal/v1/projects/{project}/invites"),
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

    let (status, list_body) = call(&pool, "GET", "/internal/v1/me/invites", invitee, None).await;
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
        &format!("/internal/v1/me/invites/{invite_id}/accept"),
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

    let (status, list_after_body) =
        call(&pool, "GET", "/internal/v1/me/invites", invitee, None).await;
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
        &format!("/internal/v1/projects/{project}/members"),
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
        &format!("/internal/v1/projects/{project}/invites"),
        owner,
        Some(json!({ "email": format!("{invitee}@example.test"), "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invite_id = invite_res["id"].as_str().expect("invite id");

    let (status, declined) = call(
        &pool,
        "POST",
        &format!("/internal/v1/me/invites/{invite_id}/decline"),
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

    let (status, list_body) = call(&pool, "GET", "/internal/v1/me/invites", invitee, None).await;
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
        &format!("/internal/v1/projects/{project}/invites"),
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
            "/internal/v1/projects/{}/members",
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
        &format!("/internal/v1/projects/{project}/members/{owner}/role"),
        owner,
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/v1/projects/{project}/members/{owner}"),
        owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A seat changed as its holder is removed from the organization waits for the removal.
#[sqlx::test]
async fn a_seat_changed_as_its_holder_is_removed_goes_in_turn(pool: PgPool) {
    let (owner, admin) = owner_and_member(&pool, "admin").await;
    let third = "user_third";
    sign_in(&pool, third).await;
    let (status, offer) = invite(&pool, &owner, &address_of(third), "member").await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    let secret = secret_of(offer["link"].as_str().expect("the link"));
    assert_eq!(accept(&pool, third, &secret).await.0, StatusCode::CREATED);
    let organization = OrganizationId::try_new(organization_of(&pool, &owner).await).unwrap();
    let project = project_of(&pool, &owner).await;

    let pin = holding_chain(&pool, &organization).await;
    let removing = {
        let (pool, owner) = (pool.clone(), owner.clone());
        tokio::spawn(async move {
            call(
                &pool,
                "DELETE",
                &format!("/internal/v1/organization/members/{third}"),
                &owner,
                None,
            )
            .await
        })
    };
    until_queued_on_advisory_locks(&pool, 1).await;
    let changing = {
        let (pool, admin) = (pool.clone(), admin.clone());
        tokio::spawn(async move {
            call(
                &pool,
                "PUT",
                &format!("/internal/v1/projects/{project}/members/{third}/role"),
                &admin,
                Some(json!({ "role": "admin" })),
            )
            .await
        })
    };
    until_queued_on_advisory_locks(&pool, 2).await;
    pin.rollback().await.unwrap();

    let (removed, body) = removing.await.unwrap();
    assert_eq!(removed, StatusCode::NO_CONTENT, "{body}");
    let (changed, body) = changing.await.unwrap();
    assert_eq!(changed, StatusCode::NOT_FOUND, "{body}");
}

/// A project deleted as one of its seat holders is removed waits for the removal.
#[sqlx::test]
async fn a_project_deleted_as_a_holder_is_removed_goes_in_turn(pool: PgPool) {
    let (owner, member) = owner_and_member(&pool, "member").await;
    let organization = OrganizationId::try_new(organization_of(&pool, &owner).await).unwrap();
    let project = project_of(&pool, &owner).await;

    let pin = holding_chain(&pool, &organization).await;
    let removing = {
        let (pool, owner, member) = (pool.clone(), owner.clone(), member.clone());
        tokio::spawn(async move {
            call(
                &pool,
                "DELETE",
                &format!("/internal/v1/organization/members/{member}"),
                &owner,
                None,
            )
            .await
        })
    };
    until_queued_on_advisory_locks(&pool, 1).await;
    let deleting = {
        let (pool, owner) = (pool.clone(), owner.clone());
        tokio::spawn(async move {
            call(
                &pool,
                "DELETE",
                &format!("/internal/v1/projects/{project}"),
                &owner,
                None,
            )
            .await
        })
    };
    until_queued_on_advisory_locks(&pool, 2).await;
    pin.rollback().await.unwrap();

    let (removed, body) = removing.await.unwrap();
    assert_eq!(removed, StatusCode::NO_CONTENT, "{body}");
    let (deleted, body) = deleting.await.unwrap();
    assert_eq!(deleted, StatusCode::OK, "{body}");
}

/// Leaving a project that is not there answers as leaving one you hold no seat on.
#[sqlx::test]
async fn leaving_a_project_that_is_not_there_answers_as_one_without_a_seat(pool: PgPool) {
    let (owner, _) = owner_and_member(&pool, "member").await;
    let stranger = "user_stranger";
    sign_in(&pool, stranger).await;

    let theirs = call(
        &pool,
        "DELETE",
        &format!(
            "/internal/v1/memberships/{}",
            project_of(&pool, &owner).await
        ),
        stranger,
        None,
    )
    .await;
    let nowhere = call(
        &pool,
        "DELETE",
        "/internal/v1/memberships/project_gone",
        stranger,
        None,
    )
    .await;
    assert_eq!(theirs.0, StatusCode::NOT_FOUND, "{}", theirs.1);
    assert_eq!(nowhere, theirs);
}
