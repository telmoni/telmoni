//! `auth.device_codes` — a device authorization (RFC 8628) the built-in
//! provider is holding for the CLI: nobody's row until a person approves
//! its code in the console, so every lane on it is the maintenance lane.

use chrono::{DateTime, Utc};
use telmoni_shared::UserId;
use telmoni_shared::db::tenant_session::{Maintenance, Scoped};
use uuid::Uuid;

use crate::db::AuthLane;

/// Where a device authorization stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Waiting for the person.
    Pending,
    /// Approved: the next poll is granted.
    Approved,
    /// Refused: the next poll is told so.
    Denied,
}

impl Status {
    fn from_column(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            _ => None,
        }
    }
}

/// One authorization, as a poll reads it.
#[derive(Debug)]
pub struct Device {
    pub id: Uuid,
    pub status: Status,
    /// The person who approved it, with the session minted for the device.
    pub approved: Option<(UserId, String)>,
    pub interval_secs: i32,
    pub last_polled_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    status: String,
    approved_by: Option<UserId>,
    sid: Option<String>,
    interval_secs: i32,
    last_polled_at: Option<DateTime<Utc>>,
    expires_at: DateTime<Utc>,
}

/// Open an authorization for a device that just asked.
pub async fn create(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    device_code_hash: &str,
    user_code: &str,
    interval_secs: i32,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO auth.device_codes
             (id, device_code_hash, user_code, interval_secs, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(device_code_hash)
    .bind(user_code)
    .bind(interval_secs)
    .bind(expires_at)
    .bind(Uuid::new_v4())
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The authorization a device is polling for, locked for the poll's
/// decision. `None` is a device code this service never minted.
pub async fn find_for_poll(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    device_code_hash: &str,
) -> sqlx::Result<Option<Device>> {
    let row = sqlx::query_as::<_, Row>(
        "SELECT id, status, approved_by, sid, interval_secs, last_polled_at, expires_at
           FROM auth.device_codes WHERE device_code_hash = $1
            FOR UPDATE",
    )
    .bind(device_code_hash)
    .fetch_optional(tx.conn())
    .await?;
    Ok(row.map(|r| Device {
        id: r.id,
        // The CHECK admits the three spellings and nothing else, so an
        // unreadable one is a row nothing wrote; treated as still waiting.
        status: Status::from_column(&r.status).unwrap_or(Status::Pending),
        approved: r.approved_by.zip(r.sid),
        interval_secs: r.interval_secs,
        last_polled_at: r.last_polled_at,
        expires_at: r.expires_at,
    }))
}

/// Note a poll, for the interval the next one is held to.
pub async fn touch(tx: &mut Scoped<'_, Maintenance<AuthLane>>, id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE auth.device_codes SET last_polled_at = now() WHERE id = $1")
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// The authorization is finished, whichever way: granted, refused or lapsed.
pub async fn delete(tx: &mut Scoped<'_, Maintenance<AuthLane>>, id: Uuid) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM auth.device_codes WHERE id = $1")
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// The person approves the device showing `user_code`, and the device gets
/// `sid` as its session. `false` when no device is waiting for that code.
pub async fn approve(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_code: &str,
    user_id: &UserId,
    sid: &str,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.device_codes
            SET status = 'approved', approved_by = $2, sid = $3
          WHERE user_code = $1 AND status = 'pending' AND expires_at > now()",
    )
    .bind(user_code)
    .bind(user_id)
    .bind(sid)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The person refuses the device showing `user_code`.
pub async fn deny(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_code: &str,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.device_codes SET status = 'denied'
          WHERE user_code = $1 AND status = 'pending' AND expires_at > now()",
    )
    .bind(user_code)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The retention sweep's share: authorizations nobody finished.
pub async fn delete_expired(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.device_codes WHERE expires_at < now()")
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}
