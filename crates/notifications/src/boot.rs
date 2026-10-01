//! The module's state from the environment: its pool as its own role, the
//! key that seals every grant, the guarded client every vendor is dialled
//! through, and the connectors this deployment registered.

use std::sync::Arc;
use std::time::Duration;

use telmoni_shared::envelope::{Kek, Vault, on_deployed_tier};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::net_guard::Egress;
use telmoni_shared::seam::Auth;

use crate::{AppState, Config, connector::Connectors};

/// The module's pool, opened as its own database role.
pub async fn open_pool(config: &Config) -> anyhow::Result<sqlx::PgPool> {
    let db_ca = telmoni_shared::db::database_ca_from_env().map_err(anyhow::Error::msg)?;
    Ok(telmoni_shared::db::create_pool_for_service(
        &config.database_url,
        "notifications",
        db_ca.as_deref(),
    )
    .await?)
}

/// The state the binary runs the module on, with `auth` as the binary links
/// it. Refuses what a deployed tier must not run with.
pub fn state_from_env(
    config: Config,
    db: sqlx::PgPool,
    auth: Arc<dyn Auth>,
) -> anyhow::Result<AppState> {
    let service_secrets = ServiceSecrets::new(
        config.service_secret.clone(),
        config.service_secret_next.clone(),
    );

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("telmoni-notifications")
        .build()?;
    let egress = Egress::guarded(Duration::from_secs(10), "telmoni-notifications")?;

    let kek = config
        .connector_kek
        .as_ref()
        .map(|value| {
            Kek::parse(
                "CONNECTOR_KEK",
                value,
                &config.kms_api_base,
                &config.metadata_api_base,
            )
        })
        .transpose()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if kek.as_ref().is_some_and(Kek::is_local) && on_deployed_tier() {
        anyhow::bail!(
            "CONNECTOR_KEK is a local: key in a Kubernetes pod; a local key is for a laptop, \
             and a deployed tier names a Cloud KMS key"
        );
    }
    let connectors = Connectors::from_config(&config);
    tracing::info!(
        slack = connectors.slack.is_some(),
        discord = connectors.discord.is_some(),
        webhook = connectors.webhook.is_some(),
        kek = kek.as_ref().map_or("none", Kek::kind),
        "connectors configured"
    );

    Ok(AppState {
        db,
        config,
        service_secrets,
        auth,
        http,
        egress,
        vault: Vault::new(),
        kek,
        connectors,
    })
}
