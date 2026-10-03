//! The connector lane end to end: the handshake, what it stores, the loop
//! that drains it, Slack's events and the teardown. Every vendor is a wiremock.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test scaffolding: asserts and fixture setup"
)]

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{body_string_contains, header as header_is, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{AuthStub, Flags, SECRET};
use telmoni_notifications::connector::{Connectors, Provider, discord, slack, webhook};
use telmoni_notifications::db::NotificationsLane;
use telmoni_notifications::{AppState, db, delivery, router};
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::envelope::{KEK_VERSION, Kek, KmsKek, LocalKek, Vault};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::net_guard::Egress;
use telmoni_shared::test_util::{apply_audit_migrations, service_pool};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Redacted, Role, UserId};
const SIGNING_SECRET: &str = "8f742231b10e8888abcd99yyyzzz85a5";
const CONNECTOR_KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
/// The KMS key the two outage tests name. The mock answers on its path.
const KMS_KEY: &str =
    "projects/telmoni-test/locations/us-central1/keyRings/telmoni/cryptoKeys/connector-kek";

const ORGANIZATION: &str = "org_conn_1";
const OTHER_ORGANIZATION: &str = "org_conn_2";
/// ORGANIZATION's owner and OTHER_ORGANIZATION's: people with ids of their
/// own, answered as owners because the seeded roster says so.
const OWNER: &str = "user_owner_1";
const OTHER_OWNER: &str = "user_owner_2";
/// A seated member of ORGANIZATION's project — never its owner.
const MEMBER: &str = "user_member";

const SLACK_WORKSPACE: &str = "T0001";

fn project_of(organization: &str) -> String {
    format!("project_{organization}")
}

fn pid(organization: &str) -> ProjectId {
    ProjectId::try_new(project_of(organization)).unwrap()
}

fn oid(organization: &str) -> OrganizationId {
    OrganizationId::try_new(organization).unwrap()
}

/// The roster auth answers from: who owns each fixture organization.
fn owner_of(organization: &str) -> &'static str {
    match organization {
        ORGANIZATION => OWNER,
        OTHER_ORGANIZATION => OTHER_OWNER,
        other => panic!("{other} is not a fixture organization"),
    }
}

fn uid(s: &str) -> UserId {
    UserId::try_new(s).expect("valid test user id")
}

/// The laptop's KEK, from the same hex the service reads out of `local:`.
fn local_kek() -> Kek {
    Kek::Local(Box::new(LocalKek::from_hex(CONNECTOR_KEY).unwrap()))
}

/// A Cloud KMS KEK pointed at the mock for both the key and the metadata server.
fn kms_kek(mocks: &MockServer) -> Kek {
    Kek::Kms(KmsKek::new(KMS_KEY, &mocks.uri(), &mocks.uri()).unwrap())
}

/// The metadata server's answer to the token request, as documented.
async fn mount_metadata_token(mocks: &MockServer) {
    Mock::given(method("GET"))
        .and(path(
            "/computeMetadata/v1/instance/service-accounts/default/token",
        ))
        .and(header_is("metadata-flavor", "Google"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "ya29.test-bearer",
            "expires_in": 3599,
            "token_type": "Bearer"
        })))
        .mount(mocks)
        .await;
}

/// Open a stored row's URL the way a send does: unwrap the key, then open.
async fn open_target(sealed: &db::SealedConnection) -> Redacted {
    let dek = local_kek()
        .unwrap_dek(
            &reqwest::Client::new(),
            &sealed.wrapped_dek,
            sealed.id.as_bytes(),
            sealed.key_version,
        )
        .await
        .unwrap();
    Vault::new()
        .open(&dek, &sealed.target(), sealed.id.as_bytes())
        .unwrap()
}

/// A stored row, sealed, read in its project's scope.
async fn sealed_row(pool: &PgPool, organization: &str, id: Uuid) -> db::SealedConnection {
    let mut tx = project_scope(pool, &pid(organization)).await.unwrap();
    let sealed = db::sealed_connection(&mut tx, id).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    sealed
}

/// Both connectors on the mock, the local KEK, the person lanes asking the
/// seeded roster who is calling, and the flag set as `flags` scripts it —
/// every flag on when it scripts nothing.
fn state(pool: PgPool, mocks: &MockServer, flags: Option<Flags>) -> Arc<AppState> {
    state_with(pool, mocks, flags, both_connectors(mocks))
}

fn both_connectors(mocks: &MockServer) -> Connectors {
    Connectors {
        slack: Some(Arc::new(slack::SlackConnector::new(
            "slack_client_id",
            Redacted::from("slack_client_secret"),
            &mocks.uri(),
        ))),
        discord: Some(Arc::new(discord::DiscordConnector::new(
            "discord_client_id",
            Redacted::from("discord_client_secret"),
            &mocks.uri(),
        ))),
        webhook: Some(Arc::new(webhook::WebhookConnector)),
    }
}

fn state_with(
    pool: PgPool,
    mocks: &MockServer,
    flags: Option<Flags>,
    connectors: Connectors,
) -> Arc<AppState> {
    state_with_kek(pool, mocks, flags, connectors, local_kek())
}

fn state_with_kek(
    pool: PgPool,
    mocks: &MockServer,
    flags: Option<Flags>,
    connectors: Connectors,
    kek: Kek,
) -> Arc<AppState> {
    state_full(
        pool,
        mocks,
        flags,
        connectors,
        kek,
        Egress::unguarded(reqwest::Client::new()),
    )
}

fn state_full(
    pool: PgPool,
    mocks: &MockServer,
    flags: Option<Flags>,
    connectors: Connectors,
    kek: Kek,
    egress: Egress,
) -> Arc<AppState> {
    let config = telmoni_notifications::Config {
        app_url: "https://app.example".into(),
        slack_signing_secret: Some(SIGNING_SECRET.into()),
        slack_api_base: mocks.uri(),
        discord_api_base: mocks.uri(),
        connector_kek: Some(format!("local:{CONNECTOR_KEY}").into()),
        kms_api_base: mocks.uri(),
        metadata_api_base: mocks.uri(),
        ..common::config()
    };
    let auth = people();
    if let Some(flags) = flags {
        auth.flags(flags);
    }
    Arc::new(AppState {
        db: service_pool(&pool, "notifications"),
        config,
        service_secrets: ServiceSecrets::new(SECRET.to_string(), None::<String>),
        auth,
        http: reqwest::Client::new(),
        egress,
        vault: Vault::new(),
        kek: Some(kek),
        connectors,
    })
}

/// The module's router, with the emit auth makes in process mounted beside
/// it as it was served.
fn app(state: &Arc<AppState>) -> Router {
    router(state.clone()).merge(common::sibling_lanes(state.clone()))
}

/// The bearer the console relays for `user`. Opaque to this module — auth is
/// what turns it into a role — and distinct per person.
fn bearer(user: &str) -> String {
    format!("Bearer tok_{user}")
}

/// A request as `user` in `organization`'s project, as the console relays it:
/// the bearer, our secret and the context — no role and no user id, since
/// auth answers both.
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

fn owner(m: &str, uri: &str, body: Option<&str>) -> Request<Body> {
    req_as(m, uri, ORGANIZATION, OWNER, body)
}

/// Everyone a test acts as, as auth answers them in process: each
/// organization's owner in its own project, and MEMBER seated in
/// ORGANIZATION's.
fn people() -> Arc<AuthStub> {
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
        MEMBER,
        ORGANIZATION,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    auth
}

/// What reached a vendor: everything the mock saw.
async fn vendor_requests(mocks: &MockServer) -> Vec<wiremock::Request> {
    mocks.received_requests().await.unwrap()
}

fn emit_req(organization: &str, project: bool, body: &str) -> Request<Body> {
    let mut b = Request::post("/internal/notifications/emit")
        .header("x-service-secret", SECRET)
        .header("x-organization-id", organization)
        .header(header::CONTENT_TYPE, "application/json");
    if project {
        b = b.header("x-project-id", project_of(organization));
    }
    b.body(Body::from(body.to_owned())).unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned()))
}

/// A connection planted straight into the table, sealed for real, so the loop
/// and the teardown can run without a handshake.
async fn seed_connection(
    pool: &PgPool,
    organization: &str,
    provider: Provider,
    workspace: &str,
    channel: &str,
    url: &str,
    token: Option<&str>,
) -> Uuid {
    let id = Uuid::now_v7();
    let v = Vault::new();
    let dek = v.new_dek().unwrap();
    let wrapped = local_kek()
        .wrap(&reqwest::Client::new(), &dek, id.as_bytes())
        .await
        .unwrap();
    let target = v.seal(&dek, &Redacted::from(url), id.as_bytes()).unwrap();
    let token = token.map(|t| v.seal(&dek, &Redacted::from(t), id.as_bytes()).unwrap());
    let mut tx = project_scope(pool, &pid(organization)).await.unwrap();
    db::insert_connection(
        &mut tx,
        &db::NewConnection {
            id,
            project_id: &pid(organization),
            organization_id: &oid(organization),
            provider,
            external_workspace_id: workspace,
            external_workspace_name: Some("Acme"),
            channel_id: channel,
            channel_name: channel,
            target: &target,
            key_version: KEK_VERSION,
            wrapped_dek: &wrapped,
            token: token.as_ref(),
            scopes: "incoming-webhook",
            event_kinds: None,
            installed_by: &uid(owner_of(organization)),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// A signed webhook planted straight into the table, the way `create_webhook`
async fn seed_webhook(pool: &PgPool, organization: &str, url: &str, secret: &str) -> Uuid {
    let id = Uuid::now_v7();
    let v = Vault::new();
    let dek = v.new_dek().unwrap();
    let wrapped = local_kek()
        .wrap(&reqwest::Client::new(), &dek, id.as_bytes())
        .await
        .unwrap();
    let target = v.seal(&dek, &Redacted::from(url), id.as_bytes()).unwrap();
    let token = v
        .seal(&dek, &Redacted::from(secret), id.as_bytes())
        .unwrap();
    let parsed = reqwest::Url::parse(url).unwrap();
    let host = webhook::host_label(&parsed);
    let fingerprint = webhook::fingerprint(&parsed);
    let mut tx = project_scope(pool, &pid(organization)).await.unwrap();
    db::insert_connection(
        &mut tx,
        &db::NewConnection {
            id,
            project_id: &pid(organization),
            organization_id: &oid(organization),
            provider: Provider::Webhook,
            external_workspace_id: &host,
            external_workspace_name: None,
            channel_id: &fingerprint,
            channel_name: &host,
            target: &target,
            key_version: KEK_VERSION,
            wrapped_dek: &wrapped,
            token: Some(&token),
            scopes: webhook::SCHEME,
            event_kinds: None,
            installed_by: &uid(owner_of(organization)),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// What a receiver writes: the reference verifier for `Telmoni-Signature`.
/// Any `v1` that verifies is enough, which is what keeps a receiver working
/// through a rotation's overlap.
fn verify_signature(secret: &str, header: &str, body: &[u8]) -> bool {
    use hmac::Mac as _;
    let mut t: Option<i64> = None;
    let mut candidates: Vec<Vec<u8>> = Vec::new();
    for part in header.split(',') {
        match part.split_once('=') {
            Some(("t", v)) => t = v.parse().ok(),
            Some(("v1", v)) => candidates.extend(hex::decode(v).ok()),
            _ => {}
        }
    }
    let Some(t) = t else {
        return false;
    };
    candidates.iter().any(|candidate| {
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(t.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        mac.verify_slice(candidate).is_ok()
    })
}

async fn breaker_count(pool: &PgPool, id: Uuid) -> i32 {
    sqlx::query_scalar("SELECT consecutive_failures FROM notifications.connections WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The requests the mock saw on one path, in order.
async fn posts_to(mocks: &MockServer, path: &str) -> Vec<wiremock::Request> {
    mocks
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == "POST" && r.url.path() == path)
        .collect()
}

/// One pending delivery, planted with no inline attempt, so the test drives the loop.
async fn seed_delivery(pool: &PgPool, organization: &str, connection: Uuid) -> Uuid {
    let mut tx = project_scope(pool, &pid(organization)).await.unwrap();
    let id = db::enqueue_delivery(
        &mut tx,
        &db::NewDelivery {
            connection_id: connection,
            project_id: &project_of(organization),
            organization_id: &oid(organization),
            kind: "member_added",
            subject: "Sam joined",
            body: "Sam accepted the invitation.",
            subject_user_id: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// A backlog of `count` pending rows planted in one statement, not one per row.
async fn seed_backlog(pool: &PgPool, organization: &str, connection: Uuid, count: i32) {
    let mut tx = project_scope(pool, &pid(organization)).await.unwrap();
    sqlx::query(
        "INSERT INTO notifications.deliveries
            (connection_id, project_id, organization_id, kind, subject, body)
         SELECT $1, $2, $3, 'member_added', 'Sam joined', 'Sam accepted the invitation.'
           FROM generate_series(1, $4)",
    )
    .bind(connection)
    .bind(project_of(organization))
    .bind(oid(organization).as_str())
    .bind(count)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Rows in the queue by status, across every project: `(pending, delivered)`.
async fn queue_counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*) FILTER (WHERE status = 'pending'),
                count(*) FILTER (WHERE status = 'delivered')
           FROM notifications.deliveries",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// How many times the loop asked KMS to open a wrapped key.
async fn decrypt_calls(mocks: &MockServer) -> usize {
    posts_to(mocks, &format!("/v1/{KMS_KEY}:decrypt"))
        .await
        .len()
}

async fn connection_status(pool: &PgPool, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status, last_error FROM notifications.connections WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn delivery_status(pool: &PgPool, id: Uuid) -> (String, i32, Option<String>) {
    sqlx::query_as::<_, (String, i32, Option<String>)>(
        "SELECT status, attempts, last_error FROM notifications.deliveries WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn feed_titles(pool: &PgPool, organization: &str) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT title FROM notifications.feed WHERE project_id = $1 ORDER BY created_at, id",
    )
    .bind(project_of(organization))
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Wait for a spawned first attempt to settle the row, or fail loudly.
async fn wait_until_settled(pool: &PgPool, id: Uuid) -> (String, i32, Option<String>) {
    for _ in 0..200 {
        let row = delivery_status(pool, id).await;
        if row.0 != "pending" {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    panic!("delivery {id} never left pending");
}

/// Slack's `oauth.v2.access` success, every field the parser reads.
fn slack_access_ok(url: &str) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "access_token": "xoxb-live-token",
        "token_type": "bot",
        "scope": "incoming-webhook",
        "bot_user_id": "U0BOT",
        "app_id": "A0APP",
        "team": { "id": SLACK_WORKSPACE, "name": "Acme" },
        "enterprise": null,
        "authed_user": { "id": "U0USER" },
        "incoming_webhook": {
            "channel": "#alerts",
            "channel_id": "C0ALERTS",
            "configuration_url": "https://acme.slack.com/services/B0HOOK",
            "url": url
        }
    })
}

fn slack_hook_url(mocks: &MockServer, tail: &str) -> String {
    format!("{}/services/T0001/B0HOOK/{tail}", mocks.uri())
}

/// The state comes back raw, is stored hashed, and rides the vendor URL with a
/// redirect built from `APP_URL`.
#[sqlx::test]
async fn authorize_mints_a_state_and_stores_only_its_hash(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(owner("POST", "/internal/connectors/slack/authorize", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let raw = body["state"].as_str().unwrap().to_owned();
    let url = body["url"].as_str().unwrap();
    assert_eq!(raw.len(), 64, "32 random bytes as hex");
    assert!(url.contains(&format!("state={raw}")), "{url}");
    assert!(
        url.contains("redirect_uri=https%3A%2F%2Fapp.example%2Fconnect%2Fslack%2Fcallback"),
        "{url}"
    );
    assert!(url.contains("scope=incoming-webhook"), "{url}");

    let stored: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT state_hash, project_id, user_id, provider FROM notifications.oauth_states",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(stored.len(), 1);
    let (hash, project, user, provider) = &stored[0];
    assert_eq!(hash, &telmoni_shared::digest::sha256_hex(raw.as_bytes()));
    assert_ne!(hash, &raw, "the raw state must not be stored");
    assert_eq!(project, &project_of(ORGANIZATION));
    assert_eq!(user, OWNER);
    assert_eq!(provider, "slack");
}

/// Connecting is the owner's and the admins'; a member is refused before
/// anything is written or asked.
#[sqlx::test]
async fn a_member_cannot_start_a_handshake(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(req_as(
            "POST",
            "/internal/connectors/discord/authorize",
            ORGANIZATION,
            MEMBER,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications.oauth_states")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    assert!(
        vendor_requests(&mocks).await.is_empty(),
        "a refused handshake reached the vendor"
    );
}

/// A provider with no credentials here cannot be connected, and the list says so.
#[sqlx::test]
async fn an_unconfigured_provider_is_refused_and_listed_as_disabled(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let slack_only = Connectors {
        discord: None,
        ..both_connectors(&mocks)
    };
    let st = state_with(pool.clone(), &mocks, None, slack_only);

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/discord/authorize",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app(&st)
        .oneshot(owner("GET", "/internal/connectors", None))
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["enabled"]["slack"], true);
    assert_eq!(body["enabled"]["discord"], false);
}

/// A switched-off flag refuses a NEW install with a problem naming the flag.
#[sqlx::test]
async fn authorize_under_the_flag_off_is_the_feature_off_problem(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let st = state(pool.clone(), &mocks, Some(Flags::ConnectorsOff));

    let resp = app(&st)
        .oneshot(owner("POST", "/internal/connectors/slack/authorize", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = json_body(resp).await;
    assert_eq!(body["type"], "/errors/tenant/feature-off");
    assert_eq!(body["flag"], "connectors");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications.oauth_states")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "a refused handshake stores no state");
}

/// An unreadable flag set is an error, never a pass.
#[sqlx::test]
async fn authorize_with_the_flag_read_broken_is_refused_not_admitted(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let st = state(pool.clone(), &mocks, Some(Flags::Unreadable));
    let resp = app(&st)
        .oneshot(owner("POST", "/internal/connectors/slack/authorize", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// The whole Slack handshake: the exchange body pinned byte for byte, the row
/// built from the RESPONSE, the URL sealed, and the audit on the chain.
#[sqlx::test]
async fn the_slack_callback_stores_the_sealed_grant_and_audits_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let hook = slack_hook_url(&mocks, "livehook");
    Mock::given(method("POST"))
        .and(path("/api/oauth.v2.access"))
        .and(header_is(
            "content-type",
            "application/x-www-form-urlencoded",
        ))
        .and(body_string_contains("client_id=slack_client_id"))
        .and(body_string_contains("client_secret=slack_client_secret"))
        .and(body_string_contains("code=c0de"))
        .and(body_string_contains(
            "redirect_uri=https%3A%2F%2Fapp.example%2Fconnect%2Fslack%2Fcallback",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(slack_access_ok(&hook)))
        .expect(1)
        .mount(&mocks)
        .await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/livehook"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(owner("POST", "/internal/connectors/slack/authorize", None))
        .await
        .unwrap();
    let raw_state = json_body(resp).await["state"].as_str().unwrap().to_owned();

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(&format!(r#"{{"code":"c0de","state":"{raw_state}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = json_body(resp).await;
    let connection = &body["connection"];
    assert_eq!(connection["provider"], "slack");
    assert_eq!(connection["external_workspace_id"], SLACK_WORKSPACE);
    assert_eq!(connection["external_workspace_name"], "Acme");
    assert_eq!(connection["channel_id"], "C0ALERTS");
    assert_eq!(connection["channel_name"], "#alerts");
    assert_eq!(connection["status"], "active");
    assert!(
        !body.to_string().contains("livehook") && !body.to_string().contains("xoxb"),
        "the response carried a credential: {body}"
    );
    let id: Uuid = connection["id"].as_str().unwrap().parse().unwrap();

    let sealed = sealed_row(&pool, ORGANIZATION, id).await;
    assert!(
        !sealed
            .target_ciphertext
            .windows(8)
            .any(|w| w == b"livehook"),
        "the URL is stored in the clear"
    );
    assert!(
        sealed.token_ciphertext.is_some(),
        "the bot token is kept, sealed, for the uninstall"
    );
    assert_eq!(sealed.key_version, KEK_VERSION);
    assert_eq!(open_target(&sealed).await.expose(), hook);
    assert!(
        local_kek()
            .unwrap_dek(
                &reqwest::Client::new(),
                &sealed.wrapped_dek,
                Uuid::now_v7().as_bytes(),
                sealed.key_version,
            )
            .await
            .is_err(),
        "the wrapped key opened under another row's id"
    );

    let (actor, action, kind, in_project, metadata): (
        String,
        String,
        String,
        Option<String>,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT actor_id, action, resource_kind, in_project, metadata
               FROM audit.events WHERE organization_id = $1",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actor, OWNER);
    assert_eq!(action, "created");
    assert_eq!(kind, "connector");
    assert_eq!(
        in_project.as_deref(),
        Some(project_of(ORGANIZATION).as_str())
    );
    assert_eq!(metadata["provider"], "slack");
    assert!(!metadata.to_string().contains("livehook"));

    assert_eq!(feed_titles(&pool, ORGANIZATION).await, ["Slack connected"]);
    let queued: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM notifications.deliveries WHERE connection_id = $1")
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(queued.len(), 1);
    let (status, _, _) = wait_until_settled(&pool, queued[0]).await;
    assert_eq!(status, "delivered");

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(&format!(r#"{{"code":"c0de","state":"{raw_state}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// The Discord handshake: Basic auth, the webhook object read into the row,
/// and no channel or guild name invented.
#[sqlx::test]
async fn the_discord_callback_reads_the_webhook_object(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let hook = format!("{}/api/webhooks/123/tok_discord", mocks.uri());
    Mock::given(method("POST"))
        .and(path("/api/oauth2/token"))
        .and(header_is(
            "authorization",
            "Basic ZGlzY29yZF9jbGllbnRfaWQ6ZGlzY29yZF9jbGllbnRfc2VjcmV0",
        ))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=dc0de"))
        .and(body_string_contains(
            "redirect_uri=https%3A%2F%2Fapp.example%2Fconnect%2Fdiscord%2Fcallback",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "token_type": "Bearer",
            "access_token": "not-kept",
            "expires_in": 604800,
            "refresh_token": "not-kept-either",
            "scope": "webhook.incoming",
            "webhook": {
                "application_id": "app",
                "name": "Telmoni alerts",
                "url": hook,
                "channel_id": "C0DISCORD",
                "token": "tok_discord",
                "type": 1,
                "avatar": null,
                "guild_id": "G0GUILD",
                "id": "123"
            }
        })))
        .expect(1)
        .mount(&mocks)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/webhooks/123/tok_discord"))
        .and(query_param("wait", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m1"})))
        .expect(1)
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/discord/authorize",
            None,
        ))
        .await
        .unwrap();
    let raw_state = json_body(resp).await["state"].as_str().unwrap().to_owned();
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/discord/callback",
            Some(&format!(r#"{{"code":"dc0de","state":"{raw_state}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let c = json_body(resp).await["connection"].clone();
    assert_eq!(c["provider"], "discord");
    assert_eq!(c["external_workspace_id"], "G0GUILD");
    assert!(
        c["external_workspace_name"].is_null(),
        "discord names no guild"
    );
    assert_eq!(c["channel_id"], "C0DISCORD");
    assert_eq!(c["channel_name"], "Telmoni alerts");
    let id: Uuid = c["id"].as_str().unwrap().parse().unwrap();
    let queued: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM notifications.deliveries WHERE connection_id = $1")
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap();
    let (status, _, _) = wait_until_settled(&pool, queued[0]).await;
    assert_eq!(
        status, "delivered",
        "the connected notice reached the webhook with wait=true"
    );
}

/// A state nobody minted, already spent, another person's or another project's
/// is refused with zero requests at the vendor.
#[sqlx::test]
async fn a_callback_with_a_state_that_is_not_the_callers_is_refused_before_the_vendor(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let st = state(pool.clone(), &mocks, None);
    let hash = |s: &str| telmoni_shared::digest::sha256_hex(s.as_bytes());

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(r#"{"code":"c0de","state":"nobody-minted-this"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    {
        let mut tx = project_scope(&pool, &pid(ORGANIZATION)).await.unwrap();
        db::insert_oauth_state(
            &mut tx,
            &hash("theirs"),
            &pid(ORGANIZATION),
            &oid(ORGANIZATION),
            &uid(MEMBER),
            Provider::Slack,
            600,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(r#"{"code":"c0de","state":"theirs"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    {
        let mut tx = project_scope(&pool, &pid(ORGANIZATION)).await.unwrap();
        db::insert_oauth_state(
            &mut tx,
            &hash("discord-one"),
            &pid(ORGANIZATION),
            &oid(ORGANIZATION),
            &uid(OWNER),
            Provider::Discord,
            600,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(r#"{"code":"c0de","state":"discord-one"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    {
        let mut tx = project_scope(&pool, &pid(OTHER_ORGANIZATION))
            .await
            .unwrap();
        db::insert_oauth_state(
            &mut tx,
            &hash("other-project"),
            &pid(OTHER_ORGANIZATION),
            &oid(OTHER_ORGANIZATION),
            &uid(OWNER),
            Provider::Slack,
            600,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(r#"{"code":"c0de","state":"other-project"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    {
        let mut tx = project_scope(&pool, &pid(ORGANIZATION)).await.unwrap();
        db::insert_oauth_state(
            &mut tx,
            &hash("stale"),
            &pid(ORGANIZATION),
            &oid(ORGANIZATION),
            &uid(OWNER),
            Provider::Slack,
            -1,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(r#"{"code":"c0de","state":"stale"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    assert!(
        vendor_requests(&mocks).await.is_empty(),
        "a refused callback asked the vendor for an exchange"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications.connections")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

/// Reconnect repairs an `errored` row in place — same id, new URL, `active`.
#[sqlx::test]
async fn a_callback_on_an_errored_rows_key_reconnects_it_in_place(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let old = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "oldhook"),
        Some("xoxb-old"),
    )
    .await;
    sqlx::query("UPDATE notifications.connections SET status = 'errored', last_error = 'slack: channel_is_archived' WHERE id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    let old_wrapped_dek = sealed_row(&pool, ORGANIZATION, old).await.wrapped_dek;
    let new_hook = slack_hook_url(&mocks, "newhook");
    Mock::given(method("POST"))
        .and(path("/api/oauth.v2.access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(slack_access_ok(&new_hook)))
        .mount(&mocks)
        .await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/newhook"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(owner("POST", "/internal/connectors/slack/authorize", None))
        .await
        .unwrap();
    let raw_state = json_body(resp).await["state"].as_str().unwrap().to_owned();
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(&format!(r#"{{"code":"c0de","state":"{raw_state}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let c = json_body(resp).await["connection"].clone();
    assert_eq!(c["id"].as_str().unwrap().parse::<Uuid>().unwrap(), old);
    assert_eq!(c["status"], "active");
    assert!(c["last_error"].is_null());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications.connections")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "a reconnect is a repair, not a second row");

    let sealed = sealed_row(&pool, ORGANIZATION, old).await;
    assert_eq!(open_target(&sealed).await.expose(), new_hook);
    assert_ne!(
        sealed.wrapped_dek, old_wrapped_dek,
        "the data key was reused"
    );
}

/// Every role lists; the list never carries the grant; another project's
/// member sees nothing.
#[sqlx::test]
async fn the_list_names_the_channel_and_never_the_grant(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "secrethook"),
        Some("xoxb-secret"),
    )
    .await;
    let st = state(pool.clone(), &mocks, None);

    for user in [OWNER, MEMBER] {
        let resp = app(&st)
            .oneshot(req_as(
                "GET",
                "/internal/connectors",
                ORGANIZATION,
                user,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{user}");
        let body = json_body(resp).await;
        assert_eq!(body["enabled"]["slack"], true);
        assert_eq!(body["enabled"]["discord"], true);
        assert_eq!(body["connections"].as_array().unwrap().len(), 1);
        assert_eq!(body["connections"][0]["channel_name"], "C0ALERTS");
        let text = body.to_string();
        assert!(
            !text.contains("secrethook") && !text.contains("xoxb"),
            "{text}"
        );
    }

    let resp = app(&st)
        .oneshot(req_as(
            "GET",
            "/internal/connectors",
            OTHER_ORGANIZATION,
            OTHER_OWNER,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["connections"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

/// `no_service` is a dead installation: revoked, queue failed, project told once.
#[sqlx::test]
async fn a_dead_installation_retires_the_connection_and_fails_its_queue(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "deadhook"),
        Some("xoxb"),
    )
    .await;
    let first = seed_delivery(&pool, ORGANIZATION, connection).await;
    let second = seed_delivery(&pool, ORGANIZATION, connection).await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/deadhook"))
        .respond_with(ResponseTemplate::new(404).set_body_string("no_service"))
        .expect(1..=2)
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let attempted = delivery::tick_once(&st).await.unwrap();
    assert_eq!(attempted, 2);

    let (status, error) = connection_status(&pool, connection).await;
    assert_eq!(status, "revoked");
    assert_eq!(error.as_deref(), Some("slack: no_service"));
    for id in [first, second] {
        let (status, _, error) = delivery_status(&pool, id).await;
        assert_eq!(status, "failed", "{id}");
        assert_eq!(error.as_deref(), Some("slack: no_service"));
    }
    let titles = feed_titles(&pool, ORGANIZATION).await;
    assert_eq!(
        titles,
        ["Slack disconnected"],
        "one notice, not one per row"
    );

    let posted_before = mocks.received_requests().await.unwrap().len();
    assert_eq!(
        delivery::tick_once(&st).await.unwrap(),
        0,
        "nothing left to lease"
    );
    assert_eq!(
        mocks.received_requests().await.unwrap().len(),
        posted_before
    );
}

/// An archived channel is the target's fault: `errored`, not `revoked`.
#[sqlx::test]
async fn a_bad_target_marks_the_connection_errored(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0OLD",
        &slack_hook_url(&mocks, "archived"),
        Some("xoxb"),
    )
    .await;
    seed_delivery(&pool, ORGANIZATION, connection).await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/archived"))
        .respond_with(ResponseTemplate::new(404).set_body_string("channel_is_archived"))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    delivery::tick_once(&st).await.unwrap();
    let (status, error) = connection_status(&pool, connection).await;
    assert_eq!(status, "errored");
    assert_eq!(error.as_deref(), Some("slack: channel_is_archived"));
    let (_, revoked_at): (String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT status, revoked_at FROM notifications.connections WHERE id = $1")
            .bind(connection)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(revoked_at.is_none(), "errored is not revoked");
}

/// An unknown 4xx fails the ROW and leaves the connection alone: retiring a
/// working channel on a guess is the worse mistake.
#[sqlx::test]
async fn an_unknown_answer_fails_the_row_and_leaves_the_connection(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "odd"),
        Some("xoxb"),
    )
    .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/odd"))
        .respond_with(ResponseTemplate::new(400).set_body_string("rollup_error"))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    delivery::tick_once(&st).await.unwrap();
    let (status, attempts, error) = delivery_status(&pool, id).await;
    assert_eq!(status, "failed");
    assert_eq!(attempts, 1, "terminal on the first attempt");
    assert!(error.unwrap().contains("rollup_error"));
    assert_eq!(connection_status(&pool, connection).await.0, "active");
    assert!(
        feed_titles(&pool, ORGANIZATION).await.is_empty(),
        "no notice for a row failure"
    );
}

/// A 429 is transient and its `Retry-After` honoured, above the backoff floor.
#[sqlx::test]
async fn a_throttle_schedules_the_next_attempt_no_sooner_than_asked(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "busy"),
        Some("xoxb"),
    )
    .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/busy"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "70")
                .set_body_string("rate_limited"),
        )
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    delivery::tick_once(&st).await.unwrap();
    let (status, attempts, _) = delivery_status(&pool, id).await;
    assert_eq!(status, "pending", "a throttle is retried");
    assert_eq!(attempts, 1);
    let wait: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (next_attempt_at - now()))::float8 FROM notifications.deliveries WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        wait > 60.0 && wait <= 70.0,
        "next attempt in {wait}s; asked for 70"
    );
    assert_eq!(
        delivery::tick_once(&st).await.unwrap(),
        0,
        "held until the gate passes"
    );
    assert_eq!(connection_status(&pool, connection).await.0, "active");
}

/// With the flag off nothing is leased and no attempt spent; the row waits.
#[sqlx::test]
async fn a_tick_under_the_flag_off_leases_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "held"),
        Some("xoxb"),
    )
    .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    let st = state(pool.clone(), &mocks, Some(Flags::ConnectorsOff));
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 0);
    let (status, attempts, _) = delivery_status(&pool, id).await;
    assert_eq!((status.as_str(), attempts), ("pending", 0));
    let posts = mocks
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == "POST")
        .count();
    assert_eq!(posts, 0);
}

/// An unreadable flag set holds the tick — never all-on.
#[sqlx::test]
async fn a_tick_with_the_flag_read_broken_holds_everything(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "held"),
        Some("xoxb"),
    )
    .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    let st = state(pool.clone(), &mocks, Some(Flags::Unreadable));
    assert!(delivery::tick_once(&st).await.is_err());
    assert_eq!(delivery_status(&pool, id).await.1, 0, "no attempt spent");
}

/// Discord posts carry `wait=true` (the fixture requires it), and a `10015`
/// retires the connection.
#[sqlx::test]
async fn discord_posts_wait_and_an_unknown_webhook_retires(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let live = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Discord,
        "G0GUILD",
        "Telmoni alerts",
        &format!("{}/api/webhooks/1/live", mocks.uri()),
        None,
    )
    .await;
    let gone = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Discord,
        "G0GUILD",
        "Old alerts",
        &format!("{}/api/webhooks/2/gone", mocks.uri()),
        None,
    )
    .await;
    let live_row = seed_delivery(&pool, ORGANIZATION, live).await;
    let gone_row = seed_delivery(&pool, ORGANIZATION, gone).await;
    Mock::given(method("POST"))
        .and(path("/api/webhooks/1/live"))
        .and(query_param("wait", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m"})))
        .expect(1)
        .mount(&mocks)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/webhooks/2/gone"))
        .and(query_param("wait", "true"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "message": "Unknown Webhook", "code": 10015
        })))
        .expect(1)
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    delivery::tick_once(&st).await.unwrap();

    assert_eq!(delivery_status(&pool, live_row).await.0, "delivered");
    assert_eq!(delivery_status(&pool, gone_row).await.0, "failed");
    assert_eq!(connection_status(&pool, live).await.0, "active");
    let (status, error) = connection_status(&pool, gone).await;
    assert_eq!(status, "revoked");
    assert_eq!(error.as_deref(), Some("discord 10015: Unknown Webhook"));
    assert_eq!(
        feed_titles(&pool, ORGANIZATION).await,
        ["Discord disconnected"]
    );
    let notice: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT connection_id, kind FROM notifications.deliveries WHERE kind = 'connector_disconnected'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(notice.len(), 1);
    assert_eq!(notice[0].0, live);
}

/// A vendor outage is retried with backoff, and the budget ends it.
#[sqlx::test]
async fn a_vendor_outage_is_retried_and_the_budget_ends_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "down"),
        Some("xoxb"),
    )
    .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/down"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    for attempt in 1..=5 {
        sqlx::query("UPDATE notifications.deliveries SET next_attempt_at = now(), lease_until = NULL WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        delivery::tick_once(&st).await.unwrap();
        let (status, attempts, _) = delivery_status(&pool, id).await;
        assert_eq!(attempts, attempt);
        assert_eq!(status, if attempt < 5 { "pending" } else { "failed" });
    }
    assert_eq!(connection_status(&pool, connection).await.0, "active");
}

/// The KEK cannot be reached: the row is HELD — pending, attempt returned, no
/// backoff — and the vendor is never asked.
#[sqlx::test]
async fn a_kek_outage_holds_the_delivery_with_its_attempt_handed_back(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "held"),
        Some("xoxb"),
    )
    .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    mount_metadata_token(&mocks).await;
    Mock::given(method("POST"))
        .and(path(format!("/v1/{KMS_KEY}:decrypt")))
        .and(header_is("authorization", "Bearer ya29.test-bearer"))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "error": {
                "code": 503,
                "message": "The service is currently unavailable.",
                "status": "UNAVAILABLE"
            }
        })))
        .expect(2)
        .mount(&mocks)
        .await;
    let st = state_with_kek(
        pool.clone(),
        &mocks,
        None,
        both_connectors(&mocks),
        kms_kek(&mocks),
    );

    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    let (status, attempts, error) = delivery_status(&pool, id).await;
    assert_eq!(
        (status.as_str(), attempts),
        ("pending", 0),
        "held, not spent"
    );
    assert!(error.unwrap().contains("UNAVAILABLE"));
    let lease: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT lease_until FROM notifications.deliveries WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(lease.is_none(), "the lease was not released");
    assert_eq!(connection_status(&pool, connection).await.0, "active");

    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    assert_eq!(delivery_status(&pool, id).await.1, 0);
    let posts = mocks
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == "POST" && r.url.path().starts_with("/services/"))
        .count();
    assert_eq!(posts, 0, "the vendor was asked without a URL in hand");
}

/// The KEK will not wrap at install: a 502 and NOTHING stored — strict, unlike
/// the loop, because a grant must never sit under a key we cannot open.
#[sqlx::test]
async fn an_install_whose_kek_will_not_wrap_is_refused_with_nothing_stored(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let hook = slack_hook_url(&mocks, "neverstored");
    Mock::given(method("POST"))
        .and(path("/api/oauth.v2.access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(slack_access_ok(&hook)))
        .expect(1)
        .mount(&mocks)
        .await;
    mount_metadata_token(&mocks).await;
    Mock::given(method("POST"))
        .and(path(format!("/v1/{KMS_KEY}:encrypt")))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "error": {
                "code": 503,
                "message": "The service is currently unavailable.",
                "status": "UNAVAILABLE"
            }
        })))
        .expect(1)
        .mount(&mocks)
        .await;
    let st = state_with_kek(
        pool.clone(),
        &mocks,
        None,
        both_connectors(&mocks),
        kms_kek(&mocks),
    );

    let resp = app(&st)
        .oneshot(owner("POST", "/internal/connectors/slack/authorize", None))
        .await
        .unwrap();
    let raw_state = json_body(resp).await["state"].as_str().unwrap().to_owned();
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/slack/callback",
            Some(&format!(r#"{{"code":"c0de","state":"{raw_state}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = json_body(resp).await;
    assert_eq!(body["type"], "/errors/connectors/key-unavailable");
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("nothing was stored"),
        "{body}"
    );
    assert!(!body.to_string().contains("neverstored"), "{body}");

    for table in ["connections", "deliveries"] {
        let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM notifications.{table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
    assert!(feed_titles(&pool, ORGANIZATION).await.is_empty());
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audit, 0);
    let posts = mocks
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == "POST" && r.url.path().starts_with("/services/"))
        .count();
    assert_eq!(
        posts, 0,
        "a connected notice went out for a grant that was not stored"
    );
}

/// A project notice reaches only that project's connections; an organization
/// notice reaches every project's without enumerating projects.
#[sqlx::test]
async fn emit_fans_out_to_active_connections_at_the_right_level(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let mine = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0MINE",
        &slack_hook_url(&mocks, "mine"),
        Some("xoxb"),
    )
    .await;
    let revoked = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0DEAD",
        &slack_hook_url(&mocks, "dead"),
        Some("xoxb"),
    )
    .await;
    sqlx::query("UPDATE notifications.connections SET status = 'revoked' WHERE id = $1")
        .bind(revoked)
        .execute(&pool)
        .await
        .unwrap();
    let theirs = seed_connection(
        &pool,
        OTHER_ORGANIZATION,
        Provider::Slack,
        "T0002",
        "C0THEIRS",
        &slack_hook_url(&mocks, "theirs"),
        Some("xoxb"),
    )
    .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(emit_req(
            ORGANIZATION,
            true,
            r#"{"kind":"member_added","title":"Sam joined","body":"."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let queued: Vec<Uuid> = sqlx::query_scalar(
        "SELECT connection_id FROM notifications.deliveries WHERE kind = 'member_added'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        queued,
        [mine],
        "one row, for the active connection on the project"
    );
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM notifications.deliveries WHERE kind = 'member_added'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(wait_until_settled(&pool, ids[0]).await.0, "delivered");

    let resp = app(&st)
        .oneshot(emit_req(
            ORGANIZATION,
            false,
            r#"{"kind":"organization_alert","title":"Alert","body":"."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let queued: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT connection_id, project_id FROM notifications.deliveries WHERE kind = 'organization_alert'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(queued, [(mine, project_of(ORGANIZATION))]);
    assert!(
        !queued.iter().any(|(c, _)| *c == theirs),
        "another tenant's connection heard this organization's notice"
    );
}

fn sign(ts: &str, body: &str) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(SIGNING_SECRET.as_bytes()).unwrap();
    mac.update(format!("v0:{ts}:{body}").as_bytes());
    format!("v0={}", hex::encode(mac.finalize().into_bytes()))
}

fn slack_event(body: &str, ts: i64, signature: Option<&str>) -> Request<Body> {
    let ts = ts.to_string();
    let signature = signature.map_or_else(|| sign(&ts, body), ToOwned::to_owned);
    Request::post("/webhooks/slack")
        .header("x-slack-request-timestamp", ts)
        .header("x-slack-signature", signature)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn uninstalled(workspace: &str) -> String {
    serde_json::json!({
        "token": "verification-token-deprecated",
        "team_id": workspace,
        "api_app_id": "A0APP",
        "event": { "type": "app_uninstalled" },
        "type": "event_callback",
        "event_id": "Ev01",
        "event_time": 1_700_000_000
    })
    .to_string()
}

/// A bad signature, a stale timestamp and a missing header are all 401 and
/// change nothing: the signature is this lane's whole auth.
#[sqlx::test]
async fn an_unverified_event_is_refused_and_changes_nothing(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "x"),
        Some("xoxb"),
    )
    .await;
    let st = state(pool.clone(), &mocks, None);
    let now = chrono::Utc::now().timestamp();
    let body = uninstalled(SLACK_WORKSPACE);

    let forged = slack_event(&body, now, Some("v0=deadbeef"));
    let resp = app(&st).oneshot(forged).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let stale = slack_event(&body, now - 360, None);
    let resp = app(&st).oneshot(stale).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "six minutes old");

    let unsigned = Request::post("/webhooks/slack")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.clone()))
        .unwrap();
    let resp = app(&st).oneshot(unsigned).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    assert_eq!(connection_status(&pool, connection).await.0, "active");
}

/// `url_verification` answers the challenge — signed like everything else.
#[sqlx::test]
async fn url_verification_echoes_the_challenge(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let st = state(pool.clone(), &mocks, None);
    let body = r#"{"token":"x","challenge":"3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P","type":"url_verification"}"#;
    let resp = app(&st)
        .oneshot(slack_event(body, chrono::Utc::now().timestamp(), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["challenge"],
        "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P"
    );
    let resp = app(&st)
        .oneshot(slack_event(
            body,
            chrono::Utc::now().timestamp(),
            Some("v0=00"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// `app_uninstalled` revokes every connection into the workspace across
/// tenants, audits once per organization, and a redelivery changes nothing.
#[sqlx::test]
async fn an_uninstall_revokes_the_workspace_across_tenants_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let mine = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0MINE",
        &slack_hook_url(&mocks, "mine"),
        Some("xoxb"),
    )
    .await;
    let theirs = seed_connection(
        &pool,
        OTHER_ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0THEIRS",
        &slack_hook_url(&mocks, "theirs"),
        Some("xoxb"),
    )
    .await;
    let elsewhere = seed_connection(
        &pool,
        OTHER_ORGANIZATION,
        Provider::Slack,
        "T0002",
        "C0ELSEWHERE",
        &slack_hook_url(&mocks, "elsewhere"),
        Some("xoxb"),
    )
    .await;
    let queued = seed_delivery(&pool, ORGANIZATION, mine).await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(slack_event(
            &uninstalled(SLACK_WORKSPACE),
            chrono::Utc::now().timestamp(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["revoked"], 2);

    for id in [mine, theirs] {
        let (status, error) = connection_status(&pool, id).await;
        assert_eq!(status, "revoked", "{id}");
        assert_eq!(error.as_deref(), Some("slack: app_uninstalled"));
    }
    assert_eq!(
        connection_status(&pool, elsewhere).await.0,
        "active",
        "another workspace is untouched"
    );
    assert_eq!(delivery_status(&pool, queued).await.0, "failed");
    assert_eq!(
        feed_titles(&pool, ORGANIZATION).await,
        ["Slack disconnected"]
    );
    assert_eq!(
        feed_titles(&pool, OTHER_ORGANIZATION).await,
        ["Slack disconnected"]
    );

    let audit: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT organization_id, actor_id, action, in_project FROM audit.events ORDER BY organization_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(audit.len(), 2, "one row per organization: {audit:?}");
    assert_eq!(audit[0].0, ORGANIZATION);
    assert_eq!(audit[1].0, OTHER_ORGANIZATION);
    for (_, actor, action, in_project) in &audit {
        assert_eq!(actor, "external:slack");
        assert_eq!(action, "updated");
        assert!(in_project.is_none());
    }

    let resp = app(&st)
        .oneshot(slack_event(
            &uninstalled(SLACK_WORKSPACE),
            chrono::Utc::now().timestamp(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["revoked"], 0);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 2, "a redelivery wrote nothing");
    assert_eq!(feed_titles(&pool, ORGANIZATION).await.len(), 1);
}

/// Removing the LAST connection into a Slack workspace uninstalls the app
/// there; removing one that shares the workspace does not.
#[sqlx::test]
async fn a_slack_disconnect_uninstalls_only_when_it_was_the_last(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let first = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ONE",
        &slack_hook_url(&mocks, "one"),
        Some("xoxb-shared"),
    )
    .await;
    let second = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0TWO",
        &slack_hook_url(&mocks, "two"),
        Some("xoxb-shared"),
    )
    .await;
    let queued = seed_delivery(&pool, ORGANIZATION, first).await;
    Mock::given(method("POST"))
        .and(path("/api/apps.uninstall"))
        .and(header_is("authorization", "Bearer xoxb-shared"))
        .and(body_string_contains("client_id=slack_client_id"))
        .and(body_string_contains("client_secret=slack_client_secret"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(owner(
            "DELETE",
            &format!("/internal/connectors/{first}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let uninstalls = |reqs: Vec<wiremock::Request>| {
        reqs.into_iter()
            .filter(|r| r.url.path() == "/api/apps.uninstall")
            .count()
    };
    assert_eq!(uninstalls(mocks.received_requests().await.unwrap()), 0);
    let gone: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications.deliveries WHERE id = $1")
            .bind(queued)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(gone, 0, "the queue cascaded with the row");

    let resp = app(&st)
        .oneshot(owner(
            "DELETE",
            &format!("/internal/connectors/{second}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(uninstalls(mocks.received_requests().await.unwrap()), 1);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications.connections")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    let audit: Vec<(String, String)> =
        sqlx::query_as("SELECT action, resource_kind FROM audit.events WHERE organization_id = $1")
            .bind(ORGANIZATION)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(audit.len(), 2);
    assert!(
        audit
            .iter()
            .all(|(a, k)| a == "deleted" && k == "connector")
    );
}

/// A Discord disconnect deletes the webhook once, a 404 still deletes the row,
/// and a member cannot.
#[sqlx::test]
async fn a_discord_disconnect_deletes_the_webhook(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Discord,
        "G0GUILD",
        "Telmoni alerts",
        &format!("{}/api/webhooks/9/tok", mocks.uri()),
        None,
    )
    .await;
    Mock::given(method("DELETE"))
        .and(path("/api/webhooks/9/tok"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "message": "Unknown Webhook", "code": 10015
        })))
        .expect(1)
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(req_as(
            "DELETE",
            &format!("/internal/connectors/{connection}"),
            ORGANIZATION,
            MEMBER,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        vendor_requests(&mocks).await.is_empty(),
        "a refused disconnect reached the vendor"
    );

    let resp = app(&st)
        .oneshot(owner(
            "DELETE",
            &format!("/internal/connectors/{connection}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications.connections")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    let theirs = seed_connection(
        &pool,
        OTHER_ORGANIZATION,
        Provider::Discord,
        "G0GUILD",
        "Theirs",
        &format!("{}/api/webhooks/8/tok", mocks.uri()),
        None,
    )
    .await;
    let resp = app(&st)
        .oneshot(owner(
            "DELETE",
            &format!("/internal/connectors/{theirs}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Disconnecting is a write, and the matrix gives a member none: the row
/// stands, nothing is audited, and the vendor is never asked.
#[sqlx::test]
async fn a_member_cannot_disconnect(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "kept"),
        Some("xoxb"),
    )
    .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(req_as(
            "DELETE",
            &format!("/internal/connectors/{connection}"),
            ORGANIZATION,
            MEMBER,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = json_body(resp).await;
    assert_eq!(body["detail"], "role member may not delete connector");

    assert_eq!(connection_status(&pool, connection).await.0, "active");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications.connections")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "the row was removed by a role that may not");
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audit, 0);
    assert!(
        vendor_requests(&mocks).await.is_empty(),
        "a refused disconnect reached the vendor"
    );
}

/// A test send reports the vendor's answer with a 200, and a dead install
/// retires the row as the loop would.
#[sqlx::test]
async fn a_test_send_reports_the_vendors_answer_and_retires_on_a_dead_install(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "testme"),
        Some("xoxb"),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/testme"))
        .and(body_string_contains("Slack is connected"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .up_to_n_times(1)
        .mount(&mocks)
        .await;
    Mock::given(method("POST"))
        .and(path("/services/T0001/B0HOOK/testme"))
        .respond_with(ResponseTemplate::new(403).set_body_string("invalid_token"))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(req_as(
            "POST",
            &format!("/internal/connectors/{connection}/test"),
            ORGANIZATION,
            MEMBER,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        vendor_requests(&mocks).await.is_empty(),
        "a refused test send reached the vendor"
    );

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &format!("/internal/connectors/{connection}/test"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({ "delivered": true, "error": null })
    );
    let touched: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_delivery_at FROM notifications.connections WHERE id = $1")
            .bind(connection)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(touched.is_some());

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &format!("/internal/connectors/{connection}/test"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({ "delivered": false, "error": "slack: invalid_token" })
    );
    assert_eq!(connection_status(&pool, connection).await.0, "revoked");

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &format!("/internal/connectors/{connection}/test"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

/// The organization purge tears every grant down upstream and removes every
/// row, sparing the other tenant.
#[sqlx::test]
async fn the_purge_tears_down_the_grants_and_removes_every_row(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let slack = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0MINE",
        &slack_hook_url(&mocks, "mine"),
        Some("xoxb-mine"),
    )
    .await;
    let shared = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        "T0SHARED",
        "C0SHARED",
        &slack_hook_url(&mocks, "shared"),
        Some("xoxb-shared"),
    )
    .await;
    let discord = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Discord,
        "G0GUILD",
        "Alerts",
        &format!("{}/api/webhooks/7/tok", mocks.uri()),
        None,
    )
    .await;
    let theirs = seed_connection(
        &pool,
        OTHER_ORGANIZATION,
        Provider::Slack,
        "T0SHARED",
        "C0THEIRS",
        &slack_hook_url(&mocks, "theirs"),
        Some("xoxb-shared"),
    )
    .await;
    seed_delivery(&pool, ORGANIZATION, slack).await;
    {
        let mut tx = project_scope(&pool, &pid(ORGANIZATION)).await.unwrap();
        db::insert_oauth_state(
            &mut tx,
            "hash",
            &pid(ORGANIZATION),
            &oid(ORGANIZATION),
            &uid(OWNER),
            Provider::Slack,
            600,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    Mock::given(method("POST"))
        .and(path("/api/apps.uninstall"))
        .and(header_is("authorization", "Bearer xoxb-mine"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .expect(1)
        .mount(&mocks)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/webhooks/7/tok"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let resp = app(&st)
        .oneshot(
            Request::post(format!(
                "/internal/notifications/organizations/{ORGANIZATION}/purge"
            ))
            .header("x-service-secret", SECRET)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let uninstalls: Vec<String> = mocks
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/api/apps.uninstall")
        .map(|r| {
            r.headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(uninstalls, ["Bearer xoxb-mine"]);
    for (table, expect) in [
        ("connections", 1i64),
        ("deliveries", 0),
        ("oauth_states", 0),
    ] {
        let left: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM notifications.{table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, expect, "{table}");
    }
    let _ = (shared, discord);
    assert_eq!(connection_status(&pool, theirs).await.0, "active");
}

#[sqlx::test]
async fn the_sweep_removes_finished_deliveries_and_expired_states(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0",
        &slack_hook_url(&mocks, "x"),
        Some("xoxb"),
    )
    .await;
    let old = seed_delivery(&pool, ORGANIZATION, connection).await;
    let fresh = seed_delivery(&pool, ORGANIZATION, connection).await;
    sqlx::query("UPDATE notifications.deliveries SET status = 'delivered', updated_at = now() - interval '40 days' WHERE id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    {
        // Seeded in the project's scope: a handshake is only ever begun there,
        // and the lane that sweeps them holds no INSERT.
        let mut tx = project_scope(&pool, &pid(ORGANIZATION)).await.unwrap();
        db::insert_oauth_state(
            &mut tx,
            "expired",
            &pid(ORGANIZATION),
            &oid(ORGANIZATION),
            &uid(OWNER),
            Provider::Slack,
            -1,
        )
        .await
        .unwrap();
        db::insert_oauth_state(
            &mut tx,
            "live",
            &pid(ORGANIZATION),
            &oid(ORGANIZATION),
            &uid(OWNER),
            Provider::Slack,
            600,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let st = state(pool.clone(), &mocks, None);
    let removed = telmoni_notifications::retention::sweep_once(&st)
        .await
        .unwrap();
    assert_eq!(removed, 2);
    let deliveries: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM notifications.deliveries")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(deliveries, [fresh]);
    let states: Vec<String> =
        sqlx::query_scalar("SELECT state_hash FROM notifications.oauth_states")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(states, ["live"]);
}

/// A webhook connects by URL: checked, secret answered ONCE, row sealed, and
/// only the host shown.
#[sqlx::test]
async fn a_webhook_is_connected_by_url_and_its_secret_is_answered_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let st = state(pool.clone(), &mocks, None);
    let body = r#"{"url":"https://hooks.example.invalid/telmoni/events?project=a#frag"}"#;

    let resp = app(&st)
        .oneshot(req_as(
            "POST",
            "/internal/connectors/webhook",
            ORGANIZATION,
            MEMBER,
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app(&st)
        .oneshot(owner("POST", "/internal/connectors/webhook", Some(body)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let answer = json_body(resp).await;
    let secret = answer["signing_secret"].as_str().unwrap().to_owned();
    assert!(
        secret.starts_with("whsec_") && secret.len() == 70,
        "{secret}"
    );
    let connection = &answer["connection"];
    assert_eq!(connection["provider"], "webhook");
    assert_eq!(connection["channel_name"], "hooks.example.invalid");
    assert_eq!(connection["external_workspace_id"], "hooks.example.invalid");
    assert_eq!(connection["channel_id"].as_str().unwrap().len(), 64);
    assert_eq!(connection["status"], "active");
    assert!(
        !answer.to_string().contains("/telmoni/events"),
        "the URL is never answered back: {answer}"
    );
    let id: Uuid = connection["id"].as_str().unwrap().parse().unwrap();

    let mut tx = maintenance_scope(&pool, NotificationsLane).await.unwrap();
    let sealed = db::sealed_connection(&mut tx, id).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        open_target(&sealed).await.expose(),
        "https://hooks.example.invalid/telmoni/events?project=a"
    );
    let dek = local_kek()
        .unwrap_dek(
            &reqwest::Client::new(),
            &sealed.wrapped_dek,
            sealed.id.as_bytes(),
            sealed.key_version,
        )
        .await
        .unwrap();
    let token = Vault::new()
        .open(&dek, &sealed.token().unwrap(), sealed.id.as_bytes())
        .unwrap();
    assert_eq!(token.expose(), secret);
    let (target_ct, token_ct, scopes): (Vec<u8>, Vec<u8>, String) = sqlx::query_as(
        "SELECT target_ciphertext, token_ciphertext, scopes
           FROM notifications.connections WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!target_ct.windows(7).any(|w| w == b"telmoni"));
    assert!(!token_ct.windows(6).any(|w| w == b"whsec_"));
    assert_eq!(scopes, webhook::SCHEME);

    let resp = app(&st)
        .oneshot(owner("GET", "/internal/connectors", None))
        .await
        .unwrap();
    let listing = json_body(resp).await;
    assert_eq!(listing["enabled"]["webhook"], true);
    let listed = listing.to_string();
    assert!(listed.contains("hooks.example.invalid"), "{listed}");
    assert!(
        !listed.contains("/telmoni/events") && !listed.contains("whsec_"),
        "{listed}"
    );

    let (action, kind, metadata): (String, String, serde_json::Value) = sqlx::query_as(
        "SELECT action, resource_kind, metadata FROM audit.events WHERE organization_id = $1",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((action.as_str(), kind.as_str()), ("created", "connector"));
    assert_eq!(metadata["provider"], "webhook");
    assert_eq!(metadata["host"], "hooks.example.invalid");
    assert!(!metadata.to_string().contains("whsec_"));
    assert_eq!(
        feed_titles(&pool, ORGANIZATION).await,
        ["Webhook connected"]
    );

    let resp = app(&st)
        .oneshot(owner("POST", "/internal/connectors/webhook", Some(body)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let resp = app(&st)
        .oneshot(req_as(
            "POST",
            "/internal/connectors/webhook",
            OTHER_ORGANIZATION,
            OTHER_OWNER,
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/webhook/authorize",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let states: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications.oauth_states")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(states, 0, "no state row for a handshake that cannot finish");
}

/// The registration-time guard through the handler: plaintext, credentials,
/// local names and every spelling of a private address are refused.
#[sqlx::test]
async fn a_webhook_registration_is_https_only_and_never_a_private_or_local_host(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let st = state(pool.clone(), &mocks, None);
    let loopback_mock = format!("{}/hooks/in", mocks.uri());
    let cases: Vec<(String, &str)> = vec![
        ("http://hooks.example.invalid/x".into(), "must be https"),
        (loopback_mock, "must be https"),
        (
            "https://user:pw@hooks.example.invalid/x".into(),
            "username or password",
        ),
        ("https://localhost/hook".into(), "local address"),
        ("https://app.localhost/hook".into(), "local address"),
        ("https://metadata.google.internal/x".into(), "local address"),
        ("https://127.0.0.1/hook".into(), "private or local"),
        ("https://127.1/hook".into(), "private or local"),
        ("https://10.0.0.9/hook".into(), "private or local"),
        ("https://192.168.1.1/hook".into(), "private or local"),
        ("https://169.254.169.254/latest".into(), "private or local"),
        ("https://100.64.0.1/hook".into(), "private or local"),
        ("https://[::1]/hook".into(), "private or local"),
        (
            "https://[::ffff:169.254.169.254]/x".into(),
            "private or local",
        ),
        ("https://[64:ff9b::a9fe:a9fe]/x".into(), "private or local"),
        ("https://[fd00::1]/hook".into(), "private or local"),
        ("not a url".into(), "must be a URL"),
    ];
    for (url, why) in cases {
        let resp = app(&st)
            .oneshot(owner(
                "POST",
                "/internal/connectors/webhook",
                Some(&serde_json::json!({ "url": url }).to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{url}");
        let body = json_body(resp).await;
        assert!(body.to_string().contains(why), "{url}: {body}");
    }
    for table in ["connections", "deliveries"] {
        let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM notifications.{table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit.events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audit, 0);
    assert!(feed_titles(&pool, ORGANIZATION).await.is_empty());
    assert!(
        vendor_requests(&mocks).await.is_empty(),
        "a refused registration reached a vendor"
    );
}

/// A delivery is a signed POST that verifies over exactly the bytes that
/// arrived, with the delivery id in a header and the body.
#[sqlx::test]
async fn a_webhook_delivery_is_signed_over_the_bytes_it_sends_and_names_its_delivery_id(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let secret = "whsec_0123456789abcdef";
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/in", mocks.uri()),
        secret,
    )
    .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    Mock::given(method("POST"))
        .and(path("/hooks/in"))
        .and(header_is("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    assert_eq!(delivery_status(&pool, id).await.0, "delivered");
    let posted = posts_to(&mocks, "/hooks/in").await;
    assert_eq!(posted.len(), 1);
    let req = &posted[0];
    let signature = req
        .headers
        .get("telmoni-signature")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(signature.starts_with("t="), "{signature}");
    assert_eq!(
        req.headers
            .get("telmoni-delivery-id")
            .unwrap()
            .to_str()
            .unwrap(),
        id.to_string()
    );
    assert!(verify_signature(secret, signature, &req.body));
    assert!(!verify_signature("whsec_other", signature, &req.body));
    let mut tampered = req.body.clone();
    tampered.push(b' ');
    assert!(!verify_signature(secret, signature, &tampered));
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["id"], id.to_string());
    assert_eq!(body["kind"], "member_added");
    assert_eq!(body["title"], "Sam joined");
    assert_eq!(body["body"], "Sam accepted the invitation.");
    assert!(body["sent_at"].as_str().unwrap().ends_with('Z'));
    assert!(
        !String::from_utf8_lossy(&req.body).contains("whsec_"),
        "the secret travels only into the MAC"
    );

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &format!("/internal/connectors/{connection}/test"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({ "delivered": true, "error": null })
    );
    let posted = posts_to(&mocks, "/hooks/in").await;
    assert_eq!(posted.len(), 2);
    let test_req = &posted[1];
    let test_id = test_req
        .headers
        .get("telmoni-delivery-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_ne!(test_id, id.to_string());
    assert!(verify_signature(
        secret,
        test_req
            .headers
            .get("telmoni-signature")
            .unwrap()
            .to_str()
            .unwrap(),
        &test_req.body
    ));
    let body: serde_json::Value = serde_json::from_slice(&test_req.body).unwrap();
    assert_eq!(body["kind"], "connector_connected");
    assert_eq!(body["title"], "Webhook is connected");
}

/// A 404 is retried and the connection stands; only the breaker — ten failed
/// sends in a row — retires a customer's endpoint.
#[sqlx::test]
async fn a_refusing_endpoint_is_retried_to_its_budget_and_ten_in_a_row_trip_the_breaker(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/dead", mocks.uri()),
        "whsec_dead",
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/hooks/dead"))
        .respond_with(ResponseTemplate::new(404).set_body_string("no such route"))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let first = seed_delivery(&pool, ORGANIZATION, connection).await;
    for attempt in 1..=5 {
        sqlx::query("UPDATE notifications.deliveries SET next_attempt_at = now(), lease_until = NULL WHERE id = $1")
            .bind(first)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
        let (status, attempts, error) = delivery_status(&pool, first).await;
        assert_eq!(attempts, attempt);
        assert_eq!(status, if attempt < 5 { "pending" } else { "failed" });
        assert_eq!(
            error.as_deref(),
            Some("endpoint returned 404 Not Found: no such route")
        );
        assert_eq!(connection_status(&pool, connection).await.0, "active");
        assert_eq!(breaker_count(&pool, connection).await, attempt);
    }

    let more: Vec<Uuid> = {
        let mut ids = Vec::new();
        for _ in 0..5 {
            ids.push(seed_delivery(&pool, ORGANIZATION, connection).await);
        }
        ids
    };
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 5);
    let (status, error) = connection_status(&pool, connection).await;
    assert_eq!(status, "errored");
    let error = error.unwrap();
    assert!(
        error.starts_with(
            "10 deliveries in a row failed; the last answer was: endpoint returned 404"
        ),
        "{error}"
    );
    for id in more {
        assert_eq!(delivery_status(&pool, id).await.0, "failed", "{id}");
    }
    assert_eq!(
        feed_titles(&pool, ORGANIZATION).await,
        ["Webhook disconnected"],
        "one notice, not one per row"
    );
    let posted_before = posts_to(&mocks, "/hooks/dead").await.len();
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 0);
    assert_eq!(posts_to(&mocks, "/hooks/dead").await.len(), posted_before);
}

/// One success closes the count, so a merely flaky endpoint is never retired.
#[sqlx::test]
async fn a_success_closes_the_breakers_count(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/flaky", mocks.uri()),
        "whsec_flaky",
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/hooks/flaky"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&mocks)
        .await;
    Mock::given(method("POST"))
        .and(path("/hooks/flaky"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    seed_delivery(&pool, ORGANIZATION, connection).await;
    seed_delivery(&pool, ORGANIZATION, connection).await;
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 2);
    assert_eq!(breaker_count(&pool, connection).await, 2);

    let ok = seed_delivery(&pool, ORGANIZATION, connection).await;
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    assert_eq!(delivery_status(&pool, ok).await.0, "delivered");
    assert_eq!(breaker_count(&pool, connection).await, 0);
    assert_eq!(connection_status(&pool, connection).await.0, "active");
}

/// **One tick drains the queue, not one batch of it.**
#[sqlx::test]
async fn one_tick_drains_a_backlog_larger_than_one_batch(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/bulk", mocks.uri()),
        "whsec_bulk",
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/hooks/bulk"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);

    let queued = i32::try_from(st.config.delivery_batch).unwrap() + 20;
    seed_backlog(&pool, ORGANIZATION, connection, queued).await;

    let attempted = delivery::tick_once(&st).await.unwrap();
    assert_eq!(attempted, u32::try_from(queued).unwrap());
    assert_eq!(
        queue_counts(&pool).await,
        (0, i64::from(queued)),
        "the whole backlog drained in one tick"
    );
}

/// The queue-depth line counts what is waiting under the SERVICE role. Read
/// without maintenance scope, forced RLS hid every row and it logged zero
/// forever — which the stalled-queue alert would have read as a healthy queue.
#[sqlx::test]
async fn the_queue_depth_sees_pending_rows_under_the_service_role(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/depth", mocks.uri()),
        "whsec_depth",
    )
    .await;
    seed_backlog(&pool, ORGANIZATION, connection, 3).await;
    let st = state(pool.clone(), &mocks, None);

    let (pending, oldest_age_secs) = delivery::queue_depth(&st).await.unwrap();
    assert_eq!(pending, 3);
    assert!(oldest_age_secs.is_some(), "a pending row has an age");
}

/// **One key unwrapped per CONNECTION per batch, never one per row.**
#[sqlx::test]
async fn a_batch_unwraps_one_key_per_connection_and_not_one_per_row(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    mount_metadata_token(&mocks).await;
    Mock::given(method("POST"))
        .and(path(format!("/v1/{KMS_KEY}:decrypt")))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "error": { "code": 503, "message": "unavailable", "status": "UNAVAILABLE" }
        })))
        .mount(&mocks)
        .await;
    let st = state_with_kek(
        pool.clone(),
        &mocks,
        None,
        both_connectors(&mocks),
        kms_kek(&mocks),
    );

    for n in 0..3 {
        let connection = seed_webhook(
            &pool,
            ORGANIZATION,
            &format!("{}/hooks/fan{n}", mocks.uri()),
            "whsec_fan",
        )
        .await;
        seed_backlog(&pool, ORGANIZATION, connection, 5).await;
    }

    assert_eq!(delivery::tick_once(&st).await.unwrap(), 15);
    assert_eq!(
        decrypt_calls(&mocks).await,
        3,
        "fifteen rows over three connections is three unwraps"
    );
    assert_eq!(queue_counts(&pool).await, (15, 0));
    let attempts: Vec<i32> =
        sqlx::query_scalar("SELECT DISTINCT attempts FROM notifications.deliveries")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(attempts, vec![0], "every attempt was handed back");
}

/// **A held batch ends the tick**: held rows are eligible again at once, so a
/// tick that kept draining would re-lease them in a tight loop.
#[sqlx::test]
async fn a_kek_outage_ends_the_tick_instead_of_draining_against_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    mount_metadata_token(&mocks).await;
    Mock::given(method("POST"))
        .and(path(format!("/v1/{KMS_KEY}:decrypt")))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "error": { "code": 503, "message": "unavailable", "status": "UNAVAILABLE" }
        })))
        .mount(&mocks)
        .await;
    let st = state_with_kek(
        pool.clone(),
        &mocks,
        None,
        both_connectors(&mocks),
        kms_kek(&mocks),
    );
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/held", mocks.uri()),
        "whsec_held",
    )
    .await;
    let queued = i32::try_from(st.config.delivery_batch).unwrap() + 20;
    seed_backlog(&pool, ORGANIZATION, connection, queued).await;

    let attempted = delivery::tick_once(&st).await.unwrap();
    assert_eq!(
        attempted,
        u32::try_from(st.config.delivery_batch).unwrap(),
        "one batch was leased, held, and the tick stopped there"
    );
    assert_eq!(
        decrypt_calls(&mocks).await,
        1,
        "one unwrap, not one per round"
    );
    assert_eq!(queue_counts(&pool).await, (i64::from(queued), 0));
    let attempts: Vec<i32> =
        sqlx::query_scalar("SELECT DISTINCT attempts FROM notifications.deliveries")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(attempts, vec![0], "every attempt was handed back");
}

/// `410 Gone` is the one answer a receiver gives that means what it says.
#[sqlx::test]
async fn a_gone_endpoint_is_a_bad_target_at_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/gone", mocks.uri()),
        "whsec_gone",
    )
    .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    Mock::given(method("POST"))
        .and(path("/hooks/gone"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    let (status, error) = connection_status(&pool, connection).await;
    assert_eq!(status, "errored");
    assert_eq!(error.as_deref(), Some("endpoint returned 410 Gone"));
    assert_eq!(delivery_status(&pool, id).await.0, "failed");
}

/// A rotation answers a new secret once, signs with it alone, and revives an
/// errored webhook.
#[sqlx::test]
async fn a_rotated_secret_signs_the_next_delivery_and_revives_an_errored_webhook(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let old_secret = "whsec_old";
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/rotated", mocks.uri()),
        old_secret,
    )
    .await;
    sqlx::query(
        "UPDATE notifications.connections
            SET status = 'errored', consecutive_failures = 10, last_error = 'dead'
          WHERE id = $1",
    )
    .bind(connection)
    .execute(&pool)
    .await
    .unwrap();
    let slack = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "norotate"),
        Some("xoxb"),
    )
    .await;
    let foreign = seed_webhook(
        &pool,
        OTHER_ORGANIZATION,
        &format!("{}/hooks/foreign", mocks.uri()),
        "whsec_foreign",
    )
    .await;
    let st = state(pool.clone(), &mocks, None);
    let rotate = |connection: Uuid| format!("/internal/connectors/{connection}/rotate");
    let hard_cut = Some(r#"{"keep_previous_for_hours":0}"#);

    let resp = app(&st)
        .oneshot(req_as(
            "POST",
            &rotate(connection),
            ORGANIZATION,
            MEMBER,
            hard_cut,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = app(&st)
        .oneshot(owner("POST", &rotate(slack), hard_cut))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = app(&st)
        .oneshot(owner("POST", &rotate(foreign), hard_cut))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "another tenant's row is invisible"
    );
    let resp = app(&st)
        .oneshot(owner("POST", &rotate(Uuid::now_v7()), hard_cut))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &rotate(connection),
            Some(r#"{"keep_previous_for_hours":25}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "an overlap past a day is refused"
    );

    let resp = app(&st)
        .oneshot(owner("POST", &rotate(connection), hard_cut))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let answer = json_body(resp).await;
    let new_secret = answer["signing_secret"].as_str().unwrap().to_owned();
    assert!(new_secret.starts_with("whsec_") && new_secret != old_secret);
    assert_eq!(answer["connection"]["status"], "active");
    assert_eq!(
        answer["connection"]["previous_secret_expires_at"],
        serde_json::Value::Null,
        "a hard cut keeps nothing"
    );
    let (status, error) = connection_status(&pool, connection).await;
    assert_eq!((status.as_str(), error), ("active", None));
    assert_eq!(breaker_count(&pool, connection).await, 0);
    let (action, metadata): (String, serde_json::Value) = sqlx::query_as(
        "SELECT action, metadata FROM audit.events
          WHERE organization_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(action, "updated");
    assert_eq!(metadata["rotated"], "signing_secret");
    assert!(!metadata.to_string().contains("whsec_"));

    Mock::given(method("POST"))
        .and(path("/hooks/rotated"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mocks)
        .await;
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    assert_eq!(delivery_status(&pool, id).await.0, "delivered");
    let posted = posts_to(&mocks, "/hooks/rotated").await;
    let signature = posted[0]
        .headers
        .get("telmoni-signature")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(verify_signature(&new_secret, signature, &posted[0].body));
    assert!(!verify_signature(old_secret, signature, &posted[0].body));
}

/// The dial-time guard: a loopback target is refused before any socket opens,
/// and the row is retired as a bad target.
#[sqlx::test]
async fn a_target_the_guard_refuses_is_retired_without_a_socket(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let hook = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/never", mocks.uri()),
        "whsec_never",
    )
    .await;
    let slack = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0NEVER",
        &slack_hook_url(&mocks, "never"),
        Some("xoxb"),
    )
    .await;
    let a = seed_delivery(&pool, ORGANIZATION, hook).await;
    let b = seed_delivery(&pool, ORGANIZATION, slack).await;
    let st = state_full(
        pool.clone(),
        &mocks,
        None,
        both_connectors(&mocks),
        local_kek(),
        Egress::guarded(std::time::Duration::from_secs(5), "telmoni-test").unwrap(),
    );

    assert_eq!(delivery::tick_once(&st).await.unwrap(), 2);
    for connection in [hook, slack] {
        let (status, error) = connection_status(&pool, connection).await;
        assert_eq!(status, "errored", "{connection}");
        assert_eq!(
            error.as_deref(),
            Some("the target points at a private or local address")
        );
    }
    for id in [a, b] {
        assert_eq!(delivery_status(&pool, id).await.0, "failed");
    }
    assert!(
        mocks.received_requests().await.unwrap().is_empty(),
        "a socket was opened to a host the guard refuses"
    );
}

/// No key, no webhook: the tile lists as unavailable and connecting is refused.
#[sqlx::test]
async fn a_deployment_without_the_webhook_lists_it_disabled_and_refuses_it(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let no_webhook = Connectors {
        webhook: None,
        ..both_connectors(&mocks)
    };
    let st = state_with(pool.clone(), &mocks, None, no_webhook);
    let resp = app(&st)
        .oneshot(owner("GET", "/internal/connectors", None))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["enabled"]["webhook"], false);
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/webhook",
            Some(r#"{"url":"https://hooks.example.invalid/x"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// The redaction reaches the delivery queue too: a row still waiting to be
/// sent goes out naming a former member, and a delivered row, which keeps
/// its body for a resend, keeps a redacted one, so a resend cannot carry
/// the name again. What the vendor already received is beyond reach, and
/// this proves nothing about it.
#[sqlx::test]
async fn erasing_a_person_rewrites_the_deliveries_that_named_them(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0ALERTS",
        &slack_hook_url(&mocks, "redacted"),
        Some("xoxb"),
    )
    .await;
    let enqueue = |subject_user_id: Option<&'static str>, subject: &'static str| {
        let pool = pool.clone();
        async move {
            let mut tx = project_scope(&pool, &pid(ORGANIZATION)).await.unwrap();
            let id = db::enqueue_delivery(
                &mut tx,
                &db::NewDelivery {
                    connection_id: connection,
                    project_id: &project_of(ORGANIZATION),
                    organization_id: &oid(ORGANIZATION),
                    kind: "member_added",
                    subject,
                    body: "Sam Leaver accepted the invitation and is now admin on the project.",
                    subject_user_id,
                },
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            id
        }
    };
    let queued = enqueue(Some("user_leaver"), "Sam Leaver joined the project").await;
    let delivered = enqueue(Some("user_leaver"), "Sam Leaver joined the project").await;
    let other = enqueue(Some("user_stayer"), "Kim Stayer joined the project").await;
    {
        let mut tx = maintenance_scope(&pool, NotificationsLane).await.unwrap();
        db::mark_delivered(&mut tx, &[delivered]).await.unwrap();
        tx.commit().await.unwrap();
    }

    let mut tx = maintenance_scope(&pool, NotificationsLane).await.unwrap();
    let redacted = db::redact_person(&mut tx, &uid("user_leaver"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(redacted, 2, "both rows that named the person");

    let row = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (String, String, Option<String>)>(
                "SELECT subject, body, subject_user_id FROM notifications.deliveries WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        row(queued).await,
        (
            "A former member joined the project".to_owned(),
            "A former member accepted the invitation and is now a member on the project."
                .to_owned(),
            None
        ),
        "the queued row goes out as rewritten"
    );
    assert_eq!(
        row(delivered).await,
        (
            "A former member joined the project".to_owned(),
            "A former member accepted the invitation and is now a member on the project."
                .to_owned(),
            None
        ),
        "the delivered row keeps a redacted body a resend would carry"
    );
    assert_eq!(
        row(other).await,
        (
            "Kim Stayer joined the project".to_owned(),
            "Sam Leaver accepted the invitation and is now admin on the project.".to_owned(),
            Some("user_stayer".to_owned())
        ),
        "a row naming somebody else is untouched"
    );
}

/// Which connections a delivery of `kind` was queued for, in id order.
async fn queued_for(pool: &PgPool, kind: &str) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT connection_id FROM notifications.deliveries WHERE kind = $1 ORDER BY connection_id",
    )
    .bind(kind)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A webhook chooses its notices: the fan-out skips one that did not choose
/// the kind, the choice is stored once each in wire order, and only a
/// webhook's owner may make it.
#[sqlx::test]
async fn a_webhook_receives_only_the_events_it_chose(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&mocks)
        .await;
    let everything = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/all", mocks.uri()),
        "whsec_all",
    )
    .await;
    let chooser = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/some", mocks.uri()),
        "whsec_some",
    )
    .await;
    let slack = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0EVENTS",
        &slack_hook_url(&mocks, "events"),
        Some("xoxb"),
    )
    .await;
    let st = state(pool.clone(), &mocks, None);
    let events = |connection: Uuid| format!("/internal/connectors/{connection}/events");
    let choose =
        Some(r#"{"event_kinds":["connector_disconnected","member_added","member_added"]}"#);

    let resp = app(&st)
        .oneshot(req_as(
            "PUT",
            &events(chooser),
            ORGANIZATION,
            MEMBER,
            choose,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    for (body, why) in [
        (r#"{"event_kinds":[]}"#, "an empty choice"),
        (
            r#"{"event_kinds":["nonexistent_event"]}"#,
            "a kind nothing emits",
        ),
        ("{}", "a replace that states no value"),
    ] {
        let resp = app(&st)
            .oneshot(owner("PUT", &events(chooser), Some(body)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{why}");
    }
    let resp = app(&st)
        .oneshot(owner("PUT", &events(slack), choose))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a vendor channel receives everything; only a webhook chooses"
    );

    let resp = app(&st)
        .oneshot(owner("PUT", &events(chooser), choose))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["connection"]["event_kinds"],
        serde_json::json!(["member_added", "connector_disconnected"]),
        "stored once each, in the enum's order"
    );
    let metadata: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM audit.events
          WHERE organization_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        metadata["event_kinds"],
        serde_json::json!(["member_added", "connector_disconnected"])
    );

    let resp = app(&st)
        .oneshot(emit_req(
            ORGANIZATION,
            true,
            r#"{"kind":"member_added","title":"Sam joined","body":"."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let resp = app(&st)
        .oneshot(emit_req(
            ORGANIZATION,
            true,
            r#"{"kind":"connector_connected","title":"Slack connected","body":"."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let mut every = vec![everything, chooser, slack];
    every.sort_unstable();
    assert_eq!(queued_for(&pool, "member_added").await, every);
    let mut not_chooser = vec![everything, slack];
    not_chooser.sort_unstable();
    assert_eq!(
        queued_for(&pool, "connector_connected").await,
        not_chooser,
        "the webhook that did not choose this kind is skipped"
    );

    let resp = app(&st)
        .oneshot(owner(
            "PUT",
            &events(chooser),
            Some(r#"{"event_kinds":null}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await["connection"]["event_kinds"],
        serde_json::Value::Null
    );
    let resp = app(&st)
        .oneshot(emit_req(
            ORGANIZATION,
            true,
            r#"{"kind":"connector_connected","title":"Discord connected","body":"."}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(
        queued_for(&pool, "connector_connected").await.len(),
        not_chooser.len() + every.len(),
        "back to every kind, the second notice reaches all three"
    );

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/webhook",
            Some(r#"{"url":"https://hooks.example.invalid/chosen","event_kinds":[]}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            "/internal/connectors/webhook",
            Some(
                r#"{"url":"https://hooks.example.invalid/chosen","event_kinds":["organization_alert"]}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json_body(resp).await;
    assert_eq!(
        created["connection"]["event_kinds"],
        serde_json::json!(["organization_alert"])
    );
    let id: Uuid = created["connection"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        !queued_for(&pool, "connector_connected").await.contains(&id),
        "its own connected notice is a kind it did not choose"
    );
}

/// Every send lands in the log with its status and timing, the body stays for
/// a resend, and the log pages newest first behind a cursor. Any role reads
/// it, and another tenant's connection is not there.
#[sqlx::test]
async fn every_send_is_logged_and_the_log_pages_newest_first(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/logged", mocks.uri()),
        "whsec_logged",
    )
    .await;
    let foreign = seed_webhook(
        &pool,
        OTHER_ORGANIZATION,
        &format!("{}/hooks/foreign", mocks.uri()),
        "whsec_foreign",
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/hooks/logged"))
        .respond_with(ResponseTemplate::new(503).set_body_string("deploying"))
        .up_to_n_times(1)
        .mount(&mocks)
        .await;
    Mock::given(method("POST"))
        .and(path("/hooks/logged"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    let log = |connection: Uuid, query: &str| {
        format!("/internal/connectors/{connection}/deliveries{query}")
    };

    let first = seed_delivery(&pool, ORGANIZATION, connection).await;
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    sqlx::query("UPDATE notifications.deliveries SET next_attempt_at = now() WHERE id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    assert_eq!(delivery_status(&pool, first).await.0, "delivered");

    let resp = app(&st)
        .oneshot(req_as(
            "GET",
            &log(connection, ""),
            ORGANIZATION,
            MEMBER,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "every role reads the log");
    let page = json_body(resp).await;
    let entry = &page["deliveries"][0];
    assert_eq!(entry["id"], first.to_string());
    assert_eq!(entry["status"], "delivered");
    assert_eq!(
        entry["body"], "Sam accepted the invitation.",
        "a delivered row keeps its text for a resend"
    );
    let attempts = entry["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{attempts:?}");
    assert_eq!(attempts[0]["outcome"], "failed");
    assert_eq!(attempts[0]["status_code"], 503);
    assert_eq!(attempts[0]["trigger"], "scheduled");
    assert!(
        attempts[0]["error"].as_str().unwrap().contains("deploying"),
        "{attempts:?}"
    );
    assert_eq!(attempts[1]["outcome"], "delivered");
    assert_eq!(attempts[1]["status_code"], 202);
    assert_eq!(attempts[1]["error"], serde_json::Value::Null);
    assert!(attempts[1]["duration_ms"].as_i64().unwrap() >= 0);
    assert_eq!(page["next_before"], serde_json::Value::Null);

    let mut newer = Vec::new();
    for _ in 0..3 {
        newer.push(seed_delivery(&pool, ORGANIZATION, connection).await);
    }
    let resp = app(&st)
        .oneshot(owner("GET", &log(connection, "?limit=2"), None))
        .await
        .unwrap();
    let page = json_body(resp).await;
    let ids: Vec<&str> = page["deliveries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [newer[2].to_string(), newer[1].to_string()]);
    assert_eq!(page["deliveries"][0]["attempts"], serde_json::json!([]));
    assert_eq!(page["next_before"], newer[1].to_string());
    let resp = app(&st)
        .oneshot(owner(
            "GET",
            &log(connection, &format!("?limit=2&before={}", newer[1])),
            None,
        ))
        .await
        .unwrap();
    let page = json_body(resp).await;
    let ids: Vec<&str> = page["deliveries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [newer[0].to_string(), first.to_string()]);
    assert_eq!(page["next_before"], serde_json::Value::Null);

    let resp = app(&st)
        .oneshot(owner("GET", &log(foreign, ""), None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "another tenant's log is invisible"
    );
}

/// A resend goes out now, under the same delivery id, is logged as manual,
/// and spends none of the row's retry budget. A failed resend leaves a
/// delivered row delivered; a row mid-send, a missing row and a stopped
/// connection are refused.
#[sqlx::test]
async fn a_delivery_is_resent_by_hand_with_its_own_id_and_logged_as_manual(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/resend", mocks.uri()),
        "whsec_resend",
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/hooks/resend"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    let id = seed_delivery(&pool, ORGANIZATION, connection).await;
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    let resend = |delivery: Uuid| {
        format!("/internal/connectors/{connection}/deliveries/{delivery}/redeliver")
    };

    let resp = app(&st)
        .oneshot(req_as("POST", &resend(id), ORGANIZATION, MEMBER, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app(&st)
        .oneshot(owner("POST", &resend(id), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        json_body(resp).await,
        serde_json::json!({ "delivered": true, "error": null })
    );
    let posted = posts_to(&mocks, "/hooks/resend").await;
    assert_eq!(posted.len(), 2);
    for req in &posted {
        assert_eq!(
            req.headers
                .get("telmoni-delivery-id")
                .unwrap()
                .to_str()
                .unwrap(),
            id.to_string(),
            "a resend is the same delivery, so a receiver that has it deduplicates"
        );
    }
    let sent: Vec<serde_json::Value> = posted
        .iter()
        .map(|req| serde_json::from_slice(&req.body).unwrap())
        .collect();
    for field in ["id", "kind", "title", "body"] {
        assert_eq!(
            sent[0][field], sent[1][field],
            "the resend carries the original {field}"
        );
    }
    let (status, attempts, _) = delivery_status(&pool, id).await;
    assert_eq!(
        (status.as_str(), attempts),
        ("delivered", 1),
        "a resend spends none of the retry budget"
    );
    let triggers: Vec<(String, String)> = sqlx::query_as(
        "SELECT trigger, outcome FROM notifications.delivery_attempts
          WHERE delivery_id = $1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        triggers,
        [
            ("scheduled".to_owned(), "delivered".to_owned()),
            ("manual".to_owned(), "delivered".to_owned()),
        ]
    );

    Mock::given(method("POST"))
        .and(path("/hooks/resend"))
        .respond_with(ResponseTemplate::new(500).set_body_string("down"))
        .with_priority(1)
        .mount(&mocks)
        .await;
    let resp = app(&st)
        .oneshot(owner("POST", &resend(id), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let answer = json_body(resp).await;
    assert_eq!(answer["delivered"], false);
    assert!(
        answer["error"].as_str().unwrap().contains("500"),
        "{answer}"
    );
    let (status, _, last_error) = delivery_status(&pool, id).await;
    assert_eq!(
        status, "delivered",
        "a failed resend does not undo a delivery that landed"
    );
    assert!(last_error.unwrap().contains("500"));
    let manual_failure: (Option<i32>, String) = sqlx::query_as(
        "SELECT status_code, outcome FROM notifications.delivery_attempts
          WHERE delivery_id = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(manual_failure, (Some(500), "failed".to_owned()));
    assert_eq!(
        breaker_count(&pool, connection).await,
        0,
        "a resend by hand is not the queue's evidence of a dead endpoint"
    );

    sqlx::query(
        "UPDATE notifications.deliveries SET lease_until = now() + interval '1 minute'
          WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let resp = app(&st)
        .oneshot(owner("POST", &resend(id), None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "a row mid-send is refused"
    );

    let resp = app(&st)
        .oneshot(owner("POST", &resend(Uuid::now_v7()), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    sqlx::query("UPDATE notifications.connections SET status = 'errored' WHERE id = $1")
        .bind(connection)
        .execute(&pool)
        .await
        .unwrap();
    let resp = app(&st)
        .oneshot(owner("POST", &resend(id), None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "a stopped connection sends nothing"
    );

    let resends: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit.events
          WHERE organization_id = $1 AND metadata->>'resent' = $2
          ORDER BY id",
    )
    .bind(ORGANIZATION)
    .bind(id.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        resends
            .iter()
            .map(|m| m["delivered"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [true, false],
        "each resend is audited with its outcome"
    );

    for _ in 0..8 {
        sqlx::query(
            "INSERT INTO notifications.delivery_attempts
                (id, delivery_id, project_id, organization_id, trigger, outcome, status_code,
                 duration_ms)
             VALUES ($1, $2, $3, $4, 'manual', 'delivered', 200, 1)",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(project_of(ORGANIZATION))
        .bind(ORGANIZATION)
        .execute(&pool)
        .await
        .unwrap();
    }
    let resp = app(&st)
        .oneshot(owner("POST", &resend(id), None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(
        json_body(resp).await["detail"]
            .as_str()
            .unwrap()
            .contains("10 times"),
        "ten resends by hand is the cap"
    );

    let slack = seed_connection(
        &pool,
        ORGANIZATION,
        Provider::Slack,
        SLACK_WORKSPACE,
        "C0RESEND",
        &slack_hook_url(&mocks, "resend"),
        Some("xoxb"),
    )
    .await;
    let chat = seed_delivery(&pool, ORGANIZATION, slack).await;
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &format!("/internal/connectors/{slack}/deliveries/{chat}/redeliver"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a chat channel does not deduplicate, so it is never resent"
    );
}

/// A rotation with an overlap signs every delivery with both secrets until the
/// overlap ends, the new secret first; past its expiry the prior secret signs
/// nothing and the sweep forgets it; a hard cut keeps nothing.
#[sqlx::test]
async fn a_rotation_with_an_overlap_signs_with_both_secrets_until_it_ends(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let mocks = MockServer::start().await;
    let old_secret = "whsec_before_the_roll";
    let connection = seed_webhook(
        &pool,
        ORGANIZATION,
        &format!("{}/hooks/overlap", mocks.uri()),
        old_secret,
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/hooks/overlap"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mocks)
        .await;
    let st = state(pool.clone(), &mocks, None);
    let rotate = format!("/internal/connectors/{connection}/rotate");
    let signature_of = |req: &wiremock::Request| {
        req.headers
            .get("telmoni-signature")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &rotate,
            Some(r#"{"keep_previous_for_hours":1}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let answer = json_body(resp).await;
    let new_secret = answer["signing_secret"].as_str().unwrap().to_owned();
    let until: chrono::DateTime<chrono::Utc> = answer["connection"]["previous_secret_expires_at"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let left = until - chrono::Utc::now();
    assert!(
        // A few seconds either way: `now()` is the database's clock, and the
        // local database runs in a VM whose clock can lead the host's.
        left > chrono::Duration::minutes(59)
            && left <= chrono::Duration::hours(1) + chrono::Duration::seconds(5),
        "{left}"
    );
    let metadata: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM audit.events
          WHERE organization_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(metadata["previous_kept_for_hours"], 1);

    let during = seed_delivery(&pool, ORGANIZATION, connection).await;
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    assert_eq!(delivery_status(&pool, during).await.0, "delivered");
    let posted = posts_to(&mocks, "/hooks/overlap").await;
    let signature = signature_of(&posted[0]);
    assert_eq!(signature.matches("v1=").count(), 2, "{signature}");
    assert!(verify_signature(&new_secret, &signature, &posted[0].body));
    assert!(
        verify_signature(old_secret, &signature, &posted[0].body),
        "a receiver still holding the old secret keeps working through the overlap"
    );
    assert!(!verify_signature(
        "whsec_other",
        &signature,
        &posted[0].body
    ));

    sqlx::query(
        "UPDATE notifications.connections
            SET prior_token_expires_at = now() - interval '1 second'
          WHERE id = $1",
    )
    .bind(connection)
    .execute(&pool)
    .await
    .unwrap();
    let after = seed_delivery(&pool, ORGANIZATION, connection).await;
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    assert_eq!(delivery_status(&pool, after).await.0, "delivered");
    let posted = posts_to(&mocks, "/hooks/overlap").await;
    let signature = signature_of(&posted[1]);
    assert_eq!(signature.matches("v1=").count(), 1, "{signature}");
    assert!(verify_signature(&new_secret, &signature, &posted[1].body));
    assert!(
        !verify_signature(old_secret, &signature, &posted[1].body),
        "past its expiry the old secret signs nothing, sweep or no sweep"
    );
    let mut tx = maintenance_scope(&pool, NotificationsLane).await.unwrap();
    assert_eq!(db::clear_expired_prior_tokens(&mut tx).await.unwrap(), 1);
    tx.commit().await.unwrap();
    let kept: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT prior_token_ciphertext FROM notifications.connections WHERE id = $1",
    )
    .bind(connection)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kept, None, "the sweep forgets a secret past its overlap");

    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &rotate,
            Some(r#"{"keep_previous_for_hours":24}"#),
        ))
        .await
        .unwrap();
    let second = json_body(resp).await["signing_secret"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = app(&st)
        .oneshot(owner(
            "POST",
            &rotate,
            Some(r#"{"keep_previous_for_hours":0}"#),
        ))
        .await
        .unwrap();
    let answer = json_body(resp).await;
    let third = answer["signing_secret"].as_str().unwrap().to_owned();
    assert_eq!(
        answer["connection"]["previous_secret_expires_at"],
        serde_json::Value::Null,
        "a hard cut ends any overlap in progress"
    );
    let last = seed_delivery(&pool, ORGANIZATION, connection).await;
    assert_eq!(delivery::tick_once(&st).await.unwrap(), 1);
    assert_eq!(delivery_status(&pool, last).await.0, "delivered");
    let posted = posts_to(&mocks, "/hooks/overlap").await;
    let signature = signature_of(&posted[2]);
    assert!(verify_signature(&third, &signature, &posted[2].body));
    assert!(!verify_signature(&second, &signature, &posted[2].body));
    assert!(!verify_signature(&new_secret, &signature, &posted[2].body));
}
