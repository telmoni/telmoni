//! The DB-skip helper the tenancy integration suites share.

use sqlx::PgPool;

/// Read `DATABASE_URL` or print a skip notice and return `None`.
#[must_use]
pub fn database_url_or_skip(test_name: &str) -> Option<String> {
    match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => Some(url),
        _ => {
            eprintln!(
                "skipping {test_name}: DATABASE_URL unset \
                 (set it to run tenancy integration tests; \
                 `make up` provisions a local Postgres)"
            );
            None
        }
    }
}

/// Probe whether any of the given schemas exist in the database.
pub async fn probe_schemas_migrated(pool: &PgPool, schemas: &[&str]) -> bool {
    let present: Vec<String> =
        sqlx::query_scalar("SELECT nspname FROM pg_namespace WHERE nspname = ANY($1)")
            .bind(schemas)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    !present.is_empty()
}
