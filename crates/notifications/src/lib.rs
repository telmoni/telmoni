//! Notifications — the in-app feed, and the connectors that repeat it:
//! Slack, Discord, and a customer's own signed webhook — as a module of the
//! `telmoni` binary. It owns the `notifications` schema and connects as the
//! `notifications` role; it asks auth who is acting through
//! [`telmoni_shared::seam::Auth`], and answers auth's notices and purges
//! through [`seam::Notifier`].
//!
//! The feed write is synchronous; every send is durable at-least-once through
//! the [`delivery`] leased-row loop.
#![expect(
    clippy::missing_errors_doc,
    reason = "service-crate baseline: error conditions are documented on the handlers themselves"
)]

use std::sync::Arc;

use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};
use sqlx::PgPool;

use telmoni_shared::envelope::{Kek, Vault};
use telmoni_shared::middleware::service_auth::{ServiceSecrets, require_service_secret};
use telmoni_shared::net_guard::Egress;

pub mod boot;
pub mod config;
pub mod connector;
pub mod db;
pub mod delivery;
pub mod handler;
pub mod notify;
pub mod retention;
pub mod seam;

pub use config::Config;

/// Shared application state passed into every handler and the delivery loop.
pub struct AppState {
    /// Postgres pool — the feed + delivery queue.
    pub db: PgPool,
    /// Loaded-once configuration.
    pub config: Config,
    /// Constant-time-comparable service secrets gating `/internal/*`.
    pub service_secrets: ServiceSecrets,
    /// Auth, in process: who is behind a person lane's bearer and what role
    /// they hold on the project or organization named, and the flag set that
    /// gates the connectors.
    pub auth: Arc<dyn telmoni_shared::seam::Auth>,
    /// ⚠ The client for hops that stay on the platform — the metadata server,
    /// KMS — and deliberately UNGUARDED, because the metadata server is the
    /// link-local address the guard refuses. Nothing a customer or a vendor
    /// named is ever dialled through this one.
    pub http: reqwest::Client,
    /// The client for everything that leaves the platform, guarded on both
    /// halves, so a private or metadata host is never dialled whoever supplied it.
    pub egress: Egress,
    /// Seals and opens grants under a per-row data key; holds no key itself.
    pub vault: Vault,
    /// What wraps each row's data key, or `None` when `CONNECTOR_KEK` is
    /// unset — in which case no connector is enabled.
    pub kek: Option<Kek>,
    /// One connector per registered vendor app, plus the signed webhook.
    pub connectors: connector::Connectors,
}

impl AppState {
    /// The readiness probe: one read of the module's own table. ⚠ It must
    /// touch a table: the pool opens nothing until the first query, so a
    /// constant "ok" would let a revision take traffic without ever having
    /// reached Postgres.
    pub async fn ready(&self) -> sqlx::Result<()> {
        sqlx::query("SELECT 1 FROM notifications.feed LIMIT 1")
            .execute(&self.db)
            .await
            .map(|_| ())
    }
}

/// The module's router: the person lanes under `/internal`, and Slack's
/// events. The binary mounts it beside the other modules' and adds the
/// health probes and the request layers once.
pub fn router(state: Arc<AppState>) -> Router {
    let person = Router::new()
        .route("/notifications/feed", get(handler::get_feed))
        .route("/notifications/read", post(handler::mark_read))
        .route("/connectors", get(handler::connectors::list_connectors))
        .route(
            "/connectors/webhook",
            post(handler::connectors::create_webhook),
        )
        .route(
            "/connectors/{provider}/authorize",
            post(handler::connectors::authorize_connector),
        )
        .route(
            "/connectors/{provider}/callback",
            post(handler::connectors::callback),
        )
        .route(
            "/connectors/{id}",
            delete(handler::connectors::delete_connector),
        )
        .route(
            "/connectors/{id}/test",
            post(handler::connectors::test_connector),
        )
        .route(
            "/connectors/{id}/rotate",
            post(handler::connectors::rotate_webhook_secret),
        )
        .route(
            "/connectors/{id}/events",
            put(handler::connectors::update_webhook_events),
        )
        .route(
            "/connectors/{id}/deliveries",
            get(handler::connectors::list_deliveries),
        )
        .route(
            "/connectors/{id}/deliveries/{delivery_id}/redeliver",
            post(handler::connectors::redeliver),
        );

    let internal = person.layer(middleware::from_fn_with_state(
        state.service_secrets.clone(),
        require_service_secret,
    ));

    Router::new()
        .route("/webhooks/slack", post(handler::slack_events::slack_events))
        .nest("/internal", internal)
        .with_state(state)
}
