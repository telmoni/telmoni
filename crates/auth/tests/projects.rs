//! `POST /internal/projects` — creating a project, and who may.
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

/// Sign somebody in, provisioning the organization they own. Returns its id,
/// as `/me` names it active.
async fn sign_in(pool: &PgPool, user: &str, email: &str) -> String {
    seed_identity(pool, user, email).await;
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
    json_body(resp).await["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned()
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

async fn call(
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    organization: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("x-organization-id", organization);
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

/// The same call, plus the `x-project-id` a project lane needs.
async fn call_project(
    pool: &PgPool,
    method: &str,
    uri: &str,
    caller: &str,
    organization: &str,
    project: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("x-organization-id", organization)
        .header("x-project-id", project);
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

/// Names of the projects `GET /internal/projects` returns for this caller.
async fn project_names(pool: &PgPool, caller: &str, organization: &str) -> Vec<String> {
    let (status, body) = call(
        pool,
        "GET",
        "/internal/projects",
        caller,
        organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect()
}

/// Put `joiner` in `owner`'s organization at `role` through the real invite lane.
async fn join_organization(pool: &PgPool, owner: &str, joiner: &str, email: &str, role: &str) {
    let (status, invite) = call(
        pool,
        "POST",
        "/internal/organization/invites",
        owner,
        &organization_of(pool, owner).await,
        Some(json!({ "email": email, "role": role })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = invite["link"]
        .as_str()
        .expect("invite link")
        .split("/invite/")
        .nth(1)
        .expect("token")
        .to_string();

    let (status, _) = call(
        pool,
        "POST",
        "/internal/invites/accept",
        joiner,
        &organization_of(pool, joiner).await,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[sqlx::test]
async fn an_owner_creates_a_project_and_it_lists(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_01";
    let organization = sign_in(&pool, owner, "projects-owner-01@example.test").await;
    assert_eq!(
        project_names(&pool, owner, &organization).await,
        vec!["Default Project"]
    );

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "  Platform  " })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    assert_eq!(created["name"], "Platform");
    assert_eq!(created["role"], "owner");
    let id = created["id"].as_str().expect("id");
    assert!(
        id.starts_with("project_"),
        "a minted project carries the project prefix, got {id}"
    );

    assert_eq!(
        project_names(&pool, owner, &organization).await,
        vec!["Default Project", "Platform"],
        "the new project is in the switcher's own read"
    );
}

#[sqlx::test]
async fn an_organization_admin_creates_a_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_02";
    let admin = "usr_projects_admin_02";
    let admin_email = "projects-admin-02@example.test";
    let organization = sign_in(&pool, owner, "projects-owner-02@example.test").await;
    sign_in(&pool, admin, admin_email).await;
    join_organization(&pool, owner, admin, admin_email, "admin").await;

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        admin,
        &organization,
        Some(json!({ "name": "Growth" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["role"], "admin");

    assert!(
        project_names(&pool, owner, &organization)
            .await
            .contains(&"Growth".to_string()),
        "the organization sees a project its admin created"
    );
}

/// ⚠ **An admin who CREATES a project must then be able to act on it.**
#[sqlx::test]
async fn an_organization_admin_can_act_on_the_project_they_created(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_07";
    let admin = "usr_projects_admin_07";
    let admin_email = "projects-admin-07@example.test";
    let organization = sign_in(&pool, owner, "projects-owner-07@example.test").await;
    sign_in(&pool, admin, admin_email).await;
    join_organization(&pool, owner, admin, admin_email, "admin").await;

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        admin,
        &organization,
        Some(json!({ "name": "Growth" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().expect("project id").to_owned();

    let (status, _) = call_project(
        &pool,
        "GET",
        "/internal/tokens",
        admin,
        &organization,
        &project_id,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an organization admin cannot reach the project they just created"
    );
}

#[sqlx::test]
async fn an_organization_member_cannot_create_a_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_03";
    let member = "usr_projects_member_03";
    let member_email = "projects-member-03@example.test";
    let organization = sign_in(&pool, owner, "projects-owner-03@example.test").await;
    sign_in(&pool, member, member_email).await;
    join_organization(&pool, owner, member, member_email, "member").await;

    let (status, _) = call(
        &pool,
        "POST",
        "/internal/projects",
        member,
        &organization,
        Some(json!({ "name": "Shadow" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    assert!(
        !project_names(&pool, owner, &organization)
            .await
            .contains(&"Shadow".to_string()),
        "a refused create writes nothing"
    );
}

#[sqlx::test]
async fn a_stranger_cannot_create_a_project_in_an_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_04";
    let stranger = "usr_projects_stranger_04";
    let organization = sign_in(&pool, owner, "projects-owner-04@example.test").await;
    sign_in(&pool, stranger, "projects-stranger-04@example.test").await;

    let (status, _) = call(
        &pool,
        "POST",
        "/internal/projects",
        stranger,
        &organization,
        Some(json!({ "name": "Trespass" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Empty, whitespace-only and over-long names are 400s; the trim comes first.
#[sqlx::test]
async fn a_name_that_is_blank_or_too_long_is_refused(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_05";
    let organization = sign_in(&pool, owner, "projects-owner-05@example.test").await;

    for name in ["", "   "] {
        let (status, _) = call(
            &pool,
            "POST",
            "/internal/projects",
            owner,
            &organization,
            Some(json!({ "name": name })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "blank name {name:?}");
    }

    let (status, _) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "x".repeat(101) })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "é".repeat(100) })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

/// The create is audited, naming the project as resource and as `in_project`.
#[sqlx::test]
async fn creating_a_project_writes_an_audit_row(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_06";
    let organization = sign_in(&pool, owner, "projects-owner-06@example.test").await;

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "Audited" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().expect("id");

    let row: (String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT action, resource_kind, resource_id, in_project
           FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'project' AND resource_id = $2",
    )
    .bind(&organization)
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .expect("one audit row for the create");

    assert_eq!(row.0, "created");
    assert_eq!(row.1, "project");
    assert_eq!(row.2.as_deref(), Some(project_id));
    assert_eq!(row.3.as_deref(), Some(project_id));
}

/// Delete the project as its owner, as the console's Settings page does.
async fn delete_project(
    pool: &PgPool,
    caller: &str,
    organization: &str,
    project_id: &str,
) -> StatusCode {
    call(
        pool,
        "DELETE",
        &format!("/internal/projects/{project_id}"),
        caller,
        organization,
        None,
    )
    .await
    .0
}

/// ⚠ DELETING A PROJECT TAKES ITS KEYS, SEATS AND INVITES WITH IT, by cascade.
#[sqlx::test]
async fn deleting_a_project_takes_its_keys_and_its_seats(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, "user_del_owner", "del-owner@example.com").await;
    sign_in(&pool, "user_del_mate", "del-mate@example.com").await;

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        "user_del_owner",
        &organization,
        Some(json!({ "name": "Doomed" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().expect("id").to_string();

    seat_on_project(
        &pool,
        "user_del_owner",
        &project_id,
        "user_del_mate",
        "del-mate@example.com",
        "admin",
    )
    .await;
    sqlx::query(
        "INSERT INTO auth.api_tokens
             (organization_id, project_id, name, token_hash, created_by, shard_key)
         VALUES ($1, $2, 'doomed key', 'hash-doomed', $3, gen_random_uuid())",
    )
    .bind(&organization)
    .bind(&project_id)
    .bind("user_del_owner")
    .execute(&pool)
    .await
    .expect("the key row");

    assert_eq!(
        delete_project(&pool, "user_del_owner", &organization, &project_id).await,
        StatusCode::OK
    );

    for (table, column) in [
        ("auth.projects", "external_id"),
        ("auth.project_members", "project_id"),
        ("auth.api_tokens", "project_id"),
    ] {
        let left: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {column} = $1"))
                .bind(&project_id)
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(left, 0, "{table} still holds rows for the deleted project");
    }
}

/// ⚠ AND THE LOG KEEPS BOTH ENDS OF THE PROJECT'S LIFE. `audit.events.in_project`
#[sqlx::test]
async fn a_deleted_projects_history_stays_on_the_organizations_chain(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, "user_del_audit", "del-audit@example.com").await;

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        "user_del_audit",
        &organization,
        Some(json!({ "name": "Shortlived" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().expect("id").to_string();

    assert_eq!(
        delete_project(&pool, "user_del_audit", &organization, &project_id).await,
        StatusCode::OK
    );

    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT action, metadata->>'name'
           FROM audit.events
          WHERE organization_id = $1 AND resource_id = $2
          ORDER BY created_at",
    )
    .bind(&organization)
    .bind(&project_id)
    .fetch_all(&pool)
    .await
    .expect("the project's rows");

    assert_eq!(
        rows,
        vec![
            ("created".to_string(), Some("Shortlived".to_string())),
            ("deleted".to_string(), Some("Shortlived".to_string())),
        ],
        "the chain holds the project's creation and its destruction, named"
    );
}

/// ⚠ OWNER ONLY, AND TIGHTER THAN CREATE: an admin may make a project they
/// cannot destroy.
#[sqlx::test]
async fn only_the_organization_owner_deletes_a_project(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, "user_del_gate_owner", "gate-owner@example.com").await;
    sign_in(&pool, "user_del_gate_admin", "gate-admin@example.com").await;
    sign_in(
        &pool,
        "user_del_gate_project_admin",
        "gate-project-admin@example.com",
    )
    .await;
    join_organization(
        &pool,
        "user_del_gate_owner",
        "user_del_gate_admin",
        "gate-admin@example.com",
        "admin",
    )
    .await;

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        "user_del_gate_owner",
        &organization,
        Some(json!({ "name": "Guarded" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().expect("id").to_string();
    seat_on_project(
        &pool,
        "user_del_gate_owner",
        &project_id,
        "user_del_gate_project_admin",
        "gate-project-admin@example.com",
        "admin",
    )
    .await;

    for refused in ["user_del_gate_admin", "user_del_gate_project_admin"] {
        assert_eq!(
            delete_project(&pool, refused, &organization, &project_id).await,
            StatusCode::FORBIDDEN,
            "{refused} deleted a project"
        );
    }

    assert_eq!(
        delete_project(&pool, "user_del_gate_owner", &organization, &project_id).await,
        StatusCode::OK
    );
}

/// ⚠ THE LAST PROJECT MAY GO, AND IT STAYS GONE: listing the organization
/// afterwards must not put a Default Project back behind the owner's delete.
#[sqlx::test]
async fn deleting_the_only_project_leaves_the_organization_empty(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, "user_del_last", "del-last@example.com").await;
    let project_id: String =
        sqlx::query_scalar("SELECT external_id FROM auth.projects WHERE organization_id = $1")
            .bind(&organization)
            .fetch_one(&pool)
            .await
            .expect("sign-in provisions a project");

    assert_eq!(
        delete_project(&pool, "user_del_last", &organization, &project_id).await,
        StatusCode::OK
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.projects WHERE external_id = $1")
        .bind(&project_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0, "the deleted project is still in the table");

    assert!(
        project_names(&pool, "user_del_last", &organization)
            .await
            .is_empty()
    );
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth.projects WHERE organization_id = $1")
            .bind(&organization)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(remaining, 0, "listing put a project back");

    assert_eq!(
        delete_project(&pool, "user_del_last", &organization, &project_id).await,
        StatusCode::BAD_REQUEST
    );
}

/// Rename the project as its owner, as the console's Settings form does.
async fn rename(pool: &PgPool, owner: &str, project_id: &str, name: &str) -> (StatusCode, Value) {
    call(
        pool,
        "PATCH",
        &format!("/internal/projects/{project_id}"),
        owner,
        &organization_of(pool, owner).await,
        Some(json!({ "name": name })),
    )
    .await
}

/// One name per organization, case-insensitively — enforced by the database,
/// so it holds for racing creates too.
#[sqlx::test]
async fn a_name_already_taken_in_the_organization_is_a_409(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_07";
    let organization = sign_in(&pool, owner, "projects-owner-07@example.test").await;

    let (status, _) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    for dup in ["Platform", "platform", "  PLATFORM  "] {
        let (status, body) = call(
            &pool,
            "POST",
            "/internal/projects",
            owner,
            &organization,
            Some(json!({ "name": dup })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "duplicate {dup:?}");
        let detail = body["detail"].as_str().unwrap_or_default();
        assert!(
            detail.contains("already exists"),
            "the 409 says what is wrong, got {body}"
        );
    }

    let other = "usr_projects_other_07";
    let others = sign_in(&pool, other, "projects-other-07@example.test").await;
    let (status, _) = call(
        &pool,
        "POST",
        "/internal/projects",
        other,
        &others,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    assert_eq!(
        project_names(&pool, owner, &organization).await,
        vec!["Default Project", "Platform"],
        "the refused duplicates wrote nothing"
    );
}

/// A project's slug follows its name and is unique within its organization: a
/// name that reads as another's takes the next number, a rename moves it, and
/// another organization holding the same slug is no collision.
#[sqlx::test]
async fn the_slug_follows_the_name_within_the_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_slug";
    let organization = sign_in(&pool, owner, "projects-owner-slug@example.test").await;
    let create = |caller: &'static str, organization: String, name: &'static str| {
        let pool = pool.clone();
        async move {
            let (status, body) = call(
                &pool,
                "POST",
                "/internal/projects",
                caller,
                &organization,
                Some(json!({ "name": name })),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{name}: {body}");
            body
        }
    };

    let first = create(owner, organization.clone(), "Web App").await;
    assert_eq!(first["slug"], "web-app");
    let second = create(owner, organization.clone(), "web-app").await;
    assert_eq!(second["slug"], "web-app-2");

    let (status, renamed) = rename(
        &pool,
        owner,
        second["id"].as_str().unwrap(),
        "Marketing Site",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["slug"], "marketing-site");

    let (_, listed) = call(
        &pool,
        "GET",
        "/internal/projects",
        owner,
        &organization,
        None,
    )
    .await;
    let slugs: Vec<&str> = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["default-project", "marketing-site", "web-app"]);

    let other = "usr_projects_other_slug";
    let others = sign_in(&pool, other, "projects-other-slug@example.test").await;
    let theirs = create(other, others, "Web App").await;
    assert_eq!(
        theirs["slug"], "web-app",
        "another organization's slug collided"
    );
}

/// Renaming INTO a taken name collides; renaming to its own name does not.
#[sqlx::test]
async fn renaming_into_a_taken_name_is_a_409(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_08";
    let organization = sign_in(&pool, owner, "projects-owner-08@example.test").await;

    let (status, platform) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, growth) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "Growth" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let growth_id = growth["id"].as_str().expect("id");
    let platform_id = platform["id"].as_str().expect("id");

    let (status, _) = rename(&pool, owner, growth_id, "platform").await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = rename(&pool, owner, platform_id, "Platform").await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(
        project_names(&pool, owner, &organization).await,
        vec!["Default Project", "Growth", "Platform"],
        "the refused rename changed nothing"
    );
}

/// ⚠ The rename rule is the create rule, counted in CHARACTERS: rename once
/// measured bytes while its message said characters.
#[sqlx::test]
async fn the_rename_limit_is_the_create_limit_and_counts_characters(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_projects_owner_09";
    let organization = sign_in(&pool, owner, "projects-owner-09@example.test").await;

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "Renamed later" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().expect("id");

    let (status, body) = rename(&pool, owner, project_id, &"é".repeat(100)).await;
    assert_eq!(status, StatusCode::OK, "100 characters is 100 characters");
    assert_eq!(body["name"], "é".repeat(100));

    let (status, _) = rename(&pool, owner, project_id, &"é".repeat(101)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    for blank in ["", "   "] {
        let (status, _) = rename(&pool, owner, project_id, blank).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "blank rename {blank:?}");
    }
}

/// `GET /internal/projects/everywhere` for `caller`, as rows in served order.
async fn everywhere(pool: &PgPool, caller: &str) -> Vec<(String, String, String)> {
    let (status, body) = call(
        pool,
        "GET",
        "/internal/projects/everywhere",
        caller,
        &organization_of(pool, caller).await,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .map(|t| {
            (
                t["organizationId"]
                    .as_str()
                    .expect("organizationId")
                    .to_string(),
                t["name"].as_str().expect("name").to_string(),
                t["role"].as_str().expect("role").to_string(),
            )
        })
        .collect()
}

fn row(organization: &str, name: &str, role: &str) -> (String, String, String) {
    (organization.to_string(), name.to_string(), role.to_string())
}

/// Seat `invitee` on one of `owner`'s projects through the real invite lane.
async fn seat_on_project(
    pool: &PgPool,
    owner: &str,
    project_id: &str,
    invitee: &str,
    email: &str,
    role: &str,
) {
    let (status, invite) = call(
        pool,
        "POST",
        &format!("/internal/projects/{project_id}/invites"),
        owner,
        &organization_of(pool, owner).await,
        Some(json!({ "email": email, "role": role })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "project invite: {invite}");
    let token = invite["link"]
        .as_str()
        .expect("invite link")
        .split("/invite/")
        .nth(1)
        .expect("token")
        .to_string();
    let (status, _) = call(
        pool,
        "POST",
        "/internal/invites/accept",
        invitee,
        &organization_of(pool, invitee).await,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

/// The switcher's second group: one read across every organization, with the
/// same reach as the per-organization list.
#[sqlx::test]
async fn the_everywhere_list_spans_organizations_with_each_persons_reach(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let alex = "usr_everywhere_alex";
    let bo = "usr_everywhere_bo";
    let cy = "usr_everywhere_cy";
    let dee = "usr_everywhere_dee";
    let alex_org = sign_in(&pool, alex, "everywhere-alex@example.test").await;
    let bo_org = sign_in(&pool, bo, "everywhere-bo@example.test").await;
    let cy_org = sign_in(&pool, cy, "everywhere-cy@example.test").await;
    let dee_org = sign_in(&pool, dee, "everywhere-dee@example.test").await;
    for (who, organization) in [
        (alex, &alex_org),
        (bo, &bo_org),
        (cy, &cy_org),
        (dee, &dee_org),
    ] {
        assert_eq!(
            project_names(&pool, who, organization).await,
            vec!["Default Project"]
        );
    }

    let (status, platform) = call(
        &pool,
        "POST",
        "/internal/projects",
        alex,
        &alex_org,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let platform_id = platform["id"].as_str().expect("id").to_string();
    let (status, _) = call(
        &pool,
        "POST",
        "/internal/projects",
        alex,
        &alex_org,
        Some(json!({ "name": "Growth" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    join_organization(&pool, alex, bo, "everywhere-bo@example.test", "admin").await;
    seat_on_project(
        &pool,
        alex,
        &platform_id,
        cy,
        "everywhere-cy@example.test",
        "member",
    )
    .await;

    assert_eq!(
        everywhere(&pool, alex).await,
        vec![
            row(&alex_org, "Default Project", "owner"),
            row(&alex_org, "Growth", "owner"),
            row(&alex_org, "Platform", "owner"),
        ]
    );

    assert_eq!(
        everywhere(&pool, bo).await,
        vec![
            row(&alex_org, "Default Project", "admin"),
            row(&alex_org, "Growth", "admin"),
            row(&alex_org, "Platform", "admin"),
            row(&bo_org, "Default Project", "owner"),
        ]
    );

    assert_eq!(
        everywhere(&pool, cy).await,
        vec![
            row(&alex_org, "Platform", "member"),
            row(&cy_org, "Default Project", "owner")
        ]
    );

    assert_eq!(
        everywhere(&pool, dee).await,
        vec![row(&dee_org, "Default Project", "owner")]
    );
}

/// Every row names its organization by its owner's address, and by name only
/// once the owner has given it one.
#[sqlx::test]
async fn every_everywhere_row_names_its_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_everywhere_named";
    let email = "everywhere-named@example.test";
    let organization = sign_in(&pool, owner, email).await;
    project_names(&pool, owner, &organization).await;

    let (status, body) = call(
        &pool,
        "GET",
        "/internal/projects/everywhere",
        owner,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["projects"].as_array().expect("projects array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["organizationId"], organization);
    assert_eq!(rows[0]["organizationOwnerEmail"], email);
    assert!(rows[0]["organizationName"].is_null());
    assert_eq!(rows[0]["role"], "owner");
}

/// ⚠ The boundary pins for the cross-organization read: it runs in the
/// maintenance lane, so the query's own WHERE is the only thing scoping it.
#[sqlx::test]
async fn the_everywhere_read_is_gated_on_the_caller_and_nothing_else(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let alex = "usr_everywhere_gate_alex";
    let dee = "usr_everywhere_gate_dee";
    let alex_org = sign_in(&pool, alex, "everywhere-gate-alex@example.test").await;
    let dee_org = sign_in(&pool, dee, "everywhere-gate-dee@example.test").await;
    project_names(&pool, alex, &alex_org).await;
    project_names(&pool, dee, &dee_org).await;
    let (status, _) = call(
        &pool,
        "POST",
        "/internal/projects",
        alex,
        &alex_org,
        Some(json!({ "name": "Private" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("GET")
                    .uri("/internal/projects/everywhere")
                    .header("x-organization-id", &alex_org),
                &pool,
                alex,
            )
            .await
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = app(pool.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/internal/projects/everywhere")
                .header("x-service-secret", SERVICE_SECRET)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a read with no bearer must be refused"
    );

    let (status, body) = call(
        &pool,
        "GET",
        "/internal/projects/everywhere",
        dee,
        &alex_org,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows: Vec<(String, String)> = body["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .map(|t| {
            (
                t["organizationId"]
                    .as_str()
                    .expect("organizationId")
                    .to_string(),
                t["name"].as_str().expect("name").to_string(),
            )
        })
        .collect();
    assert_eq!(rows, vec![(dee_org, "Default Project".to_string())]);
    assert!(
        !body.to_string().contains("Private"),
        "Alex's project leaked into Dee's reply: {body}"
    );
}

/// Every row the everywhere read offers must also be openable through the
/// per-organization list, or the switch lands on a dead address.
#[sqlx::test]
async fn every_everywhere_row_is_also_in_its_organizations_own_list(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let alex = "usr_everywhere_parity_alex";
    let bo = "usr_everywhere_parity_bo";
    let cy = "usr_everywhere_parity_cy";
    let alex_org = sign_in(&pool, alex, "everywhere-parity-alex@example.test").await;
    let bo_org = sign_in(&pool, bo, "everywhere-parity-bo@example.test").await;
    let cy_org = sign_in(&pool, cy, "everywhere-parity-cy@example.test").await;
    for (who, organization) in [(alex, &alex_org), (bo, &bo_org), (cy, &cy_org)] {
        project_names(&pool, who, organization).await;
    }
    let (status, platform) = call(
        &pool,
        "POST",
        "/internal/projects",
        alex,
        &alex_org,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let platform_id = platform["id"].as_str().expect("id").to_string();
    join_organization(
        &pool,
        alex,
        bo,
        "everywhere-parity-bo@example.test",
        "admin",
    )
    .await;
    seat_on_project(
        &pool,
        alex,
        &platform_id,
        cy,
        "everywhere-parity-cy@example.test",
        "admin",
    )
    .await;

    for who in [bo, cy] {
        let tagged_alex: Vec<String> = everywhere(&pool, who)
            .await
            .into_iter()
            .filter(|(organization, _, _)| organization == &alex_org)
            .map(|(_, name, _)| name)
            .collect();
        let listed_by_alex = project_names(&pool, who, &alex_org).await;
        assert_eq!(
            tagged_alex, listed_by_alex,
            "{who}: the everywhere rows for Alex's organization and Alex's own list disagree"
        );
    }
}

/// The organizations the console will switch this caller INTO: every one `/me`
/// lists for them, their own among them.
async fn switchable_organizations(pool: &PgPool, caller: &str, email: &str) -> Vec<String> {
    seed_identity(pool, caller, email).await;
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
                caller,
            )
            .await
            .body(Body::from(body.to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "/me failed for {caller}");
    let body = json_body(resp).await;
    body["organizations"]
        .as_array()
        .expect("organizations")
        .iter()
        .map(|m| {
            m["organizationId"]
                .as_str()
                .expect("organizationId")
                .to_string()
        })
        .collect()
}

/// The organization `/me` makes `caller` active in when the console asks for
/// `organization` — the switch itself.
async fn switch_to(pool: &PgPool, caller: &str, organization: &str) -> String {
    let resp = app(pool.clone())
        .oneshot(
            as_person(
                Request::builder()
                    .method("POST")
                    .uri("/me")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", organization)
                    .header("content-type", "application/json"),
                pool,
                caller,
            )
            .await
            .body(Body::from(json!({}).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "/me failed for {caller}");
    json_body(resp).await["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned()
}

/// ⚠ The seam the switcher balances on: what is OFFERED and what OPENS are
/// two different predicates that must agree.
#[sqlx::test]
async fn every_organization_the_switcher_offers_is_one_me_will_switch_to(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let alex = "usr_switchable_alex";
    let bo = "usr_switchable_bo";
    let cy = "usr_switchable_cy";
    let alex_email = "switchable-alex@example.test";
    let bo_email = "switchable-bo@example.test";
    let cy_email = "switchable-cy@example.test";
    let alex_org = sign_in(&pool, alex, alex_email).await;
    let bo_org = sign_in(&pool, bo, bo_email).await;
    let cy_org = sign_in(&pool, cy, cy_email).await;
    for (who, organization) in [(alex, &alex_org), (bo, &bo_org), (cy, &cy_org)] {
        project_names(&pool, who, organization).await;
    }

    let (status, platform) = call(
        &pool,
        "POST",
        "/internal/projects",
        alex,
        &alex_org,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let platform_id = platform["id"].as_str().expect("id").to_string();

    join_organization(&pool, alex, bo, bo_email, "admin").await;
    seat_on_project(&pool, alex, &platform_id, cy, cy_email, "admin").await;

    for (who, email) in [(alex, alex_email), (bo, bo_email), (cy, cy_email)] {
        let switchable = switchable_organizations(&pool, who, email).await;
        for (organization, project, _) in everywhere(&pool, who).await {
            assert!(
                switchable.contains(&organization),
                "{who}: the switcher offers {project} in {organization}, which /me will not switch to \
                 — the row is a dead end (switchable: {switchable:?})"
            );
            assert_eq!(
                switch_to(&pool, who, &organization).await,
                organization,
                "{who}: asked for {organization} to open {project}, /me made another active"
            );
        }
    }

    assert!(
        everywhere(&pool, cy)
            .await
            .iter()
            .any(|(organization, project, _)| organization == &alex_org && project == "Platform"),
        "fixture: Cy should reach Alex's Platform by seat"
    );
}

/// An organization admin is an admin on every project, and a `member` seat
/// does not lower that — in what auth enforces and in both lists that report
/// the role. Below admin the seat still decides: an organization member with
/// the same seat stays a member.
#[sqlx::test]
async fn a_member_seat_does_not_lower_an_organization_admin(pool: PgPool) {
    apply_audit_migrations(&pool).await;

    let owner = "usr_floor_owner";
    let admin = "usr_floor_admin";
    let member = "usr_floor_member";
    let organization = sign_in(&pool, owner, "floor-owner@example.test").await;
    sign_in(&pool, admin, "floor-admin@example.test").await;
    sign_in(&pool, member, "floor-member@example.test").await;

    let (status, created) = call(
        &pool,
        "POST",
        "/internal/projects",
        owner,
        &organization,
        Some(json!({ "name": "Platform" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().expect("project id").to_owned();

    join_organization(&pool, owner, admin, "floor-admin@example.test", "admin").await;
    seat_on_project(
        &pool,
        owner,
        &project_id,
        admin,
        "floor-admin@example.test",
        "member",
    )
    .await;
    seat_on_project(
        &pool,
        owner,
        &project_id,
        member,
        "floor-member@example.test",
        "member",
    )
    .await;

    let audit = format!("/internal/audit/projects/{project_id}");
    let (status, _) = call_project(
        &pool,
        "GET",
        &audit,
        admin,
        &organization,
        &project_id,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a member seat lowered an organization admin below the audit log"
    );
    let (status, _) = call_project(
        &pool,
        "GET",
        &audit,
        member,
        &organization,
        &project_id,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an organization member's member seat reached the audit log"
    );

    let (status, listed) = call(
        &pool,
        "GET",
        "/internal/projects",
        admin,
        &organization,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let platform = listed["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .find(|p| p["id"] == project_id.as_str())
        .expect("Platform listed");
    assert_eq!(platform["role"], "admin");

    assert!(
        everywhere(&pool, admin)
            .await
            .contains(&row(&organization, "Platform", "admin")),
        "the everywhere list reported the seat, not the organization admin's floor"
    );
}
