//! The module's state from its configuration: its pool as its own role, its
//! ClickHouse client as its own user, and the key its projects' hashing keys
//! are sealed under.

use std::sync::Arc;

use anyhow::Context;
use telmoni_shared::envelope::{Kek, Vault, on_deployed_tier};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::seam::{Auth, Entitlements, Notifications};

use crate::contract::EXPORTS_PER_MINUTE;
use crate::rate_limit::RateLimit;
use crate::replica::Redis;
use crate::{AppState, Config, hashing, store::Store};

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
/// cannot use, without dialling it — ClickHouse may come up after the server
/// — and a `TELEMETRY_KEK` it cannot use, a laptop's key on a tier among them.
pub fn state(
    config: Config,
    db: sqlx::PgPool,
    service_secrets: ServiceSecrets,
    auth: Arc<dyn Auth>,
    entitlements: Arc<dyn Entitlements>,
    notifications: Arc<dyn Notifications>,
) -> anyhow::Result<AppState> {
    let store = Store::connect(config.clickhouse_url.expose())
        .context("TELEMETRY_CLICKHOUSE_URL is not a ClickHouse URL")?;
    let kek = Kek::parse(
        "TELEMETRY_KEK",
        &config.telemetry_kek,
        &config.kms_api_base,
        &config.metadata_api_base,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    if kek.is_local() && on_deployed_tier() {
        anyhow::bail!(
            "TELEMETRY_KEK is a local: key in a Kubernetes pod; a local key is for a laptop, \
             and a deployed tier names a Cloud KMS key"
        );
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("telmoni-telemetry")
        .build()?;
    let redis =
        Redis::connect(config.redis_url.expose()).context("REDIS_URL is not a Redis URL")?;
    tracing::info!(kek = kek.kind(), "telemetry: hashing keys are sealed");
    Ok(AppState {
        db,
        config,
        store,
        service_secrets,
        auth,
        entitlements,
        notifications,
        http,
        vault: Vault::new(),
        kek,
        hashing_keys: hashing::Cache::default(),
        redis,
        rate_limit: RateLimit::new("telemetry:ingest:limit", EXPORTS_PER_MINUTE),
    })
}
