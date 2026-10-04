//! `auth.identities` — the person. What the identity provider last asserted
//! about them (written only with values the provider handed this service at a
//! sign-in's code exchange, never from a request body; a refresh asks the
//! provider nothing), plus the few facts that are theirs and no
//! organization's: analytics consent, and a pending deletion.

use chrono::{DateTime, Utc};
use telmoni_shared::UserId;
use telmoni_shared::db::tenant_session::{
    self, Binding, HasOrganization, Maintenance, PersonAndOrganization, Scoped,
};

use crate::db::AuthLane;

/// Bindings a person's own row is read under: theirs, theirs with the
/// organization they are acting on, and the maintenance lane the invite
/// accepts run in. The marker is spelled `tenant_session::Person` here, since
/// [`Person`] is the row.
pub trait IdentityRead: Binding {}
impl IdentityRead for tenant_session::Person {}
impl IdentityRead for PersonAndOrganization {}
impl IdentityRead for Maintenance<AuthLane> {}

/// One person, as the provider describes them.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Identity {
    pub email: String,
    pub email_verified: bool,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

impl Identity {
    /// The provider's name for this person, sanitised, or `None`.
    #[must_use]
    pub fn display_name(&self) -> Option<String> {
        display_name(self.first_name.as_deref(), self.last_name.as_deref())
    }

    /// The address, normalised the way `auth.identities.email` stores it.
    #[must_use]
    pub fn normalized_email(&self) -> String {
        normalize_email(&self.email)
    }
}

/// The address as every writer stores it: trimmed and lowercased, which the
/// column's CHECK holds them to.
#[must_use]
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// A first and last name joined and sanitised, or `None` when neither says
/// anything.
#[must_use]
pub fn display_name(first_name: Option<&str>, last_name: Option<&str>) -> Option<String> {
    let joined = [first_name, last_name]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    crate::identity::sanitize_display_name(Some(&joined))
}

/// A person as `/me` reads them: the provider's half (`auth.identities`) and
/// theirs (`auth.accounts`, defaults when they have no row).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Person {
    pub email: String,
    pub email_verified: bool,
    /// Already sanitised by the writer.
    pub display_name: Option<String>,
    pub analytics_opt_in: bool,
    pub deletion_requested_at: Option<DateTime<Utc>>,
}

/// Record what the provider just asserted, replacing what it said before.
///
/// ⚠ **Never touches `auth.accounts`.** Consent and a pending deletion are the
/// person's answers, not the provider's, and an upsert that named them would
/// reset them at every sign-in.
pub async fn record(
    tx: &mut Scoped<'_, tenant_session::Person>,
    user_id: &UserId,
    identity: &Identity,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO auth.identities
             (user_id, email, email_verified, first_name, last_name, display_name, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (user_id) DO UPDATE SET
             email = excluded.email,
             -- Proof of an address is kept: a provider that sends no claim at
             -- a later sign-in does not unprove what a link or an email
             -- change proved. A new address starts from the provider's word.
             email_verified = CASE
                 WHEN auth.identities.email = excluded.email
                     THEN auth.identities.email_verified OR excluded.email_verified
                 ELSE excluded.email_verified
             END,
             first_name = excluded.first_name,
             last_name = excluded.last_name,
             display_name = excluded.display_name,
             updated_at = now()",
    )
    .bind(user_id)
    .bind(identity.normalized_email())
    .bind(identity.email_verified)
    .bind(identity.first_name.as_deref())
    .bind(identity.last_name.as_deref())
    .bind(identity.display_name())
    .bind(telmoni_shared::derive_shard_key(user_id))
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// What the provider asserted, as it was recorded: the built-in provider
/// reads it back to mint the bearer it then asserts.
pub async fn identity<B: IdentityRead>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
) -> sqlx::Result<Option<Identity>> {
    sqlx::query_as(
        "SELECT email, email_verified, first_name, last_name
           FROM auth.identities WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// The person who holds `email`, for the built-in provider's sign-in and
/// sign-up: the only reads keyed on an address rather than a person, which
/// is why they run in the maintenance lane. Newest first, because an
/// address an external provider once recorded against another subject may
/// still be here beside the built-in account that holds it now.
pub async fn find_by_email(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    email: &str,
) -> sqlx::Result<Option<UserId>> {
    sqlx::query_scalar(
        "SELECT user_id FROM auth.identities WHERE email = $1
          ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(normalize_email(email))
    .fetch_optional(tx.conn())
    .await
}

/// Record that the person proved the address is theirs: the built-in
/// provider's verification link, or a password reset from the same inbox.
pub async fn set_verified(
    tx: &mut Scoped<'_, tenant_session::Person>,
    user_id: &UserId,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE auth.identities SET email_verified = true, updated_at = now()
          WHERE user_id = $1 AND NOT email_verified",
    )
    .bind(user_id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The person, or `None` when no sign-in's exchange has recorded them.
pub async fn get<B: IdentityRead>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
) -> sqlx::Result<Option<Person>> {
    sqlx::query_as(
        "SELECT i.email, i.email_verified, i.display_name,
                COALESCE(a.analytics_opt_in, false) AS analytics_opt_in,
                a.deletion_requested_at
           FROM auth.identities i
           LEFT JOIN auth.accounts a ON a.user_id = i.user_id
          WHERE i.user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// How to reach and name a person: their address and display name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Contact {
    pub email: String,
    pub display_name: Option<String>,
}

impl Contact {
    /// What a page or a mail prints for them.
    #[must_use]
    pub fn display(&self) -> String {
        crate::identity::display_for(self.display_name.as_deref(), &self.email)
    }
}

/// A person's address and name, read by an organization whose roster they are
/// on.
pub async fn contact<B: HasOrganization>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
) -> sqlx::Result<Option<Contact>> {
    sqlx::query_as("SELECT email, display_name FROM auth.identities WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(tx.conn())
        .await
}

/// Move the recorded address to one the provider has just confirmed, in the
/// email-change transaction. `verified` is the provider's word, not ours: the
/// next sign-in's exchange writes its answer anyway, and recording anything
/// else would only postpone `/me`'s refusal to that sign-in.
pub async fn set_email(
    tx: &mut Scoped<'_, tenant_session::Person>,
    user_id: &UserId,
    email: &str,
    verified: bool,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE auth.identities SET email = $2, email_verified = $3, updated_at = now()
          WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(normalize_email(email))
    .bind(verified)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Record whether this person wants to be counted in product analytics.
/// `false` when there is no such person, or they are on their way out.
pub async fn set_analytics_opt_in(
    tx: &mut Scoped<'_, tenant_session::Person>,
    user_id: &UserId,
    opt_in: bool,
) -> sqlx::Result<bool> {
    // Selected from the identity so a person nobody recorded writes nothing
    // (rather than failing the foreign key); the conflict arm refuses a
    // person who is leaving.
    let done = sqlx::query(
        "INSERT INTO auth.accounts
             (user_id, analytics_opt_in, shard_key)
         SELECT i.user_id, $2, i.shard_key FROM auth.identities i WHERE i.user_id = $1
         ON CONFLICT (user_id) DO UPDATE
            SET analytics_opt_in = excluded.analytics_opt_in, updated_at = now()
          WHERE auth.accounts.deletion_requested_at IS NULL",
    )
    .bind(user_id)
    .bind(opt_in)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Mark the person as leaving. `false` when they already are.
pub async fn mark_pending_deletion(
    tx: &mut Scoped<'_, tenant_session::Person>,
    user_id: &UserId,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "INSERT INTO auth.accounts
             (user_id, deletion_requested_at, shard_key)
         SELECT i.user_id, now(), i.shard_key FROM auth.identities i WHERE i.user_id = $1
         ON CONFLICT (user_id) DO UPDATE
            SET deletion_requested_at = now(), updated_at = now()
          WHERE auth.accounts.deletion_requested_at IS NULL",
    )
    .bind(user_id)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The people whose deletion was confirmed and is not finished, oldest first
/// — the deletion sweep's list.
///
/// Not someone who still owns an organization being deleted: their erasure
/// waits for its row, which waits out the finalize grace window, and listing
/// them would fail every tick in between. Someone who owns an ACTIVE one is
/// listed — that is a broken invariant, and the erasure says so.
pub async fn list_pending_deletion(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    limit: i64,
) -> sqlx::Result<Vec<(UserId, DateTime<Utc>)>> {
    sqlx::query_as(
        "SELECT a.user_id, a.deletion_requested_at
           FROM auth.accounts a
          WHERE a.deletion_requested_at IS NOT NULL
            AND NOT EXISTS (
                SELECT 1 FROM auth.organization_members m
                  JOIN auth.organizations o ON o.external_id = m.organization_id
                 WHERE m.user_id = a.user_id AND m.role = 'owner'
                   AND o.status = 'pending_deletion')
          ORDER BY a.deletion_requested_at ASC, a.user_id ASC
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(tx.conn())
    .await
}

/// The erasure's last step. Sessions and codes cascade; a membership left
/// behind makes this fail (RESTRICT), which is the point.
pub async fn delete(
    tx: &mut Scoped<'_, tenant_session::Person>,
    user_id: &UserId,
) -> sqlx::Result<bool> {
    let done = sqlx::query("DELETE FROM auth.identities WHERE user_id = $1")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() == 1)
}
