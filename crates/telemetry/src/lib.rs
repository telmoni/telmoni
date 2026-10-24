//! Telemetry — what an agent's runs leave behind — as a module of the
//! `telmoni` binary. It owns two stores, each for what it is good at:
//!
//! - **Postgres**, the `telemetry` schema, connected as the `telemetry` role:
//!   what must be transactional or private — each project's settings, its
//!   content mode among them, and its saved views and layouts ([`db`]).
//! - **ClickHouse**, the `telemetry` database, connected as the `telemetry`
//!   user: the spans, a row per finished span of every kind, and their
//!   hourly rollup — metadata only, never a content field in any mode. Every
//!   read and write goes through [`store`], the one query module, which names
//!   on each read the projects its row policy may admit.
//!
//! Its ClickHouse tables are made by [`schema`] (`telmoni migrate`); expired
//! days and months are dropped, and deleted projects' rows rewritten away, by
//! [`retention`] (`telmoni rotate`), both as the migrator's user, whom
//! `tenant_isolation` does not hold. Auth reaches it through
//! [`telmoni_shared::seam::Telemetry`] ([`seam`]) to purge an organization's
//! or a project's data and move a project's settings; the console through its
//! lanes under `/internal/v1/telemetry` ([`handler`], [`router`]). The
//! [`sweep`] finishes a purge auth's call could not.
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
    routing::{get, post, put},
};
use sqlx::PgPool;

use telmoni_shared::ApiKeyScope;
use telmoni_shared::middleware::api_key::{ApiKeyGate, require_api_key};
use telmoni_shared::middleware::service_auth::{ServiceSecrets, require_service_secret};

pub mod boot;
pub mod ceilings;
pub mod config;
pub mod contract;
pub mod db;
pub mod handler;
pub mod hashing;
pub mod ingest;
pub mod live;
pub mod rate_limit;
pub mod replica;
pub mod retention;
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
    /// Postgres pool, as the `telemetry` role: each project's settings and
    /// its saved views and layouts.
    pub db: PgPool,
    /// Loaded-once configuration.
    pub config: Config,
    /// ClickHouse, as the `telemetry` user: the spans and their rollup.
    pub store: store::Store,
    /// Constant-time-comparable service secrets gating `/internal/v1/*`.
    pub service_secrets: ServiceSecrets,
    /// Auth, in process: who is behind a lane's bearer, and what role they
    /// hold on the project it names.
    pub auth: Arc<dyn telmoni_shared::seam::Auth>,
    /// What each organization may take in, read at ingest's door: a binary
    /// that sells nothing hands it no limits.
    pub entitlements: Arc<dyn telmoni_shared::seam::Entitlements>,
    /// Notifications, in process: where the owners hear of an organization
    /// held to its plan's ceilings ([`ceilings`]).
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
    /// The machine lanes' ceilings each organization's plan sets, and the
    /// day's counts held to them ([`ceilings`]).
    pub ceilings: ceilings::Ceilings,
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

/// The module's router: the console's lanes under `/internal/v1`, behind the
/// service secret; ingest the binary nests behind the machine door
/// ([`otlp_router`]).
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
            "/telemetry/session-layout",
            get(handler::session_layout::get_session_layout)
                .put(handler::session_layout::put_session_layout),
        )
        .route(
            "/telemetry/views",
            get(handler::views::list_views).post(handler::views::create_view),
        )
        .route(
            "/telemetry/views/{id}",
            put(handler::views::update_view).delete(handler::views::delete_view),
        )
        .layer(middleware::from_fn_with_state(
            state.service_secrets.clone(),
            require_service_secret,
        ));
    Router::new()
        .nest("/internal/v1", internal)
        .with_state(state)
}

/// Ingest, for the binary to nest at `/otlp` behind the machine door:
/// `POST /v1/traces` behind a project API key holding the ingest scope and
/// nothing else — a machine reaches it straight, and the key is its gate,
/// refusing as the lane answers, in OTLP's shape, once the body is read out
/// ([`handler::ingest::refuse_unread`]). The lane's own limit, per project,
/// runs in its handler once the key is known, so the door meters it by
/// address alone. A stray path or method is answered in the same shape, and
/// as late, outside the gate, costing no key lookup.
pub fn otlp_router(state: Arc<AppState>) -> Router {
    let gate = ApiKeyGate {
        auth: state.auth.clone(),
        scope: ApiKeyScope::Ingest,
        refuse: |error, request| Box::pin(handler::ingest::refuse_unread(error, request)),
    };
    Router::new()
        .route("/v1/traces", post(handler::ingest::export))
        .route_layer(middleware::from_fn_with_state(gate, require_api_key))
        .method_not_allowed_fallback(handler::ingest::method_not_allowed)
        .fallback(handler::ingest::not_found)
        .with_state(state)
}
