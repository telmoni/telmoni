//! Audit emit helper.
//!
//! Every state-changing handler calls [`emit_audit`] inside its own
//! transaction, so the row commits with the mutation: a failed operation never
//! leaves an orphan row, and a successful one is never unrecorded. A helper
//! rather than a tower middleware for exactly that reason — by the time a
//! response is observable, the transaction has already committed.
//!
//! A failed audit write aborts the caller's transaction, which is correct:
//! failed audit = failed business operation. [`emit_audit`] is the only
//! sanctioned writer; `tests/append_only_audit.rs` fails CI on any other.

use chrono::Utc;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::db::audit_hash::row_hash;
use crate::types::{OrganizationId, ProjectId};
use crate::{types::AuditAction, types::TelmoniResourceKind};

/// Who performed an audited action, rendered to the exact `actor_id` wire form.
/// A closed type so a user id that happens to start with `service:` cannot
/// masquerade as a machine. The rendered string is a hash-chain input, so
/// `Display` must reproduce today's form byte-for-byte.
#[derive(Debug, Clone, Copy)]
pub enum Actor<'a> {
    /// A human principal: the provider sub of the acting user. Renders bare.
    User(&'a str),
    /// A service account, cron tick, or saga step. Renders `service:<name>`.
    Service(&'a str),
    /// An external system acting on the project's behalf (e.g. a vendor's webhook).
    External(&'a str),
}

impl std::fmt::Display for Actor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Actor::User(sub) => f.write_str(sub),
            Actor::Service(name) => write!(f, "service:{name}"),
            Actor::External(name) => write!(f, "external:{name}"),
        }
    }
}

/// One row of the audit log.
#[derive(Debug, Clone)]
pub struct AuditEvent<'a> {
    /// The affected organization. **The chain's root**: an organization has one
    /// verifiable chain covering everything that happened inside it.
    pub organization_id: &'a OrganizationId,
    /// Who performed the action.
    pub actor: Actor<'a>,
    /// The closed-vocabulary verb. Required.
    pub action: AuditAction,
    /// The closed-vocabulary resource kind. Required.
    pub resource_kind: TelmoniResourceKind,
    /// UUID or slug of the resource. None when the action names no single row.
    pub resource_id: Option<&'a str>,
    /// W3C traceparent — cross-stitches the audit row with the service logs.
    pub request_id: Option<&'a str>,
    /// Client IP as a string literal (e.g. `"192.0.2.1"` / `"2001:db8::1"`).
    /// Bound as `$8::inet`, so an unparseable value fails the transaction —
    /// a programmer bug, not a customer fault.
    pub ip_address: Option<&'a str>,
    /// Browser / SDK / CLI user agent. None for service-driven actions.
    pub user_agent: Option<&'a str>,
    /// Action-specific extras as a JSON object. Keep it small.
    pub metadata: Option<Value>,
    /// The project the action happened inside, when it happened inside one.
    /// Hashed when present, and never the chain's root: a project has no chain
    /// of its own, so nothing locks, sequences or verifies on it.
    pub in_project: Option<&'a ProjectId>,
}

/// The organization's newest chain row by `seq`, as its `seq` and `row_hash`.
/// Read under the organization's chain lock, in the caller's scope: one
/// `(organization_id, seq)` index probe per partition, the whole chain.
///
/// ⚠ **Never bounded by time.** A window on `created_at` was tried, and it
/// forks the chain: at a month's edge a row a replica a second ahead had just
/// written fell outside the window, the read answered the row before it, and
/// the next `seq` was minted twice. `seq` order is the lock's; `created_at`
/// is whichever clock emitted.
async fn newest_link(
    conn: &mut PgConnection,
    organization_id: &OrganizationId,
) -> sqlx::Result<Option<(i64, String)>> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT seq, row_hash
           FROM audit.events
          WHERE organization_id = $1
          ORDER BY seq DESC
          LIMIT 1",
    )
    .bind(organization_id)
    .fetch_optional(conn)
    .await
}

/// Wait for `organization_id`'s chain lock, held to the end of the caller's
/// transaction. [`emit_audit`] takes it for every row it writes, so every
/// audited change in the organization — a transfer, an ownership hand-over, a
/// deletion — holds it from its audit row to its commit. A writer takes it
/// before reading what its change depends on, so that what it read cannot
/// move under it before it commits.
pub async fn lock_chain(
    conn: &mut PgConnection,
    organization_id: &OrganizationId,
) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(organization_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Write one row to `audit.events`, linked into the organization's
/// tamper-evident hash chain. Pass the connection running the handler's
/// transaction (`&mut *tx`), so the row, the chain lock and the link read all
/// happen inside it.
pub async fn emit_audit(conn: &mut PgConnection, evt: AuditEvent<'_>) -> sqlx::Result<()> {
    let action = evt.action.to_string();
    let resource_kind = evt.resource_kind.to_string();
    let actor_id = evt.actor.to_string();

    // ⚠ **THE CHAIN IS ORGANIZATION-KEYED, AND SOME CALLERS ARE NOT.**
    // `audit.events` isolates on `app.organization_id`, and a project-scoped
    // transaction binds only `app.project_id` — so without this, every audited
    // mutation reached through `acting_project` read an empty chain (a second
    // genesis row) and was then refused by the WITH CHECK. A superuser never
    // sees it, because BYPASSRLS overrides FORCE; `tests/members.rs` is the pin.
    //
    // Bound only when nothing is, so an organization-scoped caller keeps its own
    // binding and the WITH CHECK still refuses a mislabelled row; restored to
    // empty afterwards, because an organization id left on a project-scoped
    // transaction would quietly widen every later read on it.
    let bound: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.organization_id', true)")
            .fetch_one(&mut *conn)
            .await?;
    let scope_here = bound.unwrap_or_default().is_empty();
    if scope_here {
        sqlx::query("SELECT set_config('app.organization_id', $1, true)")
            .bind(evt.organization_id)
            .execute(&mut *conn)
            .await?;
    }

    // Per ORGANIZATION, because the chain is: locking per project would let
    // two projects take the same `MAX(seq) + 1` and fork the chain this lock
    // keeps linear. Released at the caller's commit or rollback; a caller
    // already holding it takes it again without waiting.
    lock_chain(&mut *conn, evt.organization_id).await?;

    // Sampled from the app clock, not `now()` in SQL: `created_at` is a hash
    // input and must be the exact value the verifier recomputes. The chain
    // ORDERS by `seq`, never by a clock.
    let id = Uuid::now_v7();
    let created_at = Utc::now();

    // The highest-`seq` row is the link target, read under the lock, so the
    // `MAX(seq) + 1` taken here is atomic. `seq` is not hashed.
    let predecessor = newest_link(&mut *conn, evt.organization_id).await?;
    let seq: i64 = predecessor.as_ref().map_or(1, |(s, _)| s + 1);
    let prev_hash: Option<String> = predecessor.map(|(_, h)| h);

    let hash = row_hash(
        id,
        evt.organization_id.as_ref(),
        &actor_id,
        &action,
        &resource_kind,
        evt.resource_id,
        evt.metadata.as_ref(),
        created_at,
        prev_hash.as_deref(),
        evt.in_project.map(AsRef::as_ref),
    );

    sqlx::query(
        "INSERT INTO audit.events
             (id, organization_id, actor_id, action, resource_kind,
              resource_id, request_id, ip_address, user_agent, metadata,
              created_at, prev_hash, row_hash, shard_key, seq, in_project)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::inet, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(id)
    .bind(evt.organization_id)
    .bind(&actor_id)
    .bind(&action)
    .bind(&resource_kind)
    .bind(evt.resource_id)
    .bind(evt.request_id)
    .bind(evt.ip_address)
    .bind(evt.user_agent)
    .bind(evt.metadata.as_ref())
    .bind(created_at)
    .bind(&prev_hash)
    .bind(&hash)
    .bind(crate::derive_shard_key(evt.organization_id))
    .bind(seq)
    .bind(evt.in_project)
    .execute(&mut *conn)
    .await?;

    // This GUC alone, never all three: the caller is usually project-scoped,
    // and clearing all three would silently un-scope the transaction that
    // follows.
    if scope_here {
        sqlx::query("SELECT set_config('app.organization_id', '', true)")
            .execute(&mut *conn)
            .await?;
    }

    tracing::info!(
        target: "audit",
        id          = %id,
        organization_id  = %evt.organization_id,
        actor_id    = %actor_id,
        action      = %action,
        resource    = %resource_kind,
        resource_id = evt.resource_id,
        request_id  = evt.request_id,
        in_project  = evt.in_project.map(AsRef::as_ref),
        row_hash    = %hash,
        "audit event",
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Actor;

    /// The rendered actor string is a hash-chain input, so `Display` must
    /// reproduce today's exact wire form; changing it would silently invalidate
    /// every existing chain.
    #[test]
    fn actor_display_pins_the_wire_form() {
        assert_eq!(Actor::User("user_2abc").to_string(), "user_2abc");
        assert_eq!(Actor::Service("migrator").to_string(), "service:migrator");
        assert_eq!(Actor::Service("auth").to_string(), "service:auth");
        assert_eq!(Actor::External("slack").to_string(), "external:slack");
    }
}
