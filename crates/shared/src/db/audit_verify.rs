//! Verify an organization's tamper-evident audit chain.
//!
//! Walks the chained rows in `seq` order, recomputing each `row_hash` (catches
//! a field edited in place) and checking each `prev_hash` against the previous
//! row (catches deletion, reordering or insertion).

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::db::audit_hash::row_hash;

/// Why a chain failed verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakKind {
    /// The stored `row_hash` doesn't match a recompute from the row's own
    /// fields — a field was edited in place.
    RowHashMismatch,
    /// `prev_hash` doesn't match the previous row's `row_hash` — a row was
    /// deleted, reordered, or inserted out of band.
    PrevLinkMismatch,
}

/// The first broken row, if any.
#[derive(Debug, Clone, Serialize)]
pub struct ChainBreak {
    /// The `audit.events` row where verification failed.
    pub id: Uuid,
    /// How it failed.
    pub kind: BreakKind,
}

/// Result of a chain walk.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    /// Rows walked up to (and including) the first break, or the whole range.
    pub rows: u64,
    /// `None` ⇒ the chain verified cleanly.
    pub first_break: Option<ChainBreak>,
}

impl VerifyReport {
    /// Did the chain verify with no breaks?
    #[must_use]
    pub fn is_intact(&self) -> bool {
        self.first_break.is_none()
    }
}

#[derive(sqlx::FromRow)]
struct ChainRow {
    /// Selected for the page cursor, not the hash: `row_hash` does not cover it.
    seq: i64,
    id: Uuid,
    organization_id: String,
    actor_id: String,
    action: String,
    resource_kind: String,
    resource_id: Option<String>,
    metadata: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
    prev_hash: Option<String>,
    row_hash: String,
    in_project: Option<String>,
}

/// Walk + verify the ORGANIZATION's chain over the optional `[from, to)`
/// window, stopping at the first break. Under the organization's scope, or
/// the maintenance lane.
pub async fn verify_audit_chain(
    conn: &mut PgConnection,
    organization_id: &crate::types::OrganizationId,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> sqlx::Result<VerifyReport> {
    verify_audit_chain_batched(conn, organization_id, from, to, BATCH).await
}

/// How many rows one page of the walk holds.
///
/// ⚠ **Paged because the chain has no ceiling.** The default request is an
/// organization's entire history, and reading it into one `Vec` risked an OOM
/// that takes down every request sharing the instance. Sound because the table
/// is append-only and `seq` is monotonic: a forward cursor cannot skip a row or
/// see one twice.
const BATCH: i64 = 1_000;

/// The `row_hash` of the organization's row at `seq`: the link a windowed walk
/// starts from. One index probe per partition, under the caller's scope.
async fn link_at(
    conn: &mut PgConnection,
    organization_id: &crate::types::OrganizationId,
    seq: i64,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        "SELECT row_hash
           FROM audit.events
          WHERE organization_id = $1 AND seq = $2
          LIMIT 1",
    )
    .bind(organization_id)
    .bind(seq)
    .fetch_optional(conn)
    .await
}

/// The `seq` span of the organization's rows inside `[from, to)`, `None`s
/// when the window holds none. A window is chosen by time but walked by
/// `seq`, so it is resolved to `seq` once and the walk carries no time
/// predicate: a row whose clock crossed a neighbour's inside the window cannot
/// fall out of the walk and read as a break. `COALESCE`, not `$2 IS NULL OR`,
/// so the bounds prune partitions under a generic plan too.
async fn window_seq_bounds(
    conn: &mut PgConnection,
    organization_id: &crate::types::OrganizationId,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> sqlx::Result<(Option<i64>, Option<i64>)> {
    sqlx::query_as(
        "SELECT min(seq), max(seq)
           FROM audit.events
          WHERE organization_id = $1
            AND created_at >= COALESCE($2::timestamptz, '-infinity')
            AND created_at <  COALESCE($3::timestamptz, 'infinity')",
    )
    .bind(organization_id)
    .bind(from)
    .bind(to)
    .fetch_one(conn)
    .await
}

/// [`verify_audit_chain`] with the page size exposed, so a test can cross a
/// page seam — where `prev` has to survive between pages — with a few rows.
#[doc(hidden)]
pub async fn verify_audit_chain_batched(
    conn: &mut PgConnection,
    organization_id: &crate::types::OrganizationId,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    batch: i64,
) -> sqlx::Result<VerifyReport> {
    // Ordered by the per-organization `seq`, not `id`: the chain's link order
    // must not depend on cross-replica clock sync.
    const CHAIN_PAGE: &str =
        "SELECT seq, id, organization_id, actor_id, action, resource_kind, resource_id,
                metadata, created_at, prev_hash, row_hash, in_project
           FROM audit.events
          WHERE organization_id = $1
            AND seq > $2
            AND seq <= COALESCE($3::bigint, 9223372036854775807)
          ORDER BY seq ASC
          LIMIT $4";

    let (mut cursor, last_seq) = if from.is_some() || to.is_some() {
        match window_seq_bounds(&mut *conn, organization_id, from, to).await? {
            (Some(first), last) => (first - 1, last),
            (None, _) => {
                return Ok(VerifyReport {
                    rows: 0,
                    first_break: None,
                });
            }
        }
    } else {
        (i64::MIN, None)
    };

    let mut prev: Option<String> = None;
    let mut count = 0u64;
    loop {
        let rows: Vec<ChainRow> = sqlx::query_as::<_, ChainRow>(CHAIN_PAGE)
            .bind(organization_id)
            .bind(cursor)
            .bind(last_seq)
            .bind(batch)
            .fetch_all(&mut *conn)
            .await?;
        let Some(last) = rows.last() else {
            break;
        };
        // Seed the link from the row before the walk's first, so a walk that
        // starts mid-chain does not fail on its first row; a chain missing its
        // genesis still does, since `seq` 0 has no row.
        if count == 0
            && let Some(first) = rows.first().filter(|r| r.seq > 1)
        {
            prev = link_at(&mut *conn, organization_id, first.seq - 1).await?;
        }
        cursor = last.seq;
        let short = i64::try_from(rows.len()).unwrap_or(batch) < batch;

        for r in &rows {
            count += 1;

            let recomputed = row_hash(
                r.id,
                &r.organization_id,
                &r.actor_id,
                &r.action,
                &r.resource_kind,
                r.resource_id.as_deref(),
                r.metadata.as_ref(),
                r.created_at,
                r.prev_hash.as_deref(),
                r.in_project.as_deref(),
            );
            if recomputed != r.row_hash {
                return Ok(VerifyReport {
                    rows: count,
                    first_break: Some(ChainBreak {
                        id: r.id,
                        kind: BreakKind::RowHashMismatch,
                    }),
                });
            }

            if r.prev_hash.as_deref() != prev.as_deref() {
                return Ok(VerifyReport {
                    rows: count,
                    first_break: Some(ChainBreak {
                        id: r.id,
                        kind: BreakKind::PrevLinkMismatch,
                    }),
                });
            }
            prev = Some(r.row_hash.clone());
        }

        if short {
            break;
        }
    }

    Ok(VerifyReport {
        rows: count,
        first_break: None,
    })
}
