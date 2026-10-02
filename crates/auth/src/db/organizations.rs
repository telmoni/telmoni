//! Queries against `auth.organizations` — the tenant root. Nothing about a
//! person lives here: the owner is a row in `auth.organization_members`, and
//! their address and name are `auth.identities`.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::AuthLane;
use crate::model::{DeletionKind, Organization};
use telmoni_shared::db::tenant_session::{
    self, Binding, HasOrganization, Maintenance, PersonAndOrganization, ProjectAndOrganization,
    Scoped,
};
use telmoni_shared::{OrganizationId, OrganizationStatus, derive_shard_key, slug};

/// Every column `Organization` reads, in `FromRow` order. Written once because a
/// hand-copied list is how one query starts selecting something else.
const ORGANIZATION_COLUMNS: &str = "id, external_id, slug, name, status, deletion_requested_at, \
     erase_after, deletion_kind, hook_purged_at, created_at";

/// One organization by its id.
pub async fn get<B: Binding>(
    tx: &mut Scoped<'_, B>,
    external_id: &OrganizationId,
) -> sqlx::Result<Option<Organization>> {
    sqlx::query_as::<_, Organization>(&format!(
        "SELECT {ORGANIZATION_COLUMNS} FROM auth.organizations WHERE external_id = $1"
    ))
    .bind(external_id)
    .fetch_optional(tx.conn())
    .await
}

/// The longest name an organization may carry, in CHARACTERS rather than
/// bytes, so a non-Latin name is not refused at a third of an ASCII one's length.
pub const MAX_ORGANIZATION_NAME: usize = 80;

/// Write a new organization. `external_id` is freshly minted by the caller;
/// the slug is a placeholder until the owner names it.
///
/// ⚠ **`name` IS LEFT NULL, AND THAT IS THE DEFAULT WORKING.** A new
/// organization shows its owner's address, derived at read time. Seeding a
/// copy was the bug: an address moves and the copy does not. NULL means the
/// owner has not named it. Nor does the slug borrow the address: it is in
/// every path, and a path is logged.
pub async fn create<B: HasOrganization>(
    tx: &mut Scoped<'_, B>,
    external_id: &OrganizationId,
) -> sqlx::Result<Organization> {
    let slug = slug::placeholder(slug::Scope::Organization);
    sqlx::query_as::<_, Organization>(&format!(
        "INSERT INTO auth.organizations
             (id, external_id, slug, shard_key, created_at, updated_at)
         VALUES ($1, $2, $3, $4, now(), now())
         RETURNING {ORGANIZATION_COLUMNS}"
    ))
    .bind(Uuid::now_v7())
    .bind(external_id)
    .bind(&slug)
    .bind(derive_shard_key(external_id))
    .fetch_one(tx.conn())
    .await
}

/// The first of `candidates` no OTHER organization goes by, in any status.
/// The lane's read, since an organization's own binding sees no other's row;
/// the write that follows is the caller's, and `organizations_slug_key` judges
/// a race between the two.
pub async fn first_free_slug(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    external_id: &OrganizationId,
    candidates: &[String],
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT c.slug FROM unnest($2::text[]) WITH ORDINALITY AS c(slug, position)
          WHERE NOT EXISTS (SELECT 1 FROM auth.organizations o
                             WHERE o.slug = c.slug AND o.external_id <> $1)
          ORDER BY c.position
          LIMIT 1",
    )
    .bind(external_id)
    .bind(candidates)
    .fetch_optional(tx.conn())
    .await
}

/// The organization's slug; `None` when the binding sees no such row.
pub async fn slug_of<B: Binding>(
    tx: &mut Scoped<'_, B>,
    external_id: &OrganizationId,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT slug FROM auth.organizations WHERE external_id = $1")
        .bind(external_id)
        .fetch_optional(tx.conn())
        .await
}

/// Rename the organization, moving its slug with the name when `slug` is
/// given, and answer the slug it goes by now; `None` when it is not active.
/// A slug another organization took since it was found free fails on
/// `organizations_slug_key`, which the caller names.
pub async fn rename(
    tx: &mut Scoped<'_, tenant_session::Organization>,
    external_id: &OrganizationId,
    name: &str,
    slug: Option<&str>,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "UPDATE auth.organizations
            SET name = $2, slug = COALESCE($3, slug), updated_at = now()
          WHERE external_id = $1 AND status = 'active'
      RETURNING slug",
    )
    .bind(external_id)
    .bind(name)
    .bind(slug)
    .fetch_optional(tx.conn())
    .await
}

/// Flip an active organization into `pending_deletion` — the saga's durable
/// queue — recording who asked and when the row may go: `wait_seconds` from
/// now on the database's clock, the same one the sweep's listing reads.
/// Answers that moment; `None` when the organization was not active.
pub async fn mark_pending_deletion<B: HasOrganization>(
    tx: &mut Scoped<'_, B>,
    external_id: &OrganizationId,
    kind: DeletionKind,
    wait_seconds: u32,
) -> sqlx::Result<Option<DateTime<Utc>>> {
    sqlx::query_scalar(
        "UPDATE auth.organizations
            SET status = 'pending_deletion',
                deletion_requested_at = now(),
                erase_after = now() + make_interval(secs => $3),
                deletion_kind = $2,
                hook_purged_at = NULL,
                updated_at = now()
          WHERE external_id = $1 AND status = 'active'
      RETURNING erase_after",
    )
    .bind(external_id)
    .bind(kind)
    .bind(f64::from(wait_seconds))
    .fetch_optional(tx.conn())
    .await
}

/// Record that the purge hook answered for the organization, so the sweep
/// stops retrying that step. `false` when the row is not pending, or it was
/// already recorded.
pub async fn record_hook_purged(
    tx: &mut Scoped<'_, tenant_session::Organization>,
    external_id: &OrganizationId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.organizations
            SET hook_purged_at = now(), updated_at = now()
          WHERE external_id = $1 AND status = 'pending_deletion' AND hook_purged_at IS NULL",
    )
    .bind(external_id)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Bring a pending organization back to `active`, clearing every deletion
/// column. Who may, and until when, is the lane's to decide under the
/// organization's lock; this only refuses a row that is not pending.
pub async fn restore(
    tx: &mut Scoped<'_, tenant_session::Organization>,
    external_id: &OrganizationId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.organizations
            SET status = 'active',
                deletion_requested_at = NULL,
                erase_after = NULL,
                deletion_kind = NULL,
                hook_purged_at = NULL,
                updated_at = now()
          WHERE external_id = $1 AND status = 'pending_deletion'",
    )
    .bind(external_id)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// The organization's lifecycle status, read under `FOR SHARE`.
pub async fn status_for_share(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    external_id: &OrganizationId,
) -> sqlx::Result<Option<OrganizationStatus>> {
    sqlx::query_scalar::<_, OrganizationStatus>(
        "SELECT status FROM auth.organizations WHERE external_id = $1 FOR SHARE",
    )
    .bind(external_id)
    .fetch_optional(tx.conn())
    .await
}

/// The organization's lifecycle status, or `None` when there is no such row.
/// For a lane that already holds the organization's lock, where `FOR SHARE`
/// would add nothing.
pub async fn status<B: Binding>(
    tx: &mut Scoped<'_, B>,
    external_id: &OrganizationId,
) -> sqlx::Result<Option<OrganizationStatus>> {
    sqlx::query_scalar::<_, OrganizationStatus>(
        "SELECT status FROM auth.organizations WHERE external_id = $1",
    )
    .bind(external_id)
    .fetch_optional(tx.conn())
    .await
}

/// An organization already `pending_deletion` — its owner deleted it, or an
/// operator terminated it — taken by its owner's account deletion: the kind
/// becomes `account`, so neither restore lane hands it back to an owner who
/// is being erased, and its wait shortens to the finalize grace when the
/// restore window would outlast it, since nobody is left to use the window.
/// Answers when the row may go; `None` when the row is not pending.
pub async fn take_for_account(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    external_id: &OrganizationId,
    wait_seconds: u32,
) -> sqlx::Result<Option<DateTime<Utc>>> {
    sqlx::query_scalar(
        "UPDATE auth.organizations
            SET deletion_kind = $2,
                erase_after = LEAST(erase_after, now() + make_interval(secs => $3)),
                updated_at = now()
          WHERE external_id = $1 AND status = 'pending_deletion'
      RETURNING erase_after",
    )
    .bind(external_id)
    .bind(DeletionKind::Account)
    .bind(f64::from(wait_seconds))
    .fetch_optional(tx.conn())
    .await
}

/// The finalize's hard delete: the row and every child that cascades, and
/// only while the row is still `pending_deletion` and past `erase_after` on
/// the database's clock. The finalize read the standing before its purges,
/// which take a sibling round trip each, and an operator's restore is allowed
/// past the window: one that landed in between matches nothing here, so the
/// purges that ran are the worst of it and the organization stands.
pub async fn delete_ripe(
    tx: &mut Scoped<'_, tenant_session::Organization>,
    external_id: &OrganizationId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM auth.organizations
          WHERE external_id = $1 AND status = 'pending_deletion' AND erase_after <= now()",
    )
    .bind(external_id)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Where a finalize stands: the organization's status, whether its
/// `erase_after` has passed, and whether its hook purge is recorded —
/// `None` when there is no such row. Ripeness is judged by the database's
/// clock, the same one [`list_due_for_sweep`] reads, so an organization the
/// sweep lists is never refused as too soon by a skewed one.
pub async fn finalize_standing(
    tx: &mut Scoped<'_, tenant_session::Organization>,
    external_id: &OrganizationId,
) -> sqlx::Result<Option<(OrganizationStatus, bool, bool)>> {
    sqlx::query_as(
        "SELECT status, COALESCE(erase_after <= now(), false), hook_purged_at IS NOT NULL
           FROM auth.organizations
          WHERE external_id = $1",
    )
    .bind(external_id)
    .fetch_optional(tx.conn())
    .await
}

/// One pending organization the sweep owes a step.
#[derive(Debug, sqlx::FromRow)]
pub struct DueOrganization {
    pub external_id: OrganizationId,
    pub deletion_requested_at: DateTime<Utc>,
    pub erase_after: DateTime<Utc>,
    /// `erase_after` has passed: the row may go.
    pub ripe: bool,
}

/// Pending organizations the sweep has work on, soonest due first: those
/// whose hook purge never landed, and those past `erase_after`. One that
/// is purged and still inside its window is nobody's to touch and is left
/// off, so a fortnight's wait costs no calls.
pub async fn list_due_for_sweep(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    limit: i64,
) -> sqlx::Result<Vec<DueOrganization>> {
    sqlx::query_as::<_, DueOrganization>(
        "SELECT external_id, deletion_requested_at, erase_after,
                (erase_after <= now()) AS ripe
           FROM auth.organizations
          WHERE status = 'pending_deletion'
            AND (hook_purged_at IS NULL OR erase_after <= now())
          ORDER BY erase_after ASC, external_id ASC
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}
