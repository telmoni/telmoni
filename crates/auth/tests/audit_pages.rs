//! The console's two audit lists, and the chain's exports built in the
//! background.

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
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use telmoni_auth::db::{AuthLane, audit_exports, members, organization_members, projects};
use telmoni_auth::test_provider::{ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, router};
use telmoni_shared::db::audit_hash::row_hash;
use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope, project_scope};
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, UserId};

const SERVICE_SECRET: &str = "test-service-secret";
const OWNER: &str = "user_audit_owner";
const ADMIN: &str = "user_audit_admin";
const MEMBER: &str = "user_audit_member";
const STRANGER: &str = "user_audit_stranger";

fn app(pool: PgPool) -> Router {
    router(state(pool))
}

fn state(pool: PgPool) -> Arc<AppState> {
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
    Arc::new(AppState {
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
    })
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

/// Queue an export as the console's dialog does; its id.
async fn start(pool: &PgPool, organization: &str, caller: &str, body: Value) -> String {
    let (status, entry) = call(
        pool,
        "POST",
        &format!("/internal/audit/organizations/{organization}/exports"),
        caller,
        organization,
        None,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{entry}");
    assert_eq!(entry["status"], json!("queued"), "{entry}");
    entry["id"].as_str().expect("the export's id").to_owned()
}

/// One export as the caller's list shows it, `null` when it does not.
async fn listed(pool: &PgPool, organization: &str, caller: &str, id: &str) -> Value {
    let (status, body) = call(
        pool,
        "GET",
        &format!("/internal/audit/organizations/{organization}/exports"),
        caller,
        organization,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["exports"]
        .as_array()
        .expect("an exports array")
        .iter()
        .find(|e| e["id"] == json!(id))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The export once its build is done, polled as the console's bell polls it;
/// still building after ten seconds, it comes back as it stands, for the
/// caller's assert on its status to fail.
async fn finished(pool: &PgPool, organization: &str, caller: &str, id: &str) -> Value {
    let mut entry = Value::Null;
    for _ in 0..400 {
        entry = listed(pool, organization, caller, id).await;
        if entry["status"] == json!("ready") || entry["status"] == json!("failed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    entry
}

/// A file as the console's download route fetches it: the status, the
/// content type and the bytes.
async fn download(
    pool: &PgPool,
    organization: &str,
    caller: &str,
    id: &str,
) -> (StatusCode, String, Vec<u8>) {
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/internal/audit/organizations/{organization}/exports/{id}"
        ))
        .header("x-service-secret", SERVICE_SECRET)
        .header("x-organization-id", organization);
    let req = as_person(req, pool, caller)
        .await
        .body(Body::empty())
        .unwrap();
    let resp = app(pool.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, content_type, bytes.to_vec())
}

/// Seat `who` in `organization` with `role`, as an accepted invitation would.
async fn seat(pool: &PgPool, organization: &str, who: &str, role: OrganizationRole) {
    let organization = OrganizationId::try_new(organization).unwrap();
    let mut tx = organization_scope(pool, &organization).await.unwrap();
    assert!(
        organization_members::add_member(
            &mut tx,
            &organization,
            &UserId::try_new(who).unwrap(),
            role,
            &UserId::try_new(OWNER).unwrap(),
        )
        .await
        .unwrap(),
        "{who}'s roster row was not written"
    );
    tx.commit().await.unwrap();
}

/// An export row written straight to the table, as a build the request never
/// started leaves it: queued, a minute old.
async fn queued_export(pool: &PgPool, organization: &str, user: &str, format: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO auth.audit_exports
                (id, organization_id, user_id, format, range_to, created_at)
         VALUES ($1, $2, $3, $4, now(), now() - interval '1 minute')",
    )
    .bind(id)
    .bind(organization)
    .bind(user)
    .bind(format)
    .execute(pool)
    .await
    .expect("the queued export");
    id
}

/// Every row of a JSON file recomputes and links to the one before it, oldest
/// first by `seq` with no gap; the first links to a row the file does not
/// hold unless it is the chain's first.
fn verifies(organization: &str, rows: &[Value]) {
    let mut previous: Option<(i64, String)> = None;
    for row in rows {
        let seq = row["seq"].as_i64().expect("seq");
        for leaked in ["ip_address", "user_agent", "request_id"] {
            assert!(
                row.get(leaked).is_none(),
                "{leaked} is outside the hash and the file"
            );
        }
        assert!(row["created_at"].as_str().unwrap().ends_with('Z'));
        let created_at = chrono::DateTime::parse_from_rfc3339(row["created_at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc);
        let recomputed = row_hash(
            row["id"].as_str().unwrap().parse().unwrap(),
            organization,
            row["actor_id"].as_str().unwrap(),
            row["action"].as_str().unwrap(),
            row["resource_kind"].as_str().unwrap(),
            row["resource_id"].as_str(),
            row.get("metadata").filter(|m| !m.is_null()),
            created_at,
            row["prev_hash"].as_str(),
            row["in_project"].as_str(),
        );
        assert_eq!(
            row["row_hash"].as_str().unwrap(),
            recomputed,
            "row {seq} recomputes"
        );
        match &previous {
            Some((previous_seq, previous_hash)) => {
                assert_eq!(seq, previous_seq + 1, "row {seq} follows with no gap");
                assert_eq!(
                    row["prev_hash"].as_str(),
                    Some(previous_hash.as_str()),
                    "row {seq} links"
                );
            }
            None if seq == 1 => assert!(row["prev_hash"].is_null(), "the chain's first row"),
            None => assert!(
                row["prev_hash"].is_string(),
                "row {seq} links to the row before the file"
            ),
        }
        previous = Some((seq, recomputed));
    }
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

/// The export a verifier is promised, built in the background: a file of the
/// chain oldest first by `seq`, each row's hash recomputed from the file's own
/// fields alone, each `prev_hash` the hash of the row before it, and nothing
/// the hash does not cover. Recorded on the chain once built, and kept for
/// its week after the first download.
#[sqlx::test]
async fn an_export_is_built_in_the_background_and_verifies_from_the_file_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    let project = create_project(&pool, &organization, "Platform").await;
    mint(&pool, OWNER, &organization, &project, "first key").await;
    mint(&pool, OWNER, &organization, &project, "second key").await;

    let id = start(&pool, &organization, OWNER, json!({ "format": "json" })).await;
    let entry = finished(&pool, &organization, OWNER, &id).await;
    assert_eq!(entry["status"], json!("ready"), "{entry}");
    assert_eq!(entry["format"], json!("json"));
    assert!(entry["downloaded_at"].is_null(), "{entry}");

    let (status, content_type, file) = download(&pool, &organization, OWNER, &id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("application/json"),
        "{content_type}"
    );
    let body: Value = serde_json::from_slice(&file).expect("the file is one JSON document");
    assert_eq!(body["organization_id"], json!(organization));
    assert!(body["from"].is_null(), "no start was asked for: {body}");
    let rows = events(&body);
    assert!(rows.len() >= 3, "the project and both keys: {body}");
    assert_eq!(entry["row_count"], json!(rows.len()));
    assert_eq!(
        rows[0]["seq"],
        json!(1),
        "the whole chain, from its first row"
    );
    verifies(&organization, rows);

    // The export is on the chain now, after the rows the file holds.
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
    assert_eq!(status, StatusCode::OK);
    let newest = &events(&body)[0];
    assert_eq!(newest["action"], json!("exported"));
    assert_eq!(newest["actor_id"], json!(OWNER));
    assert_eq!(newest["metadata"]["export"], json!("audit_log"));
    assert_eq!(newest["metadata"]["format"], json!("json"));
    assert_eq!(newest["metadata"]["rows"], json!(rows.len()));

    // The download is noted, and the file stays for the rest of its week.
    let entry = listed(&pool, &organization, OWNER, &id).await;
    assert!(entry["downloaded_at"].is_string(), "{entry}");
    let (status, _, again) = download(&pool, &organization, OWNER, &id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again, file);
}

/// A range is cut on the chain, not the clock: the file holds every row from
/// the first written in it to the last, so it verifies on its own, and none
/// written before or after.
#[sqlx::test]
async fn a_range_export_is_one_unbroken_stretch_of_the_chain(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    let project = create_project(&pool, &organization, "Platform").await;
    mint(&pool, OWNER, &organization, &project, "before").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let from = chrono::Utc::now();
    tokio::time::sleep(Duration::from_millis(50)).await;
    mint(&pool, OWNER, &organization, &project, "inside one").await;
    mint(&pool, OWNER, &organization, &project, "inside two").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let to = chrono::Utc::now();
    tokio::time::sleep(Duration::from_millis(50)).await;
    mint(&pool, OWNER, &organization, &project, "after").await;

    let id = start(
        &pool,
        &organization,
        OWNER,
        json!({ "format": "json", "from": from, "to": to }),
    )
    .await;
    let entry = finished(&pool, &organization, OWNER, &id).await;
    assert_eq!(entry["status"], json!("ready"), "{entry}");
    let (_, _, file) = download(&pool, &organization, OWNER, &id).await;
    let body: Value = serde_json::from_slice(&file).unwrap();
    let rows = events(&body);

    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r["metadata"]["name"].as_str())
        .collect();
    assert_eq!(names, ["inside one", "inside two"], "{body}");
    assert!(
        rows[0]["seq"].as_i64().unwrap() > 1,
        "the range starts inside the chain: {body}"
    );
    verifies(&organization, rows);
}

#[sqlx::test]
async fn a_csv_export_is_its_header_and_a_line_a_row(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    let project = create_project(&pool, &organization, "Platform").await;
    mint(&pool, OWNER, &organization, &project, "a key, with a comma").await;

    let id = start(&pool, &organization, OWNER, json!({ "format": "csv" })).await;
    let entry = finished(&pool, &organization, OWNER, &id).await;
    assert_eq!(entry["status"], json!("ready"), "{entry}");

    let (status, content_type, file) = download(&pool, &organization, OWNER, &id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/csv"), "{content_type}");
    let text = String::from_utf8(file).expect("UTF-8");
    let text = text
        .strip_prefix('\u{feff}')
        .expect("the byte-order mark a spreadsheet reads UTF-8 by");
    let mut lines = text.split("\r\n").filter(|l| !l.is_empty());
    assert_eq!(
        lines.next(),
        Some(
            "seq,id,created_at,actor_id,action,resource_kind,resource_id,in_project,metadata,\
             prev_hash,row_hash"
        )
    );
    let lines: Vec<&str> = lines.collect();
    assert_eq!(
        json!(lines.len()),
        entry["row_count"],
        "one line a row: {text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("\"\"a key, with a comma\"\"")),
        "a value with a comma stays one quoted cell: {text}"
    );
}

#[sqlx::test]
async fn a_member_may_not_export_list_or_download_the_organizations_audit_log(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    sign_in(&pool, MEMBER).await;
    seat(&pool, &organization, MEMBER, OrganizationRole::Member).await;
    let id = start(&pool, &organization, OWNER, json!({ "format": "json" })).await;

    let exports = format!("/internal/audit/organizations/{organization}/exports");
    let (status, _) = call(
        &pool,
        "POST",
        &exports,
        MEMBER,
        &organization,
        None,
        Some(json!({ "format": "json" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&pool, "GET", &exports, MEMBER, &organization, None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = download(&pool, &organization, MEMBER, &id).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Another admin reads the same log, and still not somebody else's file: an
/// export is its requester's to download, and nobody else's to see.
#[sqlx::test]
async fn an_export_is_its_requesters_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    sign_in(&pool, ADMIN).await;
    seat(&pool, &organization, ADMIN, OrganizationRole::Admin).await;

    let id = start(&pool, &organization, OWNER, json!({ "format": "json" })).await;
    let entry = finished(&pool, &organization, OWNER, &id).await;
    assert_eq!(entry["status"], json!("ready"), "{entry}");

    assert!(listed(&pool, &organization, ADMIN, &id).await.is_null());
    let (status, _, _) = download(&pool, &organization, ADMIN, &id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // And by the policy, not only the `WHERE`: a query that names nobody
    // reads an export only with its own requester bound.
    let db = service_pool(&pool, ServiceRole::Auth);
    let org = OrganizationId::try_new(&organization).unwrap();
    for (who, seen) in [(ADMIN, 0_i64), (OWNER, 1)] {
        let mut tx = organization_scope(&db, &org)
            .await
            .unwrap()
            .bind_person(&UserId::try_new(who).unwrap())
            .await
            .unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.audit_exports")
            .fetch_one(tx.conn())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(rows, seen, "{who} reads {rows} exports by the policy alone");
    }
    let mut tx = organization_scope(&db, &org).await.unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.audit_exports")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(rows, 0, "the organization's scope alone reads no export");
}

/// An export is its requester's only while they may read the whole log: one
/// still queued when they are made a member is dropped by its build, file and
/// all, and recorded nowhere.
#[sqlx::test]
async fn an_export_does_not_outlive_its_requesters_role(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    sign_in(&pool, ADMIN).await;
    seat(&pool, &organization, ADMIN, OrganizationRole::Admin).await;
    let id = queued_export(&pool, &organization, ADMIN, "json").await;
    sqlx::query(
        "UPDATE auth.organization_members SET role = 'member'
          WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(&organization)
    .bind(ADMIN)
    .execute(&pool)
    .await
    .unwrap();

    let built = telmoni_auth::audit_export::build(
        &state(pool.clone()),
        id,
        &OrganizationId::try_new(&organization).unwrap(),
        &UserId::try_new(ADMIN).unwrap(),
    )
    .await
    .expect("the build");
    assert_eq!(built, telmoni_auth::audit_export::Built::Withdrawn);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.audit_exports WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "the export is gone");
    let recorded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events WHERE organization_id = $1 AND action = 'exported'",
    )
    .bind(&organization)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recorded, 0, "nothing was exported, so nothing is recorded");
}

#[sqlx::test]
async fn an_export_names_a_known_format_and_a_range_that_starts_before_it_ends(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    let later = chrono::Utc::now() + chrono::Duration::hours(1);

    for body in [
        json!({ "format": "xml" }),
        json!({ "format": "json", "rows": 10 }),
        json!({
            "format": "json",
            "from": "2026-10-08T12:00:00Z",
            "to": "2026-10-08T11:00:00Z",
        }),
        // A start still to come, once the end is cut at now.
        json!({ "format": "json", "from": later }),
    ] {
        let (status, answer) = call(
            &pool,
            "POST",
            &format!("/internal/audit/organizations/{organization}/exports"),
            OWNER,
            &organization,
            None,
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
    }
}

#[sqlx::test]
async fn three_exports_still_building_hold_off_a_fourth(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    for _ in 0..3 {
        queued_export(&pool, &organization, OWNER, "json").await;
    }

    let (status, answer) = call(
        &pool,
        "POST",
        &format!("/internal/audit/organizations/{organization}/exports"),
        OWNER,
        &organization,
        None,
        Some(json!({ "format": "json" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
}

/// Files cannot pile up: a person keeps their ten newest exports in an
/// organization, and starting another deletes the oldest finished one.
#[sqlx::test]
async fn a_new_export_makes_room_by_deleting_the_oldest_past_ten(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    // Oldest first: ten finished files, ten minutes old down to one.
    let mut ids = Vec::new();
    for age in (1..=10_i32).rev() {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO auth.audit_exports
                    (id, organization_id, user_id, format, range_to, status, file, row_count,
                     finished_at, expires_at, created_at)
             VALUES ($1, $2, $3, 'json', now(), 'ready', decode('7b7d', 'hex'), 0,
                     now(), now() + interval '7 days', now() - make_interval(mins => $4))",
        )
        .bind(id)
        .bind(&organization)
        .bind(OWNER)
        .bind(age)
        .execute(&pool)
        .await
        .expect("a finished export");
        ids.push(id);
    }

    start(&pool, &organization, OWNER, json!({ "format": "json" })).await;

    let kept: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM auth.audit_exports WHERE user_id = $1")
            .bind(OWNER)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kept.len(), 10, "ten kept, the new one among them");
    assert!(!kept.contains(&ids[0]), "the oldest made room");
    assert!(kept.contains(&ids[1]), "and only the oldest");
}

/// The sweep is the net behind the build a request starts: an export left
/// queued by a restart is built, and one past its week is refused, unlisted,
/// and then deleted, file and all.
#[sqlx::test]
async fn the_sweep_builds_what_a_restart_dropped_and_deletes_what_expired(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    create_project(&pool, &organization, "Platform").await;
    let id = queued_export(&pool, &organization, OWNER, "csv").await;

    let swept = telmoni_auth::audit_export::sweep(&state(pool.clone()))
        .await
        .expect("the sweep");
    assert_eq!(swept.finished, 1);
    let entry = listed(&pool, &organization, OWNER, &id.to_string()).await;
    assert_eq!(entry["status"], json!("ready"), "{entry}");

    sqlx::query(
        "UPDATE auth.audit_exports SET expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        listed(&pool, &organization, OWNER, &id.to_string())
            .await
            .is_null()
    );
    let (status, _, _) = download(&pool, &organization, OWNER, &id.to_string()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let swept = telmoni_auth::audit_export::sweep(&state(pool.clone()))
        .await
        .expect("the sweep");
    assert_eq!(swept.expired, 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM auth.audit_exports WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "the file is gone with its row");
}

/// What an erasure deletes of a person's exports: every one they asked for,
/// in every organization, since nobody else may download them.
#[sqlx::test]
async fn an_erased_persons_exports_go_with_them(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = sign_in(&pool, OWNER).await;
    let id = start(&pool, &organization, OWNER, json!({ "format": "json" })).await;
    let entry = finished(&pool, &organization, OWNER, &id).await;
    assert_eq!(entry["status"], json!("ready"), "{entry}");

    let db = service_pool(&pool, ServiceRole::Auth);
    let mut tx = maintenance_scope(&db, AuthLane).await.unwrap();
    let gone = audit_exports::delete_of_person(&mut tx, &UserId::try_new(OWNER).unwrap())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(gone, 1);
    assert!(listed(&pool, &organization, OWNER, &id).await.is_null());
}
