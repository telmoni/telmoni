//! Telemetry configuration, loaded once from the environment.

use telmoni_shared::Redacted;
use telmoni_shared::config::require;

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
}

impl Config {
    /// Build the configuration from process environment variables.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_url: require("TELEMETRY_DATABASE_URL")?.into(),
            clickhouse_url: require("TELEMETRY_CLICKHOUSE_URL")?.into(),
        })
    }
}
