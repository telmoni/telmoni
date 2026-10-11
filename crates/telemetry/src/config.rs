//! Telemetry configuration, loaded once from the environment.

use telmoni_shared::config::{optional, optional_base_url, require};
use telmoni_shared::{ProjectId, Redacted};

/// Loaded-once telemetry configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// The project of the platform's own the console agent's questions are
    /// recorded as runs of (`TELEMETRY_DOGFOOD_PROJECT_ID`); unset, nothing
    /// records them. A project id, as the console's Settings page shows it.
    pub dogfood_project_id: Option<ProjectId>,
    /// Postgres URL (`TELEMETRY_DATABASE_URL`) — each project's settings,
    /// opened as this module's own role. It carries the role's password.
    pub database_url: Redacted,
    /// ClickHouse's HTTP URL (`TELEMETRY_CLICKHOUSE_URL`), carrying the
    /// module's user and password as a DSN does: the spans and their rollup,
    /// read under the row policy. Required, though nothing here dials it at
    /// boot: a binary without ClickHouse is not one that can keep a span.
    pub clickhouse_url: Redacted,
    /// The key that seals each project's hashing key (`TELEMETRY_KEK`): a
    /// Cloud KMS key name, or `local:<64 hex>` for a laptop. Required: every
    /// binary ingests, and a `user_id` is hashed before it is a row.
    pub telemetry_kek: Redacted,
    /// Cloud KMS's origin (`KMS_API_BASE`); the tests point it at a mock.
    pub kms_api_base: String,
    /// The instance metadata server (`METADATA_API_BASE`), where the KMS
    /// bearer comes from.
    pub metadata_api_base: String,
    /// Redis (`REDIS_URL`), the console's own: ingest's claims on each body's
    /// digest and its rate limits, shared by every replica. Required, though
    /// nothing dials it at boot: a replica that cannot reach it stands in.
    pub redis_url: Redacted,
}

impl Config {
    /// Build the configuration from process environment variables.
    pub fn from_env() -> anyhow::Result<Self> {
        let dogfood_project_id = optional("TELEMETRY_DOGFOOD_PROJECT_ID")
            .map(|id| {
                ProjectId::try_new(&id).map_err(|e| {
                    anyhow::anyhow!("TELEMETRY_DOGFOOD_PROJECT_ID is not a project id: {e}")
                })
            })
            .transpose()?;
        Ok(Self {
            dogfood_project_id,
            database_url: require("TELEMETRY_DATABASE_URL")?.into(),
            clickhouse_url: require("TELEMETRY_CLICKHOUSE_URL")?.into(),
            telemetry_kek: require("TELEMETRY_KEK")?.into(),
            kms_api_base: optional_base_url("KMS_API_BASE")
                .unwrap_or_else(|| "https://cloudkms.googleapis.com".to_owned()),
            metadata_api_base: optional_base_url("METADATA_API_BASE")
                .unwrap_or_else(|| "http://metadata.google.internal".to_owned()),
            redis_url: require("REDIS_URL")?.into(),
        })
    }
}
