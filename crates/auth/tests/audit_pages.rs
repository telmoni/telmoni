//! The console's two audit lists.

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

use telmoni_auth::db::{members, organization_members, projects};
use telmoni_auth::test_provider::{ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::db::tenant_session::{organization_scope, project_scope};
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, UserId};

const SERVICE_SECRET: &str = "test-service-secret";
const OWNER: &str = "user_audit_owner";
const ADMIN: &str = "user_audit_admin";
const MEMBER: &str = "user_audit_member";
const STRANGER: &str = "user_audit_stranger";

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

/// Sign in, which provisions the organization and nothing in it. Returns the
/// organization's id, as `/me` names it active.
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
    json_body(resp).await["activeOrganizationId"]
        .as_str()
        .expect("/me names the active organization")
        .to_owned()
}

/// A project in `organization`, made by [`OWNER`] as the console would, so its
/// creation is the first row on its own page: sign-in makes none.
async fn create_project(pool: &PgPool, organization: &str, name: &str) -> String {
    let (status, body) = call(
        pool,
        "POST",
        "/internal/projects",
        OWNER,
        organization,
        None,
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "creating {name}: {body}");
    body["id"].as_str().expect("the project's id").to_owned()
}

/// A second project in the same organization, seeded without a row of its
/// own, whose page must not read the first's.
async fn second_project(pool: &PgPool, organization: &str) -> String {
    let id = ProjectId::new();
    let organization = OrganizationId::try_new(organization).unwrap();
    let mut tx = organization_scope(pool, &organization).await.unwrap();
    projects::create(&mut tx, &id, &organization, "Second project")
        .await
        .expect("project row");
    tx.commit().await.unwrap();
    id.to_string()
}

/// A request as the BFF makes it, with the project when there is one.
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

/// Mint a key on a project: the mutation every project page shows first.
async fn mint(pool: &PgPool, owner: &str, organization: &str, project: &str, name: &str) {
    let (status, _) = call(
        pool,
        "POST",
        "/internal/tokens",
        owner,
        organization,
        Some(project),
        Some(json!({ "name": name, "created_by": owner })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "mint on {project}");
}

fn events(body: &Value) -> &Vec<Value> {
    body["events"].as_array().expect("an events array")
}

#[sqlx::test]
async fn a_projects_page_lists_the_rows_written_inside_it_and_no_others(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    let first = create_project(&pool, &organization, "First project").await;
    let second = second_project(&pool, &organization).await;
    mint(&pool, OWNER, &organization, &first, "first key").await;
    mint(&pool, OWNER, &organization, &second, "second key").await;

    for (project, name, own_creation) in [(&first, "first key", 1), (&second, "second key", 0)] {
        let (status, body) = call(
            &pool,
            "GET",
            &format!("/internal/audit/projects/{project}"),
            OWNER,
            &organization,
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let rows = events(&body);
        assert_eq!(
            rows.len(),
            1 + own_creation,
            "the mint and this project's own creation on {project}'s page: {body}"
        );
        assert_eq!(rows[0]["resource_kind"], "token");
        assert_eq!(rows[0]["action"], "created");
        assert_eq!(rows[0]["metadata"]["name"], name);
        assert!(
            rows.iter().all(|r| r["in_project"] == json!(project)),
            "a foreign row on {project}'s page: {body}"
        );
        assert_eq!(rows[0]["in_project"], json!(project));
        assert!(
            body["next_cursor"].is_null(),
            "a page that did not fill has no cursor"
        );
    }

    let (status, body) = call(
        &pool,
        "GET",
        &format!("/internal/audit/organizations/{organization}"),
        OWNER,
        &organization,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = events(&body);
    let projects_seen: Vec<&Value> = rows.iter().map(|r| &r["in_project"]).collect();
    assert!(
        projects_seen.contains(&&json!(first)) && projects_seen.contains(&&json!(second)),
        "both projects' rows are on the organization's page: {body}"
    );
    assert!(
        rows.iter()
            .any(|r| r["in_project"].is_null() && r["resource_kind"] == "organization"),
        "the organization's own provisioning row is on its page: {body}"
    );
    assert_eq!(
        rows[0]["metadata"]["name"], "second key",
        "newest first: {body}"
    );
    for row in rows {
        assert!(
            row["row_hash"].is_string() && row["prev_hash"].is_string()
                || row["prev_hash"].is_null(),
            "every row carries its links: {row}"
        );
    }
}

#[sqlx::test]
async fn the_owner_and_admins_read_a_project_and_the_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let owners = sign_in(&pool, OWNER).await;
    let admins = sign_in(&pool, ADMIN).await;
    sign_in(&pool, MEMBER).await;
    sign_in(&pool, STRANGER).await;
    let project = create_project(&pool, &owners, "Platform").await;
    let owner_organization = OrganizationId::try_new(&owners).unwrap();
    let project_id = ProjectId::try_new(&project).unwrap();
    let added_by = UserId::try_new(OWNER).unwrap();

    let mut tx = organization_scope(&pool, &owner_organization)
        .await
        .unwrap();
    assert!(
        organization_members::add_member(
            &mut tx,
            &owner_organization,
            &UserId::try_new(ADMIN).unwrap(),
            OrganizationRole::Admin,
            &added_by,
        )
        .await
        .unwrap(),
        "the admin's roster row was not written"
    );
    tx.commit().await.unwrap();
    let mut tx = project_scope(&pool, &project_id).await.unwrap();
    members::insert(
        &mut tx,
        &project_id,
        &UserId::try_new(MEMBER).unwrap(),
        Role::Member,
        &added_by,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    mint(&pool, OWNER, &owners, &project, "the key").await;
    let project_page = format!("/internal/audit/projects/{project}");
    let organization_page = format!("/internal/audit/organizations/{owners}");

    for reader in [OWNER, ADMIN] {
        let (status, body) = call(&pool, "GET", &project_page, reader, &owners, None, None).await;
        assert_eq!(status, StatusCode::OK, "{reader}: {body}");
        let rows = events(&body);
        assert_eq!(rows.len(), 2, "{reader}: {body}");
        assert_eq!(rows[0]["resource_kind"], "token", "{reader}: {body}");
        assert_eq!(rows[1]["resource_kind"], "project", "{reader}: {body}");
    }
    for refused in [MEMBER, STRANGER] {
        let (status, _) = call(&pool, "GET", &project_page, refused, &owners, None, None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{refused} reads the project page"
        );
    }

    for reader in [OWNER, ADMIN] {
        let (status, body) = call(
            &pool,
            "GET",
            &organization_page,
            reader,
            &owners,
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reader}: {body}");
        assert!(
            events(&body)
                .iter()
                .any(|r| r["in_project"] == json!(project)),
            "the mint is on the organization's page for {reader}: {body}"
        );
    }
    for refused in [MEMBER, STRANGER] {
        let (status, _) = call(
            &pool,
            "GET",
            &organization_page,
            refused,
            &owners,
            None,
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{refused} reads the organization's chain"
        );
    }
    let (status, _) = call(&pool, "GET", &organization_page, OWNER, &admins, None, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the path must name the organization the caller stands in"
    );

    let (status, body) = call(
        &pool,
        "GET",
        &format!("/internal/audit/organizations/{admins}"),
        ADMIN,
        &admins,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = events(&body);
    assert!(
        rows.iter().all(|r| r["organization_id"] == json!(admins)),
        "a foreign organization's row on the admin's page: {body}"
    );
    assert!(
        rows.iter().all(|r| r["in_project"] != json!(project)),
        "the owner's project on the admin's page: {body}"
    );
}

/// ⚠️ **The two closed-vocabulary filters, which nothing exercised.**
#[sqlx::test]
async fn a_filter_narrows_the_page_and_a_word_outside_the_vocabulary_is_refused(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    let project = create_project(&pool, &organization, "Platform").await;
    mint(&pool, OWNER, &organization, &project, "filtered key").await;

    let page = format!("/internal/audit/projects/{project}");

    let (status, body) = call(
        &pool,
        "GET",
        &page,
        OWNER,
        &organization,
        Some(&project),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        events(&body).len() >= 2,
        "the fixture writes a project row and a token row: {body}"
    );

    let (status, body) = call(
        &pool,
        "GET",
        &format!("{page}?resource_kind=token"),
        OWNER,
        &organization,
        Some(&project),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = events(&body);
    assert!(!rows.is_empty(), "the mint is a token row: {body}");
    assert!(
        rows.iter().all(|r| r["resource_kind"] == json!("token")),
        "the filter let another kind through: {body}"
    );

    let (status, body) = call(
        &pool,
        "GET",
        &format!("{page}?action=exported"),
        OWNER,
        &organization,
        Some(&project),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(events(&body).is_empty(), "nothing was exported: {body}");

    for bad in ["?action=create", "?resource_kind=tokens", "?action="] {
        let (status, _) = call(
            &pool,
            "GET",
            &format!("{page}{bad}"),
            OWNER,
            &organization,
            Some(&project),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{bad} must be refused, not answered with an empty chain"
        );
    }
}
