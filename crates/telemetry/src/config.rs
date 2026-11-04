//! Telemetry configuration, loaded once from the environment.

use telmoni_shared::Redacted;
use telmoni_shared::config::{optional_base_url, require};

/// Loaded-once telemetry configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// Postgres URL (`TELEMETRY_DATABASE_URL`) — each project's settings,
    /// opened as this module's own role. It carries the role's password.
    pub database_url: Redacted,
    /// ClickHouse's HTTP URL (`TELEMETRY_CLICKHOUSE_URL`), carrying the
    /// module's user and password as a DSN does: the spans and their rollup,
    /// read under the row policy. Required, though nothing here dials it at
    /// boot: a binary without ClickHouse is not one that can keep a span.
    pub clickhouse_url: Redacted,
    /// The key that seals each project's hashing key (`TELEMETRY_KEK`): a
    /// Cloud KMS key name, `file:<path>` mounted in a pod, or `local:<64 hex>`
    /// for a laptop. Required: every
    /// binary ingests, and a `user_id` is hashed before it is a row.
    pub telemetry_kek: Redacted,
    /// The key that seals each project's content key (`CONTENT_KEK`), spelt
    /// as `TELEMETRY_KEK` is and never the same key: deleting a project's
    /// content key makes its content unreadable, but in a backup taken before. Required,
    /// since every binary ingests.
    pub content_kek: Redacted,
    /// The key that seals each model connection's key (`MODEL_CONNECTION_KEK`),
    /// spelt as the others are and never the same key; unset, no project
    /// connects a model.
    pub model_connection_kek: Option<Redacted>,
    /// What signs each project's tokens for a connection that stores no key
    /// (`MODEL_FEDERATION_SIGNER`): a KMS key version, or `local:` on a
    /// laptop; unset, a connection signs in to nothing.
    pub model_federation_signer: Option<Redacted>,
    /// The machine host's origin, where each project's issuer and the key set
    /// are reached (`MODEL_FEDERATION_ISSUER`), https on a tier; set with the
    /// signer, or neither.
    pub model_federation_issuer: Option<String>,
    /// Cloud KMS's origin (`KMS_API_BASE`); the tests point it at a mock.
    pub kms_api_base: String,
    /// The instance metadata server (`METADATA_API_BASE`), where the KMS
    /// bearer comes from.
    pub metadata_api_base: String,
    /// Redis (`REDIS_URL`), the console's own: ingest's claims on each body's
    /// digest and its rate limits, shared by every replica. Required, though
    /// nothing dials it at boot: a replica that cannot reach it stands in.
    pub redis_url: Redacted,
    /// The console's origin (`APP_URL`), which the links an alert's notices
    /// carry open; none, and a notice carries none.
    pub app_url: Option<String>,
}

impl Config {
    /// Build the configuration from process environment variables.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_url: require("TELEMETRY_DATABASE_URL")?.into(),
            clickhouse_url: require("TELEMETRY_CLICKHOUSE_URL")?.into(),
            telemetry_kek: require("TELEMETRY_KEK")?.into(),
            content_kek: require("CONTENT_KEK")?.into(),
            model_connection_kek: telmoni_shared::config::optional("MODEL_CONNECTION_KEK")
                .map(Into::into),
            model_federation_signer: telmoni_shared::config::optional("MODEL_FEDERATION_SIGNER")
                .map(Into::into),
            model_federation_issuer: telmoni_shared::config::optional("MODEL_FEDERATION_ISSUER")
                .map(|url| {
                    telmoni_shared::config::https_on_a_tier("MODEL_FEDERATION_ISSUER", url)
                        .map(|url| url.trim_end_matches('/').to_owned())
                })
                .transpose()?,
            kms_api_base: optional_base_url("KMS_API_BASE")
                .unwrap_or_else(|| "https://cloudkms.googleapis.com".to_owned()),
            metadata_api_base: optional_base_url("METADATA_API_BASE")
                .unwrap_or_else(|| "http://metadata.google.internal".to_owned()),
            redis_url: require("REDIS_URL")?.into(),
            app_url: Some(telmoni_shared::config::app_url()?),
        })
    }
}
