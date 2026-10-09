//! Read access to the cross-cutting `audit.events` chain.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{HasOrganization, Maintenance, Scoped};
use telmoni_shared::{AuditAction, OrganizationId, ProjectId, TelmoniResourceKind};

use crate::db::AuthLane;

/// One audit row, hashes included.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AuditRow {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub actor_id: String,
    pub action: String,
    pub resource_kind: String,
    pub resource_id: Option<String>,
    pub request_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub prev_hash: Option<String>,
    pub row_hash: Option<String>,
    /// The project the event happened inside; `None` for an organization-level
    /// event.
    pub in_project: Option<ProjectId>,
}

/// Filters for [`list`]. `cursor` paginates newest-first via the `UUIDv7` PK.
pub struct AuditQuery<'a> {
    pub organization_id: &'a OrganizationId,
    pub in_project: Option<&'a ProjectId>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub actor: Option<&'a str>,
    /// ⚠ **Typed, and it was `Option<&str>`.** A typo'd string filter matched
    /// no row and came back as an empty audit log — the one answer this surface
    /// must never give wrongly. The enum makes the typo a 400.
    pub action: Option<AuditAction>,
    pub resource_kind: Option<TelmoniResourceKind>,
    pub cursor: Option<Uuid>,
    pub limit: i64,
}

/// Distinct `organization_id`s that have at least one audit row, for the
/// nightly verify sweep.
///
/// A loose index walk, one probe per organization along
/// `events_organization_idx`: `SELECT DISTINCT` read every row of every
/// partition, and would have outgrown the statement timeout with the log.
pub async fn distinct_organizations(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar::<_, String>(
        "WITH RECURSIVE walk AS (
            (SELECT organization_id FROM audit.events ORDER BY organization_id LIMIT 1)
             UNION ALL
            SELECT (SELECT e.organization_id FROM audit.events e
                     WHERE e.organization_id > walk.organization_id
                     ORDER BY e.organization_id LIMIT 1)
              FROM walk
             WHERE walk.organization_id IS NOT NULL
         )
         SELECT organization_id FROM walk WHERE organization_id IS NOT NULL",
    )
    .fetch_all(tx.conn())
    .await
}

/// List an organization's audit rows newest-first — the whole chain, or one
/// project's rows of it — filtered and cursor-paginated.
///
/// Only the filters given reach the planner. A `$n IS NULL OR col = $n`
/// shape lost partition pruning and the actor and project indexes once the
/// prepared statement settled on a generic plan, and a rare actor then
/// walked the organization's whole history.
pub async fn list<B: HasOrganization>(
    tx: &mut Scoped<'_, B>,
    q: &AuditQuery<'_>,
) -> sqlx::Result<Vec<AuditRow>> {
    let mut sql = QueryBuilder::<Postgres>::new(
        "SELECT id, organization_id, actor_id, action, resource_kind, resource_id,
                request_id, metadata, created_at, prev_hash, row_hash, in_project
           FROM audit.events
          WHERE organization_id = ",
    );
    sql.push_bind(q.organization_id);
    if let Some(project) = q.in_project {
        sql.push(" AND in_project = ").push_bind(project);
    }
    if let Some(from) = q.from {
        sql.push(" AND created_at >= ").push_bind(from);
    }
    if let Some(to) = q.to {
        sql.push(" AND created_at < ").push_bind(to);
    }
    if let Some(actor) = q.actor {
        sql.push(" AND actor_id = ").push_bind(actor);
    }
    if let Some(action) = q.action {
        sql.push(" AND action = ").push_bind(action);
    }
    if let Some(kind) = q.resource_kind {
        sql.push(" AND resource_kind = ").push_bind(kind);
    }
    if let Some(cursor) = q.cursor {
        sql.push(" AND id < ").push_bind(cursor);
    }
    sql.push(" ORDER BY id DESC LIMIT ").push_bind(q.limit);
    sql.build_query_as::<AuditRow>().fetch_all(tx.conn()).await
}

/// One row of an export: every field the row hash covers
/// (`telmoni_shared::db::audit_hash::row_hash`), both hashes, and `seq`, the
/// chain's own order. `created_at` is the exact string the hash read, UTC to
/// the microsecond with a `Z`, so a verifier recomputes each hash from the
/// file alone. Never the address, the user agent or the request id, which the
/// hash does not cover and a file a customer forwards must not carry.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ExportRow {
    pub seq: i64,
    pub id: Uuid,
    pub created_at: String,
    pub actor_id: String,
    pub action: String,
    pub resource_kind: String,
    pub resource_id: Option<String>,
    pub in_project: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub prev_hash: Option<String>,
    pub row_hash: String,
}

/// The stretch of the chain a time range touches: the first and the last
/// `seq` written in `[from, to)`, `None` when nothing was. An export takes
/// every row between the two, so its file is one unbroken run of the chain
/// that verifies on its own.
/// ⚠ **By `seq`, never `created_at` alone.** A row's `created_at` is its
/// transaction's start and its `seq` the order the chain's lock minted, so a
/// row can carry a time just before the row ahead of it in the chain. A range
/// cut on time alone dropped that row from inside the window, and the file
/// broke where its neighbour named a `prev_hash` it did not hold.
pub async fn seq_bounds<B: HasOrganization>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    from: Option<DateTime<Utc>>,
    to: DateTime<Utc>,
) -> sqlx::Result<Option<(i64, i64)>> {
    let mut sql = QueryBuilder::<Postgres>::new(
        "SELECT min(seq), max(seq) FROM audit.events WHERE organization_id = ",
    );
    sql.push_bind(organization_id);
    sql.push(" AND created_at < ").push_bind(to);
    if let Some(from) = from {
        sql.push(" AND created_at >= ").push_bind(from);
    }
    let (first, last): (Option<i64>, Option<i64>) =
        sql.build_query_as().fetch_one(tx.conn()).await?;
    Ok(first.zip(last))
}

/// Up to `limit` chain rows after `seq` `after` and up to `last`, oldest
/// first: an export's stretch, a page at a time.
pub async fn export_page<B: HasOrganization>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    after: i64,
    last: i64,
    limit: i64,
) -> sqlx::Result<Vec<ExportRow>> {
    sqlx::query_as::<_, ExportRow>(
        "SELECT seq, id,
                to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')
                  AS created_at,
                actor_id, action, resource_kind, resource_id, in_project, metadata,
                prev_hash, row_hash
           FROM audit.events
          WHERE organization_id = $1 AND seq > $2 AND seq <= $3
          ORDER BY seq
          LIMIT $4",
    )
    .bind(organization_id)
    .bind(after)
    .bind(last)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// One audit row as the agent's index reads it: no hashes, and never the
/// address or the user agent.
#[derive(Debug, sqlx::FromRow)]
pub struct IndexedEvent {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    /// The organization's segment in console paths, for the citation. The
    /// chain outlives the organization, not the other way round: the inner
    /// join leaves a deleted organization's events unindexed, on purpose —
    /// nobody is left to read them.
    pub organization_slug: String,
    pub in_project: Option<ProjectId>,
    /// The project's segment, `None` when the event was the organization's own
    /// or the project has since been deleted.
    pub project_slug: Option<String>,
    pub actor_id: String,
    pub action: String,
    pub resource_kind: String,
    pub resource_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// How old a row must be before the index pages past it. An event's
/// `created_at` is its transaction's start, so one committed late would land
/// behind a cursor that had already moved on; a transaction of the auth role
/// that writes one does not outlive its two-minute idle cut-off (the sweeps'
/// pinged leader lock does, and writes none), and this is past it.
const INDEX_SETTLE_SECONDS: i32 = 180;

/// Every organization's events after `(after_at, after_id)`, oldest first,
/// for the agent's index. Across every tenant, so the lane.
pub async fn after_cursor(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> sqlx::Result<Vec<IndexedEvent>> {
    let (after_at, after_id) = after.unwrap_or((DateTime::<Utc>::UNIX_EPOCH, Uuid::nil()));
    sqlx::query_as::<_, IndexedEvent>(
        "SELECT e.id, e.organization_id, o.slug AS organization_slug, e.in_project,
                p.slug AS project_slug, e.actor_id, e.action, e.resource_kind, e.resource_id,
                e.metadata, e.created_at
           FROM audit.events e
           JOIN auth.organizations o ON o.external_id = e.organization_id
           LEFT JOIN auth.projects p ON p.external_id = e.in_project
          WHERE (e.created_at, e.id) > ($1, $2)
            AND e.created_at < now() - make_interval(secs => $3)
          ORDER BY e.created_at, e.id
          LIMIT $4",
    )
    .bind(after_at)
    .bind(after_id)
    .bind(INDEX_SETTLE_SECONDS)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}
