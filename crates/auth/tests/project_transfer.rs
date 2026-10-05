//! Handing a project over: the owner offers it to one of its admins, the
//! admin takes it into an organization they own, and the project changes
//! organization in one transaction. Every guard around that move — who may
//! offer, to whom, where it may land, what ends an offer, what comes with the
//! project and what stays behind — who is mailed each change, and its race
//! with a removal.
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
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::test_provider::{
    RecordingNotifications, ScriptedProvider, SiblingCall, as_person,
};
use telmoni_auth::{AppState, Config, Siblings, router};
use telmoni_shared::mail::{Mail, MailError, MailSender, NoopSender};
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{OrganizationId, UserId};

const SERVICE_SECRET: &str = "test-service-secret";
const OWNER: &str = "user_handover_owner";
const ADMIN: &str = "user_handover_admin";
const MEMBER: &str = "user_handover_member";
/// What the owner calls the project.
const PROJECT_NAME: &str = "Payments";

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
    build(pool, Arc::new(NoopSender), None)
}

fn app_mailing(pool: PgPool, sender: Arc<dyn MailSender>) -> Router {
    build(pool, sender, None)
}

/// The router, with its mail transport and, when given, a notifications
/// module to purge a moved project's connectors in.
fn build(
    pool: PgPool,
    sender: Arc<dyn MailSender>,
    notifications: Option<Arc<RecordingNotifications>>,
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
    let db = service_pool(&pool, "auth");
    let state = Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(Arc::new(
            ScriptedProvider::new(),
        ))),
        db,
        config,
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(sender)),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
        siblings: Siblings {
            notifications: notifications.map(|n| n as Arc<dyn telmoni_shared::seam::Notifications>),
            agent: None,
            purge_hook: None,
        },
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

/// One request as `caller`, standing in `organization` when given, into an
/// app of the caller's choosing, under a bearer minted from `pool`.
#[expect(
    clippy::too_many_arguments,
    reason = "every part of the request the console sends, and the pool its bearer comes from"
)]
async fn send(
    app: Router,
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    organization: Option<&str>,
    project: Option<&str>,
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
    if let Some(project) = project {
        req = req.header("x-project-id", project);
    }
    let req = as_person(req, pool, caller)
        .await
        .body(Body::from(body.unwrap_or_else(|| json!({})).to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    (status, json_body(resp).await)
}

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
        None,
        body,
    )
    .await
}

/// Sign somebody in for the first time; answers the id of the organization
/// they were provisioned with.
async fn sign_in(pool: &PgPool, user: &str) -> String {
    seed_identity(pool, user, &format!("{user}@example.test")).await;
    let (status, body) = call(pool, "POST", "/me", user, None, None).await;
    assert_eq!(status, StatusCode::OK, "sign-in failed for {user}: {body}");
    body["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned()
}

/// `caller` makes a project called `name` in `organization`, which sign-in
/// does not; answers its id.
async fn create_project(pool: &PgPool, caller: &str, organization: &str, name: &str) -> String {
    let (status, body) = call(
        pool,
        "POST",
        "/internal/projects",
        caller,
        Some(organization),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "creating {name}: {body}");
    body["id"].as_str().expect("the project's id").to_owned()
}

/// The organization a project is in now.
async fn organization_of_project(pool: &PgPool, project: &str) -> String {
    sqlx::query_scalar("SELECT organization_id FROM auth.projects WHERE external_id = $1")
        .bind(project)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The owner names the project.
async fn rename(pool: &PgPool, organization: &str, project: &str, name: &str) {
    let (status, body) = call(
        pool,
        "PATCH",
        &format!("/internal/projects/{project}"),
        OWNER,
        Some(organization),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "renaming {project}: {body}");
}

/// Seat `user` on `project` at `role` the way the product does: the owner
/// invites, they spend the link.
async fn seat(pool: &PgPool, organization: &str, project: &str, user: &str, role: &str) {
    let (status, body) = call(
        pool,
        "POST",
        &format!("/internal/projects/{project}/invites"),
        OWNER,
        Some(organization),
        Some(json!({ "email": format!("{user}@example.test"), "role": role })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite {user}: {body}");
    let secret = body["link"]
        .as_str()
        .expect("the link")
        .rsplit('/')
        .next()
        .expect("a link with a segment")
        .to_owned();
    let (status, body) = call(
        pool,
        "POST",
        "/internal/invites/accept",
        user,
        None,
        Some(json!({ "token": secret })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "accept for {user}: {body}");
}

/// The three people and the project between them.
struct Handover {
    /// The owner's organization, which the project starts in.
    source: String,
    project: String,
    /// The organization the admin was provisioned with — where the project
    /// lands.
    admins: String,
    /// The member's own organization, which nothing here may land in.
    members: String,
}

/// The owner's project, named, with an admin and a member seated on it, each
/// of whom owns an organization of their own.
async fn staffed_project(pool: &PgPool) -> Handover {
    apply_audit_migrations(pool).await;
    let source = sign_in(pool, OWNER).await;
    let project = create_project(pool, OWNER, &source, PROJECT_NAME).await;
    let admins = sign_in(pool, ADMIN).await;
    let members = sign_in(pool, MEMBER).await;
    seat(pool, &source, &project, ADMIN, "admin").await;
    seat(pool, &source, &project, MEMBER, "member").await;
    Handover {
        source,
        project,
        admins,
        members,
    }
}

async fn offer(
    pool: &PgPool,
    h: &Handover,
    caller: &str,
    organization: &str,
    to: &str,
) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        &format!("/internal/projects/{}/transfer", h.project),
        caller,
        Some(organization),
        Some(json!({ "memberId": to })),
    )
    .await
}

async fn accept(
    pool: &PgPool,
    h: &Handover,
    caller: &str,
    organization: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        &format!("/internal/projects/{}/transfer/accept", h.project),
        caller,
        Some(organization),
        body,
    )
    .await
}

async fn decline(
    pool: &PgPool,
    h: &Handover,
    caller: &str,
    organization: &str,
) -> (StatusCode, Value) {
    call(
        pool,
        "POST",
        &format!("/internal/projects/{}/transfer/decline", h.project),
        caller,
        Some(organization),
        None,
    )
    .await
}

async fn cancel(
    pool: &PgPool,
    h: &Handover,
    caller: &str,
    organization: &str,
) -> (StatusCode, Value) {
    call(
        pool,
        "DELETE",
        &format!("/internal/projects/{}/transfer", h.project),
        caller,
        Some(organization),
        None,
    )
    .await
}

/// A person's seat on the project: its role, and whether it carries an offer.
async fn seat_of(pool: &PgPool, project: &str, user: &str) -> Option<(String, bool)> {
    sqlx::query_as(
        "SELECT role, transfer_offered_at IS NOT NULL FROM auth.project_members
          WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project)
    .bind(user)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// A person's row on an organization's roster.
async fn roster_role(pool: &PgPool, organization: &str, user: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT role FROM auth.organization_members WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(organization)
    .bind(user)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// The handover rows on one organization's chain for the project, oldest
/// first, by kind.
async fn chain(pool: &PgPool, organization: &str, project: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT metadata->>'kind' FROM audit.events
          WHERE organization_id = $1 AND in_project = $2
            AND (metadata->>'kind' LIKE 'project\\_%'
                 OR metadata->>'kind' IN ('seat_folded_into_ownership', 'previous_owner_seated'))
          ORDER BY seq",
    )
    .bind(organization)
    .bind(project)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Offers on the project's seats, live or lapsed.
async fn offers_on_project(pool: &PgPool, project: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM auth.project_members
          WHERE project_id = $1 AND transfer_offered_at IS NOT NULL",
    )
    .bind(project)
    .fetch_one(pool)
    .await
    .unwrap()
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

/// Mark an organization pending deletion straight in the table, as the
/// deletion lane would, or bring it back.
async fn set_pending(pool: &PgPool, organization: &str, pending: bool) {
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

/// Who each mail went to, and what it was about, in the order sent.
fn addressed(sent: &[Mail]) -> Vec<(String, String)> {
    sent.iter()
        .map(|m| (m.to.clone(), m.subject.clone()))
        .collect()
}

#[sqlx::test]
async fn only_the_owner_offers_and_only_to_a_project_admin(pool: PgPool) {
    let h = staffed_project(&pool).await;

    let (status, body) = offer(&pool, &h, ADMIN, &h.admins, MEMBER).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an admin handed the project over: {body}"
    );

    let (status, body) = offer(&pool, &h, OWNER, &h.source, MEMBER).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a plain member was offered the project: {body}"
    );

    let (status, body) = offer(&pool, &h, OWNER, &h.source, OWNER).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = offer(&pool, &h, OWNER, &h.source, "user_nobody_here").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = offer(&pool, &h, OWNER, &h.source, ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["offeredTo"], ADMIN);
    // The address the console's live notice goes to comes from the roster,
    // never from the browser that asked.
    assert_eq!(body["offeredToEmail"], format!("{ADMIN}@example.test"));
    assert_eq!(body["ownerOrganizationId"], h.source);
    assert_eq!(
        organization_of_project(&pool, &h.project).await,
        h.source,
        "an offer changes nothing until it is accepted"
    );
    assert_eq!(
        seat_of(&pool, &h.project, ADMIN).await,
        Some(("admin".to_owned(), true))
    );
}

/// The whole handover: the project changes organization with its keys and
/// its seats, the new owner's seat is folded into the ownership, the
/// previous owner is seated as an admin and enrolled as a member, both chains
/// record it, and no organization is minted or lost.
#[sqlx::test]
async fn accepting_moves_the_project_with_its_keys_and_seats_and_both_chains_record_it(
    pool: PgPool,
) {
    let h = staffed_project(&pool).await;
    let (status, minted) = send(
        app(pool.clone()),
        &pool,
        "POST",
        "/internal/tokens",
        OWNER,
        Some(&h.source),
        Some(&h.project),
        Some(json!({ "name": "ci", "created_by": OWNER })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let organizations_before: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.organizations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );

    let (status, body) = accept(&pool, &h, MEMBER, &h.members, None).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "somebody who holds no offer accepted it: {body}"
    );

    let (status, body) = accept(&pool, &h, ADMIN, &h.admins, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["projectId"], h.project);
    assert_eq!(body["organizationId"], h.admins);
    assert_eq!(body["previousOrganizationId"], h.source);
    assert_eq!(body["previousOwner"], OWNER);
    assert_eq!(body["name"], PROJECT_NAME);

    assert_eq!(organization_of_project(&pool, &h.project).await, h.admins);
    let renamed: bool = sqlx::query_scalar(
        "SELECT metadata ? 'renamed_from' FROM audit.events
          WHERE organization_id = $1 AND metadata ->> 'kind' = 'project_received'",
    )
    .bind(&h.admins)
    .fetch_one(&pool)
    .await
    .expect("read the received row");
    assert!(!renamed, "a name that did not clash records no rename");
    let key_organization: String =
        sqlx::query_scalar("SELECT organization_id FROM auth.api_tokens WHERE id::text = $1")
            .bind(minted["id"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(key_organization, h.admins, "the key stayed behind");
    assert_eq!(
        seat_of(&pool, &h.project, ADMIN).await,
        None,
        "the new owner still holds a seat"
    );
    assert_eq!(
        seat_of(&pool, &h.project, OWNER).await,
        Some(("admin".to_owned(), false))
    );
    assert_eq!(
        seat_of(&pool, &h.project, MEMBER).await,
        Some(("member".to_owned(), false)),
        "a seat that was neither side's did not travel"
    );
    assert_eq!(
        roster_role(&pool, &h.admins, OWNER).await.as_deref(),
        Some("member"),
        "the previous owner was seated without a roster row to reach it from"
    );
    assert_eq!(
        roster_role(&pool, &h.admins, MEMBER).await.as_deref(),
        Some("member"),
        "a seat holder was left off the roster the seat is reached from"
    );
    assert_eq!(
        roster_role(&pool, &h.source, MEMBER).await.as_deref(),
        Some("member"),
        "the move took a seat holder off the roster it left"
    );
    assert_eq!(offers_on_project(&pool, &h.project).await, 0);
    let organizations_after: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.organizations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(organizations_after, organizations_before);

    assert_eq!(
        chain(&pool, &h.source, &h.project).await,
        vec!["project_offered", "project_transferred"]
    );
    assert_eq!(
        chain(&pool, &h.admins, &h.project).await,
        vec![
            "project_received",
            "seat_folded_into_ownership",
            "previous_owner_seated"
        ]
    );
    let enrolled: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'role' FROM audit.events
          WHERE organization_id = $1 AND metadata->>'kind' = 'previous_owner_enrolled'",
    )
    .bind(&h.admins)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(enrolled.as_deref(), Some("member"));
    let holders: Vec<(String, String)> = sqlx::query_as(
        "SELECT resource_id, metadata->>'seat' FROM audit.events
          WHERE organization_id = $1 AND metadata->>'kind' = 'seat_holder_enrolled'",
    )
    .bind(&h.admins)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(holders, vec![(MEMBER.to_owned(), "member".to_owned())]);

    // The roster reads the same thing the tables do, for the new owner —
    // every seat holder included, whose identity the roster joins through
    // the destination's roster row.
    let (status, body) = call(
        &pool,
        "GET",
        &format!("/internal/projects/{}/members", h.project),
        ADMIN,
        Some(&h.admins),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let members = body["members"].as_array().unwrap();
    assert_eq!(members[0]["member_id"], ADMIN);
    assert_eq!(members[0]["is_owner"], true);
    let previous = members
        .iter()
        .find(|m| m["member_id"] == OWNER)
        .expect("the previous owner is on the roster");
    assert_eq!(previous["role"], "admin");
    assert_eq!(previous["is_owner"], false);
    let holder = members
        .iter()
        .find(|m| m["member_id"] == MEMBER)
        .expect("a seat holder is on the roster the new owner reads");
    assert_eq!(holder["role"], "member");

    // Each side's listing: the project is the new owner's now, at owner, and
    // the previous owner reaches it in the organization it moved to, at
    // admin; their own organization no longer lists it.
    let (_, body) = call(
        &pool,
        "GET",
        "/internal/projects",
        ADMIN,
        Some(&h.admins),
        None,
    )
    .await;
    let listed = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == h.project)
        .expect("the new owner lists the project");
    assert_eq!(listed["role"], "owner");
    let (_, body) = call(
        &pool,
        "GET",
        "/internal/projects",
        OWNER,
        Some(&h.admins),
        None,
    )
    .await;
    let listed = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == h.project)
        .expect("the previous owner reaches the project where it went");
    assert_eq!(listed["role"], "admin");
    let (_, body) = call(
        &pool,
        "GET",
        "/internal/projects",
        OWNER,
        Some(&h.source),
        None,
    )
    .await;
    assert!(
        body["projects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["id"] != h.project),
        "the project is still listed where it left: {body}"
    );
    let (_, body) = call(&pool, "POST", "/me", OWNER, None, None).await;
    let seats: Vec<(String, String, String)> = body["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["projectId"].as_str().unwrap().to_owned(),
                s["organizationId"].as_str().unwrap().to_owned(),
                s["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        seats,
        vec![(h.project.clone(), h.admins.clone(), "admin".to_owned())],
        "{body}"
    );
    // A seat holder reaches the project where it went, too: `/me` lists a
    // seat only in an organization the person is in.
    let (_, body) = call(&pool, "POST", "/me", MEMBER, None, None).await;
    let seats: Vec<(String, String, String)> = body["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["projectId"].as_str().unwrap().to_owned(),
                s["organizationId"].as_str().unwrap().to_owned(),
                s["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        seats,
        vec![(h.project.clone(), h.admins.clone(), "member".to_owned())],
        "{body}"
    );
}

/// Where the project lands is the acceptor's call among the organizations
/// they own: the one they own when there is one; named when there are
/// several, and only one of theirs; nowhere when they own none.
#[sqlx::test]
async fn the_destination_is_one_the_acceptor_owns(pool: PgPool) {
    let h = staffed_project(&pool).await;
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );

    set_pending(&pool, &h.admins, true).await;
    let (status, body) = accept(&pool, &h, ADMIN, &h.admins, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("own no organization"),
        "{body}"
    );
    set_pending(&pool, &h.admins, false).await;

    let second = OrganizationId::new();
    sqlx::query("INSERT INTO auth.organizations (external_id, slug, name) VALUES ($1, 'org-' || md5($1), 'Second')")
        .bind(second.as_str())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
         VALUES ($1, $2, 'owner', $2)",
    )
    .bind(second.as_str())
    .bind(ADMIN)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = accept(&pool, &h, ADMIN, &h.admins, None).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "owning two, the acceptor was not asked which: {body}"
    );
    let (status, body) = accept(
        &pool,
        &h,
        ADMIN,
        &h.admins,
        Some(json!({ "organizationId": h.members })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the project landed in somebody else's organization: {body}"
    );
    assert_eq!(organization_of_project(&pool, &h.project).await, h.source);

    let (status, body) = accept(
        &pool,
        &h,
        ADMIN,
        &h.admins,
        Some(json!({ "organizationId": second.as_str() })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        organization_of_project(&pool, &h.project).await,
        second.as_str()
    );
}

/// Two organizations naming a project alike is the common case, not the odd
/// one: the project lands under the first free numbered name, judged
/// case-insensitively as the index judges, and the destination's own projects
/// keep their names.
#[sqlx::test]
async fn a_name_already_taken_in_the_destination_lands_numbered(pool: PgPool) {
    let h = staffed_project(&pool).await;
    let own = create_project(&pool, ADMIN, &h.admins, PROJECT_NAME).await;
    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        ADMIN,
        Some(&h.admins),
        Some(json!({ "name": "PAYMENTS 2" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );

    let (status, body) = accept(&pool, &h, ADMIN, &h.admins, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Payments 3", "{body}");
    assert_eq!(organization_of_project(&pool, &h.project).await, h.admins);

    let name_of = |project: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT name FROM auth.projects WHERE external_id = $1")
                .bind(project)
                .fetch_one(&pool)
                .await
                .expect("read the project's name")
        }
    };
    assert_eq!(name_of(h.project.clone()).await, "Payments 3");
    assert_eq!(name_of(own).await, PROJECT_NAME);
    assert_eq!(
        name_of(created["id"].as_str().expect("id").to_owned()).await,
        "PAYMENTS 2"
    );

    let renamed_from: Option<String> = sqlx::query_scalar(
        "SELECT metadata ->> 'renamed_from' FROM audit.events
          WHERE organization_id = $1 AND metadata ->> 'kind' = 'project_received'",
    )
    .bind(&h.admins)
    .fetch_one(&pool)
    .await
    .expect("read the received row");
    assert_eq!(renamed_from.as_deref(), Some(PROJECT_NAME));
}

/// Hand `h.project`, named `name`, to the admin, whose organization has also
/// been given `held`; answers the name it landed under.
async fn land(pool: &PgPool, h: &Handover, name: &str, held: &[&str]) -> Value {
    rename(pool, &h.source, &h.project, name).await;
    for project in held {
        let (status, created) = call(
            pool,
            "POST",
            "/internal/projects",
            ADMIN,
            Some(&h.admins),
            Some(json!({ "name": project })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
    }
    assert_eq!(
        offer(pool, h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, body) = accept(pool, h, ADMIN, &h.admins, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["name"].clone()
}

/// "Payments 2" arriving where "Payments" and a "2" already are carries the
/// series on, rather than becoming "Payments 2 2".
#[sqlx::test]
async fn a_numbered_name_continues_its_series(pool: PgPool) {
    let h = staffed_project(&pool).await;
    let landed = land(&pool, &h, "Payments 2", &["Payments", "Payments 2"]).await;
    assert_eq!(landed, "Payments 3");
}

/// A number that is part of the name is left alone: with no "Project" in the
/// destination, "Project 2024" is not a series, and moving on to 2025 would
/// say a different year.
#[sqlx::test]
async fn a_number_that_is_part_of_the_name_is_not_a_series(pool: PgPool) {
    let h = staffed_project(&pool).await;
    let landed = land(&pool, &h, "Project 2024", &["Project 2024"]).await;
    assert_eq!(landed, "Project 2024 2");
}

#[sqlx::test]
async fn demoting_or_removing_the_recipient_ends_the_offer(pool: PgPool) {
    let h = staffed_project(&pool).await;
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );

    let role_path = format!("/internal/projects/{}/members/{ADMIN}/role", h.project);
    let (status, body) = call(
        &pool,
        "PUT",
        &role_path,
        OWNER,
        Some(&h.source),
        Some(json!({ "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        accept(&pool, &h, ADMIN, &h.admins, None).await.0,
        StatusCode::CONFLICT
    );

    let (status, body) = call(
        &pool,
        "PUT",
        &role_path,
        OWNER,
        Some(&h.source),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        accept(&pool, &h, ADMIN, &h.admins, None).await.0,
        StatusCode::CONFLICT,
        "promoting them back revived an offer the demotion ended"
    );

    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, _) = call(
        &pool,
        "DELETE",
        &format!("/internal/projects/{}/members/{ADMIN}", h.project),
        OWNER,
        Some(&h.source),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(offers_on_project(&pool, &h.project).await, 0);
    assert_eq!(organization_of_project(&pool, &h.project).await, h.source);
}

#[sqlx::test]
async fn an_offer_older_than_seven_days_is_dead(pool: PgPool) {
    let h = staffed_project(&pool).await;
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );
    sqlx::query(
        "UPDATE auth.project_members SET transfer_offered_at = now() - interval '8 days'
          WHERE project_id = $1 AND user_id = $2",
    )
    .bind(&h.project)
    .bind(ADMIN)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = accept(&pool, &h, ADMIN, &h.admins, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(organization_of_project(&pool, &h.project).await, h.source);

    let (_, body) = call(
        &pool,
        "GET",
        &format!("/internal/projects/{}/members", h.project),
        OWNER,
        Some(&h.source),
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
        admin_row["transfer_offer_expires_at"],
        Value::Null,
        "a lapsed offer is still listed as live"
    );

    // Declined by nobody, as it is withdrawn by nobody: the chain records only
    // the offer the owner made.
    let (status, body) = decline(&pool, &h, ADMIN, &h.admins).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = cancel(&pool, &h, OWNER, &h.source).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        chain(&pool, &h.source, &h.project).await,
        vec!["project_offered"]
    );
}

#[sqlx::test]
async fn a_declined_or_withdrawn_offer_cannot_be_accepted(pool: PgPool) {
    let h = staffed_project(&pool).await;

    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, body) = decline(&pool, &h, ADMIN, &h.admins).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ownerOrganizationId"], h.source);
    assert_eq!(
        accept(&pool, &h, ADMIN, &h.admins, None).await.0,
        StatusCode::CONFLICT
    );

    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, body) = cancel(&pool, &h, ADMIN, &h.admins).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the recipient withdrew the owner's offer: {body}"
    );
    let (status, body) = cancel(&pool, &h, OWNER, &h.source).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["offeredTo"], ADMIN);
    assert_eq!(body["offeredToEmail"], format!("{ADMIN}@example.test"));
    assert_eq!(
        accept(&pool, &h, ADMIN, &h.admins, None).await.0,
        StatusCode::CONFLICT
    );

    assert_eq!(organization_of_project(&pool, &h.project).await, h.source);
    assert_eq!(
        chain(&pool, &h.source, &h.project).await,
        vec![
            "project_offered",
            "project_offer_declined",
            "project_offered",
            "project_offer_cancelled",
        ]
    );
}

#[sqlx::test]
async fn a_second_offer_replaces_the_first(pool: PgPool) {
    let h = staffed_project(&pool).await;
    let second = "user_handover_second_admin";
    let seconds = sign_in(&pool, second).await;
    seat(&pool, &h.source, &h.project, second, "admin").await;

    let (status, body) = offer(&pool, &h, OWNER, &h.source, ADMIN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["withdrawnFromEmail"],
        Value::Null,
        "nothing was replaced"
    );
    let (status, body) = offer(&pool, &h, OWNER, &h.source, second).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Whoever held the replaced offer is named, so their console can be told.
    assert_eq!(body["withdrawnFromEmail"], format!("{ADMIN}@example.test"));
    let replaces: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'replaces' FROM audit.events
          WHERE organization_id = $1 AND metadata->>'kind' = 'project_offered'
          ORDER BY seq DESC LIMIT 1",
    )
    .bind(&h.source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(replaces.as_deref(), Some(ADMIN));

    assert_eq!(
        accept(&pool, &h, ADMIN, &h.admins, None).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        accept(&pool, &h, second, &seconds, None).await.0,
        StatusCode::OK
    );
    assert_eq!(organization_of_project(&pool, &h.project).await, seconds);
}

/// Each side hears the other's move with no console open: the admin is
/// mailed the offer, its withdrawal and its replacement; the owner the
/// answer. A refused answer mails nobody, and a mail the provider refuses
/// undoes nothing.
#[sqlx::test]
async fn every_step_is_mailed_to_the_other_side(pool: PgPool) {
    let h = staffed_project(&pool).await;
    let outbox = Outbox::default();
    let mailing = || app_mailing(pool.clone(), Arc::new(outbox.clone()));
    let transfer = format!("/internal/projects/{}/transfer", h.project);

    let (status, body) = send(
        mailing(),
        &pool,
        "POST",
        &transfer,
        OWNER,
        Some(&h.source),
        None,
        Some(json!({ "memberId": ADMIN })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = addressed(&outbox.take());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, format!("{ADMIN}@example.test"));
    assert!(
        sent[0].1.contains("wants to hand you the project Payments"),
        "{sent:?}"
    );

    let (status, _) = send(
        mailing(),
        &pool,
        "POST",
        &format!("{transfer}/accept"),
        MEMBER,
        Some(&h.members),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(outbox.take().is_empty(), "a refused accept mailed somebody");

    let (status, _) = send(
        mailing(),
        &pool,
        "POST",
        &format!("{transfer}/decline"),
        ADMIN,
        Some(&h.admins),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sent = addressed(&outbox.take());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, format!("{OWNER}@example.test"));
    assert!(
        sent[0]
            .1
            .contains("declined to take over the project Payments"),
        "{sent:?}"
    );

    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, _) = send(
        mailing(),
        &pool,
        "DELETE",
        &transfer,
        OWNER,
        Some(&h.source),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sent = addressed(&outbox.take());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, format!("{ADMIN}@example.test"));
    assert!(
        sent[0]
            .1
            .contains("withdrew the offer of the project Payments"),
        "{sent:?}"
    );

    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );
    let (status, body) = send(
        mailing(),
        &pool,
        "POST",
        &format!("{transfer}/accept"),
        ADMIN,
        Some(&h.admins),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = addressed(&outbox.take());
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0].0,
        format!("{OWNER}@example.test"),
        "the previous owner is not the one told"
    );
    assert!(
        sent[0].1.contains("now owns the project Payments"),
        "{sent:?}"
    );
}

/// A project in an organization on its way out cannot change hands, and one
/// cannot land in an organization on its way out either.
#[sqlx::test]
async fn an_organization_being_deleted_neither_hands_a_project_over_nor_takes_one(pool: PgPool) {
    let h = staffed_project(&pool).await;
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );

    set_pending(&pool, &h.source, true).await;
    for (status, body) in [
        accept(&pool, &h, ADMIN, &h.admins, None).await,
        offer(&pool, &h, OWNER, &h.source, ADMIN).await,
        decline(&pool, &h, ADMIN, &h.admins).await,
    ] {
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(
            body["detail"], "this organization is being deleted",
            "{body}"
        );
    }
    set_pending(&pool, &h.source, false).await;

    set_pending(&pool, &h.admins, true).await;
    for body in [None, Some(json!({ "organizationId": h.admins }))] {
        let (status, body) = accept(&pool, &h, ADMIN, &h.admins, body).await;
        assert!(status.is_client_error(), "{body}");
    }
    assert_eq!(organization_of_project(&pool, &h.project).await, h.source);
    assert_eq!(
        seat_of(&pool, &h.project, ADMIN).await,
        Some(("admin".to_owned(), true))
    );
}

/// ⚠ **Somebody on their way out cannot take a project on.** Account
/// deletion reads what the person owns once, under their lock; an accept
/// that landed after that read would leave a project in an organization
/// whose owner can never sign in to hand it on. The accept queues on the
/// same lock and is refused under it.
#[sqlx::test]
async fn an_accept_queued_behind_the_acceptors_account_deletion_is_refused(pool: PgPool) {
    let h = staffed_project(&pool).await;
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
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
        let (project, admins) = (h.project.clone(), h.admins.clone());
        tokio::spawn(async move {
            call(
                &pool,
                "POST",
                &format!("/internal/projects/{project}/transfer/accept"),
                ADMIN,
                Some(&admins),
                None,
            )
            .await
        })
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
    assert_eq!(organization_of_project(&pool, &h.project).await, h.source);
    assert_eq!(
        seat_of(&pool, &h.project, ADMIN).await,
        Some(("admin".to_owned(), true))
    );
}

/// `/me` lists an open offer to whoever holds it, labelled by the project and
/// the organization it would leave, and to nobody else; once answered it is
/// gone.
#[sqlx::test]
async fn me_lists_the_offer_to_its_holder_and_to_nobody_else(pool: PgPool) {
    let h = staffed_project(&pool).await;
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );

    let (status, body) = call(&pool, "POST", "/me", ADMIN, None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let offers = body["projectOffers"].as_array().unwrap();
    assert_eq!(offers.len(), 1, "{body}");
    assert_eq!(offers[0]["projectId"], h.project);
    assert_eq!(offers[0]["name"], PROJECT_NAME);
    assert_eq!(offers[0]["organizationId"], h.source);
    assert_eq!(offers[0]["ownerEmail"], format!("{OWNER}@example.test"));
    assert!(offers[0]["expiresAt"].is_string(), "{body}");

    for who in [OWNER, MEMBER] {
        let (_, body) = call(&pool, "POST", "/me", who, None, None).await;
        assert_eq!(
            body["projectOffers"].as_array().unwrap().len(),
            0,
            "{who} was shown an offer that is not theirs: {body}"
        );
    }

    assert_eq!(
        accept(&pool, &h, ADMIN, &h.admins, None).await.0,
        StatusCode::OK
    );
    let (_, body) = call(&pool, "POST", "/me", ADMIN, None, None).await;
    assert_eq!(body["projectOffers"].as_array().unwrap().len(), 0, "{body}");
}

/// An accept and a removal of the same admin, fired together: whichever
/// wins, the project is in exactly one organization, its seat spent or
/// removed exactly once, and never moved out from under a removal that
/// already committed.
#[sqlx::test]
async fn accept_racing_a_removal_leaves_the_project_in_exactly_one_place(pool: PgPool) {
    let h = staffed_project(&pool).await;
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );

    let accepting = {
        let pool = pool.clone();
        let (project, admins) = (h.project.clone(), h.admins.clone());
        tokio::spawn(async move {
            call(
                &pool,
                "POST",
                &format!("/internal/projects/{project}/transfer/accept"),
                ADMIN,
                Some(&admins),
                None,
            )
            .await
            .0
        })
    };
    let removing = {
        let pool = pool.clone();
        let (project, source) = (h.project.clone(), h.source.clone());
        tokio::spawn(async move {
            call(
                &pool,
                "DELETE",
                &format!("/internal/projects/{project}/members/{ADMIN}"),
                OWNER,
                Some(&source),
                None,
            )
            .await
            .0
        })
    };
    let (accepted, removed) = (accepting.await.unwrap(), removing.await.unwrap());

    let seat = seat_of(&pool, &h.project, ADMIN).await;
    match (accepted, removed) {
        (StatusCode::OK, _) => {
            assert_eq!(organization_of_project(&pool, &h.project).await, h.admins);
            assert_ne!(
                removed,
                StatusCode::NO_CONTENT,
                "the transfer committed and the removal still went through"
            );
            assert_eq!(seat, None);
        }
        (_, StatusCode::NO_CONTENT) => {
            assert_eq!(organization_of_project(&pool, &h.project).await, h.source);
            assert_eq!(seat, None);
        }
        other => panic!("neither the accept nor the removal succeeded: {other:?}"),
    }
}

/// ⚠ **A stranger cannot stand in an organization's queue.** The project's id
/// arrives from the caller, and a lock is a queue: membership is checked
/// before any lock is taken, so somebody outside the project is refused at
/// once while its organization's roster is busy.
#[sqlx::test]
async fn a_stranger_is_refused_without_queueing_on_the_organizations_lock(pool: PgPool) {
    let h = staffed_project(&pool).await;
    let stranger = "user_handover_stranger";
    let strangers = sign_in(&pool, stranger).await;

    let mut busy = pool.begin().await.unwrap();
    telmoni_auth::db::locks::lock_organization(
        &mut busy,
        &OrganizationId::try_new(&h.source).unwrap(),
    )
    .await
    .unwrap();
    let transfer = format!("/internal/projects/{}/transfer", h.project);
    for (method, path, body) in [
        ("POST", transfer.clone(), Some(json!({ "memberId": ADMIN }))),
        ("DELETE", transfer.clone(), None),
        ("POST", format!("{transfer}/decline"), None),
        ("POST", format!("{transfer}/accept"), None),
    ] {
        let answered = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            call(&pool, method, &path, stranger, Some(&strangers), body),
        )
        .await;
        let (status, body) = answered.expect("the stranger queued on the organization's lock");
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
    busy.rollback().await.unwrap();
}

/// ⚠ **The connectors are purged before the move, and a purge that fails
/// moves nothing.** They are the old organization's Slack and Discord grants
/// and webhook secrets; a project arriving with them would post the new
/// owner's events into the old owner's channels. Notifications is asked in
/// process, for the project, and only a purge that answered is followed by
/// the commit.
#[sqlx::test]
async fn the_connectors_are_purged_before_the_move_and_a_refused_purge_moves_nothing(pool: PgPool) {
    let h = staffed_project(&pool).await;
    let calls = telmoni_auth::test_provider::SiblingCalls::default();
    let notifications = Arc::new(RecordingNotifications::new(calls.clone()));
    let purging = || {
        build(
            pool.clone(),
            Arc::new(NoopSender),
            Some(notifications.clone()),
        )
    };
    let accept_path = format!("/internal/projects/{}/transfer/accept", h.project);
    assert_eq!(
        offer(&pool, &h, OWNER, &h.source, ADMIN).await.0,
        StatusCode::OK
    );

    notifications.on_purge_project(Err(telmoni_shared::TelmoniError::Internal(
        "notifications is down".into(),
    )));
    let (status, body) = send(
        purging(),
        &pool,
        "POST",
        &accept_path,
        ADMIN,
        Some(&h.admins),
        None,
        None,
    )
    .await;
    assert!(
        status.is_server_error(),
        "the project moved past a refused purge: {status} {body}"
    );
    assert_eq!(
        calls.calls(),
        [SiblingCall::PurgeProject(h.project.clone())],
        "the purge was asked for the project, once"
    );
    assert_eq!(organization_of_project(&pool, &h.project).await, h.source);
    assert_eq!(
        seat_of(&pool, &h.project, ADMIN).await,
        Some(("admin".to_owned(), true)),
        "the refused accept spent the offer"
    );
    assert_eq!(
        chain(&pool, &h.admins, &h.project).await,
        Vec::<String>::new()
    );
    calls.reset();

    let (status, body) = send(
        purging(),
        &pool,
        "POST",
        &accept_path,
        ADMIN,
        Some(&h.admins),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        calls.calls(),
        [SiblingCall::PurgeProject(h.project.clone())],
        "the purge ran before the move"
    );
    assert_eq!(organization_of_project(&pool, &h.project).await, h.admins);
}
