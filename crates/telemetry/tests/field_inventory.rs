//! The field inventory: the docs page names every attribute ingest reads and
//! nothing else, and the console's lane serves the same list, from the same
//! code, to every role on a project.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, SECRET};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::customer_docs_page;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::contract::{CONVENTIONS_COMMIT, INVENTORY};
use telmoni_telemetry::router;

const LANE: &str = "/internal/telemetry/field-inventory";

/// The first cell of every row of every table on the page, backticks off:
/// the attribute each row documents.
fn page_attributes(page: &str) -> Vec<String> {
    page.lines()
        .filter(|line| line.starts_with('|'))
        .filter_map(|line| line.split('|').nth(1))
        .map(|cell| cell.trim().trim_matches('`').to_owned())
        .filter(|cell| cell != "Attribute" && !cell.starts_with("---"))
        .collect()
}

/// ⚠ Both ways: a name ingest reads with no row on the page is a field the
/// privacy claim does not document, and a row ingest does not read is a
/// promise the door does not keep.
#[test]
fn the_page_and_the_code_name_the_same_attributes() {
    let page = customer_docs_page("api/field-inventory.mdx");
    let rows = page_attributes(&page);
    let in_page: BTreeSet<String> = rows.iter().cloned().collect();
    assert_eq!(
        rows.len(),
        in_page.len(),
        "an attribute has two rows on the page"
    );
    let in_code: BTreeSet<String> = INVENTORY.iter().map(|r| r.attribute.to_owned()).collect();

    let undocumented: Vec<_> = in_code.difference(&in_page).collect();
    let phantom: Vec<_> = in_page.difference(&in_code).collect();
    assert!(
        undocumented.is_empty(),
        "attributes ingest reads with no row in field-inventory.mdx: {undocumented:?}"
    );
    assert!(
        phantom.is_empty(),
        "field-inventory.mdx documents attributes ingest does not read: {phantom:?}"
    );
    assert!(
        page.contains(CONVENTIONS_COMMIT),
        "the page names another commit of the conventions than the code pins"
    );
}

/// A request as the console sends one: the service secret, the person's
/// bearer, their organization and the project.
fn request(user: &str, organization: &OrganizationId, project: &ProjectId) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(LANE)
        .header(SERVICE_SECRET_HEADER, SECRET)
        .header("authorization", format!("Bearer tok_{user}"))
        .header("x-organization-id", organization.as_str())
        .header("x-project-id", project.as_str())
        .body(Body::empty())
        .expect("a request")
}

#[sqlx::test]
async fn every_role_on_the_project_reads_the_inventory_the_code_holds(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let auth = AuthStub::new();
    for (user, role, seat) in [
        ("user_owner", OrganizationRole::Owner, None),
        ("user_admin", OrganizationRole::Admin, None),
        ("user_member", OrganizationRole::Member, Some(Role::Member)),
    ] {
        auth.seat(user, &organization, &project, role, seat);
    }
    let app = router(common::state(&pool, auth));

    for user in ["user_owner", "user_admin", "user_member"] {
        let response = app
            .clone()
            .oneshot(request(user, &organization, &project))
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::OK, "{user}");
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let body: Value = serde_json::from_slice(&bytes).expect("a JSON body");
        assert_eq!(body["conventions_commit"], CONVENTIONS_COMMIT);
        let rows = body["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), INVENTORY.len(), "{user} read another list");
        assert_eq!(rows[0]["attribute"], "telmoni.run_id");
        assert_eq!(rows[0]["on"], "every span");
        assert_eq!(rows[0]["needs"], "any");
        assert!(
            rows.iter()
                .all(|r| r["kept"].as_str().is_some_and(|k| !k.is_empty())),
            "a row says nothing of what becomes of it"
        );
    }

    // Someone with no seat on the project reads nothing.
    let refused = app
        .clone()
        .oneshot(request("user_stranger", &organization, &project))
        .await
        .expect("the router answers");
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    // Not the console: without the service secret the lane is not there.
    let outside = Request::builder()
        .method("GET")
        .uri(LANE)
        .header("authorization", "Bearer tok_user_owner")
        .header("x-organization-id", organization.as_str())
        .header("x-project-id", project.as_str())
        .body(Body::empty())
        .expect("a request");
    let answered = app.oneshot(outside).await.expect("the router answers");
    assert_eq!(answered.status(), StatusCode::UNAUTHORIZED);
}
