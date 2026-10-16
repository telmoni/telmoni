//! A project's move lock: it keeps a write that files rows under the
//! organization it read the project in from landing beside a move of the
//! project to another organization, or beside its purge. A move or a purge
//! takes it alone ([`lock_for_move`]) to its commit; a write — a ping, a key
//! its rotation retires — only tries for it, shared ([`try_hold_in_place`]),
//! and is turned away at once, holding no connection while it waits.
//!
//! ⚠ Take it before any chain lock ([`crate::audit::lock_chain`]): a write
//! holding its project may wait for that lock, and the other order
//! deadlocks. A move given up on over its budget may still commit after its
//! caller lets the lock go.

use sqlx::PgConnection;

use crate::types::ProjectId;

/// Wait for `project_id`'s move lock alone, held to the end of the caller's
/// transaction: for a move or a purge of the project's rows.
pub async fn lock_for_move(conn: &mut PgConnection, project_id: &ProjectId) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('move:' || $1, 0))")
        .bind(project_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// `project_id`'s move lock, shared, held to the end of the caller's
/// transaction where this answers `true`: for a write that files rows under
/// the organization it reads the project in, read once this answers. `false`
/// at once where a move holds the lock or waits for it.
pub async fn try_hold_in_place(
    conn: &mut PgConnection,
    project_id: &ProjectId,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock_shared(hashtextextended('move:' || $1, 0))",
    )
    .bind(project_id)
    .fetch_one(conn)
    .await
}
