//! The console agent — a module of the `telmoni` binary that answers a
//! person's questions about their project, read-only, citing what it used.
//!
//! It owns the `agent` schema and connects as the `agent` role. Retrieval
//! is hybrid search in Postgres (pgvector for meaning, a `tsvector` for
//! exact terms, fused by reciprocal rank) over passages it indexes from the
//! docs and, through the seams, from auth's audit log and notifications'
//! feed and deliveries. Every search runs under the asker's own tenancy,
//! and every tool as the asker, so the model sees only what the console
//! would show them.
#![expect(
    clippy::missing_errors_doc,
    reason = "service-crate baseline: error conditions are documented on the handlers themselves"
)]

pub mod boot;
pub mod config;
pub mod db;
pub mod embed;
pub mod handler;
pub mod index;
pub mod model;
pub mod prompt;
pub mod rerank;
pub mod retention;
pub mod retrieve;
pub mod seam;
pub mod tools;
pub mod turn;

use std::sync::Arc;

use axum::{
    Router, middleware,
    routing::{get, post},
};
use sqlx::PgPool;

use telmoni_shared::TelmoniError;
use telmoni_shared::middleware::service_auth::{ServiceSecrets, require_service_secret};
use telmoni_shared::seam::{Auth, Notifications};

pub use config::Config;

/// Shared state for every handler and loop.
pub struct AppState {
    pub db: PgPool,
    pub config: Config,
    pub service_secrets: ServiceSecrets,
    /// Who is asking, and the audit log and roster the tools read.
    pub auth: Arc<dyn Auth>,
    /// The feed, the connectors and their deliveries the tools read.
    pub notifications: Arc<dyn Notifications>,
    /// `None` while the agent is off.
    pub model: Option<Arc<dyn model::Model>>,
    /// `None` while the agent is off.
    pub embedder: Option<Arc<dyn embed::Embedder>>,
    pub reranker: Option<Arc<rerank::Reranker>>,
}

impl AppState {
    /// The readiness probe: one read of the module's own table.
    pub async fn ready(&self) -> sqlx::Result<()> {
        sqlx::query("SELECT 1 FROM agent.cursors LIMIT 1")
            .execute(&self.db)
            .await
            .map(|_| ())
    }

    /// The embedder, or the agent's refusal when it is off.
    pub fn embedder(&self) -> Result<&dyn embed::Embedder, TelmoniError> {
        self.embedder.as_deref().ok_or(TelmoniError::AgentDisabled)
    }

    /// The loops `serve` runs while the agent is on: the indexer (the docs
    /// refresh is part of it) and the retention sweep. With it off only the
    /// sweep runs, so what an earlier configuration indexed still ages out.
    pub fn spawn(self: &Arc<Self>) {
        tokio::spawn(retention::run(self.clone()));
        if self.config.enabled() {
            tokio::spawn(index::run(self.clone()));
        }
    }
}

/// The module's lanes, under `/internal` behind the service secret.
pub fn router(state: Arc<AppState>) -> Router {
    let lanes = Router::new()
        .route("/agent/status", get(handler::status))
        .route("/agent/turns", post(handler::post_turn))
        .route("/agent/conversations", get(handler::list_conversations))
        .route(
            "/agent/conversations/{id}",
            get(handler::get_conversation).delete(handler::delete_conversation),
        )
        .layer(middleware::from_fn_with_state(
            state.service_secrets.clone(),
            require_service_secret,
        ));
    Router::new().nest("/internal", lanes).with_state(state)
}

/// The same lanes for a deployment with no agent at all: the status says
/// so, and everything else answers `agent-disabled`.
pub fn absent_router(service_secrets: ServiceSecrets) -> Router {
    let lanes = Router::new()
        .route("/agent/status", get(handler::absent_status))
        .route("/agent/turns", post(handler::absent))
        .route("/agent/conversations", get(handler::absent))
        .route(
            "/agent/conversations/{id}",
            get(handler::absent).delete(handler::absent),
        )
        .layer(middleware::from_fn_with_state(
            service_secrets,
            require_service_secret,
        ));
    Router::new().nest("/internal", lanes)
}
