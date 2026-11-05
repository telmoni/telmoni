//! The module's state from its configuration: its pool as its own role, its
//! ClickHouse client as its own user, and the key its projects' hashing keys
//! are sealed under.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use telmoni_shared::envelope::{Kek, Vault};
use telmoni_shared::heartbeat::Heartbeats;
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::seam::{Auth, Entitlements, Notifications};

use crate::contract::EXPORTS_PER_MINUTE;
use crate::db::TelemetryLane;
use crate::handler::ingest::{BODY_WAIT, IN_FLIGHT_MAX};
use crate::rate_limit::RateLimit;
use crate::replica::Redis;
use crate::{AppState, Config, hashing, store::Store};

/// The longest one call to the key service may take, its token included.
const KEY_SERVICE_WAIT: Duration = Duration::from_secs(15);

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
/// — and a `TELEMETRY_KEK` or a `CONTENT_KEK` it cannot use, a laptop's key
/// on a tier and one key named for both among them.
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
    let key = |var: &str, value: &telmoni_shared::Redacted| {
        Kek::for_process(var, value, &config.kms_api_base, &config.metadata_api_base)
            .map_err(|e| anyhow::anyhow!("{e}"))
    };
    one_key_a_purpose(&config)?;
    let kek = key("TELEMETRY_KEK", &config.telemetry_kek)?;
    let content_kek = key("CONTENT_KEK", &config.content_kek)?;
    // Bounded, so an export that keys its project ends inside an
    // organization's deletion grace.
    let http = telmoni_shared::http::client("telmoni-telemetry", KEY_SERVICE_WAIT)?;
    let redis =
        Redis::connect(config.redis_url.expose()).context("REDIS_URL is not a Redis URL")?;
    tracing::info!(
        kek = kek.kind(),
        content_kek = content_kek.kind(),
        "telemetry: hashing and content keys are sealed"
    );
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
        content_kek,
        hashing_keys: hashing::Cache::default(),
        content_keys: hashing::Cache::default(),
        settling: std::sync::Mutex::default(),
        redis,
        rate_limit: RateLimit::new("telemetry:ingest:limit", EXPORTS_PER_MINUTE),
        ceilings: crate::ceilings::Ceilings::default(),
        capacities: crate::capacity::Capacities::default(),
        heartbeats: Heartbeats::new("telemetry", TelemetryLane),
        alerts_held: crate::rules::evaluate::Held::default(),
        alerts_stopping: crate::rules::evaluate::Stopping::default(),
        body_wait: BODY_WAIT,
        ingest_slots: Arc::new(tokio::sync::Semaphore::new(IN_FLIGHT_MAX)),
    })
}

/// Refuses one key named for both purposes, so that disabling or deleting one
/// leaves the other's rows readable.
fn one_key_a_purpose(config: &Config) -> anyhow::Result<()> {
    if config.telemetry_kek.expose().trim() == config.content_kek.expose().trim() {
        anyhow::bail!("TELEMETRY_KEK and CONTENT_KEK name one key; each takes a key of its own");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(telemetry_kek: &str, content_kek: &str) -> Config {
        Config {
            database_url: "".into(),
            clickhouse_url: "".into(),
            telemetry_kek: telemetry_kek.into(),
            content_kek: content_kek.into(),
            kms_api_base: String::new(),
            metadata_api_base: String::new(),
            redis_url: "".into(),
            app_url: None,
        }
    }

    /// ⚠ One key for both, a laptop's or a KMS name, is refused naming the two
    /// variables and neither value.
    #[test]
    fn one_key_named_for_both_purposes_is_refused() {
        let local = format!("local:{}", "0".repeat(64));
        let kms = "projects/p/locations/global/keyRings/r/cryptoKeys/k";
        for same in [local.as_str(), kms] {
            let refused = one_key_a_purpose(&config(same, &format!(" {same}\n")))
                .expect_err("one key for both")
                .to_string();
            assert!(
                refused.contains("TELEMETRY_KEK") && refused.contains("CONTENT_KEK"),
                "{refused}"
            );
            assert!(!refused.contains(same), "a key's value in the refusal");
        }
        one_key_a_purpose(&config(&local, &format!("local:{}", "1".repeat(64)))).expect("two keys");
    }
}
