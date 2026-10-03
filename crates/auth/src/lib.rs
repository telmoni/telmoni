//! Auth — identities, organizations, members, roles, API tokens, sessions —
//! as a module of the `telmoni` binary. It owns the `auth` schema, connects
//! as the `auth` role, and answers the modules beside it through
//! [`telmoni_shared::seam::Auth`]; what it asks of them goes through
//! [`Siblings`].

#![deny(missing_docs)]

pub mod boot;
#[expect(missing_docs, reason = "internal wiring exposed for integration tests")]
pub mod config;
#[expect(missing_docs, reason = "internal wiring exposed for integration tests")]
pub mod db;
pub mod external;
#[expect(missing_docs, reason = "internal wiring exposed for integration tests")]
pub mod handler;
pub mod holder;
pub mod identity;
pub mod issuer;
pub mod mailer;
#[expect(missing_docs, reason = "internal wiring exposed for integration tests")]
pub mod model;
pub mod notify;
pub mod oidc;
pub mod password;
pub mod person;
pub mod provider;
pub mod seam;
pub mod smtp;
pub mod sweep;
pub mod test_provider;
pub mod token_claims;

use std::sync::Arc;

use axum::{
    Router, middleware,
    routing::{delete, get, patch, post, put},
};
use sqlx::PgPool;
use telmoni_shared::middleware::service_auth::{ServiceSecrets, require_service_secret};

pub use config::Config;
pub use provider::{AuthProvider, Authenticated, Subject, TokenResponse};

/// The outbound HTTP client the module's provider hops share: a bounded
/// timeout, and a user agent naming this crate.
pub fn http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
}

/// What a binary supplies that this library does not decide: the issuer
/// every session is minted by, the ways in, and the transport outbound mail
/// leaves by.
pub struct Providers {
    /// Mints every session's bearer and refresh token, whichever way the
    /// person signed in, and the CLI's device grant.
    pub issuer: Arc<issuer::Issuer>,
    /// The accounts held here — the login form. `None` when the form is off
    /// (`DISABLE_LOGIN_FORM`), which leaves the external provider the one
    /// way in.
    pub password: Option<Arc<password::PasswordProvider>>,
    /// An external identity provider beside the form: this crate's
    /// [`oidc::OidcProvider`], or the binary's own, with the policy it
    /// signs people in under.
    pub external: Option<external::External>,
    /// Where composed mail goes. [`smtp::SmtpSender`] sends it;
    /// [`telmoni_shared::mail::NoopSender`] logs it instead.
    pub mail: Arc<dyn telmoni_shared::mail::MailSender>,
}

/// The modules beside auth that hold rows of its tenants, as the binary
/// links them. Each is optional: a test, or a binary without the module,
/// leaves it `None`, and every call to it is a step skipped.
#[derive(Default)]
pub struct Siblings {
    /// Notifications: the notices auth raises, and what it purges of an
    /// organization, a project or a person before their rows go.
    pub notifications: Option<Arc<dyn telmoni_shared::seam::Notifications>>,
    /// The agent: the conversations and the index it holds of a person or an
    /// organization, forgotten when they go. A project that moves or goes
    /// needs no call: the agent finds what it held of it and removes that
    /// itself.
    pub agent: Option<Arc<dyn telmoni_shared::seam::Agent>>,
    /// The deployment's own module that holds something of an organization's
    /// outside this database — a subscription, say — run when a deletion is
    /// confirmed and again before the row goes.
    pub purge_hook: Option<Arc<dyn telmoni_shared::seam::PurgeHook>>,
}

/// The module's pool, opened as its own database role.
pub async fn open_pool(config: &Config) -> anyhow::Result<PgPool> {
    let db_ca = telmoni_shared::db::database_ca_from_env().map_err(anyhow::Error::msg)?;
    Ok(
        telmoni_shared::db::create_pool_for_service(&config.database_url, "auth", db_ca.as_deref())
            .await?,
    )
}

/// How long either state survives a sweep.
const EXCHANGE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone)]
enum ExchangeState {
    /// ⚠ **The `Instant` is what makes this evictable, and it is load-bearing.**
    InProgress(
        tokio::sync::watch::Receiver<Option<Result<handler::session::AuthnResult, String>>>,
        std::time::Instant,
    ),
    Completed(Box<handler::session::AuthnResult>, std::time::Instant),
}

impl ExchangeState {
    fn is_live(&self, now: std::time::Instant) -> bool {
        let (Self::InProgress(_, ts) | Self::Completed(_, ts)) = self;
        now.duration_since(*ts) < EXCHANGE_TTL
    }
}

/// Decision made when checking or starting an authorization code exchange.
pub enum CacheDecision {
    /// Result was found in cache.
    Hit(Box<handler::session::AuthnResult>),
    /// Another concurrent exchange is already in flight; wait for its outcome.
    Wait(tokio::sync::watch::Receiver<Option<Result<handler::session::AuthnResult, String>>>),
    /// This caller is the leader responsible for performing the exchange.
    Leader(tokio::sync::watch::Sender<Option<Result<handler::session::AuthnResult, String>>>),
}

/// In-memory cache of recent code exchange results, so a double-click or a
/// network retry on the callback does not spend the one-time code twice.
#[derive(Clone, Default)]
pub struct ExchangeCache(Arc<std::sync::Mutex<std::collections::HashMap<String, ExchangeState>>>);

impl ExchangeCache {
    /// Drop every entry past `EXCHANGE_TTL`.
    fn sweep(map: &mut std::collections::HashMap<String, ExchangeState>, now: std::time::Instant) {
        map.retain(|_, entry| entry.is_live(now));
    }

    /// Create a new empty exchange cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Retrieve a cached `AuthnResult` for the given authorization code if present and not expired (30s TTL).
    pub fn get(&self, code: &str) -> Option<handler::session::AuthnResult> {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::sweep(&mut map, std::time::Instant::now());
        match map.get(code) {
            Some(ExchangeState::Completed(res, _)) => Some(res.as_ref().clone()),
            _ => None,
        }
    }

    /// Check the cache or claim leader status for the given authorization code.
    pub fn start_or_join(&self, code: &str) -> CacheDecision {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::sweep(&mut map, std::time::Instant::now());

        let orphaned = matches!(
            map.get(code),
            Some(ExchangeState::InProgress(rx, _)) if rx.has_changed().is_err()
        );
        if orphaned {
            map.remove(code);
        }

        match map.get(code) {
            Some(ExchangeState::Completed(res, _)) => CacheDecision::Hit(res.clone()),
            Some(ExchangeState::InProgress(rx, _)) => CacheDecision::Wait(rx.clone()),
            None => {
                let (tx, rx) = tokio::sync::watch::channel(None);
                map.insert(
                    code.to_string(),
                    ExchangeState::InProgress(rx, std::time::Instant::now()),
                );
                CacheDecision::Leader(tx)
            }
        }
    }

    /// Cache an `AuthnResult` against the given authorization code.
    pub fn insert(&self, code: String, result: handler::session::AuthnResult) {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.insert(
            code,
            ExchangeState::Completed(Box::new(result), std::time::Instant::now()),
        );
    }

    /// Remove a code from the cache (e.g. if exchange failed).
    pub fn remove(&self, code: &str) {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.remove(code);
    }
}

/// Shared application state passed into every handler via axum `State`.
pub struct AppState {
    /// Postgres pool scoped to the `auth` schema.
    pub db: PgPool,
    /// Loaded-once module configuration.
    pub config: Config,
    /// Mints every session's tokens, and the CLI's device grant.
    pub issuer: Arc<issuer::Issuer>,
    /// The accounts held here, while the login form is on; the router
    /// mounts the password lanes on this.
    pub password: Option<Arc<password::PasswordProvider>>,
    /// The external identity provider, when one is configured, and the
    /// policy it signs people in under.
    pub external: Option<external::External>,
    /// Outbound mail: the copy, over whatever transport the binary supplied.
    pub mailer: Arc<dyn mailer::Mailer>,
    /// Idempotency cache for recent code exchange requests.
    pub exchange_cache: ExchangeCache,
    /// The modules beside this one that hold rows of its tenants.
    pub siblings: Siblings,
}

impl AppState {
    /// The state a binary runs the module on: `providers` from the
    /// environment or the binary's own, `siblings` as it links them.
    #[must_use]
    pub fn new(config: Config, db: PgPool, providers: Providers, siblings: Siblings) -> Self {
        let mailer: Arc<dyn mailer::Mailer> = Arc::new(
            mailer::ComposingMailer::new(providers.mail)
                .with_support_email(config.support_email.clone()),
        );
        Self {
            db,
            config,
            issuer: providers.issuer,
            password: providers.password,
            external: providers.external,
            mailer,
            exchange_cache: ExchangeCache::new(),
            siblings,
        }
    }

    /// The readiness probe: one keyed read of the module's own tables. ⚠ A
    /// TABLE, not `SELECT 1`: a bare ping answers healthy while the role is
    /// missing its grant, which is exactly what happened on a from-scratch
    /// tier. Keyed on the unique index: with no scope bound, an unkeyed read
    /// is a scan of every tenant's rows that admits none of them.
    pub async fn ready(&self) -> sqlx::Result<()> {
        sqlx::query("SELECT 1 FROM auth.organizations WHERE external_id = ''")
            .execute(&self.db)
            .await
            .map(|_| ())
    }
}

/// The module's router: the person lanes, the pre-session lanes, `/me` and
/// `/v1`, under this module's own gates. The binary mounts it beside the
/// other modules' and adds the health probes and the request layers once.
pub fn router(state: Arc<AppState>) -> Router {
    let secrets = ServiceSecrets::new(
        state.config.service_secret.clone(),
        state.config.service_secret_next.clone(),
    );

    // ⚠ **TWO LANES UNDER `/internal`, AND THE SPLIT IS A SECURITY BOUNDARY.**
    // The service secret is held by the console, so it proves the hop began
    // inside the platform and nothing about WHO is asking. `person` is every
    // route whose handler acts for somebody: `require_person_token` resolves
    // the bearer the console relays (one the issuer minted, looked up by its
    // hash) and refuses a revoked session, and the handler reads the person
    // from that and never from a header.
    // `service` carries no person — pre-session lanes, the invite lookup.
    //
    // A NEW ROUTE BELONGS IN `person` unless it demonstrably carries no
    // identity. That is the safe default.
    let person = Router::new()
        .route("/auth/sessions", get(handler::sessions::list))
        .route(
            "/auth/sessions/{id}/revoke",
            post(handler::sessions::revoke),
        )
        .route(
            "/projects",
            get(handler::projects::list_projects).post(handler::projects::create_project),
        )
        .route(
            "/projects/everywhere",
            get(handler::projects::list_projects_everywhere),
        )
        .route(
            "/projects/{project_id}",
            patch(handler::projects::update_project).delete(handler::projects::delete_project),
        )
        .route(
            "/projects/{project_id}/members",
            get(handler::member::list_members),
        )
        .route(
            "/projects/{project_id}/members/{member_id}/role",
            put(handler::member::update_role),
        )
        .route(
            "/projects/{project_id}/members/{member_id}",
            delete(handler::member::remove_member),
        )
        .route(
            "/projects/{project_id}/transfer",
            post(handler::project_transfer::offer).delete(handler::project_transfer::cancel),
        )
        .route(
            "/projects/{project_id}/transfer/accept",
            post(handler::project_transfer::accept),
        )
        .route(
            "/projects/{project_id}/transfer/decline",
            post(handler::project_transfer::decline),
        )
        .route(
            "/organization",
            delete(handler::organization::delete_organization)
                .patch(handler::organization::update_organization),
        )
        .route(
            "/organization/deletion-code",
            post(handler::organization::request_organization_deletion_code),
        )
        .route(
            "/organization/restore",
            post(handler::organization::restore_organization),
        )
        .route(
            "/organization/owner-transfer",
            post(handler::ownership::offer).delete(handler::ownership::cancel),
        )
        .route(
            "/organization/owner-transfer/accept",
            post(handler::ownership::accept),
        )
        .route(
            "/organization/owner-transfer/decline",
            post(handler::ownership::decline),
        )
        .route(
            "/organization/export",
            get(handler::export::export_organization),
        )
        .route("/me", delete(handler::account::delete_account))
        .route(
            "/me/deletion-code",
            post(handler::account::request_account_deletion_code),
        )
        .route(
            "/me/password-reset",
            post(handler::account::request_password_reset),
        )
        .route(
            "/me/email-change",
            post(handler::account::request_email_change),
        )
        .route(
            "/me/email-change/confirm",
            post(handler::account::confirm_email_change),
        )
        .route(
            "/me/analytics",
            put(handler::account::set_analytics_preference),
        )
        .route(
            "/organization/members",
            get(handler::organization_members::list_organization_members),
        )
        .route(
            "/organization/members/{member_id}/role",
            put(handler::organization_members::update_organization_member_role),
        )
        .route(
            "/organization/members/{member_id}",
            delete(handler::organization_members::remove_organization_member),
        )
        .route(
            "/organization/invites",
            get(handler::organization_members::list_organization_invites)
                .post(handler::organization_members::create_organization_invite),
        )
        .route(
            "/organization/invites/{invite_id}",
            delete(handler::organization_members::revoke_organization_invite),
        )
        .route(
            "/projects/{project_id}/invites",
            get(handler::invite::list_invites).post(handler::invite::create_invite),
        )
        .route(
            "/projects/{project_id}/invites/{invite_id}",
            delete(handler::invite::revoke_invite),
        )
        .route("/invites/accept", post(handler::invite::accept_invite))
        .route(
            "/me/invites",
            get(handler::invite::list_my_incoming_invites),
        )
        .route(
            "/me/invites/{invite_id}/accept",
            post(handler::invite::accept_my_incoming_invite),
        )
        .route(
            "/me/invites/{invite_id}/decline",
            post(handler::invite::decline_my_incoming_invite),
        )
        .route("/memberships/{project_id}", delete(handler::member::leave))
        .route(
            "/tokens",
            post(handler::tokens::create_token).get(handler::tokens::list_tokens),
        )
        .route("/tokens/{token_id}", delete(handler::tokens::revoke_token))
        .route(
            "/tokens/{token_id}/rotate",
            post(handler::tokens::rotate_token),
        )
        .route(
            "/audit/projects/{project_id}",
            get(handler::audit::list_project_audit),
        )
        .route(
            "/audit/organizations/{organization_id}",
            get(handler::audit::list_organization_audit),
        );
    // The device grant's two person lanes: a device's code is approved or
    // refused by whoever is signed in, however they signed in.
    let person = person
        .route(
            "/auth/device/approve",
            post(handler::session::approve_device),
        )
        .route("/auth/device/deny", post(handler::session::deny_device));
    // ⚠ `route_layer`, not `layer`: a plain `layer` also wraps the
    // fallback, so every unmatched path answered 401 instead of 404.
    let person = person.route_layer(middleware::from_fn_with_state(
        state.clone(),
        person::require_person_token,
    ));

    // `/auth/device/*` are the CLI's sign-in, the device authorization grant,
    // reached through the console's `/cli` door. Pre-session like the
    // console's own start and exchange, and the poll carries no secret but
    // the device code: a device code is worth nothing until the person
    // approves it in the console.
    let service = Router::new()
        .route("/auth/config", get(handler::session::config))
        .route("/auth/start", post(handler::session::start))
        .route("/auth/exchange", post(handler::session::exchange))
        .route("/auth/device/start", post(handler::session::device_start))
        .route("/auth/device/poll", post(handler::session::device_poll))
        .route("/auth/refresh", post(handler::session::refresh))
        .route("/auth/logout", post(handler::session::logout))
        .route("/auth/logout-url", post(handler::session::logout_url));
    // The login form's pre-session lanes, behind the console's own sign-in
    // pages. Each carries a password, a code or a link, and none a person:
    // the person is what they establish. Off with the form.
    let service = if state.password.is_some() {
        service
            .route("/auth/password/sign-up", post(handler::password::sign_up))
            .route("/auth/password/sign-in", post(handler::password::sign_in))
            .route(
                "/auth/password/verify",
                post(handler::password::verify_email),
            )
            .route(
                "/auth/password/forgot",
                post(handler::password::forgot_password),
            )
            .route(
                "/auth/password/reset",
                post(handler::password::reset_password),
            )
    } else {
        service
    };
    let service = service
        .route("/invites/look", post(handler::invite::look_up_invite))
        .route("/tokens/validate", post(handler::tokens::validate_token));

    let internal = person.merge(service).layer(middleware::from_fn_with_state(
        secrets.clone(),
        require_service_secret,
    ));

    // `/me` is a person lane like the rest: the subject is the resolved
    // bearer, and its body may neither name nor describe one. The email and
    // name it reads are `auth.identities`, which only the code exchange, the
    // refresh and an open test door write, from what signed the person in.
    let customer = Router::new()
        .route("/me", post(handler::me::me))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            person::require_person_token,
        ))
        .layer(middleware::from_fn_with_state(
            secrets.clone(),
            require_service_secret,
        ));
    // ⚠ **The test door, and nothing else.** It writes an identity from a
    // body and opens a session for it, so anyone holding the service secret
    // is anyone. Mounted only under `ALLOW_TEST_SESSION`, which the binary
    // refuses in a pod, so no deployed auth serves it.
    let customer = if state.config.allow_test_session {
        customer.merge(
            Router::new()
                .route("/test/session", post(handler::me::test_session))
                .layer(middleware::from_fn_with_state(
                    secrets.clone(),
                    require_service_secret,
                )),
        )
    } else {
        customer
    };

    // ── /v1 — the public read API a `telmoni_` token opens ──────────────────
    // Two gates, both load-bearing: `require_token` (the customer's bearer, the
    // only thing that decides which organization is read) and the service
    // secret (the hop came through the BFF). Layers apply outermost-last, so the
    // secret is checked first and a request that skipped the BFF never spends a
    // database round trip. Built by folding `V1_LANES`, the same table the
    // OpenAPI document is generated from, split on `bearer` so a future lane
    // without one does not silently inherit `require_token`.
    let mount = |router: Router<Arc<AppState>>, lane: &handler::v1::V1Lane| {
        let path = lane
            .route
            .path
            .strip_prefix("/v1")
            .unwrap_or(lane.route.path);
        router.route(
            path,
            match lane.op {
                handler::v1::V1Op::Organization => get(handler::v1::get_organization),
                handler::v1::V1Op::Members => get(handler::v1::list_members),
            },
        )
    };
    let bearer_v1 = handler::v1::V1_LANES
        .iter()
        .filter(|lane| lane.route.bearer)
        .fold(Router::new(), mount)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            handler::v1::require_token,
        ));
    let open_v1 = handler::v1::V1_LANES
        .iter()
        .filter(|lane| !lane.route.bearer)
        .fold(Router::new(), mount);
    // A path under `/v1` that no lane serves is a problem document too, not
    // axum's bare 404; the fallback sits outside `require_token`, so it names
    // no organization and costs no database round trip.
    let public_v1 = bearer_v1
        .merge(open_v1)
        .fallback(handler::v1::not_found)
        .layer(middleware::from_fn_with_state(
            secrets,
            require_service_secret,
        ));

    Router::new()
        .merge(customer)
        .nest("/v1", public_v1)
        .nest("/internal", internal)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A cancelled handler future clears nothing, and axum cancels on any
    /// client disconnect. The orphan's `watch` sender went with the future, so
    /// the next caller must take the code over rather than join a dead channel.
    #[tokio::test]
    async fn a_cancelled_leader_is_replaced_by_the_next_caller() {
        let cache = ExchangeCache::new();
        let CacheDecision::Leader(tx) = cache.start_or_join("code_a") else {
            panic!("the first caller for a code must lead");
        };
        drop(tx); // the future was dropped mid-exchange: no insert, no remove

        assert!(
            matches!(cache.start_or_join("code_a"), CacheDecision::Leader(_)),
            "an orphaned exchange must be taken over, not joined",
        );
        assert_eq!(
            cache
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "the takeover left a second entry behind"
        );
    }

    /// A leader that is still working is joined, not replaced — and its entry
    /// is still swept after the TTL, so a sender that never drops cannot pin
    /// a code.
    #[tokio::test]
    async fn a_live_leader_is_joined_until_the_sweep_takes_it() {
        let cache = ExchangeCache::new();
        let CacheDecision::Leader(_tx) = cache.start_or_join("code_a") else {
            panic!("the first caller for a code must lead");
        };

        assert!(
            matches!(cache.start_or_join("code_a"), CacheDecision::Wait(_)),
            "a second caller must wait on a leader that is still working",
        );

        {
            let mut map = cache
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ExchangeCache::sweep(
                &mut map,
                std::time::Instant::now() + EXCHANGE_TTL + Duration::from_secs(1),
            );
            assert!(map.is_empty(), "the in-progress entry outlived the sweep");
        }

        assert!(
            matches!(cache.start_or_join("code_a"), CacheDecision::Leader(_)),
            "once swept the code must be exchangeable again",
        );
    }

    /// The two states share one TTL, so a change made for the in-progress half
    /// must not quietly stop expiring completed results.
    #[tokio::test]
    async fn a_completed_result_still_expires() {
        let cache = ExchangeCache::new();
        let now = std::time::Instant::now();
        {
            let mut map = cache
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.insert(
                "code_b".into(),
                ExchangeState::Completed(
                    Box::new(handler::session::AuthnResult {
                        user_id: "user_1".into(),
                        email: None,
                        email_verified: true,
                        first_name: None,
                        last_name: None,
                        access_token: String::new(),
                        session_id: String::new(),
                        refresh_token: None,
                        expires_in: 0,
                        id_token: None,
                        auth_method: None,
                    }),
                    now,
                ),
            );
            ExchangeCache::sweep(&mut map, now + EXCHANGE_TTL - Duration::from_secs(1));
            assert_eq!(map.len(), 1, "expired early");
            ExchangeCache::sweep(&mut map, now + EXCHANGE_TTL + Duration::from_secs(1));
            assert!(map.is_empty(), "a completed result outlived its TTL");
        }
    }
}
