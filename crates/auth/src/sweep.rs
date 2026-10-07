//! The sweeps — the durable retry net behind every inline tail, and the
//! nightly checks — and the operator's two commands. The `telmoni` binary
//! runs the sweeps on a timer inside `serve`, and any of them by hand as
//! `telmoni sweep <name>`; the commands run as `telmoni terminate` and
//! `telmoni restore`.
//!
//! The deletion and audit-verify sweeps take a transaction-scoped advisory
//! lock first, so replicas running the same timer cannot both sweep one
//! tick, and a crashed leader never wedges the next: the lock's transaction
//! is pinged while the tick runs, and a leader that stops pinging loses it
//! to the role's idle cut-off. The retention sweep takes none: its DELETEs
//! are idempotent. Every step is idempotent: a failure stops that
//! organization's tail, or that person's erasure, until the next tick, and
//! never the rest of the sweep.
//!
//! **Deletion.** The `pending_deletion` mark is the durable queue. An
//! organization past `erase_after` is **finalized** (every sibling's purge,
//! then the hard delete), and one still inside its wait is **purged** (the
//! hook purge the request could not land, so nothing outside keeps acting on
//! a closed organization while its owner decides whether to bring it back).
//! Then every pending person's erasure, second, because a person who owns an
//! organization is refused until its row is gone, and both then finish in
//! one tick.
//!
//! **Retention.** What this module prunes for bounded growth and for the
//! privacy it owes, that nothing else prunes.
//!
//! **Audit verify.** Every organization's tamper-evident chain, walked and
//! recomputed. A break is logged at `error!` and fails the run; it is never
//! auto-repaired, because a forked chain is a finding.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::audit_verify::verify_audit_chain;
use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope};
use telmoni_shared::{
    AuditAction, AuthError, OrganizationId, OrganizationStatus, TelmoniError, TelmoniResourceKind,
    UserId,
};

use crate::AppState;
use crate::db::{
    AuthLane, access_tokens, audit, authorization_codes, device_codes, invites, locks,
    organization_members, organizations, refresh_tokens, sessions, tokens,
};
use crate::handler::deletion::{
    self, ErasureOutcome, FinalizeOutcome, PurgeOutcome, RESTORE_WINDOW_SECONDS, TailOutcome,
};
use crate::model::DeletionKind;

/// Advisory-lock id for the deletion sweep: ASCII `"work_del"` as i64.
const LOCK_ID_DELETION_SWEEP: i64 = 0x776F_726B_5F64_656C;

/// Advisory-lock id for the audit-verify sweep: ASCII `"work_adt"` as i64.
const LOCK_ID_AUDIT_VERIFY: i64 = 0x776F_726B_5F61_6474;

/// The most organizations, and the most people, one tick works through;
/// the rest roll onto the next.
const SWEEP_BATCH: i64 = 500;

/// What one deletion tick finished. A failure is not counted here: it fails
/// the run, and retries next tick.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DeletionSwept {
    /// Organizations finalized: their rows are gone.
    pub organizations: u32,
    /// Organizations still inside their wait whose hook purge landed.
    pub purged: u32,
    /// People erased.
    pub people: u32,
}

/// One sweep's count, before the tick reports it.
#[derive(Default)]
struct Tally {
    done: u32,
    purged: u32,
    failed: u32,
}

/// How often a leader's lock transaction is pinged: well inside the role's
/// two-minute `idle_in_transaction_session_timeout`, which ends the session,
/// and the lock with it, under a tick left idle that long.
const LEADER_HEARTBEAT: Duration = Duration::from_secs(30);

/// The longest one deletion tick runs before it stops where it is; the next
/// tick, ten minutes on, carries on from the listing.
const DELETION_TICK_BUDGET: Duration = Duration::from_mins(30);

/// The longest one audit-verify walk runs: far past a walk of every chain,
/// short of the next day's.
const AUDIT_VERIFY_BUDGET: Duration = Duration::from_hours(12);

/// Take the leader lock for one tick on a transaction of its own. `None`
/// when another replica holds it: the loser skips the tick entirely.
async fn leader_of(
    state: &AppState,
    lock_id: i64,
) -> Result<Option<sqlx::Transaction<'_, sqlx::Postgres>>, TelmoniError> {
    let mut leader_tx = state.db.begin().await?;
    let is_leader: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(lock_id)
        .fetch_one(&mut *leader_tx)
        .await?;
    Ok(is_leader.then_some(leader_tx))
}

/// Run one tick's `work` as the leader, pinging the lock's transaction while
/// it runs. ⚠ **Unpinged, a tick past the idle cut-off lost its lock
/// halfway** and another replica's tick ran beside the rest of it. A leader
/// that dies stops pinging, and the cut-off frees its lock two minutes later.
/// A ping that fails may mean the lock is gone, so the tick stops there; and
/// so does a tick past its `budget`, since a step hung on an outside call
/// would otherwise keep the lock pinged and stop every replica's sweep.
async fn as_leader<T>(
    mut leader: sqlx::Transaction<'_, sqlx::Postgres>,
    budget: Duration,
    work: impl std::future::Future<Output = Result<T, TelmoniError>>,
) -> Result<T, TelmoniError> {
    let mut work = std::pin::pin!(tokio::time::timeout(budget, work));
    let mut beat = tokio::time::interval(LEADER_HEARTBEAT);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick is immediate; the lock was taken just now.
    beat.tick().await;
    let out = loop {
        tokio::select! {
            out = &mut work => break out.unwrap_or_else(|_| {
                Err(TelmoniError::Internal(format!(
                    "the tick ran past its {} minutes and stopped; the next one carries on",
                    budget.as_secs() / 60
                )))
            }),
            _ = beat.tick() => {
                sqlx::query("SELECT 1")
                    .execute(&mut *leader)
                    .await
                    .map_err(|e| TelmoniError::internal("the sweep's leader lock was lost", e))?;
            }
        }
    };
    // The tick's outcome stands either way: a rollback that fails means the
    // connection is gone, and the lock went with it.
    if let Err(e) = leader.rollback().await {
        tracing::warn!(error = %e, "the sweep's leader transaction did not roll back cleanly");
    }
    out
}

/// One leadered deletion tick: every due organization's step, then every
/// pending person's erasure.
pub async fn deletion(state: &AppState) -> Result<DeletionSwept, TelmoniError> {
    let Some(leader) = leader_of(state, LOCK_ID_DELETION_SWEEP).await? else {
        return Ok(DeletionSwept::default());
    };

    let (organizations, people) = as_leader(leader, DELETION_TICK_BUDGET, async {
        let organizations = sweep_organizations(state).await?;
        let people = sweep_people(state).await?;
        Ok((organizations, people))
    })
    .await?;

    let mut failed = Vec::new();
    if organizations.failed > 0 {
        failed.push(format!("{} organization tail(s)", organizations.failed));
    }
    if people.failed > 0 {
        failed.push(format!("{} person erasure(s)", people.failed));
    }
    if !failed.is_empty() {
        return Err(TelmoniError::Internal(format!(
            "{} failed this tick ({} finalized, {} purged, {} erased); each retries next tick",
            failed.join(" and "),
            organizations.done,
            organizations.purged,
            people.done
        )));
    }
    Ok(DeletionSwept {
        organizations: organizations.done,
        purged: organizations.purged,
        people: people.done,
    })
}

/// The pending organizations the sweep owes a step, soonest due first: the
/// ripe ones for finalize, the rest for the hook purge the request could
/// not land. Across every organization, so the maintenance lane.
pub async fn due_organizations(
    state: &AppState,
) -> Result<Vec<organizations::DueOrganization>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let rows = organizations::list_due_for_sweep(&mut tx, SWEEP_BATCH).await?;
    tx.commit().await?;
    Ok(rows)
}

async fn sweep_organizations(state: &AppState) -> Result<Tally, TelmoniError> {
    let mut tally = Tally::default();
    for organization in due_organizations(state).await? {
        let id = &organization.external_id;
        let step = if organization.ripe {
            finalize_one(state, id).await
        } else {
            purge_one(state, id).await
        };
        match step {
            Ok(()) if organization.ripe => tally.done += 1,
            Ok(()) => tally.purged += 1,
            Err(e) => {
                tally.failed += 1;
                tracing::warn!(organization_id = %id, ripe = organization.ripe,
                    error = %e, "deletion tail step failed");
            }
        }
    }
    Ok(tally)
}

/// Only after [`sweep_organizations`]: a sole owner's erasure is refused
/// until the finalizes it waits on have landed.
async fn sweep_people(state: &AppState) -> Result<Tally, TelmoniError> {
    let mut tally = Tally::default();
    for (user_id, _) in deletion::pending_people(state, SWEEP_BATCH).await? {
        match deletion::erase_person(state, &user_id).await {
            Ok(ErasureOutcome::Erased | ErasureOutcome::AlreadyGone) => tally.done += 1,
            Err(e) => {
                tally.failed += 1;
                tracing::warn!(user_id = %user_id, error = %e, "person erasure failed");
            }
        }
    }
    Ok(tally)
}

/// One organization's finalize: every sibling purge, then the hard delete —
/// and the row never goes before the purges did. A row that is no longer
/// pending, or was restored under the purges, is an error for the log, not
/// a step done.
async fn finalize_one(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    match deletion::finalize_organization(state, organization_id).await? {
        FinalizeOutcome::Finalized => {
            tracing::info!(organization_id = %organization_id, "deletion saga finalized");
            Ok(())
        }
        FinalizeOutcome::AlreadyGone => Ok(()),
        FinalizeOutcome::NotPending => Err(TelmoniError::Internal(format!(
            "{organization_id} was listed for finalize and is no longer pending deletion"
        ))),
        FinalizeOutcome::TooSoon => Err(TelmoniError::Internal(format!(
            "{organization_id} was listed for finalize before its wait passed"
        ))),
    }
}

/// One organization's interim purge: the purge hook, recorded once it
/// lands, so the next listing leaves the organization out until its wait
/// has passed.
async fn purge_one(state: &AppState, organization_id: &OrganizationId) -> Result<(), TelmoniError> {
    match deletion::purge_pending_organization(state, organization_id).await? {
        PurgeOutcome::Purged => {
            tracing::info!(organization_id = %organization_id, "deletion saga hook purge landed; the row waits");
            Ok(())
        }
        PurgeOutcome::AlreadyGone => Ok(()),
        PurgeOutcome::NotPending => Err(TelmoniError::Internal(format!(
            "{organization_id} was listed for a purge and is no longer pending deletion"
        ))),
    }
}

/// What the retention sweep removed: counts only, no ids.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RetentionSwept {
    /// `auth.api_tokens` rows past their grace window.
    pub tokens: u64,
    /// Unaccepted invitations, to projects and to organizations, thirty
    /// days past their expiry or withdrawal.
    pub invitations: u64,
    /// Sessions revoked more than ninety days ago.
    pub sessions: u64,
    /// The issuer's grants nobody can spend any more: expired bearers,
    /// refresh tokens and codes, spent refresh tokens past the reuse window,
    /// device authorizations nobody finished.
    pub grants: u64,
}

/// Hard-delete every `api_tokens` row past its grace window, every
/// invitation nobody accepted that expired or was withdrawn more than thirty
/// days ago, every session revoked more than ninety days ago, and the
/// issuer's grants nobody can spend any more. One transaction: the counts
/// answer for what was committed. No lock needed: a DELETE is idempotent.
pub async fn retention(state: &AppState) -> Result<RetentionSwept, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let tokens = tokens::delete_expired(&mut tx).await?;
    let invitations = invites::delete_stale(&mut tx).await?;
    let sessions = sessions::delete_revoked(&mut tx).await?;
    let grants = access_tokens::delete_expired(&mut tx).await?
        + refresh_tokens::delete_stale(&mut tx).await?
        + authorization_codes::delete_expired(&mut tx).await?
        + device_codes::delete_expired(&mut tx).await?;
    tx.commit().await?;

    tracing::info!(
        tokens,
        invitations,
        sessions,
        grants,
        "retention sweep complete"
    );
    Ok(RetentionSwept {
        tokens,
        invitations,
        sessions,
        grants,
    })
}

/// One leadered audit-verify tick: enumerate the organizations with a chain,
/// verify each, report. A break logs at `error!` and fails the run after the
/// walk, so every other chain is still checked; nothing is repaired.
pub async fn audit_verify(state: &AppState) -> Result<(), TelmoniError> {
    let Some(leader) = leader_of(state, LOCK_ID_AUDIT_VERIFY).await? else {
        return Ok(());
    };

    let (verified, broken, unreadable) = as_leader(leader, AUDIT_VERIFY_BUDGET, async {
        let mut tx = maintenance_scope(&state.db, AuthLane).await?;
        let organization_ids = audit::distinct_organizations(&mut tx).await?;
        tx.commit().await?;

        let (mut verified, mut broken, mut unreadable) = (0u64, 0u64, 0u64);
        for organization_id in &organization_ids {
            let report = match verify_one_chain(state, organization_id).await {
                Ok(report) => report,
                Err(e) => {
                    unreadable += 1;
                    tracing::error!(organization_id = %organization_id, error = %e,
                        "audit chain could not be verified — this chain is UNCHECKED, not intact");
                    continue;
                }
            };
            verified += 1;
            if let Some(first_break) = &report.first_break {
                broken += 1;
                tracing::error!(
                    organization_id = %organization_id,
                    rows = report.rows,
                    first_break = %json!(first_break),
                    "AUDIT CHAIN BREAK — the tamper-evident log failed verification"
                );
            }
        }
        Ok((verified, broken, unreadable))
    })
    .await?;
    tracing::info!(
        organizations = verified,
        broken,
        unreadable,
        "audit-chain verify sweep complete"
    );

    if broken > 0 || unreadable > 0 {
        return Err(TelmoniError::Internal(format!(
            "{broken} of {verified} audit chains failed verification; \
             {unreadable} could not be read at all"
        )));
    }
    Ok(())
}

/// Walk one organization's chain in its own scope, bounded so one long
/// chain cannot hold the sweep.
async fn verify_one_chain(
    state: &AppState,
    organization_id: &str,
) -> Result<telmoni_shared::db::audit_verify::VerifyReport, TelmoniError> {
    let organization_id = OrganizationId::try_new(organization_id).map_err(|e| {
        TelmoniError::Internal(format!("audit chain names a bad organization id: {e}"))
    })?;
    let mut tx = organization_scope(&state.db, &organization_id).await?;
    sqlx::query("SET LOCAL statement_timeout = '30s'")
        .execute(&mut *tx)
        .await?;
    let report = verify_audit_chain(&mut tx, &organization_id, None, None).await?;
    tx.commit().await?;
    Ok(report)
}

/// What a termination came to: whether the hook purge landed in the call,
/// and when the row goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Terminated {
    /// The hook purge landed and is recorded; otherwise the sweep retries it.
    pub purged: bool,
    /// The end of the restore window, when the row may go.
    pub erase_after: DateTime<Utc>,
}

/// An OPERATOR closes an organization without its owner's code: the Terms'
/// suspension and termination clause. The same mark as the owner's deletion,
/// the same wait and the same tail; only the owner cannot undo it. Refused
/// (409) for one already pending; 404 for one that does not exist.
///
/// Audited as `service:operator` on the organization's own chain, so the
/// customer's audit log records that the operator closed it and when.
pub async fn terminate(
    state: &AppState,
    organization: &OrganizationId,
) -> Result<Terminated, TelmoniError> {
    let mut tx = organization_scope(&state.db, organization).await?;
    locks::lock_organization(&mut tx, organization).await?;
    match organizations::status(&mut tx, organization).await? {
        None => return Err(AuthError::NotFound("no such organization".into()).into()),
        Some(OrganizationStatus::Active) => {}
        Some(_) => {
            return Err(
                AuthError::Conflict("this organization is already being deleted".into()).into(),
            );
        }
    }
    let Some(erase_after) = organizations::mark_pending_deletion(
        &mut tx,
        organization,
        DeletionKind::Operator,
        RESTORE_WINDOW_SECONDS,
    )
    .await?
    else {
        return Err(TelmoniError::Internal(format!(
            "{organization} changed status under its lock"
        )));
    };
    organization_members::clear_offers(&mut tx, organization).await?;

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: organization,
            in_project: None,
            actor: Actor::Service("operator"),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "organization_deletion",
                "deletion": "requested",
                "by": DeletionKind::Operator,
                "erase_after": erase_after,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    tracing::warn!(organization_id = %organization, erase_after = %erase_after,
        "organization terminated by an operator");
    let purged = matches!(
        deletion::run_inline_tail(state, organization).await,
        TailOutcome::Purged
    );
    Ok(Terminated {
        purged,
        erase_after,
    })
}

/// An OPERATOR brings a pending organization back: one they terminated, or
/// one its owner deleted and asks support to recover. Allowed while the row
/// stands, window or no window, since the sweep may be what an operator is
/// correcting. Refused (409) for one an account deletion took: its owner is
/// being erased, and an active organization with a vanishing owner is the
/// invariant `erase_person` refuses to work around. 404 for none.
pub async fn restore(state: &AppState, organization: &OrganizationId) -> Result<(), TelmoniError> {
    let mut tx = organization_scope(&state.db, organization).await?;
    locks::lock_organization(&mut tx, organization).await?;
    let Some(row) = organizations::get(&mut tx, organization).await? else {
        return Err(AuthError::NotFound("no such organization".into()).into());
    };
    if row.status != OrganizationStatus::PendingDeletion {
        return Err(AuthError::Conflict("this organization is not being deleted".into()).into());
    }
    let Some(kind @ (DeletionKind::Owner | DeletionKind::Operator)) = row.deletion_kind else {
        return Err(AuthError::Conflict(
            "this organization is being deleted with its owner's account and cannot be restored"
                .into(),
        )
        .into());
    };
    if !organizations::restore(&mut tx, organization).await? {
        return Err(TelmoniError::Internal(format!(
            "{organization} changed status under its lock"
        )));
    }

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: organization,
            in_project: None,
            actor: Actor::Service("operator"),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "organization_deletion",
                "deletion": "restored",
                "by": DeletionKind::Operator,
                "requested_by": kind,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    tracing::warn!(organization_id = %organization, requested_by = %kind,
        "organization restored by an operator");
    Ok(())
}

/// The people whose erasure the sweep still owes, for a test or an operator
/// reading the queue: the same listing [`deletion()`] works through.
pub async fn pending_people(
    state: &AppState,
) -> Result<Vec<(UserId, DateTime<Utc>)>, TelmoniError> {
    deletion::pending_people(state, SWEEP_BATCH).await
}
