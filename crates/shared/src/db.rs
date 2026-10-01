//! sqlx Postgres pool factory and per-service connection helpers.

pub mod audit_hash;
pub mod audit_verify;
pub mod retention;
pub mod tenant_session;

use sqlx::{
    Executor as _, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
};

/// The env var carrying the database's own server CA, PEM.
pub const DATABASE_CA_ENV: &str = "DATABASE_CA_CERT";

/// Connection options for a DSN, pinned to the database's own CA when one is
/// given.
///
/// ⚠ **`verify-ca`, not `verify-full`.** A DSN's `sslmode=require` encrypts
/// and verifies nothing, so anything on the path that can answer TLS is
/// taken for the database. Cloud SQL signs each instance's certificate with a
/// CA of that instance's own and names the certificate after the instance,
/// not the private IP a service dials — so hostname verification would refuse
/// every connection, and checking the chain against that one CA is what pins
/// the database this stack created.
pub fn connect_options(
    database_url: &str,
    ca_pem: Option<&str>,
) -> Result<PgConnectOptions, sqlx::Error> {
    let options: PgConnectOptions = database_url.parse()?;
    Ok(match ca_pem.map(str::trim).filter(|pem| !pem.is_empty()) {
        Some(pem) => options
            .ssl_mode(PgSslMode::VerifyCa)
            .ssl_root_cert_from_pem(pem.as_bytes().to_vec()),
        None => options,
    })
}

/// [`DATABASE_CA_ENV`], refused when it is missing in a Kubernetes pod: a
/// deployed tier never connects to its database without checking whose
/// certificate answered. A laptop's docker Postgres has none to check.
pub fn database_ca_from_env() -> Result<Option<String>, String> {
    let ca = std::env::var(DATABASE_CA_ENV)
        .ok()
        .filter(|pem| !pem.trim().is_empty());
    if ca.is_none() && crate::envelope::on_deployed_tier() {
        return Err(format!(
            "{DATABASE_CA_ENV} is not set in a Kubernetes pod; the database's certificate \
             would be taken on trust"
        ));
    }
    Ok(ca)
}

/// A small pool bound to no schema, verifying the server against `ca_pem` when
/// given ([`connect_options`]). For a test's own connection, or a process
/// that holds no tables.
pub async fn create_pool(database_url: &str, ca_pem: Option<&str>) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(connect_options(database_url, ca_pem)?)
        .await
}

/// `max_connections` for [`create_pool_for_service`] — the per-service
/// application pool. Deliberately small: every runtime scales replicas
/// horizontally, so the per-instance pool multiplies straight into the
/// database's `max_connections`.
pub const SERVICE_POOL_MAX_CONNECTIONS: u32 = 5;

/// Application pool scoped to `schema`, verifying the server against `ca_pem`
/// when given ([`connect_options`]).
pub async fn create_pool_for_service(
    database_url: &str,
    schema: &str,
    ca_pem: Option<&str>,
) -> Result<PgPool, sqlx::Error> {
    if schema.is_empty()
        || !schema
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(sqlx::Error::Configuration(
            format!("schema name must be a simple identifier, got {schema:?}").into(),
        ));
    }

    let options = connect_options(database_url, ca_pem)?;

    // `SET search_path` takes no parameters, so the identifier check above is
    // the only thing between a config value and SQL injection.
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(SERVICE_POOL_MAX_CONNECTIONS)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(
                    format!(
                        "SET search_path = {schema}, public; \
                     SET application_name = '{schema}'"
                    )
                    .as_str(),
                )
                .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CA pins the server; without one, the DSN's own mode stands (a
    /// laptop's docker Postgres, which has no CA to pin).
    #[test]
    fn a_database_ca_turns_on_certificate_verification() {
        let dsn = "postgres://auth:pw@10.0.0.3/telmoni?sslmode=require";
        let pinned = connect_options(dsn, Some("-----BEGIN CERTIFICATE-----\nx\n")).unwrap();
        assert!(matches!(pinned.get_ssl_mode(), PgSslMode::VerifyCa));
        let unpinned = connect_options(dsn, None).unwrap();
        assert!(matches!(unpinned.get_ssl_mode(), PgSslMode::Require));
        let blank = connect_options(dsn, Some("  \n")).unwrap();
        assert!(matches!(blank.get_ssl_mode(), PgSslMode::Require));
    }

    /// The schema name is interpolated into `SET search_path`, so a bad one
    /// must be refused before any connection is attempted.
    #[tokio::test]
    async fn create_pool_for_service_rejects_non_identifier_schema_names() {
        let bad_names = [
            "",
            "auth; DROP TABLE projects",
            "a-b",
            "café",
            "public, pg_temp",
            "auth'--",
        ];
        for bad in bad_names {
            let result = create_pool_for_service("postgres://unused.invalid/none", bad, None).await;
            match result {
                Err(sqlx::Error::Configuration(msg)) => {
                    assert!(
                        msg.to_string().contains("simple identifier"),
                        "unexpected error message for {bad:?}: {msg}"
                    );
                }
                Err(other) => panic!("expected Configuration error for {bad:?}, got {other:?}"),
                Ok(_) => panic!("schema name {bad:?} must be rejected"),
            }
        }
    }
}
