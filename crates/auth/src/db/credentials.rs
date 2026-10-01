//! `auth.credentials` — the built-in provider's half of a person: the
//! password they sign in with, the guard against guessing it, and an address
//! change waiting on the code mailed to the new address.
//!
//! Every read and write is the PERSON's. Sign-in finds the person by address
//! through `auth.identities` first, so nothing here is ever looked up by
//! anything but `user_id`.

use chrono::{DateTime, Duration, Utc};
use telmoni_shared::UserId;
use telmoni_shared::db::tenant_session::{Person, Scoped};

/// Wrong passwords in a row before the account stops answering for a while:
/// Grafana's `brute_force_login_protection_max_attempts`.
pub const LOCKOUT_AFTER: i32 = 5;

/// How long a locked account stops answering: the window Grafana counts
/// attempts in. Short, because a lockout is something anybody who knows the
/// address can cause.
pub const LOCKOUT_MINUTES: i64 = 5;

/// Wrong codes a pending address change survives before it is dropped.
pub const PENDING_EMAIL_MAX_ATTEMPTS: i32 = 5;

/// What sign-in reads.
#[derive(Clone, sqlx::FromRow)]
pub struct Credential {
    /// The PHC string Argon2id verifies against.
    pub password_hash: String,
    pub failed_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
}

/// The hash is masked: not a secret in the sense a token is, and still
/// nothing a log line should carry.
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("password_hash", &"***")
            .field("failed_attempts", &self.failed_attempts)
            .field("locked_until", &self.locked_until)
            .finish()
    }
}

/// An address change the provider mailed a code for and is waiting on.
#[derive(Clone, sqlx::FromRow)]
pub struct PendingEmail {
    pub email: String,
    pub code_hash: String,
    pub expires_at: DateTime<Utc>,
    pub attempts: i32,
}

impl std::fmt::Debug for PendingEmail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingEmail")
            .field("expires_at", &self.expires_at)
            .field("attempts", &self.attempts)
            .finish_non_exhaustive()
    }
}

/// Write a new account's password. `false` when the address already belongs
/// to a built-in account: the unique index is what decides a race between
/// two sign-ups, and this is how it answers.
pub async fn create(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    email: &str,
    password_hash: &str,
) -> sqlx::Result<bool> {
    let inserted = sqlx::query(
        "INSERT INTO auth.credentials (user_id, email, password_hash, shard_key)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(email)
    .bind(password_hash)
    .bind(telmoni_shared::derive_shard_key(user_id))
    .execute(tx.conn())
    .await;
    match inserted {
        Ok(_) => Ok(true),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(false),
        Err(e) => Err(e),
    }
}

/// The person's password row, or `None` for somebody who signs in another way.
pub async fn get(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<Option<Credential>> {
    sqlx::query_as(
        "SELECT password_hash, failed_attempts, locked_until
           FROM auth.credentials WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// Count a wrong password. The [`LOCKOUT_AFTER`]th in a row locks the account
/// for [`LOCKOUT_MINUTES`] and starts the count again; the answer is when the
/// lock lifts, when this guess set one.
pub async fn register_failure(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<Option<DateTime<Utc>>> {
    let lifts_at = Utc::now() + Duration::minutes(LOCKOUT_MINUTES);
    let locked: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(
        "UPDATE auth.credentials
            SET failed_attempts = CASE WHEN failed_attempts + 1 >= $2 THEN 0
                                       ELSE failed_attempts + 1 END,
                locked_until    = CASE WHEN failed_attempts + 1 >= $2 THEN $3
                                       ELSE locked_until END,
                updated_at      = now()
          WHERE user_id = $1
      RETURNING CASE WHEN failed_attempts = 0 THEN locked_until END",
    )
    .bind(user_id)
    .bind(LOCKOUT_AFTER)
    .bind(lifts_at)
    .fetch_optional(tx.conn())
    .await?;
    Ok(locked.flatten())
}

/// A right password ends the count and any lock.
pub async fn clear_failures(tx: &mut Scoped<'_, Person>, user_id: &UserId) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE auth.credentials
            SET failed_attempts = 0, locked_until = NULL, updated_at = now()
          WHERE user_id = $1 AND (failed_attempts <> 0 OR locked_until IS NOT NULL)",
    )
    .bind(user_id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Replace the password, and with it any lock: a reset from the inbox is
/// stronger proof than the guesses that set the lock.
pub async fn set_password(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    password_hash: &str,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.credentials
            SET password_hash = $2, failed_attempts = 0, locked_until = NULL, updated_at = now()
          WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(password_hash)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Open an address change: the address a code was mailed to, and the code's
/// hash. Replaces any change already pending. `false` when the person has no
/// password row.
pub async fn set_pending_email(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    new_email: &str,
    code_hash: &str,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.credentials
            SET pending_email = $2, pending_email_code_hash = $3,
                pending_email_expires_at = $4, pending_email_attempts = 0,
                updated_at = now()
          WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(new_email)
    .bind(code_hash)
    .bind(expires_at)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The change waiting on its code, if one is.
pub async fn pending_email(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<Option<PendingEmail>> {
    sqlx::query_as(
        "SELECT pending_email AS email, pending_email_code_hash AS code_hash,
                pending_email_expires_at AS expires_at, pending_email_attempts AS attempts
           FROM auth.credentials
          WHERE user_id = $1 AND pending_email IS NOT NULL",
    )
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// Count a wrong code against the pending change, and drop the change after
/// [`PENDING_EMAIL_MAX_ATTEMPTS`]. `true` when this guess dropped it.
pub async fn register_pending_email_failure(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<bool> {
    let burned: Option<bool> = sqlx::query_scalar(
        "UPDATE auth.credentials
            SET pending_email_attempts = pending_email_attempts + 1,
                pending_email = CASE WHEN pending_email_attempts + 1 >= $2 THEN NULL
                                     ELSE pending_email END,
                pending_email_code_hash = CASE WHEN pending_email_attempts + 1 >= $2 THEN NULL
                                               ELSE pending_email_code_hash END,
                pending_email_expires_at = CASE WHEN pending_email_attempts + 1 >= $2 THEN NULL
                                                ELSE pending_email_expires_at END,
                updated_at = now()
          WHERE user_id = $1 AND pending_email IS NOT NULL
      RETURNING pending_email IS NULL",
    )
    .bind(user_id)
    .bind(PENDING_EMAIL_MAX_ATTEMPTS)
    .fetch_optional(tx.conn())
    .await?;
    Ok(burned.unwrap_or(false))
}

/// Drop a pending change without taking it: it expired, or the person
/// finished something that supersedes it.
pub async fn clear_pending_email(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE auth.credentials
            SET pending_email = NULL, pending_email_code_hash = NULL,
                pending_email_expires_at = NULL, pending_email_attempts = 0,
                updated_at = now()
          WHERE user_id = $1 AND pending_email IS NOT NULL",
    )
    .bind(user_id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Take the pending address as the account's. `false` when a built-in
/// account took it in the meantime: the unique index refuses, and the
/// pending change is left for the person to see the refusal.
pub async fn take_pending_email(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<bool> {
    let taken = sqlx::query(
        "UPDATE auth.credentials
            SET email = pending_email, pending_email = NULL, pending_email_code_hash = NULL,
                pending_email_expires_at = NULL, pending_email_attempts = 0,
                updated_at = now()
          WHERE user_id = $1 AND pending_email IS NOT NULL",
    )
    .bind(user_id)
    .execute(tx.conn())
    .await;
    match taken {
        Ok(done) => Ok(done.rows_affected() == 1),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(false),
        Err(e) => Err(e),
    }
}

/// The erasure's step for this table. Cascades from the identity too; this
/// is the provider answering for its own rows.
pub async fn delete(tx: &mut Scoped<'_, Person>, user_id: &UserId) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.credentials WHERE user_id = $1")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}
