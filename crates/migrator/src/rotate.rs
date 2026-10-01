//! Partition rotation — `telmoni rotate`.

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::postgres::PgPoolOptions;

use telmoni_shared::db::retention::{
    BUFFER_MONTHS, RETENTION, create_ddl, drop_ddl, harden_child_ddl, is_expired, months_to_create,
    parse_child,
};

/// Enumerate a partitioned parent's live child-partition relnames.
const CHILDREN_QUERY: &str = "
    SELECT c.relname
      FROM pg_inherits i
      JOIN pg_class c     ON c.oid = i.inhrelid
      JOIN pg_class p     ON p.oid = i.inhparent
      JOIN pg_namespace n ON n.oid = p.relnamespace
     WHERE n.nspname = $1 AND p.relname = $2";

/// Rotate every partitioned table once, failing loudly on the first DDL error;
/// rotation is idempotent, so the next run retries.
pub(crate) async fn rotate(options: &sqlx::postgres::PgConnectOptions) -> Result<()> {
    // A short lock wait, well under the services' own 2s: the DDL here queues
    // every `emit_audit` INSERT behind it while it waits on a reader, and a
    // failed run costs nothing with three months of partitions in hand.
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET lock_timeout = '1s'")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options.clone())
        .await
        .context("rotate: connect")?;

    let now = Utc::now();

    for t in RETENTION {
        // Read before the creates, so `created` counts partitions this run
        // made: `IF NOT EXISTS` succeeds either way, and the migration already
        // makes the first months, so a count of statements reads as "created
        // four" on a run that created none.
        let children: Vec<String> = sqlx::query_scalar(CHILDREN_QUERY)
            .bind(t.schema)
            .bind(t.table)
            .fetch_all(&pool)
            .await
            .with_context(|| format!("rotate: list children of {}.{}", t.schema, t.table))?;

        let mut created = 0usize;
        for m in months_to_create(now, BUFFER_MONTHS) {
            sqlx::query(&create_ddl(t, m))
                .execute(&pool)
                .await
                .with_context(|| {
                    format!("rotate: create {}.{}_{}", t.schema, t.table, m.suffix())
                })?;
            sqlx::query(&harden_child_ddl(t, m))
                .execute(&pool)
                .await
                .with_context(|| {
                    format!("rotate: harden {}.{}_{}", t.schema, t.table, m.suffix())
                })?;
            let name = format!("{}_{}", t.table, m.suffix());
            if !children.contains(&name) {
                created += 1;
            }
        }

        let mut dropped = 0usize;
        if t.drop_enabled {
            for child in children {
                if let Some(m) = parse_child(t.table, &child)
                    && is_expired(t, m, now)
                {
                    sqlx::query(&drop_ddl(t, m))
                        .execute(&pool)
                        .await
                        .with_context(|| format!("rotate: drop {}.{}", t.schema, child))?;
                    dropped += 1;
                    tracing::info!(
                        schema = t.schema,
                        table = t.table,
                        partition = %child,
                        "rotate: dropped expired partition",
                    );
                }
            }
        }

        tracing::info!(
            schema = t.schema,
            table = t.table,
            created,
            dropped,
            drop_enabled = t.drop_enabled,
            "rotate: table rotated",
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use telmoni_shared::test_util::database_url_or_skip;

    /// Pin the registry against the migrated schema: a phantom entry would
    /// starve create-ahead and, once the runway ends, fail every audited write.
    #[tokio::test]
    async fn registry_matches_migrated_schema_and_rotates() {
        let Some(url) = database_url_or_skip(module_path!()) else {
            return;
        };
        let pool = match PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(&url)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skipping {}: connect failed: {e}", module_path!());
                return;
            }
        };

        for t in RETENTION {
            let schema_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM information_schema.schemata WHERE schema_name = $1)",
            )
            .bind(t.schema)
            .fetch_one(&pool)
            .await
            .unwrap();
            if !schema_exists {
                eprintln!(
                    "skipping {}: schema {} not migrated on this DB",
                    module_path!(),
                    t.schema,
                );
                return;
            }

            let parent: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
                .bind(format!("{}.{}", t.schema, t.table))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(
                parent.is_some(),
                "RETENTION lists {}.{} but the migrated schema has no such table \
                 — remove the registry entry or restore its migration",
                t.schema,
                t.table,
            );
        }

        rotate(&telmoni_shared::db::connect_options(&url, None).expect("the test DSN parses"))
            .await
            .expect("rotate() must succeed against the migrated schema");
    }
}
