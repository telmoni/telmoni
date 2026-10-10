//! The module's state from its configuration: its pool as its own role, and
//! its ClickHouse client as its own user.

use std::sync::Arc;

use anyhow::Context;
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::seam::Auth;

use crate::{AppState, Config, store::Store};

/// The module's pool, opened as its own database role.
pub async fn open_pool(config: &Config) -> anyhow::Result<sqlx::PgPool> {
    let db_ca = telmoni_shared::db::database_ca_from_env().map_err(anyhow::Error::msg)?;
    Ok(telmoni_shared::db::create_pool_for_service(
        config.database_url.expose(),
        "telemetry",
        db_ca.as_deref(),
    )
    .await?)
}

/// The state the binary runs the module on. Refuses a ClickHouse URL it
/// cannot use, without dialling it: ClickHouse may come up after the server.
pub fn state(
    config: Config,
    db: sqlx::PgPool,
    service_secrets: ServiceSecrets,
    auth: Arc<dyn Auth>,
) -> anyhow::Result<AppState> {
    let store = Store::connect(config.clickhouse_url.expose())
        .context("TELEMETRY_CLICKHOUSE_URL is not a ClickHouse URL")?;
    Ok(AppState {
        db,
        config,
        store,
        service_secrets,
        auth,
    })
}
