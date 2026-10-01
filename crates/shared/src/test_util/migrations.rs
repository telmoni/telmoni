//! Applying a sibling service's migrations inside a test's throwaway database.

use sqlx::{Executor as _, PgPool};

/// The audit schema's migrations (`audit.events` — the tamper-evident chain).
const AUDIT_MIGRATIONS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../migrator/migrations");

/// Apply each `(schema, migrations_dir)` pair on one connection, then restore
/// `search_path`. Each set runs with `search_path` on its own schema, because
/// the migrations create objects unqualified.
async fn apply_migrations(pool: &PgPool, sets: &[(&str, &str)]) {
    let mut conn = pool.acquire().await.unwrap();
    for (schema, dir) in sets {
        conn.execute(format!("CREATE SCHEMA IF NOT EXISTS {schema}").as_str())
            .await
            .unwrap();
        conn.execute(format!("SET search_path = {schema}, public").as_str())
            .await
            .unwrap();
        sqlx::migrate::Migrator::new(std::path::Path::new(dir))
            .await
            .unwrap()
            .run(&mut *conn)
            .await
            .unwrap();
    }
    conn.execute("RESET search_path").await.unwrap();
}

/// The common case: the audit schema alone, so a mutation's `emit_audit` row
/// has somewhere to land. Without it, "failed audit = failed operation" rolls
/// the mutation back rather than failing loudly.
pub async fn apply_audit_migrations(pool: &PgPool) {
    apply_migrations(pool, &[("audit", AUDIT_MIGRATIONS)]).await;
}
