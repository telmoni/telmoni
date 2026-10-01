//! `auth.authorization_codes` — the one-time code a built-in sign-in hands
//! the console, which the console's callback spends within seconds for the
//! session's tokens. The same shape an external provider's code has, so the
//! callback cannot tell which it is talking to.

use chrono::{DateTime, Utc};
use telmoni_shared::UserId;
use telmoni_shared::db::tenant_session::{Maintenance, Person, Scoped};
use uuid::Uuid;

use crate::db::AuthLane;

/// Mint a code for the person who just proved their password.
pub async fn create(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    sid: &str,
    code_hash: &str,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO auth.authorization_codes (id, user_id, sid, code_hash, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(sid)
    .bind(code_hash)
    .bind(expires_at)
    .bind(telmoni_shared::derive_shard_key(user_id))
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Spend a code: the person and session it was minted for, or `None` for a
/// code that is unknown, spent or expired. Spent by deletion, so a second
/// presentation finds nothing.
pub async fn spend(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    code_hash: &str,
) -> sqlx::Result<Option<(UserId, String)>> {
    sqlx::query_as(
        "DELETE FROM auth.authorization_codes
          WHERE code_hash = $1 AND expires_at > now()
      RETURNING user_id, sid",
    )
    .bind(code_hash)
    .fetch_optional(tx.conn())
    .await
}

/// The retention sweep's share: codes nobody spent.
pub async fn delete_expired(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.authorization_codes WHERE expires_at < now()")
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}
