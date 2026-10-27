//! An organization's health, through the module's router and under its
//! policies: the incidents still open on the projects the person can open.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeDelta, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, SECRET};
use telmoni_shared::db::tenant_session::{organization_scope, project_scope};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::test_util::apply_audit_migrations;
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role};
use telmoni_telemetry::router;
use telmoni_telemetry::rules::incidents;

const RULES: &str = "/internal/v1/telemetry/rules";

/// `user`'s request to `uri`, acting on `organization` and, where named,
/// `project`.
async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    user: &str,
    organization: &OrganizationId,
    project: Option<&ProjectId>,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(SERVICE_SECRET_HEADER, SECRET)
        .header("authorization", format!("Bearer tok_{user}"))
        .header("x-organization-id", organization.as_str())
        .header("content-type", "application/json");
    if let Some(project) = project {
        request = request.header("x-project-id", project.as_str());
    }
    let request = request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .expect("a request");
    let response = app.clone().oneshot(request).await.expect("an answer");
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
        serde_json::from_slice(&bytes).expect("JSON")
    };
    (status, body)
}

/// An alert called `name` on `project`, made by its owner through the lane.
async fn alert(
    app: &axum::Router,
    organization: &OrganizationId,
    project: &ProjectId,
    name: &str,
) -> Uuid {
    let (status, made) = send(
        app,
        "POST",
        RULES,
        "user_owner",
        organization,
        Some(project),
        Some(&json!({
            "name": name,
            "kind": "model_call",
            "measure": "count",
            "aggregation": "count",
            "window": "hour",
            "operator": "gt",
            "alert_threshold": 2,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    made.get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse().ok())
        .expect("the alert's id")
}

/// An incident of `rule`, on `project` in `organization`, opened `hours_ago`
/// and standing at `status`, as the loop and a member would have left it.
async fn incident(
    pool: &PgPool,
    organization: &OrganizationId,
    project: &ProjectId,
    rule: Uuid,
    status: &str,
    hours_ago: i64,
) -> Uuid {
    let id = Uuid::now_v7();
    let opened: DateTime<Utc> = Utc::now() - TimeDelta::hours(hours_ago);
    sqlx::query(
        "INSERT INTO telemetry.incidents
             (id, rule_id, project_id, organization_id, status, severity, value, opened_at,
              acknowledged_at, acknowledged_by, resolved_at)
         VALUES ($1, $2, $3, $4, $5, 'alert', 3, $6,
                 CASE WHEN $5 = 'acknowledged' THEN $6 END,
                 CASE WHEN $5 = 'acknowledged' THEN 'user_owner' END,
                 CASE WHEN $5 = 'resolved' THEN $6 END)",
    )
    .bind(id)
    .bind(rule)
    .bind(project)
    .bind(organization)
    .bind(status)
    .bind(opened)
    .execute(pool)
    .await
    .expect("the incident is written");
    id
}

/// What `user`'s Overview of `organization` reads: each open incident's id
/// and its project's id, in the order answered.
async fn health(
    app: &axum::Router,
    user: &str,
    organization: &OrganizationId,
) -> Vec<(String, String)> {
    let (status, read) = send(
        app,
        "GET",
        &format!("/internal/v1/telemetry/organizations/{organization}/health"),
        user,
        organization,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    read.get("incidents")
        .and_then(Value::as_array)
        .expect("a list of incidents")
        .iter()
        .map(|incident| {
            (
                incident["id"].as_str().expect("its id").to_owned(),
                incident["project"]["id"]
                    .as_str()
                    .expect("its project")
                    .to_owned(),
            )
        })
        .collect()
}

/// Two projects: an owner and an admin on both, a member seated on the first
/// alone, and a member seated on neither.
fn organization_of_two(auth: &AuthStub) -> (OrganizationId, ProjectId, ProjectId) {
    let (organization, first, second) = (OrganizationId::new(), ProjectId::new(), ProjectId::new());
    for project in [&first, &second] {
        auth.seat(
            "user_owner",
            &organization,
            project,
            OrganizationRole::Owner,
            None,
        );
    }
    auth.seat(
        "user_admin",
        &organization,
        &first,
        OrganizationRole::Admin,
        None,
    );
    auth.seat(
        "user_member",
        &organization,
        &first,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    auth.seat(
        "user_unseated",
        &organization,
        &first,
        OrganizationRole::Member,
        None,
    );
    (organization, first, second)
}

/// ⚠ A member seated in one project of two sees that one's incidents alone.
#[sqlx::test]
async fn a_member_seated_in_one_project_of_two_sees_that_ones_incidents_alone(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let (organization, first, second) = organization_of_two(&auth);
    let app = router(common::state(&pool, auth));
    let on_first = alert(&app, &organization, &first, "Slow calls").await;
    let on_second = alert(&app, &organization, &second, "Failed runs").await;
    let older = incident(&pool, &organization, &first, on_first, "open", 2).await;
    let newer = incident(&pool, &organization, &second, on_second, "open", 1).await;

    assert_eq!(
        health(&app, "user_member", &organization).await,
        vec![(older.to_string(), first.to_string())],
        "the member's one project",
    );
    let both = vec![
        (newer.to_string(), second.to_string()),
        (older.to_string(), first.to_string()),
    ];
    assert_eq!(health(&app, "user_owner", &organization).await, both);
    assert_eq!(health(&app, "user_admin", &organization).await, both);
    assert!(
        health(&app, "user_unseated", &organization)
            .await
            .is_empty(),
        "a member seated on no project opens none of them",
    );

    let (_, read) = send(
        &app,
        "GET",
        &format!("/internal/v1/telemetry/organizations/{organization}/health"),
        "user_member",
        &organization,
        None,
        None,
    )
    .await;
    let shown = &read["incidents"][0];
    assert_eq!(shown["rule_name"], "Slow calls");
    assert_eq!(shown["rule_id"], on_first.to_string());
    assert_eq!(shown["status"], "open");
    assert_eq!(shown["severity"], "alert");
    assert_eq!(
        shown["project"],
        json!({ "id": first.as_str(), "slug": first.as_str(), "name": format!("Project {first}") }),
        "its project as auth names it, for the Overview to link",
    );
}

/// Only incidents still open, acknowledged or not, newest first; none when all is well.
#[sqlx::test]
async fn only_open_incidents_the_newest_first_and_none_when_all_is_well(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let (organization, first, second) = organization_of_two(&auth);
    let app = router(common::state(&pool, auth));
    let slow = alert(&app, &organization, &first, "Slow calls").await;
    let failed = alert(&app, &organization, &first, "Failed runs").await;
    let spend = alert(&app, &organization, &second, "Spend").await;
    assert!(health(&app, "user_owner", &organization).await.is_empty());

    incident(&pool, &organization, &first, slow, "resolved", 5).await;
    let taken = incident(&pool, &organization, &first, slow, "acknowledged", 2).await;
    let oldest = incident(&pool, &organization, &first, failed, "open", 3).await;
    let newest = incident(&pool, &organization, &second, spend, "open", 1).await;
    assert_eq!(
        health(&app, "user_owner", &organization).await,
        vec![
            (newest.to_string(), second.to_string()),
            (taken.to_string(), first.to_string()),
            (oldest.to_string(), first.to_string()),
        ],
    );

    sqlx::query(
        "UPDATE telemetry.incidents SET status = 'resolved', resolved_at = now()
          WHERE status <> 'resolved'",
    )
    .execute(&pool)
    .await
    .expect("every incident resolves");
    assert!(health(&app, "user_owner", &organization).await.is_empty());
}

/// The read keeps to the projects it is handed before its limit, not after.
#[sqlx::test]
async fn the_read_keeps_to_the_projects_it_is_handed(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let (organization, first, second) = organization_of_two(&auth);
    let state = common::state(&pool, auth);
    let app = router(state.clone());
    let slow = alert(&app, &organization, &first, "Slow calls").await;
    let busy = alert(&app, &organization, &second, "Failed runs").await;
    let mine = incident(&pool, &organization, &first, slow, "open", 2).await;
    incident(&pool, &organization, &second, busy, "open", 1).await;

    let mut tx = organization_scope(&state.db, &organization)
        .await
        .expect("the organization's binding");
    let read = incidents::open_in(&mut tx, &organization, std::slice::from_ref(&first), 1)
        .await
        .expect("the read");
    tx.commit().await.expect("commit");
    assert_eq!(
        read.iter()
            .map(|row| (row.id, row.project_id.clone()))
            .collect::<Vec<_>>(),
        vec![(mine, first)],
    );
}

/// Another organization's incidents never show, nor does a path naming it answer.
#[sqlx::test]
async fn another_organizations_incidents_never_show(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let (organization, first, _) = organization_of_two(&auth);
    let (elsewhere, theirs) = (OrganizationId::new(), ProjectId::new());
    auth.seat(
        "user_owner",
        &elsewhere,
        &theirs,
        OrganizationRole::Member,
        Some(Role::Admin),
    );
    let app = router(common::state(&pool, auth));
    let ours = alert(&app, &organization, &first, "Slow calls").await;
    let their_alert = alert(&app, &elsewhere, &theirs, "Slow calls").await;
    let open = incident(&pool, &organization, &first, ours, "open", 2).await;
    let their_incident = incident(&pool, &elsewhere, &theirs, their_alert, "open", 1).await;

    assert_eq!(
        health(&app, "user_owner", &organization).await,
        vec![(open.to_string(), first.to_string())],
    );
    assert_eq!(
        health(&app, "user_owner", &elsewhere).await,
        vec![(their_incident.to_string(), theirs.to_string())],
        "each organization's own, to a person in both",
    );

    let (status, _) = send(
        &app,
        "GET",
        &format!("/internal/v1/telemetry/organizations/{elsewhere}/health"),
        "user_owner",
        &organization,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the path names another organization"
    );
    let (status, _) = send(
        &app,
        "GET",
        &format!("/internal/v1/telemetry/organizations/{elsewhere}/health"),
        "user_member",
        &elsewhere,
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "not a member of that organization"
    );
}

/// ⚠ An organization's scope reads every one of its incidents, no other's, and writes none.
#[sqlx::test]
async fn an_organizations_scope_reads_its_incidents_and_writes_none(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let (organization, first, second) = organization_of_two(&auth);
    let (elsewhere, theirs) = (OrganizationId::new(), ProjectId::new());
    auth.seat(
        "user_owner",
        &elsewhere,
        &theirs,
        OrganizationRole::Owner,
        None,
    );
    let state = common::state(&pool, auth);
    let app = router(state.clone());
    let slow = alert(&app, &organization, &first, "Slow calls").await;
    let failed = alert(&app, &organization, &second, "Failed runs").await;
    let their_alert = alert(&app, &elsewhere, &theirs, "Slow calls").await;
    let mut ours = vec![
        incident(&pool, &organization, &first, slow, "open", 2).await,
        incident(&pool, &organization, &second, failed, "open", 1).await,
    ];
    incident(&pool, &elsewhere, &theirs, their_alert, "open", 1).await;

    let mut tx = organization_scope(&state.db, &organization)
        .await
        .expect("the organization's binding");
    let mut visible: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM telemetry.incidents")
        .fetch_all(tx.conn())
        .await
        .expect("read under the organization");
    let rewritten = sqlx::query(
        "UPDATE telemetry.incidents
            SET status = 'acknowledged', acknowledged_at = now(), acknowledged_by = 'user_owner'",
    )
    .execute(tx.conn())
    .await
    .expect("an organization's write runs, and reaches nothing")
    .rows_affected();
    tx.commit().await.expect("commit");
    visible.sort();
    ours.sort();
    assert_eq!(visible, ours, "an organization read another's incident");
    assert_eq!(rewritten, 0, "an organization's scope rewrote an incident");
}

/// ⚠ A write binds the project's organization too, which `organization_read` trusts.
#[sqlx::test]
async fn a_write_binds_the_projects_organization_too(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let (organization, first, _) = organization_of_two(&auth);
    let state = common::state(&pool, auth);
    let app = router(state.clone());
    let slow = alert(&app, &organization, &first, "Slow calls").await;
    let open = incident(&pool, &organization, &first, slow, "open", 1).await;
    let write = "UPDATE telemetry.incidents SET told_to = '{}' WHERE id = $1";

    let mut tx = project_scope(&state.db, &first).await.expect("scope");
    let alone = sqlx::query(write).bind(open).execute(tx.conn()).await;
    assert!(alone.is_err(), "a project's scope alone wrote an incident");
    tx.rollback().await.expect("rollback");

    let mut tx = project_scope(&state.db, &first)
        .await
        .expect("scope")
        .bind_organization(&OrganizationId::new())
        .await
        .expect("another organization's binding");
    let misfiled = sqlx::query(write).bind(open).execute(tx.conn()).await;
    assert!(
        misfiled.is_err(),
        "a scope naming another organization wrote an incident"
    );
    tx.rollback().await.expect("rollback");

    let (status, taken) = send(
        &app,
        "POST",
        &format!("/internal/v1/telemetry/incidents/{open}/acknowledge"),
        "user_member",
        &organization,
        Some(&first),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{taken}");
    assert_eq!(
        taken["status"], "acknowledged",
        "the lane's write binds both"
    );
}
