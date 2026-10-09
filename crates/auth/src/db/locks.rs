//! Transaction-scoped advisory locks that serialise the lanes which decide who
//! is in an organization and who owns it.
//!
//! **Why locks and not the rows.** Under READ COMMITTED an UPDATE or DELETE
//! that waited on a row lock re-checks its WHERE against the row's NEW version,
//! so a removal blocked behind an ownership transfer would go on to act on the
//! person as the new owner; and a lane that read "you are the owner" and then
//! acted could act for somebody who stopped being the owner in between. The
//! lanes that change the roster or its ownership — roles, removal and leaving,
//! both invite accepts, every ownership lane, organization and account deletion
//! — take the organization's lock first and read the roles again under it.
//! The owner's and admins' lanes that change something else (renames,
//! projects, invites out) read the role unlocked, as a sibling's `resolve`
//! does: a transfer racing one of them lets the owner of a moment ago finish.
//! `auth.organizations` itself cannot be the lock: the maintenance lane holds
//! no UPDATE on it.
//!
//! **The person's lock** is taken by every lane that changes what they own or
//! belong to, or writes their own rows: `/me`'s provisioning, both invite
//! accepts, ownership accept, account deletion and erasure, the self-service
//! lanes (`acting_person_in`), and an audit export's start, which counts the
//! person's own exports before it adds one. The accepts take it because the
//! self-service lanes decide under it whether the person is in any
//! organization at all.
//!
//! **Order: the person before any organization, organizations by id.** Account
//! deletion takes several organizations, and it, the invite accepts and
//! ownership accept take the person's first; a fixed order is what keeps any
//! two of them from deadlocking.
//!
//! Released at commit or rollback. The keys are namespaced so neither can ever
//! collide with the audit chain's per-organization lock (`emit_audit`), which
//! hashes the bare organization id.

use sqlx::PgConnection;
use telmoni_shared::{OrganizationId, UserId};

/// Serialise the lanes that decide what a person holds or belongs to: `/me`'s
/// provisioning, the invite accepts, and the account's deletion.
pub async fn lock_person(conn: &mut PgConnection, user_id: &UserId) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('person:' || $1, 0))")
        .bind(user_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Serialise the two lanes that give an address to a built-in account: a
/// sign-up and a confirmed address change. Each checks nobody holds the
/// address before taking it, and two doing so at once would both pass.
pub async fn lock_email(conn: &mut PgConnection, email: &str) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('email:' || $1, 0))")
        .bind(email)
        .execute(conn)
        .await?;
    Ok(())
}

/// Serialise the lanes that change an organization's roster or its owner.
pub async fn lock_organization(
    conn: &mut PgConnection,
    organization_id: &OrganizationId,
) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('membership:' || $1, 0))")
        .bind(organization_id)
        .execute(conn)
        .await?;
    Ok(())
}
