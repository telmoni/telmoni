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

/// The project's windows, those ending last first.
pub async fn mutes<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    limit: i64,
) -> sqlx::Result<Vec<MuteRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {MUTE_COLUMNS} FROM telemetry.mutes WHERE project_id = $1
          ORDER BY ends_at DESC, id LIMIT $2"
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

/// One of the project's windows ended now by `by`, where it had not ended;
/// one yet to start ends a moment after its start. Answers it as it stands.
pub async fn end<B: HasProject>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    id: Uuid,
    by: &UserId,
) -> sqlx::Result<Option<MuteRow>> {
    sqlx::query(
        "UPDATE telemetry.mutes
            SET ends_at = greatest(starts_at + interval '1 microsecond', now()), ended_by = $3
          WHERE project_id = $1 AND id = $2 AND ends_at > now()",
    )
    .bind(project_id)
    .bind(id)
    .bind(by)
    .execute(tx.conn())
    .await?;
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {MUTE_COLUMNS} FROM telemetry.mutes WHERE project_id = $1 AND id = $2"
    )))
    .bind(project_id)
    .bind(id)
    .fetch_optional(tx.conn())
    .await
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
