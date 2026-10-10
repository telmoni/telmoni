//! `GET /internal/organization/export` — the record an owner or an admin
//! downloads.

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

use telmoni_auth::test_provider::{ScriptedProvider, as_person, bearer_in};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};

const SERVICE_SECRET: &str = "test-service-secret";
const OWNER: &str = "user_export_owner";
/// The session the owner's sign-in belongs to; its id must never leave in
/// the export.
const PROVIDER_SID: &str = "session_abc123";

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

/// The organization `/me` made the caller active in — on a first sign-in, the
/// one it just provisioned.
fn active_organization(me: &Value) -> String {
    me.get("activeOrganizationId")
        .and_then(Value::as_str)
        .expect("/me names the active organization")
        .to_owned()
}

/// Sign in, which provisions the organization, and answer its id.
async fn sign_in(pool: &PgPool, user: &str) -> String {
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
    active_organization(&json_body(resp).await)
}

/// The sign-in that records a session: the bearer belongs to session `sid`
/// and the console passes the browser's user agent along.
async fn sign_in_from_a_browser(pool: &PgPool, user: &str, sid: &str, user_agent: &str) -> String {
    seed_identity(pool, user, &format!("{user}@example.test")).await;
    let body = json!({ "userAgent": user_agent });
    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/me")
                .header("x-service-secret", SERVICE_SECRET)
                .header(
                    "authorization",
                    format!("Bearer {}", bearer_in(pool, user, sid).await),
                )
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "sign-in failed for {user}");
    let me = json_body(resp).await;
    assert!(
        me.get("sessionRowId").is_some_and(Value::is_string),
        "a sign-in records its session's row: {me}"
    );
    active_organization(&me)
}

/// Put `user` on `organization`'s roster at `role`.
async fn seat(pool: &PgPool, organization: &str, user: &str, role: &str) {
    sqlx::query(
        "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(organization)
    .bind(user)
    .bind(role)
    .bind(OWNER)
    .execute(pool)
    .await
    .expect("seat the member");
}

/// A request as the BFF makes it.
async fn call(
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    organization: &str,
    project: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("x-organization-id", organization);
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

/// `caller`'s export of `organization`.
async fn export(pool: &PgPool, caller: &str, organization: &str) -> (StatusCode, Value) {
    call(
        pool,
        "GET",
        "/internal/organization/export",
        caller,
        organization,
        None,
        None,
    )
    .await
}

/// Everything the export is meant to reach, seeded in one tenancy. Answers the
/// owner's organization and its project.
async fn seed(pool: &PgPool) -> (String, String) {
    apply_audit_migrations(pool).await;
    let organization = sign_in_from_a_browser(pool, OWNER, PROVIDER_SID, "Firefox").await;
    let (status, created) = call(
        pool,
        "POST",
        "/internal/projects",
        OWNER,
        &organization,
        None,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "project: {created}");
    let project = created
        .get("id")
        .and_then(Value::as_str)
        .expect("the project's id")
        .to_owned();

    let (status, _) = call(
        pool,
        "POST",
        "/internal/tokens",
        OWNER,
        &organization,
        Some(&project),
        Some(json!({ "name": "deploy key", "created_by": OWNER })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "mint");

    let (status, _) = call(
        pool,
        "POST",
        &format!("/internal/projects/{project}/invites"),
        OWNER,
        &organization,
        Some(&project),
        Some(json!({ "email": "guest@example.test", "role": "member" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite");

    (organization, project)
}

/// Every string in a JSON document, so a value moved to another key still counts.
fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => {
            for item in items {
                strings(item, out);
            }
        }
        Value::Object(map) => {
            for (k, value) in map {
                out.push(k.clone());
                strings(value, out);
            }
        }
        _ => {}
    }
}

/// The export answers with the organization's record, not empty sections,
/// and taking it is on the chain.
#[sqlx::test]
async fn export_carries_the_record(pool: PgPool) {
    let (organization, project) = seed(&pool).await;

    let (status, body) = export(&pool, OWNER, &organization).await;
    assert_eq!(status, StatusCode::OK, "export failed: {body}");

    assert_eq!(
        body["organization"][0]["organization_id"], organization,
        "the organization row"
    );
    assert_eq!(
        body["projects"][0]["project_id"], project,
        "the projects list"
    );
    assert_eq!(
        body["organization_members"][0]["member_id"], OWNER,
        "the roster, which the owner is a row of"
    );
    assert_eq!(body["organization_members"][0]["role"], "owner");

    let detail = &body["project_detail"][0];
    assert_eq!(detail["project_id"], project);
    assert_eq!(detail["api_keys"][0]["name"], "deploy key", "the keys list");
    assert_eq!(
        detail["invites"][0]["email"], "guest@example.test",
        "the invitations list"
    );

    let events = body["audit_events"]
        .as_array()
        .expect("an audit_events array");
    assert!(
        !events.is_empty(),
        "the chain has rows (sign-up, the mint, the invitation) and the export read none"
    );
    let first = &events[0];
    for field in ["created_at", "actor_id", "action", "resource_kind"] {
        assert!(
            first.get(field).is_some_and(|v| !v.is_null()),
            "an audit row without {field}: {first}"
        );
    }
    assert_eq!(body["audit_truncated"], json!(false));

    let exported: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events
          WHERE organization_id = $1 AND action = 'exported' AND actor_id = $2",
    )
    .bind(&organization)
    .bind(OWNER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(exported, 1, "the export itself is recorded once");
}

/// The values that must never leave in a forwardable file: every token hash,
/// the session handle, and every `shard_key`.
#[sqlx::test]
async fn export_redacts_every_secret(pool: PgPool) {
    let (organization, _) = seed(&pool).await;

    let (status, body) = export(&pool, OWNER, &organization).await;
    assert_eq!(status, StatusCode::OK, "export failed: {body}");

    let mut found = Vec::new();
    strings(&body, &mut found);
    for secret in ["token_hash", "provider_sid", "shard_key", PROVIDER_SID] {
        assert!(
            !found.iter().any(|s| s == secret),
            "the export carries {secret}"
        );
    }
}

/// ⚠ **There is no personal export, inside this one either.** The file is the
/// organization's: the account of whoever takes it, their sessions — which
/// span every organization they sign in to — and their conversations with
/// the agent stay out, so a file an admin forwards carries none of them.
#[sqlx::test]
async fn the_export_carries_nothing_of_the_person_taking_it(pool: PgPool) {
    let (organization, _) = seed(&pool).await;

    let (status, body) = export(&pool, OWNER, &organization).await;
    assert_eq!(status, StatusCode::OK, "export failed: {body}");

    for personal in ["person", "sessions", "agent_conversations"] {
        assert!(
            body.get(personal).is_none(),
            "the export carries {personal}"
        );
    }
    // Under whatever key: their address and their session's browser.
    let mut found = Vec::new();
    strings(&body, &mut found);
    for theirs in ["user_export_owner@example.test", "Firefox"] {
        assert!(
            !found.iter().any(|s| s.contains(theirs)),
            "the export carries the owner's {theirs}"
        );
    }
}

/// Somebody outside the organization cannot take its export.
#[sqlx::test]
async fn an_export_cannot_be_taken_for_another_organization(pool: PgPool) {
    let (organization, _) = seed(&pool).await;
    sign_in(&pool, "user_export_other").await;

    let (status, _) = export(&pool, "user_export_other", &organization).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// ⚠ **A member does not take it.** The roster, the pending invitation
/// addresses, the key metadata and the audit chain are for the owner and its
/// admins, who read them in the console already; a member on the roster is
/// refused like a stranger.
#[sqlx::test]
async fn a_member_does_not_take_the_export(pool: PgPool) {
    let (organization, _) = seed(&pool).await;
    let member = "user_export_plain";
    sign_in(&pool, member).await;
    seat(&pool, &organization, member, "member").await;

    let (status, body) = export(&pool, member, &organization).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// An admin takes the organization's whole record, as its owner does, and
/// taking it is on the chain under their name.
#[sqlx::test]
async fn an_admin_takes_the_whole_export(pool: PgPool) {
    let (organization, project) = seed(&pool).await;
    let admin = "user_export_admin";
    sign_in(&pool, admin).await;
    seat(&pool, &organization, admin, "admin").await;
    // Seated by an invitation they accepted, as everyone in another
    // organization is: the invitation is no longer the organization's to
    // list, so it is not in the file. (Its address can still be in the audit
    // chain's details, which the file carries as the console shows them.)
    sqlx::query(
        "INSERT INTO auth.organization_invites
             (organization_id, email, role, token_hash, invited_by, expires_at, accepted_at,
              accepted_by)
         VALUES ($1, 'user_export_admin@example.test', 'admin', 'hash-of-an-accepted-invite',
                 $2, now() + interval '1 day', now(), $3)",
    )
    .bind(&organization)
    .bind(OWNER)
    .bind(admin)
    .execute(&pool)
    .await
    .expect("seed the accepted invitation");

    let (status, body) = export(&pool, admin, &organization).await;
    assert_eq!(status, StatusCode::OK, "export failed: {body}");
    assert!(
        body["organization_invites"]
            .as_array()
            .is_some_and(|invites| invites.is_empty()),
        "an accepted invitation: {body}"
    );

    let roster: Vec<&Value> = body["organization_members"]
        .as_array()
        .expect("an organization_members array")
        .iter()
        .map(|m| &m["member_id"])
        .collect();
    assert!(
        roster.contains(&&json!(OWNER)) && roster.contains(&&json!(admin)),
        "the whole roster: {body}"
    );
    let detail = &body["project_detail"][0];
    assert_eq!(detail["project_id"], project);
    assert_eq!(detail["api_keys"][0]["name"], "deploy key", "the keys list");
    assert_eq!(
        detail["invites"][0]["email"], "guest@example.test",
        "the invitations list"
    );
    assert!(
        body["audit_events"]
            .as_array()
            .is_some_and(|events| !events.is_empty()),
        "the chain: {body}"
    );

    let exported: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events
          WHERE organization_id = $1 AND action = 'exported' AND actor_id = $2",
    )
    .bind(&organization)
    .bind(admin)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        exported, 1,
        "the admin's export is recorded under the admin"
    );
}

/// ⚠ **Being on somebody else's project does not put it in your export.**
/// `auth.projects` lets a member read the projects they were let into, and
/// `auth.organization_members` the rows that name them anywhere, and the export
/// trusted RLS alone — so a member's download listed the other organization's
/// project and then its roster, its pending invitations and its API keys.
#[sqlx::test]
async fn an_export_carries_nothing_from_a_project_the_caller_is_only_a_member_of(pool: PgPool) {
    let (foreign_organization, foreign) = seed(&pool).await;
    let member = "user_export_member";
    let own_organization = sign_in(&pool, member).await;
    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        member,
        &own_organization,
        None,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let own = created["id"].as_str().expect("the project's id").to_owned();
    seat(&pool, &foreign_organization, member, "member").await;
    sqlx::query(
        "INSERT INTO auth.project_members (project_id, user_id, role, added_by)
         VALUES ($1, $2, 'admin', $3)",
    )
    .bind(&foreign)
    .bind(member)
    .bind(OWNER)
    .execute(&pool)
    .await
    .expect("seed the membership");

    let (status, body) = export(&pool, member, &own_organization).await;
    assert_eq!(status, StatusCode::OK, "export failed: {body}");

    let listed: Vec<&Value> = body["projects"]
        .as_array()
        .expect("a projects array")
        .iter()
        .map(|p| &p["project_id"])
        .collect();
    assert_eq!(
        listed,
        vec![&json!(own)],
        "only the member's own project: {body}"
    );
    let roster: Vec<&Value> = body["organization_members"]
        .as_array()
        .expect("an organization_members array")
        .iter()
        .map(|m| &m["member_id"])
        .collect();
    assert_eq!(
        roster,
        vec![&json!(member)],
        "only the roster of the organization exported: {body}"
    );

    let mut found = Vec::new();
    strings(&body, &mut found);
    for leaked in [
        foreign.as_str(),
        foreign_organization.as_str(),
        OWNER,
        "deploy key",
        "guest@example.test",
        "user_export_owner@example.test",
    ] {
        assert!(
            !found.iter().any(|s| s == leaked),
            "the member's export carries the other organization's {leaked}"
        );
    }
}
