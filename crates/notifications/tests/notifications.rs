//! Notifications end-to-end: emit writes the feed, dedup keys collapse a
//! replay, and the feed reads back in the scope the headers name.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test scaffolding"
)]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

use common::{AuthStub, SECRET};
use telmoni_notifications::db::NotificationsLane;
use telmoni_notifications::{AppState, db, router};
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, service_pool};
use telmoni_shared::{NotificationKind, OrganizationRole, Role};

const ORGANIZATION: &str = "org_notif_1";
const OTHER_ORGANIZATION: &str = "org_notif_2";

/// ORGANIZATION's owner and OTHER_ORGANIZATION's: people with ids of their
/// own, answered as owners because the seeded roster says so.
const OWNER: &str = "user_owner_1";
const OTHER_OWNER: &str = "user_owner_2";

/// A seated member of ORGANIZATION's project — never its owner.
const USER: &str = "user_1";

/// The module's router, with the lanes auth calls in process mounted beside
/// it as they were served.
fn app(state: Arc<AppState>) -> Router {
    router(state.clone()).merge(common::sibling_lanes(state))
}

/// The bearer the console relays for `user`. Opaque to this module — auth is
/// what turns it into a role — and distinct per person.
fn bearer(user: &str) -> String {
    format!("Bearer tok_{user}")
}

/// The roster auth answers from: who owns each fixture organization.
fn owner_of(organization: &str) -> &'static str {
    match organization {
        ORGANIZATION => OWNER,
        OTHER_ORGANIZATION => OTHER_OWNER,
        other => panic!("{other} is not a fixture organization"),
    }
}

/// A request as `organization`'s owner, in its project.
fn bff_req(m: &str, uri: &str, organization: &str, body: Option<&str>) -> Request<Body> {
    req_as(m, uri, organization, owner_of(organization), body)
}

/// A request as `user` acting in `organization`'s project, as the console
/// relays it: the bearer, our secret and the context — no role, no user id.
fn req_as(m: &str, uri: &str, organization: &str, user: &str, body: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method(m)
        .uri(uri)
        .header(header::AUTHORIZATION, bearer(user))
        .header("x-service-secret", SECRET)
        .header("x-organization-id", organization)
        .header("x-project-id", project_of(organization));
    if body.is_some() {
        b = b.header(header::CONTENT_TYPE, "application/json");
    }
    b.body(body.map_or(Body::empty(), |s| Body::from(s.to_owned())))
        .unwrap()
}

fn emit_req(organization: &str, body: &str) -> Request<Body> {
    Request::post("/internal/notifications/emit")
        .header("x-service-secret", SECRET)
        .header("x-project-id", project_of(organization))
        .header("x-organization-id", organization)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

/// What an organization alert's emitter sends: the organization and NO project.
fn organization_emit_req(organization: &str, body: &str) -> Request<Body> {
    Request::post("/internal/notifications/emit")
        .header("x-service-secret", SECRET)
        .header("x-organization-id", organization)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

/// What the console's `organizationHeaders` sends: no project.
fn organization_req(m: &str, uri: &str, organization: &str, user: &str) -> Request<Body> {
    Request::builder()
        .method(m)
        .uri(uri)
        .header(header::AUTHORIZATION, bearer(user))
        .header("x-service-secret", SECRET)
        .header("x-organization-id", organization)
        .body(Body::empty())
        .unwrap()
}

/// Feed rows for the fixture organization, read past RLS on the raw pool.
async fn feed_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM notifications.feed WHERE project_id = $1")
        .bind(pid(ORGANIZATION).to_string())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("JSON body")
}

async fn feed_titles(st: &Arc<AppState>, req: Request<Body>) -> (Vec<String>, i64) {
    let resp = app(st.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let titles = body
        .get("items")
        .and_then(serde_json::Value::as_array)
        .expect("items")
        .iter()
        .map(|i| {
            i.get("title")
                .and_then(serde_json::Value::as_str)
                .expect("title")
                .to_owned()
        })
        .collect();
    let unread = body
        .get("unread")
        .and_then(serde_json::Value::as_i64)
        .expect("unread");
    (titles, unread)
}

/// The project a test organization's rows hang off; these tables are project-keyed.
fn project_of(organization: &str) -> String {
    format!("project_{organization}")
}

fn oid(s: &str) -> telmoni_shared::OrganizationId {
    telmoni_shared::OrganizationId::try_new(s).expect("valid test organization id")
}

/// The project a test organization's rows hang off, distinct from its id.
fn pid(s: &str) -> telmoni_shared::ProjectId {
    telmoni_shared::ProjectId::try_new(project_of(s)).expect("valid test project id")
}

/// Auth as every person lane asks it, with everyone a test acts as seated:
/// each organization's owner, on its page and in its project, and USER,
/// seated as a member of ORGANIZATION's project.
fn auth_stub() -> Arc<AuthStub> {
    let auth = AuthStub::new();
    for organization in [ORGANIZATION, OTHER_ORGANIZATION] {
        auth.seat(
            owner_of(organization),
            organization,
            OrganizationRole::Owner,
            Some(Role::Owner),
        );
    }
    auth.seat(
        USER,
        ORGANIZATION,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    auth
}

/// The module with its person lanes asking `auth`.
fn state(pool: PgPool, auth: &Arc<AuthStub>) -> Arc<AppState> {
    Arc::new(AppState {
        db: service_pool(&pool, ServiceRole::Notifications),
        config: common::config(),
        service_secrets: ServiceSecrets::new(SECRET.to_string(), None::<String>),
        auth: auth.clone(),
        http: reqwest::Client::new(),
        egress: telmoni_shared::net_guard::Egress::unguarded(reqwest::Client::new()),
        vault: telmoni_shared::envelope::Vault::new(),
        kek: None,
        connectors: telmoni_notifications::connector::Connectors::default(),
    })
}

/// The feed's root is the ORGANIZATION, and the emit takes it from a header.
#[sqlx::test]
async fn an_emit_with_no_organization_is_refused(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let state = state(pool, &auth);
    let resp = app(state)
        .oneshot(
            Request::post("/internal/notifications/emit")
                .header("x-service-secret", SECRET)
                .header("x-project-id", project_of(ORGANIZATION))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"kind":"member_added","title":"T","body":"B"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// ⚠ **THE PROOF THAT THE PERSON GATE IS MOUNTED**: every other test carries
/// a bearer. Without one the lane hands the headers to auth, in process, and
/// auth's refusal is the answer — the lane never serves a person it could not
/// name. The service-to-service emit lane never asks.
#[sqlx::test]
async fn a_request_with_no_bearer_is_refused_even_with_a_valid_service_secret(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool, &auth);

    let req = Request::builder()
        .method("GET")
        .uri("/internal/notifications/feed")
        .header("x-service-secret", SECRET)
        .header("x-organization-id", ORGANIZATION)
        .header("x-project-id", project_of(ORGANIZATION))
        .body(Body::empty())
        .unwrap();
    let resp = app(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a lane that acts for a person served a request with no bearer"
    );
    assert_eq!(
        auth.asked(),
        1,
        "the lane answered for a person without asking auth who they were"
    );

    let resp = app(state)
        .oneshot(emit_req(
            ORGANIZATION,
            r#"{"kind":"member_added","title":"T","body":"B"}"#,
        ))
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "the person gate reached the emit auth makes"
    );
    assert_eq!(
        auth.asked(),
        1,
        "the emit asked auth about a person it does not carry"
    );
}

#[sqlx::test]
async fn emit_writes_the_feed_and_read_is_project_scoped(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool.clone(), &auth);

    let resp = app(state.clone())
        .oneshot(emit_req(
            ORGANIZATION,
            r#"{"kind":"member_added","title":"Token rotated","body":"Security token rotated."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let resp = app(state.clone())
        .oneshot(bff_req(
            "GET",
            "/internal/notifications/feed",
            ORGANIZATION,
            None,
        ))
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["unread"], 1);
    assert_eq!(body["items"][0]["kind"], "member_added");

    let resp = app(state.clone())
        .oneshot(bff_req(
            "GET",
            "/internal/notifications/feed",
            OTHER_ORGANIZATION,
            None,
        ))
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 0);

    let resp = app(state.clone())
        .oneshot(bff_req(
            "POST",
            "/internal/notifications/read",
            ORGANIZATION,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp)
            .await
            .get("marked")
            .and_then(serde_json::Value::as_i64),
        Some(1)
    );
}

/// The deletion saga's organization purge, at the database layer.
#[sqlx::test]
async fn an_organization_purge_clears_every_table_and_spares_every_other_organization(
    pool: PgPool,
) {
    for organization in [ORGANIZATION, OTHER_ORGANIZATION] {
        let mut atx = project_scope(&pool, &pid(organization)).await.unwrap();
        db::insert_feed(
            &mut atx,
            Some(&pid(organization)),
            &oid(organization),
            &db::NewFeedItem {
                subject_user_id: None,
                kind: NotificationKind::MemberAdded,
                title: "Token rotated",
                body: "Security token rotated.",
                metadata: &serde_json::json!({}),
                dedup_key: None,
            },
        )
        .await
        .unwrap()
        .expect("no dedup key, always a fresh row");
        atx.commit().await.unwrap();
    }

    let mut tx = maintenance_scope(&pool, NotificationsLane).await.unwrap();
    let purged = db::purge_organization(&mut tx, &oid(ORGANIZATION))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(purged, 1, "the organization's one feed row");

    let gone: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications.feed WHERE project_id = $1")
            .bind(pid(ORGANIZATION).to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(gone, 0, "notifications.feed still holds the purged rows");
    let kept: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications.feed WHERE project_id = $1")
            .bind(pid(OTHER_ORGANIZATION).to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        kept, 1,
        "teardown of one tenant must not touch another's feed"
    );
}

/// Every live kind rides the emit surface and passes the feed's kind CHECK.
#[sqlx::test]
async fn active_notification_kinds_emit_and_land_on_the_feed(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool.clone(), &auth);

    for kind in ["member_added", "member_added", "organization_alert"] {
        let body = format!(
            r#"{{"kind":"{kind}","title":"Alert","body":"An event occurred.","metadata":{{"id":"00000000-0000-0000-0000-000000000000"}}}}"#
        );
        let resp = app(state.clone())
            .oneshot(emit_req(ORGANIZATION, &body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED, "kind {kind} refused");
    }

    let resp = app(state.clone())
        .oneshot(bff_req(
            "GET",
            "/internal/notifications/feed",
            ORGANIZATION,
            None,
        ))
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
}

/// An organization alert is accepted with its emitter's own words and a
/// link in `metadata`, and lands on the organization's feed.
#[sqlx::test]
async fn an_organization_alert_is_accepted_in_the_shape_its_emitter_sends(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool.clone(), &auth);

    let body = r#"{"kind":"organization_alert","title":"Action needed","body":"Something outside the console needs the owner.","metadata":{"url":"http://localhost:3000/organization"}}"#;
    let resp = app(state.clone())
        .oneshot(organization_emit_req(ORGANIZATION, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let (titles, _) = feed_titles(
        &state,
        organization_req("GET", "/internal/notifications/feed", ORGANIZATION, OWNER),
    )
    .await;
    assert_eq!(
        titles,
        ["Action needed"],
        "the emit landed on the organization's page"
    );
}

/// ⚠ **The organization's feed reads with no project and no role.**
#[sqlx::test]
async fn an_organization_level_notice_is_read_and_cleared_on_the_organizations_page(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool, &auth);

    let resp = app(state.clone())
        .oneshot(organization_emit_req(
            ORGANIZATION,
            r#"{"kind":"organization_alert","title":"Action needed","body":"."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let page = || organization_req("GET", "/internal/notifications/feed", ORGANIZATION, OWNER);
    let (titles, unread) = feed_titles(&state, page()).await;
    assert_eq!(titles, ["Action needed"]);
    assert_eq!(unread, 1);

    let resp = app(state.clone())
        .oneshot(organization_req(
            "POST",
            "/internal/notifications/read",
            ORGANIZATION,
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["marked"], 1);

    let (_, unread) = feed_titles(&state, page()).await;
    assert_eq!(
        unread, 0,
        "mark-read cleared a different set than the count reads"
    );
}

/// The two pages are two sets: neither scope's notices surface in the other.
#[sqlx::test]
async fn a_projects_notice_and_the_organizations_are_two_feeds(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool, &auth);

    for req in [
        emit_req(
            ORGANIZATION,
            r#"{"kind":"member_added","title":"in the project","body":"."}"#,
        ),
        organization_emit_req(
            ORGANIZATION,
            r#"{"kind":"organization_alert","title":"for the organization","body":"."}"#,
        ),
    ] {
        let resp = app(state.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }

    let (project, _) = feed_titles(
        &state,
        bff_req("GET", "/internal/notifications/feed", ORGANIZATION, None),
    )
    .await;
    assert_eq!(project, ["in the project"]);

    let (organization, _) = feed_titles(
        &state,
        organization_req("GET", "/internal/notifications/feed", ORGANIZATION, OWNER),
    )
    .await;
    assert_eq!(organization, ["for the organization"]);
}

/// The organization's feed is its owner's and admins': a seated member is
/// refused.
#[sqlx::test]
async fn the_organizations_feed_is_closed_to_a_seated_member(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool, &auth);
    let resp = app(state)
        .oneshot(organization_req(
            "GET",
            "/internal/notifications/feed",
            ORGANIZATION,
            USER,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// A seat in the project is enough to read the project's feed — auth's
/// answer carries the role, and every role reads — while the organization's
/// page stays its owner's and admins'.
#[sqlx::test]
async fn a_viewer_reads_the_projects_feed_and_is_refused_the_organizations(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool, &auth);

    let resp = app(state.clone())
        .oneshot(emit_req(
            ORGANIZATION,
            r#"{"kind":"member_added","title":"Sam joined","body":"."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let (titles, unread) = feed_titles(
        &state,
        req_as(
            "GET",
            "/internal/notifications/feed",
            ORGANIZATION,
            USER,
            None,
        ),
    )
    .await;
    assert_eq!(titles, ["Sam joined"]);
    assert_eq!(unread, 1);

    let resp = app(state)
        .oneshot(organization_req(
            "GET",
            "/internal/notifications/feed",
            ORGANIZATION,
            USER,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(resp).await["detail"],
        "only an organization owner or admin may act on the organization"
    );
}

#[sqlx::test]
async fn the_retention_sweep_executes_and_removes_rows_past_the_window(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool.clone(), &auth);

    let resp = app(state.clone())
        .oneshot(emit_req(
            ORGANIZATION,
            r#"{"kind":"member_added","title":"Old news","body":"Past retention."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    {
        let mut tx = project_scope(&pool, &pid(ORGANIZATION)).await.unwrap();
        sqlx::query("UPDATE notifications.feed SET created_at = now() - interval '400 days'")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    let removed = telmoni_notifications::retention::sweep_once(&state)
        .await
        .expect("the sweep statement must prepare and run");
    assert!(removed >= 1, "the backdated feed row is past retention");

    let resp = app(state.clone())
        .oneshot(bff_req(
            "GET",
            "/internal/notifications/feed",
            ORGANIZATION,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["items"].as_array().unwrap().len(), 0);
}

#[sqlx::test]
async fn an_emit_replayed_with_its_dedup_key_returns_the_first_feed_row_and_enqueues_nothing(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let state = state(pool.clone(), &auth);

    let body = r#"{"kind":"member_added","title":"T","body":"B","dedup_key":"auth:0192-test"}"#;
    let first = app(state.clone())
        .oneshot(emit_req(ORGANIZATION, body))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_id = json_body(first).await["feed_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(feed_count(&pool).await, 1);

    let replay = app(state)
        .oneshot(emit_req(ORGANIZATION, body))
        .await
        .unwrap();
    assert_eq!(
        replay.status(),
        StatusCode::OK,
        "a replay is success, not conflict"
    );
    let v = json_body(replay).await;
    assert_eq!(v["feed_id"], first_id.as_str(), "the FIRST row's id: {v}");
    assert_eq!(v["deduplicated"], true);
    assert_eq!(
        feed_count(&pool).await,
        1,
        "the replay landed on the first row rather than writing a second"
    );
}

/// An organization-level row has no project, and a plain unique index treats
/// that NULL as distinct from every other: its replay would never collide.
#[sqlx::test]
async fn an_organization_level_emit_replayed_with_its_dedup_key_is_deduplicated(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let state = state(pool.clone(), &auth);

    let body =
        r#"{"kind":"organization_alert","title":"T","body":"B","dedup_key":"alerts:org-level"}"#;
    let first = app(state.clone())
        .oneshot(organization_emit_req(ORGANIZATION, body))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first_id = json_body(first).await["feed_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let replay = app(state)
        .oneshot(organization_emit_req(ORGANIZATION, body))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let v = json_body(replay).await;
    assert_eq!(v["feed_id"], first_id.as_str(), "the FIRST row's id: {v}");
    assert_eq!(v["deduplicated"], true);

    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications.feed WHERE organization_id = $1 AND project_id IS NULL",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1, "the replay wrote a second organization-level row");
}

#[sqlx::test]
async fn two_dedup_keys_are_two_notifications(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let state = state(pool.clone(), &auth);
    for key in ["auth:a", "auth:b"] {
        let body =
            format!(r#"{{"kind":"member_added","title":"T","body":"B","dedup_key":"{key}"}}"#);
        let resp = app(state.clone())
            .oneshot(emit_req(ORGANIZATION, &body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }
    assert_eq!(feed_count(&pool).await, 2);
}

#[sqlx::test]
async fn an_emit_with_no_dedup_key_inserts_every_time(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let state = state(pool.clone(), &auth);
    for _ in 0..2 {
        let resp = app(state.clone())
            .oneshot(emit_req(
                ORGANIZATION,
                r#"{"kind":"member_added","title":"T","body":"B"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }
    assert_eq!(feed_count(&pool).await, 2);
}

#[sqlx::test]
async fn two_organizations_may_reuse_one_dedup_key(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = AuthStub::new();
    let state = state(pool.clone(), &auth);
    let body = r#"{"kind":"member_added","title":"T","body":"B","dedup_key":"auth:shared"}"#;
    for organization in [ORGANIZATION, OTHER_ORGANIZATION] {
        let resp = app(state.clone())
            .oneshot(emit_req(organization, body))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "organization {organization}"
        );
    }
    for organization in [ORGANIZATION, OTHER_ORGANIZATION] {
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notifications.feed WHERE project_id = $1")
                .bind(project_of(organization))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rows, 1, "organization {organization}");
    }
}

/// One organization's notices never appear on another's page.
#[sqlx::test]
async fn one_organizations_notices_never_appear_on_another_organization(pool: PgPool) {
    let auth = auth_stub();
    let st = state(pool, &auth);
    for (organization, title) in [
        (ORGANIZATION, "api is down"),
        (OTHER_ORGANIZATION, "cron is silent"),
    ] {
        let resp = app(st.clone())
            .oneshot(
                Request::post("/internal/notifications/emit")
                    .header("x-service-secret", SECRET)
                    .header("x-project-id", project_of(organization))
                    .header("x-organization-id", organization)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"kind":"member_added","title":"{title}","body":"."}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success());
    }

    let (first, _) = feed_titles(
        &st,
        bff_req("GET", "/internal/notifications/feed", ORGANIZATION, None),
    )
    .await;
    assert_eq!(first, ["api is down"]);
}

/// P9's purge lane, driven at the PATH auth actually calls.
#[sqlx::test]
async fn the_purge_auth_calls_removes_every_trace_of_the_organization(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let organization = "org_leaving";

    {
        let mut tx = project_scope(&pool, &pid(organization)).await.unwrap();
        db::insert_feed(
            &mut tx,
            Some(&pid(organization)),
            &oid(organization),
            &db::NewFeedItem {
                subject_user_id: None,
                kind: NotificationKind::MemberAdded,
                title: "T",
                body: "B",
                metadata: &serde_json::json!({}),
                dedup_key: None,
            },
        )
        .await
        .unwrap()
        .expect("no dedup key, always a fresh row");
        tx.commit().await.unwrap();
    }

    let auth = AuthStub::new();
    let resp = app(state(pool.clone(), &auth))
        .oneshot(
            Request::post(format!(
                "/internal/notifications/organizations/{organization}/purge"
            ))
            .header("x-service-secret", SECRET)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["purged"], 1);

    for table in ["feed"] {
        let mut tx = project_scope(&pool, &pid(organization)).await.unwrap();
        let left: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM notifications.{table} WHERE project_id = $1"
        ))
        .bind(pid(organization))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            left, 0,
            "`{table}` still holds the deleted organization's rows"
        );
    }
}

/// The redaction auth's `erase_person` calls: every notice that named the
/// person, on every organization's feed, is rewritten to name a former
/// member and forgets whom it named; a notice naming somebody else, and one
/// naming nobody, are untouched; a second call finds nothing.
#[sqlx::test]
async fn erasing_a_person_rewrites_every_notice_that_named_them(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool.clone(), &auth);

    let joined = |who: &str, name: &str, role: &str| {
        format!(
            r#"{{"kind":"member_added","subject_user_id":"{who}","title":"{name} joined the project","body":"{name} accepted the invitation and is now {role} on the project.","metadata":{{"project_id":"p","role":"{role}"}}}}"#
        )
    };
    let left = |who: &str, name: &str| {
        format!(
            r#"{{"kind":"member_left","subject_user_id":"{who}","title":"{name} left the project","body":"{name} left the project.","metadata":{{"project_id":"p"}}}}"#
        )
    };
    for (organization, body) in [
        (ORGANIZATION, joined("user_leaver", "Sam Leaver", "admin")),
        (ORGANIZATION, left("user_leaver", "Sam Leaver")),
        (OTHER_ORGANIZATION, joined("user_leaver", "sam@example.test", "member")),
        (ORGANIZATION, joined("user_stayer", "Kim Stayer", "member")),
        (
            ORGANIZATION,
            r#"{"kind":"connector_connected","title":"Slack connected","body":"Posting to #general."}"#.to_owned(),
        ),
    ] {
        let resp = app(state.clone())
            .oneshot(emit_req(organization, &body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }

    let redact = || {
        app(state.clone()).oneshot(
            Request::post("/internal/notifications/people/user_leaver/redact")
                .header("x-service-secret", SECRET)
                .body(Body::empty())
                .unwrap(),
        )
    };
    let resp = redact().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["redacted"], 3);

    for (organization, role) in [(ORGANIZATION, "admin"), (OTHER_ORGANIZATION, "member")] {
        let resp = app(state.clone())
            .oneshot(bff_req(
                "GET",
                "/internal/notifications/feed",
                organization,
                None,
            ))
            .await
            .unwrap();
        let body = json_body(resp).await;
        let items = body["items"].as_array().unwrap();
        let former: Vec<&serde_json::Value> = items
            .iter()
            .filter(|i| i["title"] == "A former member joined the project")
            .collect();
        assert_eq!(former.len(), 1, "{organization}: {body}");
        assert_eq!(
            former[0]["body"],
            format!("A former member accepted the invitation and is now {role} on the project."),
            "{organization}: the role survives the redaction, the name does not"
        );
        if organization == ORGANIZATION {
            let former_left: Vec<&serde_json::Value> = items
                .iter()
                .filter(|i| i["title"] == "A former member left the project")
                .collect();
            assert_eq!(former_left.len(), 1, "former left in {organization}");
            assert_eq!(former_left[0]["body"], "A former member left the project.");
        }
        let text = body.to_string();
        assert!(!text.contains("Sam Leaver"), "{organization}: {text}");
        assert!(!text.contains("sam@example.test"), "{organization}: {text}");
    }
    let resp = app(state.clone())
        .oneshot(bff_req(
            "GET",
            "/internal/notifications/feed",
            ORGANIZATION,
            None,
        ))
        .await
        .unwrap();
    let text = json_body(resp).await.to_string();
    assert!(text.contains("Kim Stayer joined the project"), "{text}");
    assert!(text.contains("Slack connected"), "{text}");

    let named: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications.feed WHERE subject_user_id = $1")
            .bind("user_leaver")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(named, 0, "a redacted notice still says whom it named");

    let resp = redact().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["redacted"],
        0,
        "the redaction is idempotent"
    );
}

/// ⚠ **A notice naming a person that the redaction has no words for stops
/// the erasure**, rather than leaving the name in a feed after the person is
/// gone. Today `member_added` and `member_left` name anyone; a producer that starts
/// naming people on another kind adds its words to `redact_person` or its
/// emits fail every erasure they touch, loudly.
#[sqlx::test]
async fn a_notice_the_redaction_cannot_rewrite_fails_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let auth = auth_stub();
    let state = state(pool.clone(), &auth);

    let resp = app(state.clone())
        .oneshot(emit_req(
            ORGANIZATION,
            r#"{"kind":"connector_connected","subject_user_id":"user_leaver","title":"Sam connected Slack","body":"Sam connected #general."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let resp = app(state.clone())
        .oneshot(
            Request::post("/internal/notifications/people/user_leaver/redact")
                .header("x-service-secret", SECRET)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_server_error(),
        "answered {} for a notice it could not redact",
        resp.status()
    );
    let (title, named): (String, Option<String>) = sqlx::query_as(
        "SELECT title, subject_user_id FROM notifications.feed WHERE organization_id = $1",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        title, "Sam connected Slack",
        "a failed redaction changed the row"
    );
    assert_eq!(named.as_deref(), Some("user_leaver"));
}

/// The project purge auth calls before a project changes organization and
/// after one is deleted, at the path it calls: the project's rows go, and
/// another project's stay.
#[sqlx::test]
async fn the_project_purge_auth_calls_removes_the_projects_rows_and_spares_its_siblings(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    for organization in [ORGANIZATION, OTHER_ORGANIZATION] {
        let mut tx = project_scope(&pool, &pid(organization)).await.unwrap();
        db::insert_feed(
            &mut tx,
            Some(&pid(organization)),
            &oid(organization),
            &db::NewFeedItem {
                subject_user_id: None,
                kind: NotificationKind::MemberAdded,
                title: "T",
                body: "B",
                metadata: &serde_json::json!({}),
                dedup_key: None,
            },
        )
        .await
        .unwrap()
        .expect("no dedup key, always a fresh row");
        tx.commit().await.unwrap();
    }

    let auth = AuthStub::new();
    let resp = app(state(pool.clone(), &auth))
        .oneshot(
            Request::post(format!(
                "/internal/notifications/projects/{}/purge",
                pid(ORGANIZATION)
            ))
            .header("x-service-secret", SECRET)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["purged"], 1);

    assert_eq!(
        feed_count(&pool).await,
        0,
        "the project's feed row survived its purge"
    );
    let kept: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications.feed WHERE project_id = $1")
            .bind(pid(OTHER_ORGANIZATION).to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kept, 1, "a purge of one project reached another's feed");
}
