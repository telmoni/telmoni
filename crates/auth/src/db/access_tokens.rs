//! `auth.access_tokens` — the bearers the issuer mints, one row per token.
//! Each grant writes one beside its refresh token; the person lane resolves
//! the one it is handed here on every request.

use chrono::{DateTime, Utc};
use telmoni_shared::UserId;
use telmoni_shared::db::tenant_session::{Binding, Maintenance, Person, Scoped};
use uuid::Uuid;

use crate::db::AuthLane;

/// Write a token. Under the person at sign-in and under the lane at refresh,
/// which carries the old refresh token and no person.
pub async fn create<B: Binding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    sid: &str,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO auth.access_tokens (id, user_id, sid, token_hash, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(sid)
    .bind(token_hash)
    .bind(expires_at)
    .bind(telmoni_shared::derive_shard_key(user_id))
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// What a bearer resolves to.
#[derive(Debug, sqlx::FromRow)]
pub struct Resolved {
    /// The person it names.
    pub user_id: UserId,
    /// The session it belongs to.
    pub sid: String,
    /// When it stops being taken.
    pub expires_at: DateTime<Utc>,
    /// Its session was ended here, or its person has asked to be deleted.
    pub refused: bool,
}

/// The token `token_hash` names, whatever its state; `None` is a token this
/// service never minted, or one already deleted. One statement, because
/// every person request pays for it: the refusal is read beside the row, so
/// a session ended here or a person on their way out is refused whether or
/// not their tokens have been deleted yet.
pub async fn resolve(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    token_hash: &str,
) -> sqlx::Result<Option<Resolved>> {
    sqlx::query_as::<_, Resolved>(
        "SELECT t.user_id, t.sid, t.expires_at,
                EXISTS (SELECT 1 FROM auth.sessions s
                         WHERE s.provider_sid = t.sid AND s.user_id = t.user_id
                           AND s.revoked_at IS NOT NULL)
             OR EXISTS (SELECT 1 FROM auth.accounts a
                         WHERE a.user_id = t.user_id AND a.deletion_requested_at IS NOT NULL)
                AS refused
           FROM auth.access_tokens t
          WHERE t.token_hash = $1",
    )
    .bind(token_hash)
    .fetch_optional(tx.conn())
    .await
}

/// End a session's bearers: sign-out, a revoke from the sessions page, and
/// a refresh token presented twice.
pub async fn delete_for_sid(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    sid: &str,
) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.access_tokens WHERE sid = $1")
        .bind(sid)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}

/// End every bearer the person holds: a password reset, and the erasure.
pub async fn delete_for_person(tx: &mut Scoped<'_, Person>, user_id: &UserId) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.access_tokens WHERE user_id = $1")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}

/// The retention sweep's share: bearers past their expiry, which nothing
/// takes any more.
pub async fn delete_expired(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.access_tokens WHERE expires_at < now()")
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}
