//! The layout a project's sessions and traces open in, through the module's
//! own router: every role reads it, none until an owner or admin sets one of
//! the project's saved layouts, under the organization's chain lock; a
//! layout of another kind, or of another project, is no default; and a
//! layout deleted takes the default with it, so the project opens in the
//! console's own default again.
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
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::router;

const LANE: &str = "/internal/v1/telemetry/session-layout";
const VIEWS: &str = "/internal/v1/telemetry/views";
const OWNER: &str = "user_owner";
const ADMIN: &str = "user_admin";
const MEMBER: &str = "user_member";
const STRANGER: &str = "user_stranger";

/// The module's router, auth as it holds it, a project the roster seats its
/// three people on, and another of the organization's, its owner's alone.
struct Project {
    app: Router,
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
    Project {
        app: router(common::state(pool, auth.clone())),
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

    /// The setting as `user` reads it.
    async fn setting(&self, user: &str) -> Value {
        let (status, body) = self.send(self.request("GET", LANE, user, None)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    /// A saved view of `kind` on `project`, by its id.
    async fn save(&self, project: &ProjectId, kind: &str, name: &str) -> String {
        let (status, body) = self
            .send(self.on(
                project,
                "POST",
                VIEWS,
                OWNER,
                Some(json!({ "name": name, "kind": kind })),
            ))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body.get("id")
            .and_then(Value::as_str)
            .expect("an id")
            .to_owned()
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

#[sqlx::test]
async fn every_role_reads_the_layout_and_owners_and_admins_set_it(pool: PgPool) {
    let p = project(&pool);
    for user in [OWNER, ADMIN, MEMBER] {
        assert_eq!(
            p.setting(user).await,
            json!({ "view_id": null }),
            "{user}: the default until one is set, with no row"
        );
    }
    assert_eq!(organization_of(&pool, &p.project).await, None);

    let (status, _) = p
        .send(p.request("PUT", LANE, MEMBER, Some(json!({ "view_id": null }))))
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a member applies and sets none"
    );
    assert_eq!(
        p.auth.read_again(),
        0,
        "refused before the chain lock was taken"
    );

    let (status, set) = p
        .send(p.request("PUT", LANE, ADMIN, Some(json!({ "view_id": null }))))
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set, json!({ "view_id": null }));
    assert_eq!(
        p.auth.chain_held(),
        [true],
        "auth was asked again under the organization's chain lock"
    );
    assert_eq!(
        organization_of(&pool, &p.project).await,
        Some(p.organization.to_string()),
        "the setting gives the project the settings row its purges find it by"
    );
    assert_eq!(p.setting(MEMBER).await, set);

    for body in [
        json!({ "layout": "simple<script>" }),
        json!({ "view_id": "not-a-view<script>" }),
        json!({ "preset": "simple" }),
        json!({ "view_id": "11111111-1111-4111-8111-111111111111", "name": "x" }),
    ] {
        let (status, problem) = p
            .send(p.request("PUT", LANE, OWNER, Some(body.clone())))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            !problem["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("script")
        );
    }
    assert_eq!(
        p.setting(MEMBER).await,
        json!({ "view_id": null }),
        "a refusal changes nothing"
    );

    let (status, _) = p.send(p.request("GET", LANE, STRANGER, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = p.send(p.on(&p.other, "GET", LANE, OWNER, None)).await;
    assert_eq!(status, StatusCode::OK, "another project reads its own");
}

/// A saved layout of the project stands as its default until it is deleted;
/// a list view, or a layout of another project, is no default.
#[sqlx::test]
async fn a_saved_layout_is_the_default_until_it_is_deleted(pool: PgPool) {
    let p = project(&pool);
    let list = p.save(&p.project, "traces", "Errors").await;
    let elsewhere = p.save(&p.other, "layout", "Theirs").await;
    for id in [
        list.as_str(),
        elsewhere.as_str(),
        "11111111-1111-4111-8111-111111111111",
    ] {
        let (status, body) = p
            .send(p.request("PUT", LANE, OWNER, Some(json!({ "view_id": id }))))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}: {body}");
    }

    let layout = p.save(&p.project, "layout", "Review").await;
    let (status, set) = p
        .send(p.request("PUT", LANE, OWNER, Some(json!({ "view_id": layout }))))
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set, json!({ "view_id": layout }));
    assert_eq!(p.setting(MEMBER).await, set);

    let (status, _) = p
        .send(p.request("DELETE", &format!("{VIEWS}/{layout}"), ADMIN, None))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        p.setting(MEMBER).await,
        json!({ "view_id": null }),
        "the project opens in the default again once its layout is gone"
    );
    assert_eq!(
        organization_of(&pool, &p.project).await,
        Some(p.organization.to_string()),
        "the settings row stands"
    );
}
