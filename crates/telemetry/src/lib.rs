//! Telemetry — what an agent's runs leave behind — as a module of the
//! `telmoni` binary. It owns two stores, each for what it is good at:
//!
//! - **Postgres**, the `telemetry` schema, connected as the `telemetry` role:
//!   what must be transactional or private — each project's settings, its
//!   content mode among them ([`db`]).
//! - **ClickHouse**, the `telemetry` database, connected as the `telemetry`
//!   user: the spans, a row per finished span of every kind, and their
//!   hourly rollup — metadata only, never a content field in any mode. Every
//!   read and write goes through [`store`], the one query module, which names
//!   on each read the projects its row policy may admit.
//!
//! Its ClickHouse tables are made by [`schema`], which `telmoni migrate` runs
//! after the Postgres sets, and their expired days and months are dropped by
//! [`retention`], which `telmoni rotate` runs: both as the migrator's user,
//! whom `tenant_isolation` does not hold. Auth reaches it through
//! [`telmoni_shared::seam::Telemetry`] ([`seam`]) to purge an organization's
//! or a project's settings and to move a project's, and the console through
//! its lanes under `/internal/telemetry` ([`handler`], [`router`]), which ask
//! auth who is acting through [`telmoni_shared::seam::Auth`].
//!
//! ⚠ **Its readiness reads Postgres alone.** The server is one process for
//! every module: a ClickHouse outage must leave sign-in, and every console
//! page that does not read spans, answering.
#![expect(
    clippy::missing_errors_doc,
    reason = "service-crate baseline: error conditions are documented on the callers"
)]

use std::sync::Arc;

use axum::{Router, middleware, routing::get};
use sqlx::PgPool;

use telmoni_shared::middleware::service_auth::{ServiceSecrets, require_service_secret};

pub mod boot;
pub mod config;
pub mod db;
pub mod handler;
pub mod retention;
pub mod schema;
pub mod seam;
pub mod store;

pub use config::Config;

/// Shared module state, handed to the seam and to every handler.
pub struct AppState {
    /// Postgres pool, as the `telemetry` role: each project's settings.
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
/// service secret. The binary mounts it beside the other modules' and adds
/// the health probes and the request layers once.
pub fn router(state: Arc<AppState>) -> Router {
    let internal = Router::new()
        .route(
            "/telemetry/content-mode",
            get(handler::content_mode::get_content_mode)
                .put(handler::content_mode::put_content_mode),
        )
        .layer(middleware::from_fn_with_state(
            state.service_secrets.clone(),
            require_service_secret,
        ));
    Router::new().nest("/internal", internal).with_state(state)
}
