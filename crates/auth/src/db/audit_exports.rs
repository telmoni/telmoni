//! `auth.audit_exports` — an organization's audit chain built into a file in
//! the background, for the owner or admin who asked, and kept for them alone
//! until it expires. The building is [`crate::audit_export`].
//!
//! ⚠ **The requester's alone, by the policy and by every `WHERE`.** The
//! `tenant_isolation` policy admits a row only with both the organization and
//! its requester bound, so every query here runs under
//! [`PersonAndOrganization`] or the lane, and each still names the tenant and
//! the person in its `WHERE`: RLS is the floor. And no list ever selects
//! `file`: it is the chain again, megabytes of it.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{Maintenance, PersonAndOrganization, Scoped};
use telmoni_shared::{OrganizationId, UserId};

use crate::audit_export::{ExportFailure, ExportFormat, ExportStatus};
use crate::db::AuthLane;

/// The columns of an [`ExportEntry`], in its order.
const ENTRY: &str = "id, format, range_from, range_to, status, failure, row_count, \
                     octet_length(file)::bigint AS bytes, created_at, finished_at, expires_at, \
                     downloaded_at";

/// One export as its requester's list shows it: never the file itself.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ExportEntry {
    pub id: Uuid,
    pub format: ExportFormat,
    pub range_from: Option<DateTime<Utc>>,
    pub range_to: DateTime<Utc>,
    pub status: ExportStatus,
    /// Why a failed one failed; `None` for any other.
    pub failure: Option<ExportFailure>,
    pub row_count: Option<i64>,
    /// The file's size, once there is one.
    pub bytes: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub downloaded_at: Option<DateTime<Utc>>,
}

/// What a build needs of the export it claimed.
#[derive(Debug, sqlx::FromRow)]
pub struct Claimed {
    pub format: ExportFormat,
    pub range_from: Option<DateTime<Utc>>,
    pub range_to: DateTime<Utc>,
    /// This claim's attempt, the fence every later write checks: a build
    /// whose lease ran out and was taken over writes nothing.
    pub attempts: i32,
}

/// A finished file, as the download reads it.
#[derive(Debug, sqlx::FromRow)]
pub struct ExportFile {
    pub format: ExportFormat,
    pub file: Vec<u8>,
}

/// One export a build is owed, as the lane lists it.
#[derive(Debug, sqlx::FromRow)]
pub struct Due {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub user_id: UserId,
}

/// Queue an export, answered as its list shows it.
pub async fn create(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
    format: ExportFormat,
    range_from: Option<DateTime<Utc>>,
    range_to: DateTime<Utc>,
) -> sqlx::Result<ExportEntry> {
    sqlx::query_as::<_, ExportEntry>(&format!(
        "INSERT INTO auth.audit_exports
                (id, organization_id, user_id, format, range_from, range_to)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING {ENTRY}"
    ))
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .bind(format)
    .bind(range_from)
    .bind(range_to)
    .fetch_one(tx.conn())
    .await
}

/// How many of the person's exports in the organization are still being
/// built.
pub async fn unfinished_count(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM auth.audit_exports
          WHERE organization_id = $1 AND user_id = $2
            AND status IN ('queued', 'running')",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_one(tx.conn())
    .await
}

/// Delete the person's finished exports in the organization past their newest
/// `keep`, file and all, so a new one makes room for itself rather than
/// piling files up. One still building is never deleted, and counts.
pub async fn keep_newest(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    user_id: &UserId,
    keep: i64,
) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "DELETE FROM auth.audit_exports
          WHERE organization_id = $1 AND user_id = $2
            AND status IN ('ready', 'failed')
            AND id NOT IN (SELECT id FROM auth.audit_exports
                            WHERE organization_id = $1 AND user_id = $2
                            ORDER BY created_at DESC, id DESC
                            LIMIT $3)",
    )
    .bind(organization_id)
    .bind(user_id)
    .bind(keep)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

/// The person's latest exports in the organization, newest first, leaving out
/// the ones past their expiry that the sweep has not deleted yet.
pub async fn list(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    user_id: &UserId,
    limit: i64,
) -> sqlx::Result<Vec<ExportEntry>> {
    sqlx::query_as::<_, ExportEntry>(&format!(
        "SELECT {ENTRY} FROM auth.audit_exports
          WHERE organization_id = $1 AND user_id = $2
            AND (expires_at IS NULL OR expires_at > now())
          ORDER BY created_at DESC, id DESC
          LIMIT $3"
    ))
    .bind(organization_id)
    .bind(user_id)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// Take an export to build: one still queued, or one whose last build's lease
/// ran out, while it has attempts left. `None` when another build holds it,
/// it is finished, or its attempts are spent.
pub async fn claim(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
    max_attempts: i32,
    lease_secs: i32,
) -> sqlx::Result<Option<Claimed>> {
    sqlx::query_as::<_, Claimed>(
        "UPDATE auth.audit_exports
            SET status = 'running', attempts = attempts + 1,
                lease_until = now() + make_interval(secs => $5)
          WHERE id = $1 AND organization_id = $2 AND user_id = $3 AND attempts < $4
            AND (status = 'queued' OR (status = 'running' AND lease_until < now()))
          RETURNING format, range_from, range_to, attempts",
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .bind(max_attempts)
    .bind(lease_secs)
    .fetch_optional(tx.conn())
    .await
}

/// Give up on an export whose attempts are spent and whose last build
/// stopped without finishing it.
pub async fn fail_spent(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
    max_attempts: i32,
    keep_secs: i32,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.audit_exports
            SET status = 'failed', failure = 'error', lease_until = NULL,
                finished_at = now(), expires_at = now() + make_interval(secs => $5)
          WHERE id = $1 AND organization_id = $2 AND user_id = $3 AND attempts >= $4
            AND (status = 'queued' OR (status = 'running' AND lease_until < now()))",
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .bind(max_attempts)
    .bind(keep_secs)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Store the finished file, fenced on the claim's attempt. `false` when the
/// lease ran out and another build took the export over.
#[expect(
    clippy::too_many_arguments,
    reason = "the row's key, its requester, the fence and what it stores, each bound once"
)]
pub async fn finish_ready(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
    attempts: i32,
    file: &[u8],
    row_count: i64,
    keep_secs: i32,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.audit_exports
            SET status = 'ready', file = $5, row_count = $6, lease_until = NULL,
                finished_at = now(), expires_at = now() + make_interval(secs => $7)
          WHERE id = $1 AND organization_id = $2 AND user_id = $3 AND attempts = $4
            AND status = 'running'",
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .bind(attempts)
    .bind(file)
    .bind(row_count)
    .bind(keep_secs)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Fail an export for good, fenced on the claim's attempt: one no retry can
/// build (`too_large`), or the last attempt's error (`error`).
pub async fn finish_failed(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
    attempts: i32,
    failure: ExportFailure,
    keep_secs: i32,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.audit_exports
            SET status = 'failed', failure = $5, lease_until = NULL,
                finished_at = now(), expires_at = now() + make_interval(secs => $6)
          WHERE id = $1 AND organization_id = $2 AND user_id = $3 AND attempts = $4
            AND status = 'running'",
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .bind(attempts)
    .bind(failure)
    .bind(keep_secs)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Hand a build that hit an error back to the queue for the next attempt,
/// fenced on the claim's attempt.
pub async fn release(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
    attempts: i32,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.audit_exports
            SET status = 'queued', lease_until = NULL
          WHERE id = $1 AND organization_id = $2 AND user_id = $3 AND attempts = $4
            AND status = 'running'",
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .bind(attempts)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Drop an unfinished export whose requester no longer administers its
/// organization: nobody else may download it, so nothing of it is kept.
pub async fn withdraw(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "DELETE FROM auth.audit_exports
          WHERE id = $1 AND organization_id = $2 AND user_id = $3 AND finished_at IS NULL",
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The person's finished file, while it is kept.
pub async fn file_of(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<Option<ExportFile>> {
    sqlx::query_as::<_, ExportFile>(
        "SELECT format, file FROM auth.audit_exports
          WHERE id = $1 AND organization_id = $2 AND user_id = $3
            AND status = 'ready' AND expires_at > now()",
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// Note the first download, so the bell stops offering the file.
pub async fn mark_downloaded(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    id: Uuid,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE auth.audit_exports SET downloaded_at = now()
          WHERE id = $1 AND organization_id = $2 AND user_id = $3 AND downloaded_at IS NULL",
    )
    .bind(id)
    .bind(organization_id)
    .bind(user_id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The exports a build should have finished by now and has not: queued for
/// longer than `settle_secs`, its inline build lost to a restart, or running
/// past its lease. Across every organization, so the lane.
pub async fn unfinished_past_due(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    settle_secs: i32,
    limit: i64,
) -> sqlx::Result<Vec<Due>> {
    sqlx::query_as::<_, Due>(
        "SELECT id, organization_id, user_id FROM auth.audit_exports
          WHERE (status = 'queued' AND created_at < now() - make_interval(secs => $1))
             OR (status = 'running' AND lease_until < now())
          ORDER BY created_at
          LIMIT $2",
    )
    .bind(settle_secs)
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// Delete every export past its expiry, file and all.
pub async fn delete_expired(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.audit_exports WHERE expires_at < now()")
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}

/// Delete every export the person asked for, in every organization: nobody
/// else may download one, and each is the chain's rows again.
pub async fn delete_of_person(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_id: &UserId,
) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.audit_exports WHERE user_id = $1")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}
