//! A project's monitors through the module's own router: every role on the
//! project reads them, and its owners and admins make, change, pause, resume
//! and delete them, each change recorded on the organization's chain; a
//! monitor is made under the organization's chain lock and held to its
//! plan's ceiling across the organization's projects; one slug and one agent
//! each find one monitor; a field the lane cannot take is a 400 that names
//! it and echoes none of it; a paused monitor is resumed whatever its saved
//! timing, and saves none that never comes; what a schedule means is
//! answered without saving; a project reads its own monitors and no other's;
//! and the purges take a project's monitors with it, as a move takes them
//! along.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, Ceiling, PausedOrganization, SECRET};
use telmoni_shared::audit::lock_chain;
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::db::{self, TelemetryLane};
use telmoni_telemetry::{AppState, router};

const LANE: &str = "/internal/telemetry/monitors";
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
    project_under(pool, OrganizationId::new(), Arc::new(NoLimits))
}

/// A project of `organization`, under `entitlements`.
fn project_under(
    pool: &PgPool,
    organization: OrganizationId,
    entitlements: Arc<dyn Entitlements>,
) -> Project {
    let (project, other) = (ProjectId::new(), ProjectId::new());
    let auth = AuthStub::probing(pool);
    for (user, role, seat) in [
        (OWNER, OrganizationRole::Owner, None),
        (ADMIN, OrganizationRole::Admin, None),
        (MEMBER, OrganizationRole::Member, Some(Role::Member)),
    ] {
        auth.seat(user, &organization, &project, role, seat);
    }
    auth.seat(OWNER, &organization, &other, OrganizationRole::Owner, None);
    let state = common::state_with_entitlements(pool, auth.clone(), entitlements);
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

    /// The slugs of the monitors `user` reads on the project, in order.
    async fn slugs(&self, user: &str) -> Vec<String> {
        let (status, body) = self.send(self.request("GET", LANE, user, None)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body.get("monitors")
            .and_then(Value::as_array)
            .expect("monitors")
            .iter()
            .map(|m| {
                m.get("slug")
                    .and_then(Value::as_str)
                    .expect("a slug")
                    .to_owned()
            })
            .collect()
    }
}

/// A monitor as the New monitor dialog sends one: nightly at two, London
/// time, ten minutes of grace, held to a stall timeout and a step cap.
fn nightly() -> Value {
    json!({
        "name": "Nightly report",
        "schedule": "0 2 * * *",
        "timezone": "Europe/London",
        "grace_s": 600,
        "expected_duration_s": 300,
        "bounds": { "stall_after_s": 120, "max_steps": 40 },
    })
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

async fn monitors_of(pool: &PgPool, project: &ProjectId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM telemetry.monitors WHERE project_id = $1")
        .bind(project)
        .fetch_one(pool)
        .await
        .expect("count a project's monitors")
}

/// What the organization's chain records of its monitors, in order.
async fn recorded(pool: &PgPool, organization: &OrganizationId) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT action FROM audit.events
          WHERE organization_id = $1 AND resource_kind = 'monitor'
          ORDER BY seq",
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .expect("read the chain")
}

#[sqlx::test]
async fn every_role_reads_the_monitors_and_owners_and_admins_make_them(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    assert!(p.slugs(MEMBER).await.is_empty());
    assert_eq!(organization_of(&pool, &p.project).await, None);

    let (status, _) = p
        .send(p.request("POST", LANE, MEMBER, Some(nightly())))
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a member reads monitors and makes none"
    );
    assert_eq!(
        p.auth.read_again(),
        0,
        "refused before the chain lock was taken"
    );

    let (status, made) = p
        .send(p.request("POST", LANE, ADMIN, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["slug"], "nightly-report", "the slug from the name");
    assert_eq!(made["agent"], "nightly-report", "the agent from the slug");
    assert_eq!(made["name"], "Nightly report");
    assert_eq!(made["schedule"], "0 2 * * *");
    assert_eq!(made["interval_s"], Value::Null);
    assert_eq!(made["timezone"], "Europe/London");
    assert_eq!(made["grace_s"], 600);
    assert_eq!(made["expected_duration_s"], 300);
    assert_eq!(
        made["bounds"],
        json!({ "stall_after_s": 120, "max_steps": 40, "max_cost_usd": null })
    );
    assert_eq!(made["state"], "healthy");
    assert!(made["next_due_at"].is_string(), "due under its schedule");
    assert_eq!(
        p.auth.chain_held(),
        [true],
        "auth was asked again under the organization's chain lock"
    );
    assert_eq!(
        organization_of(&pool, &p.project).await,
        Some(p.organization.to_string()),
        "the first monitor gives the project the settings row its purges find it by"
    );
    for user in [OWNER, ADMIN, MEMBER] {
        assert_eq!(p.slugs(user).await, ["nightly-report"], "{user}");
    }
    let one = format!("{LANE}/nightly-report");
    let (status, got) = p.send(p.request("GET", &one, MEMBER, None)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["id"], made["id"]);

    // One slug and one agent per monitor in a project.
    let (status, body) = p
        .send(p.request(
            "POST",
            LANE,
            OWNER,
            Some(json!({ "name": "Nightly Report" })),
        ))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "its slug again: {body}");
    assert_eq!(
        body["detail"], "this project has a monitor by that slug already",
        "{body}"
    );
    let (status, body) = p
        .send(p.request(
            "POST",
            LANE,
            OWNER,
            Some(json!({ "name": "Other", "agent": "nightly-report" })),
        ))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "its agent again: {body}");
    assert_eq!(
        body["detail"], "this project has a monitor for that agent already",
        "{body}"
    );

    // One with no schedule: it expects no run, and has no due time.
    let (status, support) = p
        .send(p.request(
            "POST",
            LANE,
            OWNER,
            Some(json!({
                "name": "Support bot",
                "slug": "support",
                "agent": "support-bot",
                "bounds": { "max_cost_usd": 2.5 },
            })),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{support}");
    assert_eq!(support["state"], "healthy");
    assert_eq!(support["next_due_at"], Value::Null);
    assert_eq!(support["grace_s"], Value::Null);
    assert_eq!(support["timezone"], "UTC");
    assert_eq!(support["bounds"]["max_cost_usd"], 2.5);
    assert_eq!(
        p.slugs(MEMBER).await,
        ["nightly-report", "support"],
        "the same state, so by name"
    );

    // Saved over: everything but the slug, and the due time stands while the
    // timing does.
    let (status, _) = p
        .send(p.request("PUT", &one, MEMBER, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let mut moved = nightly();
    moved["slug"] = json!("weekly-report");
    let (status, body) = p.send(p.request("PUT", &one, OWNER, Some(moved))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "its slug is fixed: {body}");
    let renamed = json!({
        "name": "Nightly digest",
        "schedule": "0 2 * * *",
        "timezone": "Europe/London",
        "grace_s": 900,
        "bounds": { "max_steps": 50 },
    });
    let (status, over) = p
        .send(p.request("PUT", &one, OWNER, Some(renamed.clone())))
        .await;
    assert_eq!(status, StatusCode::OK, "{over}");
    assert_eq!(over["id"], made["id"]);
    assert_eq!(over["name"], "Nightly digest");
    assert_eq!(over["grace_s"], 900);
    assert_eq!(over["expected_duration_s"], Value::Null);
    assert_eq!(
        over["bounds"],
        json!({ "stall_after_s": null, "max_steps": 50, "max_cost_usd": null })
    );
    assert_eq!(
        over["next_due_at"], made["next_due_at"],
        "the timing stands, so the due time does"
    );
    let (status, same) = p.send(p.request("PUT", &one, OWNER, Some(renamed))).await;
    assert_eq!(status, StatusCode::OK, "{same}");
    let mut later = nightly();
    later["schedule"] = json!("0 3 * * *");
    let (status, later) = p.send(p.request("PUT", &one, OWNER, Some(later))).await;
    assert_eq!(status, StatusCode::OK, "{later}");
    assert_ne!(
        later["next_due_at"], made["next_due_at"],
        "a new timing, a new due time"
    );

    // Paused: never late, and the pause is the only thing that changes.
    let pause = format!("{one}/pause");
    let (status, _) = p.send(p.request("POST", &pause, MEMBER, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, paused) = p.send(p.request("POST", &pause, ADMIN, None)).await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["state"], "paused");
    assert_eq!(paused["next_due_at"], Value::Null);
    let (status, again) = p.send(p.request("POST", &pause, ADMIN, None)).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["state"], "paused");
    assert_eq!(
        p.slugs(MEMBER).await,
        ["nightly-report", "support"],
        "the worst first: paused before healthy"
    );
    let resume = format!("{one}/resume");
    let (status, resumed) = p.send(p.request("POST", &resume, ADMIN, None)).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["state"], "healthy");
    assert!(resumed["next_due_at"].is_string(), "due again from now");
    let (status, again) = p.send(p.request("POST", &resume, ADMIN, None)).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["state"], "healthy");

    // Gone, once.
    let (status, _) = p.send(p.request("DELETE", &one, MEMBER, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = p.send(p.request("DELETE", &one, ADMIN, None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = p.send(p.request("DELETE", &one, ADMIN, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = p.send(p.request("GET", &one, MEMBER, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = p.send(p.request("PUT", &one, OWNER, Some(nightly()))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = p
        .send(p.request("GET", &format!("{LANE}/Not-A-Slug"), OWNER, None))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no slug's shape");
    assert_eq!(p.slugs(MEMBER).await, ["support"]);
    let (status, _) = p.send(p.request("GET", LANE, STRANGER, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The chain records each change that changed something, and nothing for
    // a save over that changed nothing or a pause of one paused already.
    assert_eq!(
        recorded(&pool, &p.organization).await,
        [
            "created", "created", "updated", "updated", "updated", "updated", "deleted"
        ]
    );
}

/// A monitor is held to the plan's ceiling, counted across the organization's
/// projects, and a paused organization takes none in.
#[sqlx::test]
async fn a_monitor_is_held_to_the_plans_ceiling_across_the_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let ceiling = Ceiling::of(&pool, 1);
    let p = project_under(&pool, OrganizationId::new(), ceiling.clone());
    let (status, body) = p
        .send(p.on(
            &p.other,
            "POST",
            LANE,
            OWNER,
            Some(json!({ "name": "Elsewhere" })),
        ))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, problem) = p
        .send(p.request("POST", LANE, OWNER, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{problem}");
    assert_eq!(problem["type"], "/errors/plan/limit-reached");
    assert_eq!(problem["entitlement"], "pro");
    assert_eq!(problem["detail"], "Move to Pro for more.");
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
    assert_eq!(
        p.auth.chain_held(),
        [true, true],
        "auth asked again under the organization's chain lock"
    );
    assert_eq!(
        ceiling.asked(),
        [false, false],
        "the ceiling asked before the lock, never under it"
    );

    let organization = OrganizationId::new();
    let paused = project_under(
        &pool,
        organization.clone(),
        Arc::new(PausedOrganization(organization)),
    );
    let (status, problem) = paused
        .send(paused.request("POST", LANE, OWNER, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{problem}");
    assert_eq!(problem["detail"], "This organization is paused.");
    assert_eq!(monitors_of(&pool, &paused.project).await, 0);
}

/// A project reads and writes its own monitors and no other's: the row
/// policy and each query's own `WHERE` both name the project.
#[sqlx::test]
async fn a_project_reads_its_own_monitors_and_no_others(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let (status, made) = p
        .send(p.request("POST", LANE, OWNER, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");

    let (status, body) = p.send(p.on(&p.other, "GET", LANE, OWNER, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["monitors"],
        json!([]),
        "another project's monitor is not here"
    );
    let one = format!("{LANE}/nightly-report");
    for (method, path, body) in [
        ("GET", one.clone(), None),
        ("PUT", one.clone(), Some(nightly())),
        ("DELETE", one.clone(), None),
        ("POST", format!("{one}/pause"), None),
        ("POST", format!("{one}/resume"), None),
    ] {
        let (status, _) = p.send(p.on(&p.other, method, &path, OWNER, body)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {path} from another project"
        );
    }
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
}

/// ⚠ A field the lane cannot take is a 400 that echoes nothing typed, naming a field it knows.
#[sqlx::test]
async fn a_monitor_the_lane_cannot_take_is_a_400_that_echoes_none_of_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    for (body, field) in [
        (json!({ "name": "" }), "name"),
        (json!({ "name": "two\nlines<script>" }), "name"),
        (
            json!({ "name": "Report", "slug": "Nightly<script>" }),
            "slug",
        ),
        (json!({ "name": "Report", "slug": "schedule" }), "word"),
        (
            json!({ "name": "Report", "schedule": "0 0 2 * * *<script>", "grace_s": 0 }),
            "schedule",
        ),
        (
            json!({ "name": "Report", "schedule": "0 0 30 2 *", "grace_s": 0 }),
            "never",
        ),
        (
            json!({ "name": "Report", "interval_s": 60, "grace_s": 0, "timezone": "Mars/Olympus<script>" }),
            "timezone",
        ),
        (
            json!({ "name": "Report", "schedule": "0 2 * * *" }),
            "grace_s",
        ),
        (
            json!({ "name": "Report", "bounds": { "max_steps": 0 } }),
            "max_steps",
        ),
        (
            json!({ "name": "Report", "spend<script>": 1 }),
            "fields this lane takes",
        ),
    ] {
        let (status, problem) = p
            .send(p.request("POST", LANE, OWNER, Some(body.clone())))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {problem}");
        let detail = problem["detail"].as_str().unwrap_or_default();
        assert!(detail.contains(field), "{body}: {detail}");
        assert!(!detail.contains("script"), "{body} was echoed: {detail}");
    }
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
}

/// A paused monitor is resumed whatever its saved timing: one that cannot be
/// read now — as only a change of ours leaves one — resumes due at no time,
/// recorded as any resume is, rather than refused with a 400 for a request
/// that sent no timing. And a timing saved over a paused monitor is searched
/// as one made is, so a line that never comes is refused as it is sent.
#[sqlx::test]
async fn a_paused_monitor_is_resumed_whatever_its_saved_timing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let p = project(&pool);
    let (status, made) = p
        .send(p.request("POST", LANE, OWNER, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let one = format!("{LANE}/nightly-report");
    let (status, paused) = p
        .send(p.request("POST", &format!("{one}/pause"), OWNER, None))
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");

    // Paused, a line that never comes is refused as it is sent, and one that
    // comes is saved, the monitor still paused and due at no time.
    let mut never = nightly();
    never["schedule"] = json!("0 0 30 2 *");
    let (status, problem) = p.send(p.request("PUT", &one, OWNER, Some(never))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("never comes"),
        "{problem}"
    );
    let mut later = nightly();
    later["schedule"] = json!("0 3 * * *");
    let (status, saved) = p.send(p.request("PUT", &one, OWNER, Some(later))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["state"], "paused");
    assert_eq!(saved["next_due_at"], Value::Null);

    // A zone the database no longer names, written straight into the row as
    // a change of ours would leave it: the resume goes through, due at no
    // time.
    let written = sqlx::query(
        "UPDATE telemetry.monitors SET timezone = 'Mars/Olympus'
          WHERE project_id = $1 AND slug = 'nightly-report'",
    )
    .bind(&p.project)
    .execute(&pool)
    .await
    .expect("write the monitor's zone")
    .rows_affected();
    assert_eq!(written, 1);
    let (status, resumed) = p
        .send(p.request("POST", &format!("{one}/resume"), OWNER, None))
        .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["state"], "healthy");
    assert_eq!(
        resumed["next_due_at"],
        Value::Null,
        "due at a time its timing could not give"
    );
    assert_eq!(
        recorded(&pool, &p.organization).await,
        ["created", "updated", "updated", "updated"],
        "made, paused, saved over and resumed"
    );

    // Saved again in a zone the database names, it is due again.
    let (status, saved) = p.send(p.request("PUT", &one, OWNER, Some(nightly()))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert!(saved["next_due_at"].is_string(), "{saved}");
}

/// What a schedule means is answered as it is typed, to every role, and
/// saves nothing.
#[sqlx::test]
async fn what_a_schedule_means_is_answered_without_saving(pool: PgPool) {
    let p = project(&pool);
    let lane = format!("{LANE}/schedule");
    let (status, body) = p
        .send(p.request(
            "POST",
            &lane,
            MEMBER,
            Some(json!({ "schedule": "0 9 * * *", "timezone": "Europe/London" })),
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let next: Vec<&str> = body["next"]
        .as_array()
        .expect("the next due times")
        .iter()
        .map(|t| t.as_str().expect("a time"))
        .collect();
    assert_eq!(next.len(), 3);
    assert!(next.windows(2).all(|w| w[0] < w[1]), "in order: {next:?}");
    assert!(
        next.iter()
            .all(|t| t.ends_with("08:00:00Z") || t.ends_with("09:00:00Z")),
        "nine in London: {next:?}"
    );

    let (status, body) = p
        .send(p.request("POST", &lane, MEMBER, Some(json!({ "interval_s": 900 }))))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["next"].as_array().map(Vec::len), Some(3));
    let (status, body) = p
        .send(p.request("POST", &lane, MEMBER, Some(json!({}))))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["next"], json!([]), "no schedule, no due time");
    let (status, problem) = p
        .send(p.request(
            "POST",
            &lane,
            MEMBER,
            Some(json!({ "schedule": "every day at nine<script>" })),
        ))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("schedule") && !detail.contains("script"),
        "{detail}"
    );
    let (status, _) = p
        .send(p.request("POST", &lane, STRANGER, Some(json!({}))))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
}

/// The purges take a project's monitors with it: a deleted project's, and
/// those of every project an organization holds; a move takes them to the
/// organization that now holds their project; and another organization's
/// stay.
#[sqlx::test]
async fn the_purges_take_a_projects_monitors_with_it(pool: PgPool) {
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
            "INSERT INTO telemetry.monitors (id, project_id, organization_id, slug, name, agent)
             VALUES (gen_random_uuid(), $1, $2, 'report', 'Report', 'report')",
        )
        .bind(project)
        .bind(organization)
        .execute(&pool)
        .await
        .expect("a monitor");
    }

    let mut tx = maintenance_scope(&p.state.db, TelemetryLane)
        .await
        .expect("the lane");
    assert!(
        db::move_project(&mut tx, &p.other, &other_organization)
            .await
            .expect("move")
    );
    let moved_to: String =
        sqlx::query_scalar("SELECT organization_id FROM telemetry.monitors WHERE project_id = $1")
            .bind(&p.other)
            .fetch_one(tx.conn())
            .await
            .expect("the moved monitor's organization");
    assert_eq!(
        moved_to,
        other_organization.to_string(),
        "moved with its project"
    );
    assert_eq!(
        db::purge_project(&mut tx, &p.other).await.expect("purge"),
        2,
        "its monitor and its settings row"
    );
    assert_eq!(
        db::purge_organization(&mut tx, &p.organization)
            .await
            .expect("purge"),
        2
    );
    tx.commit().await.expect("commit");

    assert_eq!(monitors_of(&pool, &p.project).await, 0);
    assert_eq!(monitors_of(&pool, &p.other).await, 0);
    assert_eq!(
        monitors_of(&pool, &elsewhere).await,
        1,
        "another organization's project keeps its monitor"
    );
}

/// The plan's ceiling is read within a second, before the transaction and
/// the lock, so a deployment slow to answer it — billing's pool spent — holds
/// nobody on it: an answer not had in time is no ceiling, and the monitor is
/// made.
#[sqlx::test]
async fn a_ceiling_not_answered_in_time_is_no_ceiling(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let slow = Ceiling::answered_after(&pool, 0, Duration::from_secs(3));
    let p = project_under(&pool, OrganizationId::new(), slow.clone());
    let (status, body) = p
        .send(p.request("POST", LANE, OWNER, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(monitors_of(&pool, &p.project).await, 1);
    assert_eq!(
        slow.asked(),
        [false],
        "asked once, with the chain lock not held"
    );
}

/// The ceiling is read before the organization's chain lock is taken, so
/// nobody else in the organization waits on the answer: with the lock held
/// elsewhere, making a monitor is refused for the lock as every other change
/// is, the ceiling asked all the same and never under it.
#[sqlx::test]
async fn the_ceiling_is_read_before_the_chain_lock(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let quick = Ceiling::of(&pool, 5);
    let p = project_under(&pool, OrganizationId::new(), quick.clone());
    let mut held = pool.begin().await.expect("a transaction to hold the lock");
    lock_chain(&mut held, &p.organization)
        .await
        .expect("the chain lock");
    let (status, problem) = p
        .send(p.request("POST", LANE, OWNER, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert_eq!(
        problem["detail"],
        "another change to this project's organization is under way; try again"
    );
    assert_eq!(
        quick.asked(),
        [true],
        "asked once, before the lane tried the lock"
    );
    assert_eq!(monitors_of(&pool, &p.project).await, 0);
    held.rollback().await.expect("let the lock go");
    let (status, body) = p
        .send(p.request("POST", LANE, OWNER, Some(nightly())))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(quick.asked(), [true, false]);
}
