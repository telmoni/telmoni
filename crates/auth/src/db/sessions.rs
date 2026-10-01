//! `auth.sessions` — one row per browser sign-in, and the PERSON's, whichever
//! organization the person then acts in. `provider_sid` is the session id the
//! issuer minted, which every token of the session carries. Three lanes carry
//! no person: the refresh and logout lanes hold a row id or that `sid`, the
//! nightly prune nobody's.

use chrono::{DateTime, Utc};
use sqlx::prelude::FromRow;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{Maintenance, Person, Scoped};
use telmoni_shared::{UserId, derive_shard_key, text::truncate_on_char_boundary};

use crate::db::AuthLane;

/// A session as the settings page lists it.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct SessionRow {
    pub id: Uuid,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// How many BYTES of a `User-Agent` are kept.
const USER_AGENT_MAX: usize = 400;

/// Record a sign-in. Returns the row id, which the BFF seals into the cookie.
pub async fn create(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    provider_sid: Option<&str>,
    user_agent: Option<&str>,
) -> sqlx::Result<Uuid> {
    let ua = user_agent.map(|value| truncate_on_char_boundary(value, USER_AGENT_MAX));
    sqlx::query_scalar(
        "INSERT INTO auth.sessions
             (id, user_id, provider_sid, user_agent, shard_key)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(provider_sid)
    .bind(ua)
    .bind(derive_shard_key(user_id))
    .fetch_one(tx.conn())
    .await
}

/// One person's live sessions, newest first.
pub async fn list(tx: &mut Scoped<'_, Person>, user_id: &UserId) -> sqlx::Result<Vec<SessionRow>> {
    sqlx::query_as::<_, SessionRow>(
        "SELECT id, user_agent, created_at, last_seen_at
           FROM auth.sessions
          WHERE user_id = $1 AND revoked_at IS NULL
          ORDER BY created_at DESC, id DESC",
    )
    .bind(user_id)
    .fetch_all(tx.conn())
    .await
}

/// The row for a session, or a new one. `/me` calls this on every render
/// with the session the bearer belongs to, so it must be idempotent under a race:
/// the partial unique index on `provider_sid` makes the second insert a
/// no-op, and the re-read after it returns the winner.
pub async fn find_or_create_for_sid(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    provider_sid: &str,
    user_agent: Option<&str>,
) -> sqlx::Result<Uuid> {
    if let Some(id) = find_by_sid(tx, provider_sid).await? {
        return Ok(id);
    }
    let ua = user_agent.map(|value| truncate_on_char_boundary(value, USER_AGENT_MAX));
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO auth.sessions
             (id, user_id, provider_sid, user_agent, shard_key)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (provider_sid) WHERE provider_sid IS NOT NULL DO NOTHING
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(provider_sid)
    .bind(ua)
    .bind(derive_shard_key(user_id))
    .fetch_optional(tx.conn())
    .await?;
    match inserted {
        Some(id) => Ok(id),
        None => find_by_sid(tx, provider_sid)
            .await?
            .ok_or(sqlx::Error::RowNotFound),
    }
}

/// The row id for a session, revoked or not.
pub async fn find_by_sid(
    tx: &mut Scoped<'_, Person>,
    provider_sid: &str,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar("SELECT id FROM auth.sessions WHERE provider_sid = $1")
        .bind(provider_sid)
        .fetch_optional(tx.conn())
        .await
}

/// Hard-delete sessions revoked more than ninety days ago: the nightly
/// retention sweep. A revoked row is what the bearer lookup refuses on
/// (`access_tokens::resolve`), so it outlives every token of its session;
/// live rows are never pruned, since the row is the person's own record of a
/// session that may still be theirs.
pub async fn delete_revoked(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    let result =
        sqlx::query("DELETE FROM auth.sessions WHERE revoked_at < now() - interval '90 days'")
            .execute(tx.conn())
            .await?;
    Ok(result.rows_affected())
}

/// Whether a row (by the id the console holds) has been revoked. `None` for
/// no such row. The refresh lane carries the row id alone.
pub async fn is_revoked(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    id: Uuid,
) -> sqlx::Result<Option<bool>> {
    sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM auth.sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(tx.conn())
        .await
}

/// Bump `last_seen_at` on token refresh — refresh granularity, not per
/// request, hence "last seen". `false` means the row was ended elsewhere.
pub async fn touch_by_id(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    id: Uuid,
) -> sqlx::Result<bool> {
    let updated = sqlx::query(
        "UPDATE auth.sessions SET last_seen_at = now()
          WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(tx.conn())
    .await?;
    Ok(updated.rows_affected() == 1)
}

/// End the row for a session, on sign-out. Returns the row id, or `None`
/// when nothing live carried that `sid`. The logout lane carries the `sid`
/// alone.
pub async fn revoke_by_sid(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    provider_sid: &str,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(
        "UPDATE auth.sessions SET revoked_at = now()
          WHERE provider_sid = $1 AND revoked_at IS NULL
      RETURNING id",
    )
    .bind(provider_sid)
    .fetch_optional(tx.conn())
    .await
}

/// Mark a session revoked and hand back its `sid`, so the caller can end its
/// tokens too. Idempotent on `revoked_at IS NULL`, so a double-click ends
/// them once; scoped by person as well as `id`, because an id from a request
/// body is caller input.
pub async fn revoke(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    id: Uuid,
) -> sqlx::Result<Option<Option<String>>> {
    sqlx::query_scalar::<_, Option<String>>(
        "UPDATE auth.sessions SET revoked_at = now()
          WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL
      RETURNING provider_sid",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// Revoke every live session of one person, and hand back the `sid` of each. ⚠ There is deliberately no `revoke_others`: a narrower variant can
/// only ever revoke a subset of what the caller meant.
pub async fn revoke_all_for_person(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<Vec<Option<String>>> {
    sqlx::query_scalar::<_, Option<String>>(
        "UPDATE auth.sessions SET revoked_at = now()
          WHERE user_id = $1 AND revoked_at IS NULL
      RETURNING provider_sid",
    )
    .bind(user_id)
    .fetch_all(tx.conn())
    .await
}
