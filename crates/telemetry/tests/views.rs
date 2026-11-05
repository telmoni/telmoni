//! A project's saved dashboards through the module's own router: every role
//! on the project reads them, and its owners and admins save, save over and
//! delete them; a dashboard is kept as the page would draw it, two of one
//! name are refused in any case, and a project keeps a hundred at most; the
//! first one saved gives the project the settings row the purges find it by,
//! filed under the organization that holds the project while its chain lock
//! is held; a project reads its own dashboards and no other's, and the purges
//! take a project's dashboards with it.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, SECRET};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::db::{self, TelemetryLane};
use telmoni_telemetry::{AppState, router};

const LANE: &str = "/internal/v1/telemetry/views";
const OWNER: &str = "user_owner";
const ADMIN: &str = "user_admin";
const MEMBER: &str = "user_member";
const STRANGER: &str = "user_stranger";

/// The module's router, auth as it holds it, a project the roster seats its
/// three people on, and another of the organization's, its owner's alone.
struct Project {
    app: Router,
    state: Arc<AppState>,
    auth: Arc<AuthStub>,
    organization: OrganizationId,
    project: ProjectId,
    other: ProjectId,
}

fn project(pool: &PgPool) -> Project {
    let (organization, project, other) =
        (OrganizationId::new(), ProjectId::new(), ProjectId::new());
    let auth = AuthStub::probing(pool);
    for (user, role, seat) in [
        (OWNER, OrganizationRole::Owner, None),
        (ADMIN, OrganizationRole::Admin, None),
        (MEMBER, OrganizationRole::Member, Some(Role::Member)),
    ] {
        auth.seat(user, &organization, &project, role, seat);
    }
    auth.seat(OWNER, &organization, &other, OrganizationRole::Owner, None);
    let state = common::state(pool, auth.clone());
    Project {
        app: router(state.clone()),
        state,
        auth,
        organization,
        project,
        other,
    }
}

impl Project {
    /// A request as the console sends one, on `project`.
    fn on(
        &self,
        project: &ProjectId,
        method: &str,
        path: &str,
        user: &str,
        body: Option<Value>,
    ) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{user}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", project.as_str());
        match body {
            Some(body) => builder
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .expect("a request")
    }

    /// A request on the project its three people are seated on.
    fn request(&self, method: &str, path: &str, user: &str, body: Option<Value>) -> Request<Body> {
        self.on(&self.project, method, path, user, body)
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("the router answers");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("a JSON body")
        };
        (status, body)
    }

    /// The names of the dashboards `user` reads on the project, in order.
    async fn names(&self, user: &str) -> Vec<String> {
        self.names_of(user, LANE).await
    }

    async fn names_of(&self, user: &str, path: &str) -> Vec<String> {
        let (status, body) = self.send(self.request("GET", path, user, None)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body.get("views")
            .and_then(Value::as_array)
            .expect("views")
            .iter()
            .map(|v| {
                v.get("name")
                    .and_then(Value::as_str)
                    .expect("a name")
                    .to_owned()
            })
            .collect()
    }
}

async fn organization_of(pool: &PgPool, project: &ProjectId) -> Option<String> {
    sqlx::query_scalar(
        "SELECT organization_id FROM telemetry.project_settings WHERE project_id = $1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await
    .expect("read a project's settings row")
}

async fn views_of(pool: &PgPool, project: &ProjectId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM telemetry.trace_views WHERE project_id = $1")
        .bind(project)
        .fetch_one(pool)
        .await
        .expect("count a project's views")
}

/// A dashboard's body: its name and charts that draw.
fn dashboard(name: &str) -> Value {
    json!({ "name": name, "query": { "charts": [{ "measure": "spend" }] } })
}

#[sqlx::test]
async fn every_role_reads_the_dashboards_and_owners_and_admins_save_them(pool: PgPool) {
    let p = project(&pool);
    assert!(p.names(MEMBER).await.is_empty());
    assert_eq!(organization_of(&pool, &p.project).await, None);

    let spend = json!({
        "name": "  Spend ",
        "query": { "charts": [
            { "measure": "spend", "group": "tag", "tag": " team " },
            { "measure": "latency" },
        ] },
    });
    let (status, _) = p
        .send(p.request("POST", LANE, MEMBER, Some(spend.clone())))
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a member reads dashboards and saves none"
    );
    assert_eq!(
        p.auth.read_again(),
        0,
        "refused before the chain lock was taken"
    );

    let (status, saved) = p.send(p.request("POST", LANE, ADMIN, Some(spend))).await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["name"], "Spend");
    assert_eq!(
        saved["query"],
        json!({ "charts": [
            { "measure": "spend", "group": "tag", "tag": "team" },
            { "measure": "latency" },
        ] }),
        "kept as the page draws it"
    );
    assert_eq!(
        p.auth.chain_held(),
        [true],
        "auth was asked again under the organization's chain lock"
    );
    assert_eq!(
        organization_of(&pool, &p.project).await,
        Some(p.organization.to_string()),
        "the first dashboard gives the project the settings row its purges find it by"
    );

    for user in [OWNER, ADMIN, MEMBER] {
        assert_eq!(p.names(user).await, ["Spend"], "{user}");
    }

    // One name per dashboard, whatever its case.
    let (status, body) = p
        .send(p.request("POST", LANE, OWNER, Some(dashboard("spend"))))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = p
        .send(p.request("POST", LANE, OWNER, Some(dashboard("All"))))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(p.names(MEMBER).await, ["All", "Spend"], "by name");

    let id = saved["id"].as_str().expect("an id").to_owned();
    let one = format!("{LANE}/{id}");
    let latency = json!({ "name": "Latency", "query": { "charts": [{ "measure": "latency" }] } });
    let (status, _) = p
        .send(p.request("PUT", &one, MEMBER, Some(latency.clone())))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = p
        .send(p.request("PUT", &one, OWNER, Some(dashboard("ALL"))))
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "saved over onto another's name: {body}"
    );
    let (status, over) = p.send(p.request("PUT", &one, OWNER, Some(latency))).await;
    assert_eq!(status, StatusCode::OK, "{over}");
    assert_eq!(over["id"], saved["id"]);
    assert_eq!(
        over["query"],
        json!({ "charts": [{ "measure": "latency" }] })
    );
    assert_eq!(p.names(MEMBER).await, ["All", "Latency"]);

    let (status, _) = p.send(p.request("DELETE", &one, MEMBER, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = p.send(p.request("DELETE", &one, ADMIN, None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = p.send(p.request("DELETE", &one, ADMIN, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(p.names(MEMBER).await, ["All"]);

    for path in [format!("{LANE}/not-a-dashboard"), format!("{LANE}/{id}")] {
        let (status, _) = p
            .send(p.request("PUT", &path, OWNER, Some(dashboard("x"))))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = p.send(p.request("GET", LANE, STRANGER, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// A project reads and writes its own dashboards and no other's: the row
/// policy and each query's own `WHERE` both name the project.
#[sqlx::test]
async fn a_project_reads_its_own_dashboards_and_no_others(pool: PgPool) {
    let p = project(&pool);
    let (status, saved) = p
        .send(p.request("POST", LANE, OWNER, Some(dashboard("Mine"))))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let id = saved["id"].as_str().expect("an id");

    let (status, body) = p.send(p.on(&p.other, "GET", LANE, OWNER, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["views"],
        json!([]),
        "another project's dashboard is not here"
    );
    for method in ["PUT", "DELETE"] {
        let body = (method == "PUT").then(|| dashboard("Taken"));
        let (status, _) = p
            .send(p.on(&p.other, method, &format!("{LANE}/{id}"), OWNER, body))
            .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} from another project"
        );
    }
    assert_eq!(views_of(&pool, &p.project).await, 1);
}

/// ⚠ A name or charts the page would refuse are a 400 that echoes none of
/// what a person typed, and so is a key no dashboard holds.
#[sqlx::test]
async fn a_dashboard_the_page_would_refuse_is_a_400_that_echoes_none_of_it(pool: PgPool) {
    let p = project(&pool);
    let charts = json!({ "charts": [{ "measure": "spend" }] });
    for body in [
        json!({ "name": "", "query": charts }),
        json!({ "name": "   ", "query": charts }),
        json!({ "name": "d".repeat(61), "query": charts }),
        json!({ "name": "two\nlines<script>", "query": charts }),
        json!({ "name": "None" }),
        json!({ "name": "Cost", "query": { "charts": [{ "measure": "cost<script>" }] } }),
        json!({ "name": "List", "query": { "agent": ["support-bot<script>"] } }),
        json!({ "name": "Kind", "kind": "traces<script>", "query": charts }),
        json!({ "query": charts }),
    ] {
        let (status, problem) = p
            .send(p.request("POST", LANE, OWNER, Some(body.clone())))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let detail = problem["detail"].as_str().unwrap_or_default();
        assert!(!detail.contains("script"), "{body} was echoed: {detail}");
    }
    assert_eq!(views_of(&pool, &p.project).await, 0);
}

/// A hundred dashboards at most: one more is refused.
#[sqlx::test]
async fn a_project_keeps_a_hundred_dashboards_at_most(pool: PgPool) {
    let p = project(&pool);
    sqlx::query(
        "INSERT INTO telemetry.trace_views (id, project_id, name, query)
         SELECT gen_random_uuid(), $1, 'dashboard ' || n, '{\"charts\": []}'
           FROM generate_series(1, 100) AS n",
    )
    .bind(&p.project)
    .execute(&pool)
    .await
    .expect("a hundred dashboards");
    let (status, body) = p
        .send(p.request("POST", LANE, OWNER, Some(dashboard("One more"))))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(views_of(&pool, &p.project).await, 100);
}

/// The purges take a project's dashboards with it: a deleted project's, and
/// those of every project an organization's settings rows name; a dashboard
/// of another organization's project stays.
#[sqlx::test]
async fn the_purges_take_a_projects_dashboards_with_it(pool: PgPool) {
    let p = project(&pool);
    let (other_organization, elsewhere) = (OrganizationId::new(), ProjectId::new());
    for (organization, project) in [
        (&p.organization, &p.project),
        (&p.organization, &p.other),
        (&other_organization, &elsewhere),
    ] {
        sqlx::query(
            "INSERT INTO telemetry.project_settings (project_id, organization_id) VALUES ($1, $2)",
        )
        .bind(project)
        .bind(organization)
        .execute(&pool)
        .await
        .expect("a settings row");
        sqlx::query(
            "INSERT INTO telemetry.trace_views (id, project_id, name, query)
             VALUES (gen_random_uuid(), $1, 'Spend', '{\"charts\": [{\"measure\": \"spend\"}]}')",
        )
        .bind(project)
        .execute(&pool)
        .await
        .expect("a dashboard");
    }

    let mut tx = maintenance_scope(&p.state.db, TelemetryLane)
        .await
        .expect("the lane");
    assert_eq!(
        db::purge_project(&mut tx, &p.other).await.expect("purge"),
        2,
        "its dashboard and its settings row"
    );
    assert_eq!(
        db::purge_organization(&mut tx, &p.organization)
            .await
            .expect("purge"),
        2
    );
    tx.commit().await.expect("commit");

    assert_eq!(views_of(&pool, &p.project).await, 0);
    assert_eq!(views_of(&pool, &p.other).await, 0);
    assert_eq!(
        views_of(&pool, &elsewhere).await,
        1,
        "another organization's project keeps its dashboard"
    );
}
