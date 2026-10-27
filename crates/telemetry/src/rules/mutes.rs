//! Maintenance windows: an alert, or every alert of a project, muted from a
//! start to an end. A muted alert is evaluated as ever and its incidents
//! open and resolve in the feed, but nothing is told to a connector while the
//! window holds. Each window is kept after it ends, the record of who muted
//! what and when, and of who ended one early.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{HasProject, Maintenance, Scoped};
use telmoni_shared::{ProjectId, UserId};

use crate::db::TelemetryLane;

/// The longest a window runs.
pub const MUTE_DAYS_MAX: i64 = 30;

/// A window as its row keeps it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MuteRow {
    pub id: Uuid,
    pub project_id: ProjectId,
    /// The alert muted; none, every alert of the project.
    pub rule_id: Option<Uuid>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub ended_by: Option<String>,
}

const MUTE_COLUMNS: &str =
    "id, project_id, rule_id, starts_at, ends_at, created_by, created_at, ended_by";

/// How many of the project's windows have not ended.
pub async fn unended<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM telemetry.mutes
          WHERE project_id = $1 AND ends_at > now() AND ended_by IS NULL",
    )
    .bind(project_id)
    .fetch_one(tx.conn())
    .await
}

/// The project's windows, those not ended first, then those ending last: one
/// ended before it began keeps its end ahead.
pub async fn mutes<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    limit: i64,
) -> sqlx::Result<Vec<MuteRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {MUTE_COLUMNS} FROM telemetry.mutes WHERE project_id = $1
          ORDER BY ended_by IS NULL AND ends_at > now() DESC, ends_at DESC, id LIMIT $2"
    )))
    .bind(project_id)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// A new window of the project's, for `rule_id` or every alert.
pub async fn insert<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    rule_id: Option<Uuid>,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    by: &UserId,
) -> sqlx::Result<MuteRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO telemetry.mutes (id, project_id, rule_id, starts_at, ends_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING {MUTE_COLUMNS}"
    )))
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(rule_id)
    .bind(starts_at)
    .bind(ends_at)
    .bind(by)
    .fetch_one(tx.conn())
    .await
}

/// The project's ended windows past its newest `kept` by their end, gone.
pub async fn prune<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    kept: i64,
) -> sqlx::Result<u64> {
    sqlx::query(
        "DELETE FROM telemetry.mutes
          WHERE id IN (SELECT id FROM telemetry.mutes
                        WHERE project_id = $1 AND NOT (ended_by IS NULL AND ends_at > now())
                        ORDER BY ends_at DESC, id
                       OFFSET $2)",
    )
    .bind(project_id)
    .bind(kept)
    .execute(tx.conn())
    .await
    .map(|done| done.rows_affected())
}

/// One of the project's windows ended now by `by`, unless it has ended; one
/// yet to start ends just after its start. Answers it as it stands, and
/// whether this ended it.
pub async fn end<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    by: &UserId,
) -> sqlx::Result<(Option<MuteRow>, bool)> {
    let ended = sqlx::query(
        "UPDATE telemetry.mutes
            SET ends_at = greatest(starts_at + interval '1 microsecond', now()), ended_by = $3
          WHERE project_id = $1 AND id = $2 AND ends_at > now() AND ended_by IS NULL",
    )
    .bind(project_id)
    .bind(id)
    .bind(by)
    .execute(tx.conn())
    .await?
    .rows_affected()
        > 0;
    let row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {MUTE_COLUMNS} FROM telemetry.mutes WHERE project_id = $1 AND id = $2"
    )))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await?;
    Ok((row, ended))
}

/// Whether a window mutes the alert at `at`: its own, or its project's.
pub async fn in_force(
    tx: &mut Scoped<'_, Maintenance<TelemetryLane>>,
    project_id: &ProjectId,
    rule_id: Uuid,
    at: DateTime<Utc>,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM telemetry.mutes
                         WHERE project_id = $1 AND (rule_id IS NULL OR rule_id = $2)
                           AND starts_at <= $3 AND ends_at > $3)",
    )
    .bind(project_id)
    .bind(rule_id)
    .bind(at)
    .fetch_one(tx.conn())
    .await
}
