//! Lanes for the shared crate's own harness tests, which enter every service's
//! lane with the same SQL to prove what each may see. A service declares its
//! own typed [`Lane`](crate::db::tenant_session::Lane) marker in its own crate;
//! a harness that walks the lanes at run time takes the role from the enum
//! instead, and gets a plain transaction no query function accepts.

use sqlx::{PgPool, Postgres, Transaction};

use crate::db::tenant_session::MaintenanceLane;

/// A transaction in `lane`, chosen at run time, as the superuser the tests
/// connect as. For raw SQL only.
pub async fn enter(
    pool: &PgPool,
    lane: MaintenanceLane,
) -> sqlx::Result<Transaction<'_, Postgres>> {
    let mut tx = pool.begin().await?;
    sqlx::query(lane.set_role()).execute(&mut *tx).await?;
    Ok(tx)
}
