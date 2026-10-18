//! Telemetry — what an agent's runs leave behind — as a module of the
//! `telmoni` binary. It owns two stores, each for what it is good at:
//!
//! - **Postgres**, the `telemetry` schema, connected as the `telemetry` role:
//!   what must be transactional or private — each project's settings, its
//!   content mode among them, its saved views, its monitors and ping keys,
//!   its runs still going, and the outbox of what it hands on ([`db`]).
//! - **ClickHouse**, the `telemetry` database, connected as the `telemetry`
//!   user: the spans, a row per finished span of every kind, and their
//!   hourly rollup — metadata only, never a content field in any mode. Every
//!   read and write goes through [`store`], the one query module, which names
//!   on each read the projects its row policy may admit.
//!
//! Its ClickHouse tables are made by [`schema`], which `telmoni migrate` runs
//! after the Postgres sets, and their expired days and months are dropped,
//! and the rows deleted projects left rewritten away, by [`retention`], which
//! `telmoni rotate` runs: both as the migrator's user, whom `tenant_isolation`
//! does not hold. Auth reaches it through [`telmoni_shared::seam::Telemetry`]
//! ([`seam`]) to purge an organization's or a project's spans, totals and
//! settings and to move a project's settings, and the console through
//! its lanes under `/internal/telemetry` ([`handler`], [`router`]), which ask
//! auth who is acting through [`telmoni_shared::seam::Auth`]. A crontab
//! reaches the ping lane, `/ping/{ping_key}/{slug}` ([`ping_router`]), with
//! the project's ping key in the URL and nothing else; what a ping ends
//! reaches ClickHouse, and what it discovers reaches the project's feed,
//! through the [`outbox`], which a loop every replica runs drains, and the
//! keys' [`rotation`] retires a replaced key a week after its last ping; a
//! third loop, the [`sweep`], finishes a deleted project's purge that auth's
//! call could not.
//!
//! ⚠ **Its readiness reads Postgres alone.** The server is one process for
//! every module: a ClickHouse outage must leave sign-in, and every console
//! page that does not read spans, answering.
#![expect(
    clippy::missing_errors_doc,
    reason = "service-crate baseline: error conditions are documented on the callers"
)]

use std::sync::Arc;

use axum::{
    Router, middleware,
    routing::{MethodFilter, delete, get, on, post, put},
};
use sqlx::PgPool;

use telmoni_shared::ApiKeyScope;
use telmoni_shared::middleware::api_key::{ApiKeyGate, require_api_key};
use telmoni_shared::middleware::service_auth::{ServiceSecrets, require_service_secret};

pub mod boot;
pub mod config;
pub mod contract;
pub mod db;
pub mod handler;
pub mod hashing;
pub mod ingest;
pub mod live;
pub mod notify;
pub mod outbox;
pub mod rate_limit;
pub mod replica;
pub mod retention;
pub mod rotation;
pub mod schedule;
pub mod schema;
pub mod seam;
pub mod sessions;
pub mod store;
pub mod sweep;
pub mod traces;
pub mod users;

pub use config::Config;

/// Shared module state, handed to the seam and to every handler.
pub struct AppState {
    /// Postgres pool, as the `telemetry` role: each project's settings, its
    /// saved views, its monitors and ping keys, its runs still going, and the
    /// outbox of what it hands on.
    pub db: PgPool,
    /// Loaded-once configuration.
    pub config: Config,
    /// ClickHouse, as the `telemetry` user: the spans and their rollup.
    pub store: store::Store,
    /// Constant-time-comparable service secrets gating `/internal/*`.
    pub service_secrets: ServiceSecrets,
    /// Auth, in process: who is behind a lane's bearer, and what role they
    /// hold on the project it names.
    pub auth: Arc<dyn telmoni_shared::seam::Auth>,
    /// What each organization may take in, read at ingest's door: a binary
    /// that sells nothing hands it no limits.
    pub entitlements: Arc<dyn telmoni_shared::seam::Entitlements>,
    /// Notifications, in process: where a notice of this module's goes once
    /// the outbox hands it over — a monitor a first ping made, for now.
    pub notifications: Arc<dyn telmoni_shared::seam::Notifications>,
    /// The client Cloud KMS is called through, where `TELEMETRY_KEK` names a
    /// key there.
    pub http: reqwest::Client,
    /// Seals and opens each project's hashing key under a data key of the
    /// row's own; holds no key itself.
    pub vault: telmoni_shared::envelope::Vault,
    /// What wraps each row's data key: `TELEMETRY_KEK`.
    pub kek: telmoni_shared::envelope::Kek,
    /// Each project's hashing key, opened once per process ([`hashing`]).
    pub hashing_keys: hashing::Cache,
    /// Redis, as this replica holds it ([`replica`]): ingest's claims and
    /// limits, shared with every other replica.
    pub redis: replica::Redis,
    /// Ingest's rate limit, per project.
    pub rate_limit: rate_limit::RateLimit,
    /// How long a ping waits out a move or a purge of its project
    /// (`handler::pings::MOVE_WAIT`); a field so a suite can wait longer.
    pub move_wait: std::time::Duration,
    /// How long an export's body may take to arrive
    /// (`handler::ingest::BODY_WAIT`); a field so a suite can wait less.
    pub body_wait: std::time::Duration,
}

impl AppState {
    /// The readiness probe: one read of the module's own Postgres table. ⚠ A
    /// TABLE, not `SELECT 1`, which answers while the role lacks its grant;
    /// keyed on the primary key, as auth's is, so it reads one index entry
    /// however the policies are written. It must not touch ClickHouse, whose
    /// outage is ingest's to answer.
    pub async fn ready(&self) -> sqlx::Result<()> {
        sqlx::query("SELECT 1 FROM telemetry.project_settings WHERE project_id = ''")
            .execute(&self.db)
            .await
            .map(|_| ())
    }
}

/// The module's router: the console's lanes under `/internal`, behind the
/// service secret, and ingest at `/otlp/v1/traces`, behind a project API key
/// holding the ingest scope and nothing else — a machine reaches it straight,
/// and the key is its gate. The binary mounts it beside the other modules'
/// and adds the health probes and the request layers once.
pub fn router(state: Arc<AppState>) -> Router {
    let internal = Router::new()
        .route(
            "/telemetry/content-mode",
            get(handler::content_mode::get_content_mode)
                .put(handler::content_mode::put_content_mode),
        )
        .route(
            "/telemetry/field-inventory",
            get(handler::field_inventory::get_field_inventory),
        )
        .route("/telemetry/traces", get(handler::traces::list_traces))
        .route("/telemetry/traces/facets", get(handler::traces::get_facets))
        .route(
            "/telemetry/traces/histogram",
            get(handler::traces::get_histogram),
        )
        .route(
            "/telemetry/traces/export",
            get(handler::traces::export_traces),
        )
        .route(
            "/telemetry/traces/{run_id}",
            get(handler::traces::get_trace),
        )
        .route("/telemetry/users", get(handler::users::list_users))
        .route("/telemetry/sessions", get(handler::sessions::list_sessions))
        .route(
            "/telemetry/sessions/{session_id}",
            get(handler::sessions::get_session),
        )
        .route(
            "/telemetry/views",
            get(handler::views::list_views).post(handler::views::create_view),
        )
        .route(
            "/telemetry/views/{id}",
            put(handler::views::update_view).delete(handler::views::delete_view),
        )
        .route(
            "/telemetry/monitors",
            get(handler::monitors::list_monitors).post(handler::monitors::create_monitor),
        )
        .route(
            "/telemetry/monitors/schedule",
            post(handler::monitors::preview_schedule),
        )
        .route(
            "/telemetry/monitors/{slug}",
            get(handler::monitors::get_monitor)
                .put(handler::monitors::update_monitor)
                .delete(handler::monitors::delete_monitor),
        )
        .route(
            "/telemetry/monitors/{slug}/pause",
            post(handler::monitors::pause_monitor),
        )
        .route(
            "/telemetry/monitors/{slug}/resume",
            post(handler::monitors::resume_monitor),
        )
        .route(
            "/telemetry/ping-keys",
            get(handler::ping_keys::list_ping_keys).post(handler::ping_keys::create_ping_key),
        )
        .route(
            "/telemetry/ping-keys/{id}",
            delete(handler::ping_keys::delete_ping_key),
        )
        .layer(middleware::from_fn_with_state(
            state.service_secrets.clone(),
            require_service_secret,
        ));
    let gate = ApiKeyGate {
        auth: state.auth.clone(),
        scope: ApiKeyScope::Ingest,
        refuse: handler::ingest::refuse,
    };
    let otlp = Router::new()
        .route("/otlp/v1/traces", post(handler::ingest::export))
        .route_layer(middleware::from_fn_with_state(gate, require_api_key));
    Router::new()
        .nest("/internal", internal)
        .merge(otlp)
        .with_state(state)
}

/// The ping lane, for the binary to nest at `/ping` behind the machine
/// door: `/{ping_key}/{slug}` and its `/start`, `/checkpoint` and `/fail`,
/// `GET` or `POST`, the key in the path its one gate
/// ([`handler::pings`]). A path no lane serves, or a method none takes, is
/// answered as the other machine lanes answer one; and no answer, those
/// included, is one a cache may keep ([`handler::pings::never_stored`]).
pub fn ping_router(state: Arc<AppState>) -> Router {
    // `GET`'s handler answers `HEAD` too, as axum serves a `GET` route.
    let lane = MethodFilter::GET.or(MethodFilter::POST);
    Router::new()
        .route("/{ping_key}/{slug}", on(lane, handler::pings::success))
        .route("/{ping_key}/{slug}/start", on(lane, handler::pings::start))
        .route(
            "/{ping_key}/{slug}/checkpoint",
            on(lane, handler::pings::checkpoint),
        )
        .route("/{ping_key}/{slug}/fail", on(lane, handler::pings::fail))
        .method_not_allowed_fallback(handler::pings::method_not_allowed)
        .fallback(handler::pings::not_found)
        .layer(middleware::map_response(handler::pings::never_stored))
        .with_state(state)
}
