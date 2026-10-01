//! `auth.external_identities` — which person here an external provider's
//! subject is. The provider's own name for them (`sub`) is the one thing a
//! provider promises to keep stable, so it is the key; an address is what
//! the provider asserts about them and may change.

use telmoni_shared::UserId;
use telmoni_shared::db::tenant_session::{Maintenance, Person, Scoped};

use crate::db::AuthLane;

/// The person an external subject was linked to, if any. The exchange lane's
/// read: no person is known before it, so it runs in the maintenance lane.
pub async fn find(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    provider: &str,
    subject: &str,
) -> sqlx::Result<Option<UserId>> {
    sqlx::query_scalar(
        "SELECT user_id FROM auth.external_identities WHERE provider = $1 AND subject = $2",
    )
    .bind(provider)
    .bind(subject)
    .fetch_optional(tx.conn())
    .await
}

/// Link a subject to the person, as them. `false` when the subject is
/// already somebody's: two first sign-ins racing, and the other one won.
pub async fn link(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    provider: &str,
    subject: &str,
) -> sqlx::Result<bool> {
    let inserted = sqlx::query(
        "INSERT INTO auth.external_identities (user_id, provider, subject, shard_key)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(provider)
    .bind(subject)
    .bind(telmoni_shared::derive_shard_key(user_id))
    .execute(tx.conn())
    .await;
    match inserted {
        Ok(_) => Ok(true),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(false),
        Err(e) => Err(e),
    }
}

/// The provider and subject a person signs in through, when they do.
pub async fn for_person(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<Option<(String, String)>> {
    sqlx::query_as(
        "SELECT provider, subject FROM auth.external_identities
          WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}
