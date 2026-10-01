//! Notifications configuration, loaded once from the environment.

use telmoni_shared::Redacted;
use telmoni_shared::config::{env_parse, optional, optional_base_url, require};

/// One vendor's OAuth application, as the operator registered it.
#[derive(Clone, Debug)]
pub struct OAuthApp {
    pub client_id: String,
    pub client_secret: Redacted,
}

/// Loaded-once notifications configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// Postgres URL (`NOTIFICATIONS_DATABASE_URL`) — the feed + delivery
    /// queue, opened as this module's own role.
    pub database_url: String,
    /// Primary service secret for `x-service-secret` (gates `/internal/*`).
    pub service_secret: Redacted,
    /// Optional rotation secondary, accepted during a rotation window.
    pub service_secret_next: Option<Redacted>,
    /// The console's public origin (`APP_URL`), which the OAuth redirect URIs
    /// are built from; a wrong value is a handshake that never comes back.
    pub app_url: String,
    /// The Slack app, or `None` when `SLACK_CLIENT_ID` / `SLACK_CLIENT_SECRET`
    pub slack: Option<OAuthApp>,
    /// What Slack signs its events with. Without it the events route refuses
    /// everything, or anyone could retire a connection by naming its workspace.
    pub slack_signing_secret: Option<Redacted>,
    /// Slack's API origin (`SLACK_API_BASE`, default `https://slack.com`).
    pub slack_api_base: String,
    /// The Discord application, or `None` when its credentials are unset.
    pub discord: Option<OAuthApp>,
    /// Discord's API origin (`DISCORD_API_BASE`, default `https://discord.com`).
    pub discord_api_base: String,
    /// The key that wraps every row's data key (`CONNECTOR_KEK`): a Cloud KMS
    /// key name, or `local:<64 hex>` for a laptop. `None` disables every
    /// connector: a grant this module cannot seal is one it must not accept.
    pub connector_kek: Option<Redacted>,
    /// Cloud KMS's origin (`KMS_API_BASE`); the tests point it at a mock.
    pub kms_api_base: String,
    /// The instance metadata server (`METADATA_API_BASE`), where the KMS
    /// bearer comes from.
    pub metadata_api_base: String,
    /// Delivery-loop poll cadence, seconds: the IDLE heartbeat of an empty
    /// queue, not the send rate.
    pub delivery_poll_secs: u64,
    /// Rows leased per batch (`NOTIFICATIONS_DELIVERY_BATCH`, default 200).
    pub delivery_batch: i64,
    /// Sends in flight at once, across every connection — the real throughput
    /// ceiling. Nothing paces per vendor; `Retry-After` is the only brake.
    pub delivery_concurrency: usize,
    /// Batches one tick will drain before waiting for the next. The cap keeps
    /// a flag thrown mid-drain effective within this many batches.
    pub delivery_drain_rounds: u32,
    /// Attempts per delivery before it is terminally `failed`
    pub delivery_max_attempts: i32,
    /// Delivery backoff base, seconds; the wait grows fourfold per attempt.
    pub delivery_backoff_secs: i64,
}

impl Config {
    /// Build the configuration from process environment variables.
    pub fn from_env() -> anyhow::Result<Self> {
        let config = Self {
            database_url: require("NOTIFICATIONS_DATABASE_URL")?,
            service_secret: require("SERVICE_SECRET")?.into(),
            service_secret_next: optional("SERVICE_SECRET_NEXT").map(Into::into),
            app_url: require("APP_URL")?.trim_end_matches('/').to_owned(),
            slack: oauth_app("SLACK_CLIENT_ID", "SLACK_CLIENT_SECRET")?,
            slack_signing_secret: optional("SLACK_SIGNING_SECRET").map(Into::into),
            slack_api_base: optional_base_url("SLACK_API_BASE")
                .unwrap_or_else(|| "https://slack.com".to_owned()),
            discord: oauth_app("DISCORD_CLIENT_ID", "DISCORD_CLIENT_SECRET")?,
            discord_api_base: optional_base_url("DISCORD_API_BASE")
                .unwrap_or_else(|| "https://discord.com".to_owned()),
            connector_kek: optional("CONNECTOR_KEK").map(Into::into),
            kms_api_base: optional_base_url("KMS_API_BASE")
                .unwrap_or_else(|| "https://cloudkms.googleapis.com".to_owned()),
            metadata_api_base: optional_base_url("METADATA_API_BASE")
                .unwrap_or_else(|| "http://metadata.google.internal".to_owned()),
            delivery_poll_secs: env_parse("NOTIFICATIONS_DELIVERY_POLL_SECS", 5)?,
            delivery_batch: env_parse("NOTIFICATIONS_DELIVERY_BATCH", 200)?,
            delivery_concurrency: env_parse("NOTIFICATIONS_DELIVERY_CONCURRENCY", 64)?,
            delivery_drain_rounds: env_parse("NOTIFICATIONS_DELIVERY_DRAIN_ROUNDS", 20)?,
            delivery_max_attempts: env_parse("NOTIFICATIONS_DELIVERY_MAX_ATTEMPTS", 5)?,
            delivery_backoff_secs: env_parse("NOTIFICATIONS_DELIVERY_BACKOFF_SECS", 30)?,
        };
        if config.delivery_backoff_secs < 0 {
            anyhow::bail!("NOTIFICATIONS_DELIVERY_BACKOFF_SECS must not be negative");
        }
        if config.delivery_max_attempts < 1 {
            anyhow::bail!("NOTIFICATIONS_DELIVERY_MAX_ATTEMPTS must be at least 1");
        }
        if config.delivery_poll_secs < 1 {
            anyhow::bail!("NOTIFICATIONS_DELIVERY_POLL_SECS must be at least 1");
        }
        if config.delivery_batch < 1 {
            anyhow::bail!("NOTIFICATIONS_DELIVERY_BATCH must be at least 1");
        }
        if config.delivery_concurrency < 1 {
            anyhow::bail!("NOTIFICATIONS_DELIVERY_CONCURRENCY must be at least 1");
        }
        if config.delivery_drain_rounds < 1 {
            anyhow::bail!("NOTIFICATIONS_DELIVERY_DRAIN_ROUNDS must be at least 1");
        }
        if (config.slack.is_some() || config.discord.is_some()) && config.connector_kek.is_none() {
            anyhow::bail!(
                "SLACK_* or DISCORD_* credentials are set but CONNECTOR_KEK is not; \
                 a connector cannot store a grant it cannot seal"
            );
        }
        if config.slack.is_some() && config.slack_signing_secret.is_none() {
            anyhow::bail!(
                "SLACK_CLIENT_ID is set but SLACK_SIGNING_SECRET is not; without it an \
                 uninstall from Slack's side can never retire a connection"
            );
        }
        Ok(config)
    }
}

/// Both halves of one vendor app, or neither.
fn oauth_app(id_var: &str, secret_var: &str) -> anyhow::Result<Option<OAuthApp>> {
    match (optional(id_var), optional(secret_var)) {
        (Some(client_id), Some(secret)) => Ok(Some(OAuthApp {
            client_id,
            client_secret: secret.into(),
        })),
        (None, None) => Ok(None),
        (Some(_), None) => anyhow::bail!("{id_var} is set but {secret_var} is not"),
        (None, Some(_)) => anyhow::bail!("{secret_var} is set but {id_var} is not"),
    }
}
