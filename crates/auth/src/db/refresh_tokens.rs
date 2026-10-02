//! `auth.refresh_tokens` — the issuer's refresh grants, one row per token.
//! A refresh spends the row and mints the next for the same session; a spent
//! row presented again is a token somebody copied, and ends every token of
//! that session.

use chrono::{DateTime, Utc};
use telmoni_shared::UserId;
use telmoni_shared::db::tenant_session::{Binding, Maintenance, Person, Scoped};
use uuid::Uuid;

use crate::db::AuthLane;

/// How long a spent row is kept, for the reuse check. Longer than any
/// client's retry, shorter than the table is worth.
const SPENT_RETENTION: &str = "1 day";

/// A token spent this recently may be presented again, and earns another
/// token of the same session rather than ending it. Two tabs, or a page and
/// its heartbeat, refresh from one cookie at once; without this the second
/// would read as a copy and sign them both out. A copied token replayed
/// inside the window goes undetected, which is the trade every rotation
/// scheme with a reuse interval makes, and thirty seconds is a short one.
const REUSE_GRACE_SECS: i64 = 30;

/// Write a token. Under the person at sign-in and under the lane at refresh,
/// which carries the old token and no person.
pub async fn create<B: Binding>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    sid: &str,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO auth.refresh_tokens (id, user_id, sid, token_hash, expires_at, shard_key)
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

/// What presenting a token earned.
#[derive(Debug)]
pub enum Spent {
    /// A live token, now spent, or one spent within the last few seconds:
    /// the session goes on with a new one.
    Live { user_id: UserId, sid: String },
    /// A token spent longer ago than that. Every token of its session, its
    /// bearers included, is gone now.
    Reused,
    /// Past its expiry; gone.
    Expired,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    user_id: UserId,
    sid: String,
    used_at: Option<DateTime<Utc>>,
    expires_at: DateTime<Utc>,
}

/// Spend the token `token_hash` names, at `now`. `None` is a token this
/// service never minted, or one the sweep has forgotten.
pub async fn spend(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    token_hash: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<Spent>> {
    let Some(row) = sqlx::query_as::<_, Row>(
        "SELECT id, user_id, sid, used_at, expires_at
           FROM auth.refresh_tokens WHERE token_hash = $1
            FOR UPDATE",
    )
    .bind(token_hash)
    .fetch_optional(tx.conn())
    .await?
    else {
        return Ok(None);
    };
    if let Some(used_at) = row.used_at {
        if used_at + chrono::Duration::seconds(REUSE_GRACE_SECS) > now {
            return Ok(Some(Spent::Live {
                user_id: row.user_id,
                sid: row.sid,
            }));
        }
        revoke_sid(tx, &row.sid).await?;
        crate::db::access_tokens::delete_for_sid(tx, &row.sid).await?;
        // The session itself ends, not only its tokens: a request resolved
        // before the reuse asks the session, not a token, whether it still
        // stands (`seam::Auth::resolve_again`), and the sessions page shows it.
        crate::db::sessions::revoke_by_sid(tx, &row.sid).await?;
        return Ok(Some(Spent::Reused));
    }
    if row.expires_at <= now {
        sqlx::query("DELETE FROM auth.refresh_tokens WHERE id = $1")
            .bind(row.id)
            .execute(tx.conn())
            .await?;
        return Ok(Some(Spent::Expired));
    }
    sqlx::query("UPDATE auth.refresh_tokens SET used_at = now() WHERE id = $1")
        .bind(row.id)
        .execute(tx.conn())
        .await?;
    Ok(Some(Spent::Live {
        user_id: row.user_id,
        sid: row.sid,
    }))
}

/// End a session's tokens: sign-out, and a reuse.
pub async fn revoke_sid(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    sid: &str,
) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.refresh_tokens WHERE sid = $1")
        .bind(sid)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}

/// End every token the person holds: a password reset, and the erasure.
pub async fn delete_for_person(tx: &mut Scoped<'_, Person>, user_id: &UserId) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.refresh_tokens WHERE user_id = $1")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}

/// The retention sweep's share: expired tokens, and spent ones past the
/// reuse window.
pub async fn delete_stale(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "DELETE FROM auth.refresh_tokens
          WHERE expires_at < now() OR used_at < now() - $1::interval",
    )
    .bind(SPENT_RETENTION)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}
