//! `auth.confirmation_codes` — the email-gated second factor for the
//! irreversible things a person can do to their own account, or to an
//! organization they own.
//!
//! A 6-digit code is hashed at rest, emailed, and CONSUMED in the same
//! transaction as the act it authorizes, so a replay cannot authorize a second.
//! Every code is the PERSON's: it is their inbox that proves it.

use chrono::{DateTime, Utc};
use telmoni_shared::db::tenant_session::{HasPerson, Person, Scoped};
use telmoni_shared::{OrganizationId, UserId};
use uuid::Uuid;

/// What a confirmation code authorizes — the stored `purpose`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Close an organization the person owns: the terminal, cross-service
    /// hard delete.
    OrganizationDeletion,
    /// Erase the person: their identity, their sign-in at the provider, and
    /// every organization they own alone.
    AccountDeletion,
    /// Move the address this account signs in with.
    EmailChange,
    /// Prove the address a built-in account signed up with is the person's:
    /// the link in the first mail.
    EmailVerification,
    /// Choose a new password for a built-in account from its inbox alone.
    PasswordReset,
}

impl Purpose {
    /// The stored spelling, matching the CHECK.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OrganizationDeletion => "organization_deletion",
            Self::AccountDeletion => "account_deletion",
            Self::EmailChange => "email_change",
            Self::EmailVerification => "email_verification",
            Self::PasswordReset => "password_reset",
        }
    }

    /// Every purpose, for the test that holds this enum to the CHECK.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::OrganizationDeletion,
            Self::AccountDeletion,
            Self::EmailChange,
            Self::EmailVerification,
            Self::PasswordReset,
        ]
    }
}

/// One act a code is minted for and spent on: the purpose, and for an
/// organization's deletion, WHICH organization.
///
/// ⚠ **The organization is part of the key.** A person may own several, and a
/// code mailed to delete one must not delete another — the `subject` column
/// is matched on consume, and this type is what makes a caller name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act<'a> {
    /// Delete this organization.
    OrganizationDeletion(&'a OrganizationId),
    /// Delete the person's account.
    AccountDeletion,
    /// Change the person's address.
    EmailChange,
    /// Verify the address a built-in account signed up with.
    EmailVerification,
    /// Reset a built-in account's password.
    PasswordReset,
}

impl<'a> Act<'a> {
    /// The stored purpose.
    #[must_use]
    pub const fn purpose(self) -> Purpose {
        match self {
            Self::OrganizationDeletion(_) => Purpose::OrganizationDeletion,
            Self::AccountDeletion => Purpose::AccountDeletion,
            Self::EmailChange => Purpose::EmailChange,
            Self::EmailVerification => Purpose::EmailVerification,
            Self::PasswordReset => Purpose::PasswordReset,
        }
    }

    /// The organization the act is bound to, when it has one.
    #[must_use]
    pub const fn subject(self) -> Option<&'a OrganizationId> {
        match self {
            Self::OrganizationDeletion(organization) => Some(organization),
            Self::AccountDeletion
            | Self::EmailChange
            | Self::EmailVerification
            | Self::PasswordReset => None,
        }
    }
}

/// TTL for an emailed confirmation code: a narrow window for a leaked code,
/// long enough to paste from an email client.
pub const CODE_TTL_MINUTES: i64 = 15;

/// SHA-256 hex of a code; the plaintext is emailed, never stored.
#[must_use]
pub fn hash_code(code: &str) -> String {
    telmoni_shared::digest::sha256_hex(code.as_bytes())
}

/// A 6-digit numeric confirmation code from CSPRNG bytes. Every purpose mints
/// here: a second generator is a second chance to reach for a weaker source.
#[must_use]
pub fn generate_code() -> String {
    let b = Uuid::new_v4().into_bytes();
    let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) % 1_000_000;
    format!("{n:06}")
}

/// Insert a live confirmation code (hashed), burning the person's earlier
/// live code of the same purpose so exactly one is ever live to count guesses
/// against. Under the person's lock: two requests at once would otherwise
/// both burn and both insert, and `confirmation_codes_one_live_key` would
/// turn the second into a 500 rather than a second budget of guesses.
///
/// Also the person's own retention: nothing else prunes this table, and a
/// spent code is kept only as long as the daily send cap counts it.
pub async fn create(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    act: Act<'_>,
    code_hash: &str,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    crate::db::locks::lock_person(&mut *tx, user_id).await?;

    sqlx::query(
        "DELETE FROM auth.confirmation_codes
          WHERE user_id = $1 AND created_at < now() - interval '2 days'",
    )
    .bind(user_id)
    .execute(tx.conn())
    .await?;

    sqlx::query(
        "UPDATE auth.confirmation_codes SET consumed_at = now()
          WHERE user_id = $1 AND purpose = $2 AND consumed_at IS NULL",
    )
    .bind(user_id)
    .bind(act.purpose().as_str())
    .execute(tx.conn())
    .await?;

    sqlx::query(
        "INSERT INTO auth.confirmation_codes
             (id, user_id, purpose, subject, code_hash, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(act.purpose().as_str())
    .bind(act.subject())
    .bind(code_hash)
    .bind(expires_at)
    .bind(telmoni_shared::derive_shard_key(user_id))
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Resolve a live (unconsumed, unexpired) code, returning its id. The purpose
/// AND the subject are in the predicate, not checked afterwards: a table
/// serving several acts that matched on the hash alone would hand each one the
/// others' keys.
pub async fn find_live<B: HasPerson>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    act: Act<'_>,
    code_hash: &str,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM auth.confirmation_codes
          WHERE user_id = $1 AND purpose = $2
            AND subject IS NOT DISTINCT FROM $3
            AND code_hash = $4
            AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(user_id)
    .bind(act.purpose().as_str())
    .bind(act.subject())
    .bind(code_hash)
    .fetch_optional(tx.conn())
    .await
}

/// Mark a code consumed, guarded on liveness so a double submit cannot
/// re-consume. `false` when nothing transitioned.
pub async fn consume<B: HasPerson>(tx: &mut Scoped<'_, B>, id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.confirmation_codes SET consumed_at = now()
          WHERE id = $1 AND consumed_at IS NULL",
    )
    .bind(id)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Drop every code row the person holds, of every purpose. ⚠ A confirmed
/// email change calls it as part of the act: it also kills a live DELETION
/// code mailed to the inbox the person is leaving, which could otherwise still
/// authorize an irreversible cascade.
pub async fn delete_all_for_person(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<u64> {
    let result = sqlx::query("DELETE FROM auth.confirmation_codes WHERE user_id = $1")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
    Ok(result.rows_affected())
}

/// How many wrong guesses a single code survives before it is burned.
pub const MAX_ATTEMPTS: i32 = 5;

/// Record a wrong guess against this person's live code of `purpose`, and burn
/// the code after [`MAX_ATTEMPTS`]. It counts against the ROW, which works
/// because only one code per purpose is ever live. Burning rather than
/// locking: a lock would be a denial an attacker could aim at somebody else's
/// account.
pub async fn register_failure<B: HasPerson>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
    purpose: Purpose,
) -> sqlx::Result<bool> {
    // One statement: the burn lands on the row the count did, which the
    // unique live-code index makes the only one there is.
    let burned: Option<bool> = sqlx::query_scalar(
        "UPDATE auth.confirmation_codes
            SET attempts = attempts + 1,
                consumed_at = CASE WHEN attempts + 1 >= $3 THEN now() END
          WHERE user_id = $1 AND purpose = $2
            AND consumed_at IS NULL AND expires_at > now()
      RETURNING attempts >= $3",
    )
    .bind(user_id)
    .bind(purpose.as_str())
    .bind(MAX_ATTEMPTS)
    .fetch_optional(tx.conn())
    .await?;
    Ok(burned.unwrap_or(false))
}

/// How many codes of one purpose this person has been issued since `since`.
/// ⚠ Counts ISSUES, not failures: a lane that mails an address the caller
/// typed is a relay pointed at strangers unless something caps it.
pub async fn count_since(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
    purpose: Purpose,
    since: DateTime<Utc>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM auth.confirmation_codes
          WHERE user_id = $1 AND purpose = $2 AND created_at > $3",
    )
    .bind(user_id)
    .bind(purpose.as_str())
    .bind(since)
    .fetch_one(tx.conn())
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use sqlx::PgPool;
    use telmoni_shared::db::tenant_session::{organization_scope, person_scope};

    fn person(s: &str) -> UserId {
        UserId::try_new(s).expect("valid test user id")
    }

    fn organization(s: &str) -> OrganizationId {
        OrganizationId::try_new(s).expect("valid test organization id")
    }

    fn any_slug() -> String {
        telmoni_shared::slug::placeholder(telmoni_shared::slug::Scope::Organization)
    }

    async fn seed(pool: &PgPool, user: &str) {
        telmoni_shared::test_util::seed_identity(pool, user, &format!("{user}@example.com")).await;
    }

    fn live_for(minutes: i64) -> DateTime<Utc> {
        Utc::now() + ChronoDuration::minutes(minutes)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_find_consume_roundtrip(pool: PgPool) -> sqlx::Result<()> {
        seed(&pool, "user_del").await;
        let mut tx = person_scope(&pool, &person("user_del")).await?;
        create(
            &mut tx,
            &person("user_del"),
            Act::AccountDeletion,
            "hash-1",
            live_for(15),
        )
        .await?;

        let id = find_live(&mut tx, &person("user_del"), Act::AccountDeletion, "hash-1")
            .await?
            .expect("live code resolves");
        assert!(consume(&mut tx, id).await?, "first consume transitions");
        assert!(!consume(&mut tx, id).await?, "second consume is a no-op");
        assert!(
            find_live(&mut tx, &person("user_del"), Act::AccountDeletion, "hash-1")
                .await?
                .is_none(),
            "a consumed code is not live"
        );
        tx.commit().await?;
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn wrong_hash_and_expired_are_not_live(pool: PgPool) -> sqlx::Result<()> {
        seed(&pool, "user_del").await;
        let mut tx = person_scope(&pool, &person("user_del")).await?;
        create(
            &mut tx,
            &person("user_del"),
            Act::EmailChange,
            "hash-expired",
            Utc::now() - ChronoDuration::seconds(1),
        )
        .await?;
        assert!(
            find_live(
                &mut tx,
                &person("user_del"),
                Act::EmailChange,
                "hash-expired"
            )
            .await?
            .is_none(),
            "an expired code is not live"
        );

        create(
            &mut tx,
            &person("user_del"),
            Act::EmailChange,
            "hash-live",
            live_for(15),
        )
        .await?;
        assert!(
            find_live(&mut tx, &person("user_del"), Act::EmailChange, "wrong")
                .await?
                .is_none(),
            "a wrong hash misses"
        );
        assert!(
            find_live(&mut tx, &person("user_del"), Act::EmailChange, "hash-live")
                .await?
                .is_some(),
            "the right hash resolves"
        );
        tx.commit().await?;
        Ok(())
    }

    /// ⚠ A code minted to delete one organization must not delete another the
    /// same person owns: the organization is matched, not just the purpose.
    #[sqlx::test(migrations = "./migrations")]
    async fn an_organization_deletion_code_names_its_organization(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        seed(&pool, "user_owner").await;
        for org in ["org_first", "org_second"] {
            let mut otx = organization_scope(&pool, &organization(org)).await?;
            crate::db::organizations::create(&mut otx, &organization(org), "Acme", &any_slug())
                .await?;
            otx.commit().await?;
        }
        let mut tx = person_scope(&pool, &person("user_owner")).await?;
        create(
            &mut tx,
            &person("user_owner"),
            Act::OrganizationDeletion(&organization("org_first")),
            "hash-first",
            live_for(15),
        )
        .await?;
        assert!(
            find_live(
                &mut tx,
                &person("user_owner"),
                Act::OrganizationDeletion(&organization("org_second")),
                "hash-first",
            )
            .await?
            .is_none(),
            "the code for one organization deletes no other"
        );
        assert!(
            find_live(
                &mut tx,
                &person("user_owner"),
                Act::OrganizationDeletion(&organization("org_first")),
                "hash-first",
            )
            .await?
            .is_some()
        );
        tx.commit().await?;
        Ok(())
    }

    /// A live organization-deletion code goes with its organization, and every
    /// code goes with its person. Postgres runs cascades bypassing row
    /// security, so this holds for the production role, not just the
    /// superuser test role.
    #[sqlx::test(migrations = "./migrations")]
    async fn codes_cascade_with_their_organization_and_their_person(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        seed(&pool, "user_casc").await;
        {
            let mut otx = organization_scope(&pool, &organization("org_casc")).await?;
            crate::db::organizations::create(
                &mut otx,
                &organization("org_casc"),
                "Acme",
                &any_slug(),
            )
            .await?;
            otx.commit().await?;
        }
        {
            let mut tx = person_scope(&pool, &person("user_casc")).await?;
            create(
                &mut tx,
                &person("user_casc"),
                Act::OrganizationDeletion(&organization("org_casc")),
                "hash-casc",
                live_for(15),
            )
            .await?;
            tx.commit().await?;
        }
        sqlx::query("DELETE FROM auth.organizations WHERE external_id = $1")
            .bind(organization("org_casc"))
            .execute(&pool)
            .await?;
        let remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM auth.confirmation_codes WHERE user_id = $1")
                .bind(person("user_casc"))
                .fetch_one(&pool)
                .await?;
        assert_eq!(remaining, 0, "the organization's deletion took its code");

        {
            let mut tx = person_scope(&pool, &person("user_casc")).await?;
            create(
                &mut tx,
                &person("user_casc"),
                Act::EmailChange,
                "hash-email",
                live_for(15),
            )
            .await?;
            tx.commit().await?;
        }
        sqlx::query("DELETE FROM auth.identities WHERE user_id = $1")
            .bind(person("user_casc"))
            .execute(&pool)
            .await?;
        let remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM auth.confirmation_codes WHERE user_id = $1")
                .bind(person("user_casc"))
                .fetch_one(&pool)
                .await?;
        assert_eq!(remaining, 0, "the person's erasure took their codes");
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn delete_all_clears_the_person(pool: PgPool) -> sqlx::Result<()> {
        seed(&pool, "user_del").await;
        let mut otx = organization_scope(&pool, &organization("org_del")).await?;
        crate::db::organizations::create(&mut otx, &organization("org_del"), "Acme", &any_slug())
            .await?;
        otx.commit().await?;
        let mut tx = person_scope(&pool, &person("user_del")).await?;
        create(
            &mut tx,
            &person("user_del"),
            Act::OrganizationDeletion(&organization("org_del")),
            "h1",
            live_for(15),
        )
        .await?;
        create(
            &mut tx,
            &person("user_del"),
            Act::EmailChange,
            "h2",
            live_for(15),
        )
        .await?;
        assert_eq!(
            delete_all_for_person(&mut tx, &person("user_del")).await?,
            2
        );
        assert!(
            find_live(
                &mut tx,
                &person("user_del"),
                Act::OrganizationDeletion(&organization("org_del")),
                "h1",
            )
            .await?
            .is_none()
        );
        tx.commit().await?;
        Ok(())
    }
}
