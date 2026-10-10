//! The two deletions, driven through the real router with their tails inline:
//! an owner deleting an organization (`DELETE /internal/organization`), and a
//! person deleting their account (`DELETE /internal/me`), with the sweep's
//! steps that finish whatever the inline tails do not — and, for an
//! organization, the hard delete only the sweep performs, once its wait has
//! passed, with every purge run first. And the operator's termination, which
//! goes the same way.
//!
//! The sweep's steps and the operator's commands are `telmoni_auth::sweep`,
//! called in process. This suite mounts them as the `/internal` lanes they
//! answered ([`sweep_lanes`]), so the saga is driven with
//! the same requests, in the same order, and the assertions read as they did.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test scaffolding")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::handler::deletion::{self, FinalizeOutcome, PurgeOutcome};
use telmoni_auth::sweep;
use telmoni_auth::test_provider::{
    Call, RecordingHook, RecordingNotifications, RecordingTelemetry, ScriptedProvider, SiblingCall,
    SiblingCalls, as_person, bearer,
};
use telmoni_auth::{AppState, AuthProvider, Authenticated, Config, Siblings, router};
use telmoni_shared::seam::Auth as _;
use telmoni_shared::test_util::{apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{AuthError, OrganizationId, OrganizationStatus, TelmoniError, UserId};

const SERVICE_SECRET: &str = "test-service-secret";
/// The person deleting, and the owner of `ORGANIZATION`.
const USER: &str = "user_inline_del";
/// The organization being deleted — on its own, or with its owner's account.
const ORGANIZATION: &str = "org_inline_del";
/// Somebody else's organization, which no deletion here may take with it.
const SURVIVOR: &str = "org_survivor";
const SURVIVOR_OWNER: &str = "user_survivor_owner";
/// A second person on `ORGANIZATION`'s roster.
const COLLEAGUE: &str = "user_colleague";
/// The plaintext of every code these tests plant.
const CODE: &str = "123456";

/// Records every mail handed to the transport.
#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<telmoni_shared::mail::Mail>>);

impl Recorder {
    fn mails(&self) -> Vec<telmoni_shared::mail::Mail> {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl telmoni_shared::mail::MailSender for Recorder {
    async fn send(
        &self,
        mail: &telmoni_shared::mail::Mail,
    ) -> Result<(), telmoni_shared::mail::MailError> {
        self.0.lock().unwrap().push(mail.clone());
        Ok(())
    }
}

/// The sweep's steps and the operator's commands, mounted as the lanes they
/// answered until the sweeps moved in process: the same paths, the same
/// statuses. What the scheduler and the operator's commands call directly,
/// this suite calls over the router, so the saga's assertions read as they
/// did. Nothing gates them: the real lanes' service-secret gate is gone with
/// the lanes.
fn sweep_lanes(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/internal/organizations/pending-deletion",
            get(due_organizations_lane),
        )
        .route(
            "/internal/organizations/{organization_id}",
            axum::routing::delete(finalize_lane),
        )
        .route(
            "/internal/organizations/{organization_id}/purge",
            post(purge_pending_lane),
        )
        .route(
            "/internal/organizations/{organization_id}/terminate",
            post(terminate_lane),
        )
        .route(
            "/internal/people/pending-deletion",
            get(pending_people_lane),
        )
        .route("/internal/people/{user_id}/erase", post(erase_lane))
        .with_state(state)
}

fn organization_id_of(raw: &str) -> Result<OrganizationId, TelmoniError> {
    OrganizationId::try_new(raw)
        .map_err(|e| AuthError::BadRequest(format!("invalid organization id: {e}")).into())
}

async fn due_organizations_lane(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<Value>, TelmoniError> {
    let organizations: Vec<Value> = sweep::due_organizations(&state)
        .await?
        .into_iter()
        .map(|due| {
            json!({
                "organization_id": due.external_id,
                "deletion_requested_at": due.deletion_requested_at,
                "erase_after": due.erase_after,
                "ripe": due.ripe,
            })
        })
        .collect();
    Ok(axum::Json(json!({
        "count": organizations.len(),
        "organizations": organizations,
    })))
}

async fn finalize_lane(
    State(state): State<Arc<AppState>>,
    Path(organization_id): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    match deletion::finalize_organization(&state, &organization_id_of(&organization_id)?).await? {
        FinalizeOutcome::Finalized | FinalizeOutcome::AlreadyGone => Ok(StatusCode::NO_CONTENT),
        FinalizeOutcome::NotPending => Err(AuthError::Conflict(
            "this organization is not pending deletion: request deletion first".into(),
        )
        .into()),
        FinalizeOutcome::TooSoon => {
            Err(AuthError::Conflict("this organization's wait has not passed".into()).into())
        }
    }
}

async fn purge_pending_lane(
    State(state): State<Arc<AppState>>,
    Path(organization_id): Path<String>,
) -> Result<axum::response::Response, TelmoniError> {
    match deletion::purge_pending_organization(&state, &organization_id_of(&organization_id)?)
        .await?
    {
        PurgeOutcome::Purged => {
            Ok((StatusCode::OK, axum::Json(json!({ "purged": true }))).into_response())
        }
        PurgeOutcome::AlreadyGone => Ok(StatusCode::NO_CONTENT.into_response()),
        PurgeOutcome::NotPending => Err(AuthError::Conflict(
            "this organization is not pending deletion: request deletion first".into(),
        )
        .into()),
    }
}

async fn terminate_lane(
    State(state): State<Arc<AppState>>,
    Path(organization_id): Path<String>,
) -> Result<(StatusCode, axum::Json<Value>), TelmoniError> {
    let terminated = sweep::terminate(&state, &organization_id_of(&organization_id)?).await?;
    Ok((
        StatusCode::ACCEPTED,
        axum::Json(json!({
            "purged": terminated.purged,
            "erase_after": terminated.erase_after,
        })),
    ))
}

async fn pending_people_lane(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<Value>, TelmoniError> {
    let people: Vec<Value> = sweep::pending_people(&state)
        .await?
        .into_iter()
        .map(|(user_id, requested_at)| {
            json!({ "user_id": user_id, "deletion_requested_at": requested_at })
        })
        .collect();
    Ok(axum::Json(
        json!({ "count": people.len(), "people": people }),
    ))
}

async fn erase_lane(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    let user_id = UserId::try_new(&user_id)
        .map_err(|e| AuthError::BadRequest(format!("invalid user id: {e}")))?;
    deletion::erase_person(&state, &user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The real auth router around `provider`: the purge hook, notifications
/// and telemetry as recorders when given, and mail at `sender`. The state
/// comes back beside the router, for the seams a test asks directly.
fn build_around(
    pool: PgPool,
    provider: Arc<dyn AuthProvider>,
    hook: Option<Arc<RecordingHook>>,
    notifications: Option<Arc<RecordingNotifications>>,
    telemetry: Option<Arc<RecordingTelemetry>>,
    tail_budget_ms: u64,
    sender: Arc<dyn telmoni_shared::mail::MailSender>,
) -> (Router, Arc<AppState>) {
    let config = Config {
        database_url: String::new(), // pool already built by #[sqlx::test]
        service_secret: SERVICE_SECRET.into(),
        service_secret_next: None,
        allow_test_session: false,
        redirect_uri: "http://localhost:3000/auth/callback".into(),
        app_url: "http://localhost:3000".into(),
        mail_from: "Telmoni <test@example.com>".into(),
        support_email: None,
        deletion_tail_budget_ms: tail_budget_ms,
    };
    let db = service_pool(&pool, "auth");
    let state = Arc::new(AppState {
        issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
        password: None,
        external: Some(telmoni_auth::test_provider::external(provider)),
        db,
        config,
        mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(sender)),
        exchange_cache: telmoni_auth::ExchangeCache::new(),
        siblings: Siblings {
            notifications: notifications.map(|n| n as Arc<dyn telmoni_shared::seam::Notifications>),
            telemetry: telemetry.map(|t| t as Arc<dyn telmoni_shared::seam::Telemetry>),
            agent: None,
            purge_hook: hook.map(|h| h as Arc<dyn telmoni_shared::seam::PurgeHook>),
        },
    });
    (
        router(state.clone()).merge(sweep_lanes(state.clone())),
        state,
    )
}

/// The real auth router around a scripted identity provider, whose handle
/// comes back beside it: script its answers before the request, and read
/// what the lane asked of it after.
fn build(
    pool: PgPool,
    hook: Option<Arc<RecordingHook>>,
    notifications: Option<Arc<RecordingNotifications>>,
    tail_budget_ms: u64,
    sender: Arc<dyn telmoni_shared::mail::MailSender>,
) -> (Router, Arc<ScriptedProvider>, Arc<AppState>) {
    let provider = Arc::new(ScriptedProvider::new());
    let (router, state) = build_around(
        pool,
        provider.clone(),
        hook,
        notifications,
        None,
        tail_budget_ms,
        sender,
    );
    (router, provider, state)
}

/// The real auth router with (optionally) the purge hook recorded.
fn app(
    pool: PgPool,
    hook: Option<Arc<RecordingHook>>,
    tail_budget_ms: u64,
) -> (Router, Arc<ScriptedProvider>) {
    app_with_siblings(pool, hook, None, tail_budget_ms)
}

/// [`app`], with the state beside it for the seams a test asks directly.
fn app_and_state(
    pool: PgPool,
    hook: Option<Arc<RecordingHook>>,
    tail_budget_ms: u64,
) -> (Router, Arc<ScriptedProvider>, Arc<AppState>) {
    build(
        pool,
        hook,
        None,
        tail_budget_ms,
        Arc::new(telmoni_shared::mail::NoopSender),
    )
}

/// The same router with notifications recorded too.
fn app_with_siblings(
    pool: PgPool,
    hook: Option<Arc<RecordingHook>>,
    notifications: Option<Arc<RecordingNotifications>>,
    tail_budget_ms: u64,
) -> (Router, Arc<ScriptedProvider>) {
    let (router, provider, _) = build(
        pool,
        hook,
        notifications,
        tail_budget_ms,
        Arc::new(telmoni_shared::mail::NoopSender),
    );
    (router, provider)
}

/// The same router, mailing to `sender`.
fn app_mailing(
    pool: PgPool,
    sender: Arc<dyn telmoni_shared::mail::MailSender>,
) -> (Router, Arc<ScriptedProvider>) {
    let (router, provider, _) = build(pool, None, None, 8_000, sender);
    (router, provider)
}

/// Both modules beside auth as recorders on one timeline, unscripted: every
/// purge and redaction lands. A test scripts what it wants to fail.
fn siblings() -> (
    SiblingCalls,
    Arc<RecordingHook>,
    Arc<RecordingNotifications>,
) {
    let calls = SiblingCalls::default();
    (
        calls.clone(),
        Arc::new(RecordingHook::new(calls.clone())),
        Arc::new(RecordingNotifications::new(calls)),
    )
}

/// The purge hook's call for `organization`, as the timeline records it.
fn hook_purge(organization: &str) -> SiblingCall {
    SiblingCall::HookPurge(organization.to_owned())
}

/// Notifications' organization purge, as the timeline records it.
fn notifications_purge(organization: &str) -> SiblingCall {
    SiblingCall::PurgeOrganization(organization.to_owned())
}

/// Notifications' redaction of `user`, as the timeline records it.
fn notifications_redact(user: &str) -> SiblingCall {
    SiblingCall::RedactPerson(user.to_owned())
}

/// How many times the purge hook was asked.
fn hook_purges(calls: &SiblingCalls) -> usize {
    calls.count(|c| matches!(c, SiblingCall::HookPurge(_)))
}

/// An identity provider whose user delete hangs for `delay` before it
/// answers — what the account tail's budget has to cut off. Every other
/// call goes straight to the scripted provider underneath.
struct Hung {
    inner: ScriptedProvider,
    delay: Duration,
}

#[async_trait::async_trait]
impl AuthProvider for Hung {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        sign_up: bool,
        login_hint: Option<&str>,
    ) -> Result<String, TelmoniError> {
        self.inner
            .authorize_url(redirect_uri, state, sign_up, login_hint)
            .await
    }

    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<Authenticated, TelmoniError> {
        self.inner.exchange_code(code, redirect_uri).await
    }

    async fn logout_url(&self, id_token: Option<&str>, return_to: &str) -> String {
        self.inner.logout_url(id_token, return_to).await
    }

    async fn delete_user(&self, subject: &str) -> Result<(), TelmoniError> {
        tokio::time::sleep(self.delay).await;
        self.inner.delete_user(subject).await
    }
}

/// The real auth router around a provider whose user delete hangs for
/// `delay`, with no hook and no siblings.
fn app_with_hung_provider(pool: PgPool, delay: Duration, tail_budget_ms: u64) -> Router {
    build_around(
        pool,
        Arc::new(Hung {
            inner: ScriptedProvider::new(),
            delay,
        }),
        None,
        None,
        None,
        tail_budget_ms,
        Arc::new(telmoni_shared::mail::NoopSender),
    )
    .0
}

/// How many times the lane asked the provider to delete a user.
fn provider_deletes(provider: &ScriptedProvider) -> usize {
    provider.count(|c| matches!(c, Call::DeleteUser { .. }))
}

/// The provider failing a user delete: the erasure is not done, and the
/// sweep retries it.
fn provider_down() -> Result<(), TelmoniError> {
    Err(TelmoniError::Internal("provider down".into()))
}

/// A request the console makes for `actor`, in `organization` when it names one.
async fn request(
    pool: &PgPool,
    method: &str,
    uri: &str,
    actor: &str,
    organization: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("content-type", "application/json");
    if let Some(organization) = organization {
        builder = builder.header("x-organization-id", organization);
    }
    let builder = as_person(builder, pool, actor).await;
    match body {
        Some(b) => builder.body(Body::from(b.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

/// The owner deleting the organization the console has active.
async fn delete_organization(
    pool: &PgPool,
    actor: &str,
    organization: &str,
    code: &str,
) -> Request<Body> {
    request(
        pool,
        "DELETE",
        "/internal/organization",
        actor,
        Some(organization),
        Some(json!({ "code": code })),
    )
    .await
}

/// A person deleting their account.
async fn delete_account(pool: &PgPool, actor: &str, code: &str) -> Request<Body> {
    request(
        pool,
        "DELETE",
        "/internal/me",
        actor,
        None,
        Some(json!({ "code": code })),
    )
    .await
}

/// A call from the sweep lanes: the service secret and no person.
fn service(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-service-secret", SERVICE_SECRET)
        .header("content-type", "application/json");
    match body {
        Some(b) => builder.body(Body::from(b.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).expect("JSON body")
}

fn email_of(user: &str) -> String {
    format!("{}@example.test", user.replace('_', "-"))
}

/// Each seeded organization's one project.
fn project_of(organization: &str) -> String {
    format!("project_{}", organization.trim_start_matches("org_"))
}

async fn seed_person(pool: &PgPool, user: &str) {
    seed_identity(pool, user, &email_of(user)).await;
}

/// An organization, its owner's row on the roster, and one project. Seeded
/// `pending_deletion` as its owner's deletion leaves it: its wait just begun.
async fn seed_organization(pool: &PgPool, id: &str, owner: &str, status: &str) {
    sqlx::query(
        "INSERT INTO auth.organizations
             (external_id, slug, name, status, deletion_requested_at, erase_after, deletion_kind)
         VALUES ($1, 'org-' || md5($1), 'Acme', $2,
                 CASE WHEN $2 = 'pending_deletion' THEN now() END,
                 CASE WHEN $2 = 'pending_deletion' THEN now() + interval '15 minutes' END,
                 CASE WHEN $2 = 'pending_deletion' THEN 'owner' END)",
    )
    .bind(id)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
         VALUES ($1, $2, 'owner', $2)",
    )
    .bind(id)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO auth.projects
             (external_id, organization_id, name, slug, status, deletion_requested_at)
         VALUES ($1, $2, $3, 'project-' || md5($1), $4,
                 CASE WHEN $4 = 'pending_deletion' THEN now() END)",
    )
    .bind(project_of(id))
    .bind(id)
    .bind(format!("Project {id}"))
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
}

async fn name_organization(pool: &PgPool, id: &str, name: &str) {
    sqlx::query("UPDATE auth.organizations SET name = $2 WHERE external_id = $1")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
}

/// Put `user` on an organization's roster at `role`.
async fn seat(pool: &PgPool, organization: &str, user: &str, role: &str) {
    sqlx::query(
        "INSERT INTO auth.organization_members (organization_id, user_id, role, added_by)
         VALUES ($1, $2, $3, $2)",
    )
    .bind(organization)
    .bind(user)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

/// Give `user` a seat on a project.
async fn seat_on_project(pool: &PgPool, project: &str, user: &str, role: &str) {
    sqlx::query(
        "INSERT INTO auth.project_members (project_id, user_id, role, added_by)
         VALUES ($1, $2, $3, $2)",
    )
    .bind(project)
    .bind(user)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

/// A live code whose plaintext is `CODE`. `subject` is the organization an
/// `organization_deletion` code is bound to, and nothing for any other purpose.
async fn plant_code(pool: &PgPool, user: &str, purpose: &str, subject: Option<&str>) {
    sqlx::query(
        "INSERT INTO auth.confirmation_codes (user_id, purpose, subject, code_hash, expires_at)
         VALUES ($1, $2, $3, $4, now() + interval '15 minutes')",
    )
    .bind(user)
    .bind(purpose)
    .bind(subject)
    .bind(telmoni_shared::digest::sha256_hex(CODE.as_bytes()))
    .execute(pool)
    .await
    .unwrap();
}

/// The owner, their organization with one project, and a live code to delete
/// that organization.
async fn seed_organization_deletion(pool: &PgPool, status: &str) {
    apply_audit_migrations(pool).await;
    seed_person(pool, USER).await;
    seed_organization(pool, ORGANIZATION, USER, status).await;
    plant_code(pool, USER, "organization_deletion", Some(ORGANIZATION)).await;
}

/// The person, the organization they own alone, and a live code to delete
/// their account.
async fn seed_account_deletion(pool: &PgPool) {
    apply_audit_migrations(pool).await;
    seed_person(pool, USER).await;
    seed_organization(pool, ORGANIZATION, USER, "active").await;
    plant_code(pool, USER, "account_deletion", None).await;
}

/// The person, owning nothing, and a live code to delete their account — the
/// deletion whose every step can run in the request.
async fn seed_account_deletion_owning_nothing(pool: &PgPool) {
    apply_audit_migrations(pool).await;
    seed_person(pool, USER).await;
    plant_code(pool, USER, "account_deletion", None).await;
}

/// Mark `organization` as its owner's deletion does, by hand: pending, its
/// wait just begun.
async fn mark_pending(pool: &PgPool, organization: &str) {
    sqlx::query(
        "UPDATE auth.organizations
            SET status = 'pending_deletion', deletion_requested_at = now(),
                erase_after = now() + interval '15 minutes', deletion_kind = 'owner'
          WHERE external_id = $1",
    )
    .bind(organization)
    .execute(pool)
    .await
    .unwrap();
}

/// Move `organization`'s `erase_after` into the past, as the sweep finds it
/// once its wait has passed.
async fn let_the_wait_pass(pool: &PgPool, organization: &str) {
    sqlx::query(
        "UPDATE auth.organizations SET erase_after = now() - interval '1 minute'
          WHERE external_id = $1",
    )
    .bind(organization)
    .execute(pool)
    .await
    .unwrap();
}

/// The wait an organization's mark set: `erase_after` less
/// `deletion_requested_at`, in seconds, with who asked.
async fn wait_and_kind(pool: &PgPool, organization: &str) -> (f64, String) {
    sqlx::query_as(
        "SELECT extract(epoch FROM erase_after - deletion_requested_at)::float8, deletion_kind
           FROM auth.organizations WHERE external_id = $1",
    )
    .bind(organization)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The sweep's listing: every organization auth owes a step, and whether
/// its wait has passed (`ripe`), in the order listed.
async fn listed_organizations(pool: &PgPool) -> Vec<(String, bool)> {
    let resp = app(pool.clone(), None, 8_000)
        .0
        .oneshot(service(
            "GET",
            "/internal/organizations/pending-deletion",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    json_body(resp)
        .await
        .get("organizations")
        .and_then(Value::as_array)
        .expect("the listing names its organizations")
        .iter()
        .map(|o| {
            (
                o["organization_id"].as_str().unwrap().to_owned(),
                o["ripe"]
                    .as_bool()
                    .expect("the listing says whether the wait has passed"),
            )
        })
        .collect()
}

/// The organizations auth says may go: the listing's ripe ones.
async fn ripe_organizations(pool: &PgPool) -> Vec<String> {
    listed_organizations(pool)
        .await
        .into_iter()
        .filter_map(|(organization, ripe)| ripe.then_some(organization))
        .collect()
}

/// The sweep's finalize for `organization`.
fn finalize(organization: &str) -> Request<Body> {
    service(
        "DELETE",
        &format!("/internal/organizations/{organization}"),
        None,
    )
}

/// The sweep's interim hook purge for `organization`.
fn purge_lane(organization: &str) -> Request<Body> {
    service(
        "POST",
        &format!("/internal/organizations/{organization}/purge"),
        None,
    )
}

/// An operator's termination of `organization`, as `telmoni terminate` runs it.
fn terminate(organization: &str) -> Request<Body> {
    service(
        "POST",
        &format!("/internal/organizations/{organization}/terminate"),
        None,
    )
}

/// Somebody else's organization, with `user` on its roster and a seat at
/// `role` on its project — what a deletion must reach, or must leave alone.
async fn seed_survivor_with(pool: &PgPool, user: &str, role: &str) {
    seed_person(pool, SURVIVOR_OWNER).await;
    seed_organization(pool, SURVIVOR, SURVIVOR_OWNER, "active").await;
    seat(pool, SURVIVOR, user, "member").await;
    seat_on_project(pool, &project_of(SURVIVOR), user, role).await;
}

/// Mark `user` as having confirmed their account's deletion.
async fn mark_person_pending(pool: &PgPool, user: &str) {
    sqlx::query("INSERT INTO auth.accounts (user_id, deletion_requested_at) VALUES ($1, now())")
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
}

async fn org_status(pool: &PgPool, organization: &str) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM auth.organizations WHERE external_id = $1")
        .bind(organization)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn audit_count(pool: &PgPool, organization: &str, action: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit.events WHERE organization_id = $1 AND action = $2",
    )
    .bind(organization)
    .bind(action)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Where a person's account stands.
#[derive(Debug, PartialEq, Eq)]
enum Person {
    Live,
    /// They confirmed deleting it and the erasure has not finished.
    Pending,
    Erased,
}

async fn person(pool: &PgPool, user: &str) -> Person {
    let row: Option<Option<chrono::DateTime<chrono::Utc>>> = sqlx::query_scalar(
        "SELECT a.deletion_requested_at FROM auth.identities i
               LEFT JOIN auth.accounts a ON a.user_id = i.user_id
              WHERE i.user_id = $1",
    )
    .bind(user)
    .fetch_optional(pool)
    .await
    .unwrap();
    match row {
        None => Person::Erased,
        Some(None) => Person::Live,
        Some(Some(_)) => Person::Pending,
    }
}

/// `user`'s rows in `organization`: its roster, and seats on its projects.
async fn rows_in(pool: &PgPool, organization: &str, user: &str) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM auth.organization_members
                  WHERE organization_id = $1 AND user_id = $2),
                (SELECT count(*) FROM auth.project_members m
                   JOIN auth.projects p ON p.external_id = m.project_id
                  WHERE p.organization_id = $1 AND m.user_id = $2)",
    )
    .bind(organization)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The removals an erasure recorded on `organization`'s chain: which project
/// (none for the roster row), and the role that was lost.
async fn erasure_removals(pool: &PgPool, organization: &str) -> Vec<(Option<String>, String)> {
    sqlx::query_as(
        "SELECT in_project, metadata->>'role' FROM audit.events
          WHERE organization_id = $1 AND action = 'deleted' AND resource_kind = 'member'
            AND resource_id = $2 AND actor_id = 'service:deletion-saga'
            AND metadata->>'kind' = 'account_deletion'
          ORDER BY in_project NULLS LAST",
    )
    .bind(organization)
    .bind(USER)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The six digits a code mail carried.
fn code_in(text: &str) -> String {
    let (_, rest) = text
        .split_once("code is ")
        .expect("the mail states its code");
    rest.chars().take_while(char::is_ascii_digit).collect()
}

/// Three keys across `ORGANIZATION`'s two projects, one of them mid-rotation (a
/// future `revoked_at`), which must die with the rest.
async fn seed_keys(pool: &PgPool) -> [&'static str; 3] {
    let second_project = format!("{}_second", project_of(ORGANIZATION));
    sqlx::query(
        "INSERT INTO auth.projects (external_id, organization_id, name, slug)
         VALUES ($1, $2, 'Second', 'second')",
    )
    .bind(&second_project)
    .bind(ORGANIZATION)
    .execute(pool)
    .await
    .unwrap();
    let first_project = project_of(ORGANIZATION);
    let keys = [
        ("telmoni_live_first", first_project.as_str(), None),
        ("telmoni_live_second", second_project.as_str(), None),
        ("telmoni_in_grace", first_project.as_str(), Some("1 day")),
    ];
    for (raw, project, grace) in keys {
        sqlx::query(
            "INSERT INTO auth.api_tokens (organization_id, project_id, name, token_hash,
                                          created_by, revoked_at)
             VALUES ($1, $2, $3, $4, $5,
                     CASE WHEN $6::text IS NULL THEN NULL ELSE now() + $6::interval END)",
        )
        .bind(ORGANIZATION)
        .bind(project)
        .bind(raw)
        .bind(telmoni_shared::digest::sha256_hex(raw.as_bytes()))
        .bind(USER)
        .bind(grace)
        .execute(pool)
        .await
        .unwrap();
    }
    keys.map(|(raw, _, _)| raw)
}

/// How many of the organization's keys are still unrevoked (a future
/// `revoked_at` counts as live: that is a rotation grace).
async fn unrevoked_keys(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM auth.api_tokens
          WHERE organization_id = $1 AND (revoked_at IS NULL OR revoked_at > now())",
    )
    .bind(ORGANIZATION)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Whether each key validates, in the order given.
async fn keys_validate(pool: &PgPool, keys: [&str; 3]) -> [bool; 3] {
    let (app, _provider) = app(pool.clone(), None, 8_000);
    let mut out = Vec::with_capacity(keys.len());
    for raw in keys {
        let resp = app
            .clone()
            .oneshot(service(
                "POST",
                "/internal/tokens/validate",
                Some(json!({ "token": raw })),
            ))
            .await
            .unwrap();
        out.push(resp.status() == StatusCode::OK);
    }
    out.try_into().expect("one answer per key")
}

/// None of the organization's keys validates.
async fn assert_no_key_validates(pool: &PgPool, keys: [&str; 3]) {
    assert_eq!(
        keys_validate(pool, keys).await,
        [false; 3],
        "a key still validates"
    );
}

// ── Deleting an organization ─────────────────────────────────────────────────

/// With every sibling healthy, the owner's confirm runs the purge hook in the
/// same request — whatever it stops, it stops at once — and nothing else: the
/// row waits out its grace, the sweep lists nothing and finalize refuses.
/// Past it, finalize purges notifications for the first time and the hook
/// again, and only then deletes the row. The identity provider is never
/// asked for anything, because nobody's account goes with an organization.
#[sqlx::test]
async fn confirm_runs_the_hook_inline_and_the_row_waits_out_the_grace(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let (calls, hook, notifications) = siblings();
    let (router, provider) =
        app_with_siblings(pool.clone(), Some(hook), Some(notifications), 8_000);
    let app = || router.clone();

    let resp = app()
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let answer = json_body(resp).await;
    assert_eq!(answer["purged"], true);
    let erase_after: Option<String> = sqlx::query_scalar(
        "SELECT erase_after::text FROM auth.organizations WHERE external_id = $1",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(erase_after.is_some(), "the mark set no erase_after");
    assert!(
        answer["erase_after"].is_string(),
        "the answer does not say when the row goes: {answer}"
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion"),
        "the row went before its wait had passed"
    );
    assert_eq!(audit_count(&pool, ORGANIZATION, "updated").await, 1);
    assert_eq!(audit_count(&pool, ORGANIZATION, "deleted").await, 0);
    assert_eq!(
        calls.calls(),
        vec![hook_purge(ORGANIZATION)],
        "the request purged more than the hook: notifications goes at finalize, after every request in flight"
    );
    assert_eq!(
        person(&pool, USER).await,
        Person::Live,
        "the owner's account went with the organization"
    );

    // Inside the wait: a request that was authorized before the mark may
    // still be landing on a sibling.
    assert!(
        listed_organizations(&pool).await.is_empty(),
        "listed for the sweep inside the wait with the hook already purged"
    );
    let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT, "finalized too soon");
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );

    let_the_wait_pass(&pool, ORGANIZATION).await;
    assert_eq!(
        listed_organizations(&pool).await,
        vec![(ORGANIZATION.to_owned(), true)]
    );
    let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await,
        None,
        "the row is gone"
    );
    assert_eq!(audit_count(&pool, ORGANIZATION, "deleted").await, 1);
    assert_eq!(
        calls.calls(),
        vec![
            hook_purge(ORGANIZATION),
            notifications_purge(ORGANIZATION),
            hook_purge(ORGANIZATION),
        ],
        "finalize did not purge every sibling, in order, before the row went"
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "the provider was asked to delete somebody: nobody's account goes with an organization"
    );
}

/// ⚠ **Fifteen minutes, whoever asked.** An owner's deletion sets
/// `erase_after` that far past the mark, with who asked recorded, and so
/// does an operator's termination: there is no restore, so the wait covers
/// requests in flight and nothing else. Longer keeps a closed organization's
/// data past the erasure the privacy policy promises, usually within the hour.
#[sqlx::test]
async fn the_wait_is_fifteen_minutes_for_an_owners_deletion_and_a_termination(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    seed_person(&pool, SURVIVOR_OWNER).await;
    seed_organization(&pool, SURVIVOR, SURVIVOR_OWNER, "active").await;
    let (app, _provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .clone()
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let resp = app.oneshot(terminate(SURVIVOR)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    assert_eq!(
        wait_and_kind(&pool, ORGANIZATION).await,
        (900.0, "owner".to_owned())
    );
    assert_eq!(
        wait_and_kind(&pool, SURVIVOR).await,
        (900.0, "operator".to_owned())
    );
}

/// ⚠ **Fifteen minutes for an organization an account takes with it too.**
/// The wait covers requests in flight and nothing else: longer than a
/// sibling's thirty-second authorize cache, the console's ten-second fetch
/// and a connector handshake, and no longer.
#[sqlx::test]
async fn the_finalize_grace_is_fifteen_minutes(pool: PgPool) {
    seed_account_deletion(&pool).await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(
        wait_and_kind(&pool, ORGANIZATION).await,
        (900.0, "account".to_owned())
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "the provider was asked while the organization's row still stands"
    );
}

/// A wrong code destroys nothing, on either deletion: 400, organization intact,
/// the person intact, and each real code unconsumed with the guess counted.
#[sqlx::test]
async fn a_wrong_code_destroys_nothing(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    plant_code(&pool, USER, "account_deletion", None).await;
    let (calls, hook, _) = siblings();
    let (app, provider) = app(pool.clone(), Some(hook), 8_000);

    for wrong in [
        delete_organization(&pool, USER, ORGANIZATION, "000000").await,
        delete_account(&pool, USER, "000000").await,
    ] {
        let uri = wrong.uri().to_string();
        let resp = app.clone().oneshot(wrong).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{uri}");
    }

    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("active")
    );
    assert_eq!(person(&pool, USER).await, Person::Live);
    assert_eq!(
        provider_deletes(&provider),
        0,
        "a wrong code reached the provider"
    );
    assert!(calls.calls().is_empty(), "a wrong code reached the hook");
    let live: Vec<(String, i32)> = sqlx::query_as(
        "SELECT purpose, attempts FROM auth.confirmation_codes
          WHERE user_id = $1 AND consumed_at IS NULL AND expires_at > now()
          ORDER BY purpose",
    )
    .bind(USER)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        live,
        vec![
            ("account_deletion".to_owned(), 1),
            ("organization_deletion".to_owned(), 1),
        ],
        "the real codes survive the bad guesses, and each guess is counted"
    );
}

/// The hook fails: the request reports the purge as not landed, and finalize —
/// which runs it again — refuses to delete the row until it does. The row
/// must never go while something outside may still act on it.
#[sqlx::test]
async fn a_failed_hook_purge_stops_the_hard_delete(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let (calls, hook, _) = siblings();
    hook.down();
    let (router, provider) = app(pool.clone(), Some(hook), 8_000);
    let app = || router.clone();

    let resp = app()
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["purged"], false);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion"),
        "the row waits for the deletion sweep"
    );

    // Inside the wait the sweep is owed the hook purge alone, and the
    // interim lane fails as long as the hook does.
    assert_eq!(
        listed_organizations(&pool).await,
        vec![(ORGANIZATION.to_owned(), false)],
        "an unpurged organization is what the sweep retries the purge for"
    );
    let resp = app().oneshot(purge_lane(ORGANIZATION)).await.unwrap();
    assert!(
        resp.status().is_server_error(),
        "the purge lane answered {} with the hook failing",
        resp.status()
    );

    let_the_wait_pass(&pool, ORGANIZATION).await;
    let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert!(
        resp.status().is_server_error(),
        "finalize answered {} with the hook still failing",
        resp.status()
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion"),
        "the row went while the hook had not purged"
    );
    assert_eq!(
        audit_count(&pool, ORGANIZATION, "deleted").await,
        0,
        "no finalize audit row without a finalize"
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "an organization's deletion asked the provider to delete somebody"
    );
    assert_eq!(
        hook_purges(&calls),
        3,
        "the request, the interim purge and the finalize each asked the hook once"
    );
}

/// The finalize's FIRST step fails → nothing after it runs: no second hook
/// purge, no hard delete, and the row survives for the retry. The request
/// itself never asks notifications for anything.
#[sqlx::test]
async fn a_failed_notifications_purge_stops_the_finalize_before_the_hook(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let (calls, hook, notifications) = siblings();
    notifications.down();
    let (router, _provider) =
        app_with_siblings(pool.clone(), Some(hook), Some(notifications), 8_000);
    let app = || router.clone();

    let resp = app()
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(
        json_body(resp).await["purged"],
        true,
        "the request's tail is the hook alone, and the hook is up"
    );

    let_the_wait_pass(&pool, ORGANIZATION).await;
    let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert!(
        resp.status().is_server_error(),
        "finalize answered {} with notifications failing",
        resp.status()
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    assert_eq!(
        audit_count(&pool, ORGANIZATION, "deleted").await,
        0,
        "no finalize audit row without a finalize"
    );
    assert_eq!(
        calls.calls(),
        vec![hook_purge(ORGANIZATION), notifications_purge(ORGANIZATION)],
        "the request's hook purge, then the finalize's notifications purge, and no second hook purge"
    );
}

/// Telemetry's purge runs in the finalize, after notifications' and before
/// the hook's second, and one that fails stops the finalize there: the row
/// survives for the retry, which purges every sibling again, in order.
#[sqlx::test]
async fn the_finalize_purges_telemetry_and_a_failed_purge_stops_it(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let (calls, hook, notifications) = siblings();
    let telemetry = Arc::new(RecordingTelemetry::new(calls.clone()));
    telemetry.failing_purges(1);
    let (router, _state) = build_around(
        pool.clone(),
        Arc::new(ScriptedProvider::new()),
        Some(hook),
        Some(notifications),
        Some(telemetry),
        8_000,
        Arc::new(telmoni_shared::mail::NoopSender),
    );
    let app = || router.clone();
    let telemetry_purge = SiblingCall::TelemetryPurgeOrganization(ORGANIZATION.to_owned());

    let resp = app()
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let_the_wait_pass(&pool, ORGANIZATION).await;
    calls.reset();

    let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert!(
        resp.status().is_server_error(),
        "finalize answered {} with telemetry failing",
        resp.status()
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    assert_eq!(
        calls.calls(),
        vec![notifications_purge(ORGANIZATION), telemetry_purge.clone()],
        "the finalize went past a failed telemetry purge"
    );
    calls.reset();

    let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await,
        None,
        "the row is gone"
    );
    assert_eq!(
        calls.calls(),
        vec![
            notifications_purge(ORGANIZATION),
            telemetry_purge,
            hook_purge(ORGANIZATION),
        ],
        "the retry did not purge every sibling, in order, before the row went"
    );
}

/// ⚠ **From the mark on, nothing acts in the organization** — its owner
/// included. A second confirm, a rename, a code request, the audit log, a
/// project lane, a module's authorize question and a person's own act
/// recorded onto its chain are all refused with the one sentence, and none
/// of them queues a tail. A request authorized before the mark is what the
/// wait is for; one after it never starts.
#[sqlx::test]
async fn a_pending_organization_is_refused_on_every_lane_that_acts_in_it(pool: PgPool) {
    seed_organization_deletion(&pool, "pending_deletion").await;
    let (calls, hook, _) = siblings();
    let (app, provider, state) = app_and_state(pool.clone(), Some(hook), 8_000);
    let project = project_of(ORGANIZATION);

    let lanes: Vec<(&str, String, Option<Value>)> = vec![
        (
            "DELETE",
            "/internal/organization".into(),
            Some(json!({ "code": CODE })),
        ),
        ("POST", "/internal/organization/deletion-code".into(), None),
        (
            "PATCH",
            "/internal/organization".into(),
            Some(json!({ "name": "Renamed" })),
        ),
        ("GET", "/internal/organization/members".into(), None),
        (
            "GET",
            format!("/internal/audit/organizations/{ORGANIZATION}"),
            None,
        ),
        (
            "PUT",
            "/internal/me/analytics".into(),
            Some(json!({ "opt_in": true })),
        ),
        (
            "PUT",
            "/internal/me/default-organization".into(),
            Some(json!({ "organizationId": ORGANIZATION })),
        ),
        (
            "POST",
            "/internal/auth/sessions/00000000-0000-0000-0000-000000000000/revoke".into(),
            None,
        ),
    ];
    for (method, uri, body) in lanes {
        let resp = app
            .clone()
            .oneshot(request(&pool, method, &uri, USER, Some(ORGANIZATION), body).await)
            .await
            .unwrap();
        let status = resp.status();
        let answer = json_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {answer}");
        assert_eq!(
            answer["detail"], "this organization is being deleted",
            "{method} {uri}: {answer}"
        );
    }

    // A module's authorize question, in process, with and without a project.
    for project in [None, Some(project.as_str())] {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", bearer(&pool, USER).await)
                .parse()
                .unwrap(),
        );
        headers.insert("x-organization-id", ORGANIZATION.parse().unwrap());
        if let Some(project) = project {
            headers.insert("x-project-id", project.parse().unwrap());
        }
        let refused = state
            .resolve(&headers)
            .await
            .expect_err("a module was answered for a pending organization");
        let problem = refused.to_problem_details();
        assert_eq!(problem.status, 403, "project {project:?}: {problem:?}");
        assert_eq!(
            problem.detail.as_deref(),
            Some("this organization is being deleted"),
            "project {project:?}"
        );
    }

    // A project lane, keyed on the project rather than the organization.
    let resp = app
        .clone()
        .oneshot(
            as_person(
                Request::builder()
                    .method("GET")
                    .uri("/internal/tokens")
                    .header("x-service-secret", SERVICE_SECRET)
                    .header("x-organization-id", ORGANIZATION)
                    .header("x-project-id", &project),
                &pool,
                USER,
            )
            .await
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let answer = json_body(resp).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "GET /internal/tokens: {answer}"
    );
    assert_eq!(answer["detail"], "this organization is being deleted");

    // The project listing is a read that answers strangers with nothing, and
    // answers its own members the same once the organization is going: no
    // project in it is theirs to open.
    let resp = app
        .oneshot(
            request(
                &pool,
                "GET",
                "/internal/projects",
                USER,
                Some(ORGANIZATION),
                None,
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["projects"], json!([]));

    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    assert_eq!(
        audit_count(&pool, ORGANIZATION, "updated").await,
        0,
        "a refused lane wrote onto the chain"
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "a refused lane asked the provider to delete somebody"
    );
    assert!(calls.calls().is_empty(), "a refused lane queued a tail");
}

/// The budget bounds the tail, so a hung sibling cannot outlast the BFF's window.
#[sqlx::test]
async fn the_organization_tail_is_bounded_by_its_budget(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let (_, hook, _) = siblings();
    hook.delaying(Duration::from_secs(2));

    let started = std::time::Instant::now();
    let resp = app(pool.clone(), Some(hook), 100)
        .0
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["purged"], false);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the budget cut the tail off well before the mock's 2s delay"
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
}

/// ⚠ **Deleting an organization deletes nobody.** Its members' rows go with
/// it, and that is all: their accounts, and their places in every other
/// organization, stay exactly as they were — the owner's included.
#[sqlx::test]
async fn deleting_an_organization_touches_nobodys_account(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    seed_person(&pool, COLLEAGUE).await;
    seat(&pool, ORGANIZATION, COLLEAGUE, "admin").await;
    seat_on_project(&pool, &project_of(ORGANIZATION), COLLEAGUE, "admin").await;
    seed_survivor_with(&pool, COLLEAGUE, "member").await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .clone()
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let_the_wait_pass(&pool, ORGANIZATION).await;
    let resp = app.oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    assert_eq!(org_status(&pool, ORGANIZATION).await, None);
    assert_eq!(
        rows_in(&pool, ORGANIZATION, COLLEAGUE).await,
        (0, 0),
        "the member's rows outlived the organization"
    );
    assert_eq!(person(&pool, COLLEAGUE).await, Person::Live);
    assert_eq!(person(&pool, USER).await, Person::Live);
    assert_eq!(
        rows_in(&pool, SURVIVOR, COLLEAGUE).await,
        (1, 1),
        "the member lost their place in an organization nobody deleted"
    );
    let erasures: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit.events WHERE metadata->>'kind' = 'account_deletion'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(erasures, 0, "an organization's deletion erased somebody");
    assert_eq!(
        provider_deletes(&provider),
        0,
        "an organization's deletion asked the provider to delete somebody"
    );
}

/// Only the owner deletes an organization: naming one you are not in is a 403,
/// and so is being its admin. Neither refusal spends or counts the owner's code.
#[sqlx::test]
async fn only_the_owner_deletes_an_organization(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    seed_person(&pool, SURVIVOR_OWNER).await;
    seed_organization(&pool, SURVIVOR, SURVIVOR_OWNER, "active").await;
    seed_person(&pool, COLLEAGUE).await;
    seat(&pool, ORGANIZATION, COLLEAGUE, "admin").await;
    let (calls, hook, _) = siblings();

    for (actor, organization) in [(USER, SURVIVOR), (COLLEAGUE, ORGANIZATION)] {
        let resp = app(pool.clone(), Some(hook.clone()), 8_000)
            .0
            .oneshot(delete_organization(&pool, actor, organization, CODE).await)
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "{actor} deleted {organization}"
        );
    }

    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("active")
    );
    assert_eq!(org_status(&pool, SURVIVOR).await.as_deref(), Some("active"));
    let (live, attempts): (bool, i32) = sqlx::query_as(
        "SELECT consumed_at IS NULL, attempts FROM auth.confirmation_codes WHERE user_id = $1",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(live && attempts == 0, "a refusal spent or counted the code");
    assert!(calls.calls().is_empty(), "a refusal reached the hook");
}

/// ⚠ **A code deletes the organization it was mailed for and no other.** The
/// owner of two is mailed a code naming the one in `x-organization-id`; typed
/// under the other, it is a wrong code.
#[sqlx::test]
async fn a_code_deletes_only_the_organization_it_was_mailed_for(pool: PgPool) {
    const SECOND: &str = "org_inline_second";
    apply_audit_migrations(&pool).await;
    seed_person(&pool, USER).await;
    seed_organization(&pool, ORGANIZATION, USER, "active").await;
    name_organization(&pool, ORGANIZATION, "Alpha Robotics").await;
    seed_organization(&pool, SECOND, USER, "active").await;
    name_organization(&pool, SECOND, "Beta Labs").await;
    let recorder = Arc::new(Recorder::default());

    let resp = app_mailing(pool.clone(), recorder.clone())
        .0
        .oneshot(
            request(
                &pool,
                "POST",
                "/internal/organization/deletion-code",
                USER,
                Some(ORGANIZATION),
                None,
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let mails = recorder.mails();
    assert_eq!(mails.len(), 1, "{mails:?}");
    assert_eq!(mails[0].to, email_of(USER), "the code went to the owner");
    assert!(
        mails[0].subject.contains("Alpha Robotics") && !mails[0].subject.contains("Beta Labs"),
        "the mail does not name the organization it deletes: {}",
        mails[0].subject
    );
    let code = code_in(&mails[0].text);

    let (app, _provider) = app(pool.clone(), None, 8_000);
    let resp = app
        .clone()
        .oneshot(delete_organization(&pool, USER, SECOND, &code).await)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a code for one organization deleted another"
    );
    assert_eq!(org_status(&pool, SECOND).await.as_deref(), Some("active"));
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("active")
    );

    let resp = app
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, &code).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["purged"], true);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    assert_eq!(org_status(&pool, SECOND).await.as_deref(), Some("active"));
}

/// ⚠ **AN OMITTED `x-organization-id` IS A REFUSAL, not a skipped check**, on
/// every lane that acts in, or records onto, the organization it names — for
/// somebody in an organization, as the owner here is. The person's own lanes
/// that act in none — the reset, opening an email change, the account's code
/// and its deletion — take none, and the three recorded ones take none from
/// somebody in no organization (`me_organizations.rs`).
#[sqlx::test]
async fn a_missing_organization_header_is_refused_on_every_lane_that_reads_one(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let (calls, hook, _) = siblings();

    let lanes: &[(&str, &str, Option<Value>)] = &[
        (
            "DELETE",
            "/internal/organization",
            Some(json!({ "code": CODE })),
        ),
        ("POST", "/internal/organization/deletion-code", None),
        (
            "POST",
            "/internal/me/email-change/confirm",
            Some(json!({ "currentCode": "111111", "newCode": "222222" })),
        ),
        (
            "PUT",
            "/internal/me/analytics",
            Some(json!({ "opt_in": true })),
        ),
        (
            "POST",
            "/internal/auth/sessions/00000000-0000-0000-0000-000000000000/revoke",
            None,
        ),
    ];

    for (method, uri, body) in lanes {
        let resp = app(pool.clone(), Some(hook.clone()), 8_000)
            .0
            .oneshot(request(&pool, method, uri, USER, None, body.clone()).await)
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{method} {uri} was served without an x-organization-id header",
        );
    }
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("active")
    );
    assert!(calls.calls().is_empty(), "a refused lane reached the hook");
}

/// Only the owner is mailed a code to delete an organization: acting under one
/// you are not in is a 403, and so is being its admin.
#[sqlx::test]
async fn only_the_owner_is_mailed_an_organization_deletion_code(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    seed_person(&pool, SURVIVOR_OWNER).await;
    seed_organization(&pool, SURVIVOR, SURVIVOR_OWNER, "active").await;
    seed_person(&pool, COLLEAGUE).await;
    seat(&pool, ORGANIZATION, COLLEAGUE, "admin").await;
    let recorder = Arc::new(Recorder::default());

    for (actor, organization) in [(USER, SURVIVOR), (COLLEAGUE, ORGANIZATION)] {
        let resp = app_mailing(pool.clone(), recorder.clone())
            .0
            .oneshot(
                request(
                    &pool,
                    "POST",
                    "/internal/organization/deletion-code",
                    actor,
                    Some(organization),
                    None,
                )
                .await,
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "{actor} was mailed a code for {organization}"
        );
    }
    assert!(
        recorder.mails().is_empty(),
        "a refused request mailed a code"
    );
    let minted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.confirmation_codes
          WHERE user_id IN ($1, $2) AND consumed_at IS NULL",
    )
    .bind(USER)
    .bind(COLLEAGUE)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(minted, 1, "only the seeded code is live");
}

/// ⚠ **A deletion request ends every key the organization holds, at once.**
/// The revoke ran one UPDATE keyed on the organization inside an organization
/// scope, and `api_tokens`' policy keys on the project — so under the real
/// role it matched nothing, and every key kept working through a tail that
/// can stay pending for days. A key mid-rotation (a future `revoked_at`) must
/// die with the rest.
#[sqlx::test]
async fn a_deletion_request_closes_every_key_without_revoking_it(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let keys = seed_keys(&pool).await;
    assert_eq!(
        keys_validate(&pool, keys).await,
        [true, true, true],
        "the seeded keys do not validate before the deletion"
    );

    let (_, hook, _) = siblings();
    hook.down();
    let (app, provider) = app(pool.clone(), Some(hook), 8_000);
    let resp = app
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "the tail stalls on the hook"
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );

    // Closed by the organization's status, not by a revoke: the rows are as
    // they were, and go with the row at finalize.
    assert_no_key_validates(&pool, keys).await;
    assert_eq!(
        unrevoked_keys(&pool).await,
        3,
        "an owner's deletion revoked keys the row's deletion takes anyway"
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "an organization's deletion asked the provider to delete somebody"
    );
}

/// The backstop under the revoke: a key the revoke never reached still stops
/// the moment its organization is no longer active.
#[sqlx::test]
async fn a_pending_deletion_organizations_key_does_not_validate(pool: PgPool) {
    seed_organization_deletion(&pool, "pending_deletion").await;
    let raw = "telmoni_missed_by_the_revoke";
    sqlx::query(
        "INSERT INTO auth.api_tokens (organization_id, project_id, name, token_hash, created_by)
         VALUES ($1, $2, 'missed', $3, $4)",
    )
    .bind(ORGANIZATION)
    .bind(project_of(ORGANIZATION))
    .bind(telmoni_shared::digest::sha256_hex(raw.as_bytes()))
    .bind(USER)
    .execute(&pool)
    .await
    .unwrap();

    let resp = app(pool.clone(), None, 8_000)
        .0
        .oneshot(service(
            "POST",
            "/internal/tokens/validate",
            Some(json!({ "token": raw })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// What the inline tail does not finish, the sweep does: past the grace
/// window the pending row is listed, and finalize takes it once its purges
/// land — and harmlessly again. Finalize will not take an organization nobody
/// asked to delete.
#[sqlx::test]
async fn the_sweep_finishes_a_stalled_organization_deletion(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let (calls, hook, _) = siblings();
    // The hook is down for the request and the first interim purge, then
    // back: for the purge that lands, its harmless repeat, and the finalize.
    hook.failing(2);
    let (router, _provider) = app(pool.clone(), Some(hook), 8_000);
    let app = || router.clone();

    let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "finalize replaced a request"
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("active")
    );

    let resp = app()
        .oneshot(delete_organization(&pool, USER, ORGANIZATION, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["purged"], false);
    assert_eq!(
        listed_organizations(&pool).await,
        vec![(ORGANIZATION.to_owned(), false)],
        "listed for finalize inside the wait, or not listed for the purge"
    );

    // The interim purge, with the hook still down: the row stays owed.
    let resp = app().oneshot(purge_lane(ORGANIZATION)).await.unwrap();
    assert!(
        resp.status().is_server_error(),
        "the purge lane answered {} with the hook down",
        resp.status()
    );
    assert_eq!(
        listed_organizations(&pool).await,
        vec![(ORGANIZATION.to_owned(), false)]
    );

    // The hook back: the purge lands, and the listing leaves the organization
    // out until its wait has passed.
    let resp = app().oneshot(purge_lane(ORGANIZATION)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["purged"], true);
    assert!(
        listed_organizations(&pool).await.is_empty(),
        "a purged organization inside its wait is nobody's to touch"
    );
    let resp = app().oneshot(purge_lane(ORGANIZATION)).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the purge lane is idempotent"
    );

    let_the_wait_pass(&pool, ORGANIZATION).await;
    assert_eq!(
        ripe_organizations(&pool).await,
        vec![ORGANIZATION.to_owned()]
    );
    for attempt in ["finalize", "a retried finalize"] {
        let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "{attempt}");
    }
    assert_eq!(org_status(&pool, ORGANIZATION).await, None);
    assert_eq!(audit_count(&pool, ORGANIZATION, "deleted").await, 1);
    assert!(listed_organizations(&pool).await.is_empty());
    let resp = app().oneshot(purge_lane(ORGANIZATION)).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "the purge lane for an organization already gone"
    );
    assert_eq!(
        hook_purges(&calls),
        5,
        "two refused, then the purge that landed, its repeat, and the finalize's"
    );
}

// ── Deleting an account ──────────────────────────────────────────────────────

/// Owning nothing, nothing waits: the person is erased in the same request —
/// at the provider, and here — and their codes go with them.
#[sqlx::test]
async fn an_account_that_owns_nothing_is_erased_in_the_request(pool: PgPool) {
    seed_account_deletion_owning_nothing(&pool).await;
    let (_, hook, _) = siblings();
    let (app, provider) = app(pool.clone(), Some(hook), 8_000);

    let resp = app
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["deleted"], true);
    assert_eq!(provider_deletes(&provider), 1);
    assert_eq!(person(&pool, USER).await, Person::Erased);
    let codes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM auth.confirmation_codes WHERE user_id = $1")
            .bind(USER)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(codes, 0, "the person's codes outlived them");
}

/// ⚠ **An owned organization's row waits out the grace window, and its owner
/// waits for the row.** The request purges it and answers 202; the people
/// listing leaves the person off while they still own it, so the sweep never
/// fails on the wait; past the window the organization is finalized, and only
/// then is the person listed and erased — at the provider, and here.
#[sqlx::test]
async fn an_account_that_owns_an_organization_is_finished_by_the_sweep(pool: PgPool) {
    seed_account_deletion(&pool).await;
    let (calls, hook, _) = siblings();
    let (router, provider) = app(pool.clone(), Some(hook), 8_000);
    let app = || router.clone();
    let listed = || async {
        let resp = app()
            .oneshot(service("GET", "/internal/people/pending-deletion", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        json_body(resp)
            .await
            .get("people")
            .and_then(Value::as_array)
            .expect("the listing names its people")
            .iter()
            .filter_map(|p| p.get("user_id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect::<Vec<String>>()
    };

    let resp = app()
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["deleted"], false);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    let requested: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'with' FROM audit.events
          WHERE organization_id = $1 AND action = 'updated'",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(requested.as_deref(), Some("account_deletion"));
    assert_eq!(person(&pool, USER).await, Person::Pending);
    assert!(
        listed().await.is_empty(),
        "listed while their organization's row still stands"
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "the provider was asked while the organization's row still stands"
    );

    let_the_wait_pass(&pool, ORGANIZATION).await;
    let resp = app().oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(org_status(&pool, ORGANIZATION).await, None);
    assert_eq!(audit_count(&pool, ORGANIZATION, "deleted").await, 1);
    assert_eq!(listed().await, vec![USER.to_owned()]);

    let resp = app()
        .oneshot(service(
            "POST",
            &format!("/internal/people/{USER}/erase"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(provider_deletes(&provider), 1);
    assert_eq!(person(&pool, USER).await, Person::Erased);
    assert!(listed().await.is_empty());
    assert_eq!(
        hook_purges(&calls),
        2,
        "the request's purge and the finalize's, and no more"
    );
}

/// The confirm path removes the person's memberships in other organizations,
/// each one on that organization's chain, and leaves the organizations be.
#[sqlx::test]
async fn the_inline_tail_removes_other_organization_memberships(pool: PgPool) {
    seed_account_deletion_owning_nothing(&pool).await;
    seed_survivor_with(&pool, USER, "admin").await;
    let (_, hook, _) = siblings();
    let (app, provider) = app(pool.clone(), Some(hook), 8_000);

    let resp = app
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(provider_deletes(&provider), 1);
    assert_eq!(
        org_status(&pool, SURVIVOR).await.as_deref(),
        Some("active"),
        "somebody else's organization went too"
    );
    assert_eq!(
        rows_in(&pool, SURVIVOR, USER).await,
        (0, 0),
        "the rows on the surviving organization outlived the person"
    );
    assert_eq!(
        erasure_removals(&pool, SURVIVOR).await,
        vec![
            (Some(project_of(SURVIVOR)), "admin".to_owned()),
            (None, "member".to_owned()),
        ],
        "one row for each place the person left"
    );
    assert_eq!(person(&pool, SURVIVOR_OWNER).await, Person::Live);
    assert_eq!(person(&pool, USER).await, Person::Erased);
}

/// ⚠ **An organization other people are in is theirs too.** Its owner cannot
/// delete their account out from under it: a 409 naming it, with nothing
/// marked and the code left live, until it is handed over or emptied — then
/// the same code goes through.
#[sqlx::test]
async fn an_account_that_owns_a_shared_organization_is_refused_until_it_is_emptied(pool: PgPool) {
    seed_account_deletion(&pool).await;
    name_organization(&pool, ORGANIZATION, "Shared Robotics").await;
    seed_person(&pool, COLLEAGUE).await;
    seat(&pool, ORGANIZATION, COLLEAGUE, "member").await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .clone()
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = json_body(resp).await.to_string();
    assert!(
        body.contains("Shared Robotics"),
        "the refusal does not name the organization: {body}"
    );
    assert_eq!(person(&pool, USER).await, Person::Live);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("active")
    );
    assert_eq!(rows_in(&pool, ORGANIZATION, COLLEAGUE).await, (1, 0));
    let (live, attempts): (bool, i32) = sqlx::query_as(
        "SELECT consumed_at IS NULL, attempts FROM auth.confirmation_codes WHERE user_id = $1",
    )
    .bind(USER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        live && attempts == 0,
        "the refusal spent or counted the code"
    );

    sqlx::query(
        "DELETE FROM auth.organization_members WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(ORGANIZATION)
    .bind(COLLEAGUE)
    .execute(&pool)
    .await
    .unwrap();

    let resp = app
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "the code kept for later"
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    assert_eq!(person(&pool, USER).await, Person::Pending);
    assert_eq!(person(&pool, COLLEAGUE).await, Person::Live);
    // Not even once it goes through: the provider waits for the organization
    // the account takes with it, whose row waits out the grace window.
    assert_eq!(
        provider_deletes(&provider),
        0,
        "the provider was asked while the organization's row still stands"
    );
}

/// Two organizations may well go by the same name, so the refusal counts the
/// ones that share a label instead of naming the same thing twice.
#[sqlx::test]
async fn the_refusal_counts_shared_organizations_that_share_a_label(pool: PgPool) {
    seed_account_deletion(&pool).await;
    let second = "org_inline_second";
    seed_organization(&pool, second, USER, "active").await;
    name_organization(&pool, ORGANIZATION, "Acme").await;
    name_organization(&pool, second, "Acme").await;
    seed_person(&pool, COLLEAGUE).await;
    seat(&pool, ORGANIZATION, COLLEAGUE, "member").await;
    seat(&pool, second, COLLEAGUE, "member").await;

    let resp = app(pool.clone(), None, 8_000)
        .0
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = json_body(resp).await.to_string();
    assert!(body.contains("Acme (2 organizations)"), "{body}");
    assert_eq!(person(&pool, USER).await, Person::Live);
}

/// An owned organization already being deleted does not hold the account up:
/// it goes anyway, and the erasure waits for its row. Refusing over it would
/// tell the person to hand over something the transfer lane refuses.
#[sqlx::test]
async fn an_owned_organization_already_being_deleted_does_not_block_the_account(pool: PgPool) {
    seed_account_deletion(&pool).await;
    let going = "org_inline_going";
    seed_organization(&pool, going, USER, "pending_deletion").await;
    seed_person(&pool, COLLEAGUE).await;
    seat(&pool, going, COLLEAGUE, "member").await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    let status = resp.status();
    let body = json_body(resp).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(
        org_status(&pool, going).await.as_deref(),
        Some("pending_deletion")
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion"),
        "the organization owned alone was not taken with the account"
    );
    assert_eq!(person(&pool, USER).await, Person::Pending);
    assert_eq!(person(&pool, COLLEAGUE).await, Person::Live);

    // ⚠ The one already pending is taken with the account: its kind becomes
    // `account`, so its chain says what took it, and the take never lengthens
    // its wait: the earlier moment stands. Its chain records the take.
    let (wait, kind) = wait_and_kind(&pool, going).await;
    assert_eq!(kind, "account", "the kind still names the owner's request");
    assert!(
        wait > 0.0 && wait <= 900.0 + 5.0,
        "the take lengthened the wait: {wait}"
    );
    assert_eq!(
        audit_count(&pool, going, "updated").await,
        1,
        "the take is audited on the organization's chain"
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "the provider was asked while an owned organization's row still stands"
    );
}

/// ⚠ **The provider goes FIRST**: never wipe local rows for a person who still
/// exists upstream. It fails → the person stays pending with every place they
/// hold elsewhere, their sessions already ended, and the gate refusing them.
#[sqlx::test]
async fn a_failed_provider_erasure_leaves_the_person_and_their_memberships(pool: PgPool) {
    seed_account_deletion_owning_nothing(&pool).await;
    seed_survivor_with(&pool, USER, "admin").await;
    sqlx::query(
        "INSERT INTO auth.sessions (user_id, provider_sid, user_agent) VALUES ($1, $2, 'agent')",
    )
    .bind(USER)
    .bind("sess_inline_del")
    .execute(&pool)
    .await
    .unwrap();
    let (app, provider) = app(pool.clone(), None, 8_000);
    provider.on_delete_user(provider_down());

    let resp = app
        .clone()
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["deleted"], false);
    assert_eq!(provider_deletes(&provider), 1);
    assert_eq!(person(&pool, USER).await, Person::Pending);
    assert_eq!(
        rows_in(&pool, SURVIVOR, USER).await,
        (1, 1),
        "local rows went for a person who still exists at the provider"
    );
    assert!(erasure_removals(&pool, SURVIVOR).await.is_empty());
    let revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM auth.sessions WHERE user_id = $1")
            .bind(USER)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(revoked, "a session outlived the deletion request");

    let resp = app
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a person whose deletion is pending was served; the sweep finishes it"
    );
}

/// The budget bounds the account's tails too, so a hung provider cannot
/// outlast the BFF's window.
#[sqlx::test]
async fn the_account_tail_is_bounded_by_its_budget(pool: PgPool) {
    seed_account_deletion_owning_nothing(&pool).await;

    let started = std::time::Instant::now();
    let resp = app_with_hung_provider(pool.clone(), Duration::from_secs(2), 100)
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["deleted"], false);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the budget cut the tail off well before the provider's 2s delay"
    );
    assert_eq!(person(&pool, USER).await, Person::Pending);
}

/// ⚠ **Keys stop at the mark on this flow too**, and the hook still comes
/// before anything irreversible: a stalled hook purge leaves the owned
/// organization pending with every key dead, and the provider never asked.
#[sqlx::test]
async fn a_stalled_account_deletion_has_revoked_every_key_and_spared_the_provider(pool: PgPool) {
    seed_account_deletion(&pool).await;
    let keys = seed_keys(&pool).await;
    let (calls, hook, _) = siblings();
    hook.down();
    let (app, provider) = app(pool.clone(), Some(hook), 8_000);

    let resp = app
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(hook_purges(&calls), 1, "the request asked the hook once");
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    assert_eq!(person(&pool, USER).await, Person::Pending);
    assert_eq!(
        provider_deletes(&provider),
        0,
        "the provider was asked before the hook purge landed"
    );
    // Revoked outright, unlike an owner's deletion, and counted on the
    // chain: the person's erasure accounts for the keys their organizations
    // held.
    assert_no_key_validates(&pool, keys).await;
    assert_eq!(
        unrevoked_keys(&pool).await,
        0,
        "a key outlived the account deletion"
    );
}

/// What the inline tail does not finish, the sweep does: the person is
/// listed, and the erase lane takes them — once, and harmlessly again.
#[sqlx::test]
async fn the_sweep_finishes_a_stalled_account_deletion(pool: PgPool) {
    seed_account_deletion_owning_nothing(&pool).await;
    let (app, provider) = app(pool.clone(), None, 8_000);
    // Down for the request; back, unscripted, for the sweep.
    provider.on_delete_user(provider_down());
    let resp = app
        .clone()
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(provider_deletes(&provider), 1);

    let resp = app
        .clone()
        .oneshot(service("GET", "/internal/people/pending-deletion", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let pending = json_body(resp).await;
    assert_eq!(pending["count"], 1, "{pending}");
    assert_eq!(pending["people"][0]["user_id"], USER, "{pending}");

    let erase = format!("/internal/people/{USER}/erase");
    for attempt in ["the erasure", "a retried erasure"] {
        let resp = app
            .clone()
            .oneshot(service("POST", &erase, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "{attempt}");
    }
    assert_eq!(person(&pool, USER).await, Person::Erased);
    assert_eq!(
        provider_deletes(&provider),
        2,
        "the provider is asked once by the request and once by the erasure that lands; the retry finds nobody to erase"
    );

    let resp = app
        .oneshot(service("GET", "/internal/people/pending-deletion", None))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["count"], 0);
}

/// The erase lane removes the person's memberships in other organizations,
/// each audited there, after the provider and before the identity.
#[sqlx::test]
async fn erasing_a_person_removes_their_memberships_elsewhere(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_person(&pool, USER).await;
    mark_person_pending(&pool, USER).await;
    seed_survivor_with(&pool, USER, "admin").await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .oneshot(service(
            "POST",
            &format!("/internal/people/{USER}/erase"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(provider_deletes(&provider), 1);

    assert_eq!(
        rows_in(&pool, SURVIVOR, USER).await,
        (0, 0),
        "an erased person holds no membership row anywhere"
    );
    assert_eq!(
        erasure_removals(&pool, SURVIVOR).await,
        vec![
            (Some(project_of(SURVIVOR)), "admin".to_owned()),
            (None, "member".to_owned()),
        ],
        "one row for each place the person left"
    );
    assert_eq!(person(&pool, USER).await, Person::Erased);
}

/// An invitation from the survivor's owner — to its project when `project` is
/// set, to the organization itself otherwise — addressed to `to`, and accepted
/// by `accepted_by` if anyone.
async fn seed_invitation(pool: &PgPool, project: bool, to: &str, accepted_by: Option<&str>) {
    let sql = if project {
        "INSERT INTO auth.member_invites
             (id, project_id, email, role, token_hash, invited_by, expires_at,
              accepted_at, accepted_by)
         VALUES (gen_random_uuid(), $1, $2, 'admin', gen_random_uuid()::text, $3,
                 now() + interval '7 days', CASE WHEN $4::text IS NOT NULL THEN now() END, $4)"
    } else {
        "INSERT INTO auth.organization_invites
             (id, organization_id, email, role, token_hash, invited_by, expires_at,
              accepted_at, accepted_by)
         VALUES (gen_random_uuid(), $1, $2, 'member', gen_random_uuid()::text, $3,
                 now() + interval '7 days', CASE WHEN $4::text IS NOT NULL THEN now() END, $4)"
    };
    sqlx::query(sql)
        .bind(if project {
            project_of(SURVIVOR)
        } else {
            SURVIVOR.to_owned()
        })
        .bind(email_of(to))
        .bind(SURVIVOR_OWNER)
        .bind(accepted_by)
        .execute(pool)
        .await
        .unwrap();
}

/// The addresses still standing on the survivor's invitations, both kinds.
async fn invitation_addresses(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT email FROM auth.member_invites
          UNION ALL
         SELECT email FROM auth.organization_invites
          ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// ⚠ **An erased person's address does not outlive them on the invitations
/// they accepted** — rows the sending organization holds, which nothing else
/// ever deletes. Keyed on who accepted, not on the address: an invitation to
/// the same address that nobody accepted is the sender's to revoke, and one a
/// colleague accepted is theirs.
#[sqlx::test]
async fn erasing_a_person_deletes_the_invitations_they_accepted(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_person(&pool, USER).await;
    seed_person(&pool, COLLEAGUE).await;
    mark_person_pending(&pool, USER).await;
    seed_survivor_with(&pool, USER, "admin").await;
    seed_invitation(&pool, false, USER, Some(USER)).await;
    seed_invitation(&pool, true, USER, Some(USER)).await;
    seed_invitation(&pool, false, USER, None).await;
    seed_invitation(&pool, true, COLLEAGUE, Some(COLLEAGUE)).await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    for _ in 0..2 {
        let resp = app
            .clone()
            .oneshot(service(
                "POST",
                &format!("/internal/people/{USER}/erase"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NO_CONTENT,
            "a second run is a no-op"
        );
    }

    assert_eq!(person(&pool, USER).await, Person::Erased);
    assert_eq!(
        provider_deletes(&provider),
        1,
        "the second run asked the provider for a person already gone"
    );
    assert_eq!(
        invitation_addresses(&pool).await,
        vec![email_of(COLLEAGUE), email_of(USER)],
        "the two invitations the person accepted went; the unaccepted one to \
         their address and the colleague's accepted one stayed"
    );
    let unaccepted: bool = sqlx::query_scalar(
        "SELECT accepted_at IS NULL FROM auth.organization_invites WHERE email = $1",
    )
    .bind(email_of(USER))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        unaccepted,
        "the address left standing is on the invitation nobody accepted"
    );
}

/// ⚠ The erase lane is a SERVICE lane, and anything holding the service secret
/// reaches it: a person who never asked to be deleted is refused before the
/// provider is touched, and keeps every place they hold.
#[sqlx::test]
async fn the_erase_lane_refuses_a_live_person(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_person(&pool, USER).await;
    seed_survivor_with(&pool, USER, "admin").await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .oneshot(service(
            "POST",
            &format!("/internal/people/{USER}/erase"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    assert_eq!(person(&pool, USER).await, Person::Live);
    assert_eq!(
        rows_in(&pool, SURVIVOR, USER).await,
        (1, 1),
        "a live person keeps their memberships"
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "the provider was asked to delete a person who never asked to go"
    );
}

/// ⚠ Nor does it erase somebody who still owns an organization: that
/// organization's deletion finishes first, or its owner would be a sign-in the
/// provider no longer has. The expected wait, so a conflict the sweep retries
/// every tick — not an error logged every tick.
#[sqlx::test]
async fn the_erase_lane_waits_for_an_organization_the_person_still_owns(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_person(&pool, USER).await;
    seed_organization(&pool, ORGANIZATION, USER, "pending_deletion").await;
    mark_person_pending(&pool, USER).await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .oneshot(service(
            "POST",
            &format!("/internal/people/{USER}/erase"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    assert_eq!(person(&pool, USER).await, Person::Pending);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "the provider was asked while the organization's row still stands"
    );
}

/// ⚠ **An owned organization that is still active is a stuck erasure, and says
/// so.** Its deletion was never requested, so waiting would never end; the
/// lane answers an error the logs alert on instead of the retry-me conflict,
/// and touches nothing — the provider least of all.
#[sqlx::test]
async fn the_erase_lane_calls_out_an_active_organization_the_person_still_owns(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    seed_person(&pool, USER).await;
    seed_organization(&pool, ORGANIZATION, USER, "active").await;
    mark_person_pending(&pool, USER).await;
    let (app, provider) = app(pool.clone(), None, 8_000);

    let resp = app
        .oneshot(service(
            "POST",
            &format!("/internal/people/{USER}/erase"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    assert_eq!(person(&pool, USER).await, Person::Pending);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("active")
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "a stuck erasure touched the provider"
    );
}

/// A module asks where an organization stands before it lets something land
/// for it — the purge hook's module, for what a vendor delivers after the
/// purge. Active, being deleted, or gone; gone is `None`, an answer, never
/// an error a caller could read as a deploy skew.
#[sqlx::test]
async fn the_standing_seam_answers_active_being_deleted_and_gone(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    seed_person(&pool, SURVIVOR_OWNER).await;
    seed_organization(&pool, SURVIVOR, SURVIVOR_OWNER, "pending_deletion").await;
    let (_, _provider, state) = app_and_state(pool.clone(), None, 8_000);

    for (organization, standing) in [
        (ORGANIZATION, Some(OrganizationStatus::Active)),
        (SURVIVOR, Some(OrganizationStatus::PendingDeletion)),
        ("org_never_was", None),
    ] {
        let answered = state
            .organization_standing(&OrganizationId::try_new(organization).unwrap())
            .await
            .unwrap();
        assert_eq!(answered, standing, "{organization}");
    }
}

/// An invitation from an organization being deleted is withdrawn with it:
/// not listed for the person it went to, and neither its link nor its id takes
/// them in — the answer an expired one gets, not a refusal from deeper down.
#[sqlx::test]
async fn an_invitation_from_an_organization_being_deleted_is_withdrawn_with_it(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    name_organization(&pool, ORGANIZATION, "Alpha Robotics").await;
    seed_person(&pool, COLLEAGUE).await;
    let (router, _provider) = app(pool.clone(), None, 8_000);
    let app = || router.clone();

    let resp = app()
        .oneshot(
            request(
                &pool,
                "POST",
                "/internal/organization/invites",
                USER,
                Some(ORGANIZATION),
                Some(json!({ "email": email_of(COLLEAGUE), "role": "member" })),
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let invite = json_body(resp).await;
    let id = invite.get("id").and_then(Value::as_str).unwrap().to_owned();
    let token = invite
        .get("link")
        .and_then(Value::as_str)
        .and_then(|link| link.rsplit('/').next())
        .unwrap()
        .to_owned();
    mark_pending(&pool, ORGANIZATION).await;

    let resp = app()
        .oneshot(request(&pool, "GET", "/internal/me/invites", COLLEAGUE, None, None).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["invites"], json!([]));

    for (uri, body) in [
        (format!("/internal/me/invites/{id}/accept"), None),
        (format!("/internal/me/invites/{id}/decline"), None),
        (
            "/internal/invites/accept".to_owned(),
            Some(json!({ "token": token })),
        ),
    ] {
        let resp = app()
            .oneshot(request(&pool, "POST", &uri, COLLEAGUE, None, body).await)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
    }
    let resp = app()
        .oneshot(service(
            "POST",
            "/internal/invites/look",
            Some(json!({ "token": token })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(rows_in(&pool, ORGANIZATION, COLLEAGUE).await, (0, 0));
}

// ── An operator's termination ────────────────────────────────────────────────

/// ⚠ **An operator closes an organization without its owner's code**, and it
/// stands where an owner's deletion would: closed to every lane, the hook
/// purged, its row waiting out the grace, and the chain recording it as
/// `service:operator`. Terminating what is already pending, or does not
/// exist, is refused.
#[sqlx::test]
async fn an_operator_terminates_without_a_code(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let keys = seed_keys(&pool).await;
    let (calls, hook, _) = siblings();
    let (router, provider) = app(pool.clone(), Some(hook), 8_000);
    let app = || router.clone();

    let resp = app().oneshot(terminate(ORGANIZATION)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let answer = json_body(resp).await;
    assert_eq!(answer["purged"], true, "{answer}");
    assert!(answer["erase_after"].is_string(), "{answer}");
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion")
    );
    assert_no_key_validates(&pool, keys).await;
    let (actor, by): (String, Option<String>) = sqlx::query_as(
        "SELECT actor_id, metadata->>'by' FROM audit.events
          WHERE organization_id = $1 AND action = 'updated'",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (actor.as_str(), by.as_deref()),
        ("service:operator", Some("operator"))
    );

    // The owner: refused on every lane.
    let resp = app()
        .oneshot(
            request(
                &pool,
                "PATCH",
                "/internal/organization",
                USER,
                Some(ORGANIZATION),
                Some(json!({ "name": "Renamed" })),
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app().oneshot(terminate(ORGANIZATION)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT, "terminated twice");
    let resp = app().oneshot(terminate("org_never_was")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        provider_deletes(&provider),
        0,
        "a termination asked the provider to delete somebody"
    );
    assert_eq!(
        hook_purges(&calls),
        1,
        "the termination's purge, and nothing at the refusals"
    );
}

/// ⚠ **A ripe organization whose hook purge never landed has the hook run
/// before anything else.** The request's purge failed and the wait passed
/// before the interim lane could land it; now notifications is down. Behind
/// notifications, the hook purge would wait out that outage with whatever it
/// stops — a subscription charging, say — running all the while, for a
/// sibling that has nothing to do with it. The hook goes first and is
/// recorded; the finalize still fails on notifications, and the retry
/// finishes it.
#[sqlx::test]
async fn a_ripe_organizations_unrecorded_hook_purge_lands_before_notifications_can_fail_it(
    pool: PgPool,
) {
    seed_organization_deletion(&pool, "pending_deletion").await;
    let_the_wait_pass(&pool, ORGANIZATION).await;
    let (calls, hook, notifications) = siblings();
    notifications.down();
    let (app, provider) = app_with_siblings(pool.clone(), Some(hook), Some(notifications), 8_000);

    let resp = app.oneshot(finalize(ORGANIZATION)).await.unwrap();
    assert!(
        resp.status().is_server_error(),
        "finalize answered {} with notifications failing",
        resp.status()
    );
    assert_eq!(
        calls.calls(),
        vec![hook_purge(ORGANIZATION), notifications_purge(ORGANIZATION)],
        "the hook first, then notifications, and nothing after the failure"
    );
    let recorded: bool = sqlx::query_scalar(
        "SELECT hook_purged_at IS NOT NULL FROM auth.organizations WHERE external_id = $1",
    )
    .bind(ORGANIZATION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        recorded,
        "the hook was not purged and recorded before notifications failed the finalize"
    );
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("pending_deletion"),
        "the row waits for the retry"
    );
    assert_eq!(
        provider_deletes(&provider),
        0,
        "a finalize asked the provider to delete somebody"
    );
}

/// The interim purge follows a request, never replaces one: an active
/// organization is refused, and the hook is not asked.
#[sqlx::test]
async fn the_purge_lane_refuses_an_active_organization(pool: PgPool) {
    seed_organization_deletion(&pool, "active").await;
    let (calls, hook, _) = siblings();

    let resp = app(pool.clone(), Some(hook), 8_000)
        .0
        .oneshot(purge_lane(ORGANIZATION))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(
        org_status(&pool, ORGANIZATION).await.as_deref(),
        Some("active")
    );
    assert!(
        calls.calls().is_empty(),
        "the hook was asked for an active organization"
    );
}

// ── The notices that named a person ──────────────────────────────────────────

/// The erasure asks notifications to rewrite every notice that named the
/// person twice, both after the provider and before their identity goes:
/// once before the agent's first erase, which reads the feed again, and once
/// after their memberships go, for a notice announced while they still had
/// them.
#[sqlx::test]
async fn erasing_a_person_redacts_the_notices_that_named_them(pool: PgPool) {
    seed_account_deletion_owning_nothing(&pool).await;
    seed_survivor_with(&pool, USER, "admin").await;
    let (calls, hook, notifications) = siblings();
    let (app, provider) =
        app_with_siblings(pool.clone(), Some(hook), Some(notifications.clone()), 8_000);
    // How many user deletes the provider had been asked for by the time the
    // redaction was: the order of an erasure's steps.
    let deletes_before_redact = Arc::new(AtomicUsize::new(usize::MAX));
    let noting = (provider.clone(), deletes_before_redact.clone());
    notifications.observing(move |call| {
        if matches!(call, SiblingCall::RedactPerson(_)) {
            noting
                .1
                .store(provider_deletes(&noting.0), Ordering::SeqCst);
        }
    });

    let resp = app
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(person(&pool, USER).await, Person::Erased);

    assert_eq!(provider_deletes(&provider), 1);
    assert_eq!(
        calls.calls(),
        [notifications_redact(USER), notifications_redact(USER)],
        "before the agent's first erase, and after the memberships"
    );
    assert_eq!(
        deletes_before_redact.load(Ordering::SeqCst),
        1,
        "the provider first, the feeds last"
    );
}

/// ⚠ **A redaction that fails stops the erasure before the identity goes**,
/// for the sweep to retry: the person stays pending with their memberships
/// already gone and the provider already asked once, and the erase lane
/// finishes it once notifications answers. The second pass is the one that
/// fails here, as it is the one after the memberships went.
#[sqlx::test]
async fn a_failed_redaction_leaves_the_person_pending_for_the_sweep(pool: PgPool) {
    seed_account_deletion_owning_nothing(&pool).await;
    seed_survivor_with(&pool, USER, "admin").await;
    let (calls, hook, notifications) = siblings();
    notifications.on_redact_person(Ok(0)).failing_redactions(1);
    let (router, provider) =
        app_with_siblings(pool.clone(), Some(hook), Some(notifications), 8_000);
    let app = || router.clone();

    let resp = app()
        .oneshot(delete_account(&pool, USER, CODE).await)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(resp).await["deleted"], false);
    assert_eq!(person(&pool, USER).await, Person::Pending);
    assert_eq!(provider_deletes(&provider), 1);
    assert_eq!(
        rows_in(&pool, SURVIVOR, USER).await,
        (0, 0),
        "the memberships were removed before the redaction was asked"
    );

    let resp = app()
        .oneshot(service(
            "POST",
            &format!("/internal/people/{USER}/erase"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(person(&pool, USER).await, Person::Erased);
    // The retry asks the provider again, for a person already gone there,
    // which a provider that can delete answers as done.
    assert_eq!(provider_deletes(&provider), 2);
    assert_eq!(
        calls.calls(),
        [
            notifications_redact(USER),
            notifications_redact(USER),
            notifications_redact(USER),
            notifications_redact(USER),
        ],
        "the first pass, the second that failed, and the retry's two"
    );
}
