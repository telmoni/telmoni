//! A project's move lock: what keeps a write that files rows under the
//! organization it read the project in from landing beside a move of the
//! project to another organization — the move missing the rows the write had
//! not committed, and the write refused an update of a row the move had.
//!
//! A move — a transfer's accept, an owner's move, the settling of either —
//! takes it alone ([`lock_for_move`]) across telemetry's move and holds it to
//! the caller's commit, waiting for the writes in flight as long as its
//! `lock_timeout` lets it. A write — a ping, a key its rotation retires —
//! only tries for it, shared ([`try_hold_in_place`]), as the first lock of
//! its transaction, and reads the organization once it holds it: a move
//! that committed meanwhile is seen, and none commits beside the write. A
//! move holding the lock, or waiting for it, turns a write away at once, to
//! let go of its connection and try again, so writes kept out by a move hold
//! no connection while they wait — they take turns on the pool with the
//! move's own work — and never wait on each other.
//!
//! ⚠ **Before any chain lock.** A write holding its project in place may wait
//! for its organization's chain lock ([`crate::audit::lock_chain`]), so a
//! move that took a chain lock first would wait on that write in a circle,
//! which Postgres breaks by aborting one side.
//!
//! ⚠ **The caller's lock, not telemetry's.** A move given up on over its
//! budget may still commit after the caller let the lock go, beside a write
//! let in since, until the settling that follows takes the lock again.
//!
//! Advisory, so it names no row: no writer of the project's rows waits on it
//! but those two, whatever order they take their rows in.

use sqlx::PgConnection;

use crate::types::ProjectId;

/// Wait for `project_id`'s move lock alone, held to the end of the caller's
/// transaction: for a move of the project's rows to another organization.
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
