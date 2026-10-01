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

/// One audit row as the agent's index reads it: no hashes, and never the
/// address or the user agent.
#[derive(Debug, sqlx::FromRow)]
pub struct IndexedEvent {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub in_project: Option<ProjectId>,
    pub actor_id: String,
    pub action: String,
    pub resource_kind: String,
    pub resource_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// How old a row must be before the index pages past it. An event's
/// `created_at` is its transaction's start, so one committed late would land
/// behind a cursor that had already moved on; no transaction of the auth
/// role outlives its two-minute idle cut-off, and this is past it.
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
        "SELECT id, organization_id, in_project, actor_id, action, resource_kind, resource_id,
                metadata, created_at
           FROM audit.events
          WHERE (created_at, id) > ($1, $2)
            AND created_at < now() - make_interval(secs => $3)
          ORDER BY created_at, id
          LIMIT $4",
    )
    .bind(after_at)
    .bind(after_id)
    .bind(INDEX_SETTLE_SECONDS)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}
