//! The two deletion tails, run inside the confirming request with the
//! deletion sweep as the retry net — and, for an organization, the hard
//! delete that only the sweep performs.
//!
//! - **An organization's**: the mark closes it and starts a wait; the purge
//!   hook runs in the request ([`crate::Siblings::purge_hook`]: whatever a
//!   module of the deployment's own holds for the organization outside this
//!   database — a subscription, say — goes now, not when the wait ends); and
//!   once `erase_after` has passed the sweep runs every sibling purge — a
//!   hook purge nobody has landed first, then notifications, then the hook
//!   again — and deletes the row, under the organization's lock and only
//!   while it is still pending and ripe. The wait is the FINALIZE GRACE,
//!   [`FINALIZE_GRACE_SECONDS`], whoever asked: there is no restore. Its
//!   members' memberships go with it; no person is touched.
//! - **A person's**: the identity provider's user → the notices that named
//!   them, rewritten by notifications → their conversations with the agent,
//!   and its copies of what named them → their memberships elsewhere, each
//!   removed and audited on that organization, and the invitations they
//!   accepted, which held their address → the notices and the agent once
//!   more → the identity (sessions and codes cascade). It waits for every
//!   organization they owned to be gone first; their owner row would block it.
//!
//! ⚠ **Why the row waits, whoever asked.** The mark closes the organization to
//! every lane (`organization_role_or_forbidden`), but a request that had its
//! authorization answered before the mark is still running: a connector
//! handshake holds its answer across a vendor exchange and a KMS wrap before
//! it writes. A grant stored on a sibling after an early purge would
//! otherwise outlive the organization with nothing left to sweep it. So the
//! notifications purge runs only at finalize, the hook's runs again there,
//! and the row goes only then.
//!
//! **Awaited is load-bearing.** A spawned task outlives nothing: the pod that
//! ran it can be replaced by the next rollout or drained off its node the
//! moment the response is sent, and the tail dies with it. Fire-and-forget
//! here is a silent data-deletion bug.
//!
//! The marks commit before the tails start, so a crash, a timeout or a failed
//! step leaves the row pending for the sweep. Every step is idempotent, which is
//! what makes running it from two places safe. "Gone" at the provider is its own
//! error code, never a bare 404.
//!
//! The hook before the hard delete, because the row must never go while
//! something outside this database still acts on the organization — a
//! subscription still charging, say.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope, person_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::{
    AuditAction, AuthError, OrganizationId, OrganizationRole, OrganizationStatus, ProjectId,
    TelmoniError, TelmoniResourceKind, UserId,
};

use crate::{
    AppState,
    db::{AuthLane, identities, invites, locks, members, organization_members, organizations},
};

/// How long an organization stays `pending_deletion` before its row may go,
/// whoever asked for its deletion: fifteen minutes. There is no restore, so
/// the wait covers requests in flight and nothing else.
///
/// ⚠ **Longer than anything that could still be writing to it.** The console
/// abandons a fetch at ten seconds, and a connector handshake holds its
/// answer across a vendor's OAuth exchange and a KMS wrap. Fifteen minutes is
/// far past both, and well inside the hour the privacy policy promises
/// (`the_wait_is_fifteen_minutes_for_an_owners_deletion_and_a_termination`,
/// `the_finalize_grace_is_fifteen_minutes`).
pub const FINALIZE_GRACE_SECONDS: u32 = 900;

/// Outcome of one inline tail attempt.
pub enum TailOutcome {
    /// The hook purged and recorded; the row waits for `erase_after`.
    Purged,
    /// The step failed or the budget expired; the sweep retries it.
    StillPending,
}

/// Terminal states of the finalize hard delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeOutcome {
    /// This call deleted the organization rows and emitted the terminal audit row.
    Finalized,
    /// The organization was already gone (idempotent retry, or a raced finalize).
    AlreadyGone,
    /// The organization exists and is not `pending_deletion`: finalize follows
    /// a request, never replaces one.
    NotPending,
    /// `erase_after` has not passed: a request authorized before the mark
    /// may still be landing on a sibling. The sweep's listing does not name
    /// it for finalize yet.
    TooSoon,
}

/// Terminal states of the sweep's interim hook purge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeOutcome {
    /// The hook purged and it is recorded, now or already.
    Purged,
    /// The organization was already gone.
    AlreadyGone,
    /// The organization is not `pending_deletion`.
    NotPending,
}

/// Terminal states of a person's erasure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErasureOutcome {
    /// This call erased them.
    Erased,
    /// They were already gone.
    AlreadyGone,
}

/// Run an organization's inline tail — the hook purge — bounded by
/// `deletion_tail_budget_ms`. Never errors: the deletion is already committed,
/// so a failure is logged and reported as [`TailOutcome::StillPending`]. Call
/// only AFTER the mark commits.
pub async fn run_inline_tail(state: &AppState, organization_id: &OrganizationId) -> TailOutcome {
    let budget = std::time::Duration::from_millis(state.config.deletion_tail_budget_ms);
    match tokio::time::timeout(budget, purge_hook_and_record(state, organization_id)).await {
        Ok(Ok(())) => TailOutcome::Purged,
        Ok(Err(e)) => {
            tracing::warn!(organization_id = %organization_id, error = %e,
                "inline deletion tail failed; the sweep retries the hook purge");
            TailOutcome::StillPending
        }
        Err(_) => {
            tracing::warn!(organization_id = %organization_id, budget_ms = state.config.deletion_tail_budget_ms,
                "inline deletion tail hit its budget; the sweep retries the hook purge");
            TailOutcome::StillPending
        }
    }
}

/// Notifications' and the agent's teardown for one organization, before its
/// row goes. A module not linked is nothing to purge; an error stops the
/// tail it is part of.
pub(crate) async fn purge_organization_siblings(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    if let Some(notifications) = state.siblings.notifications.as_ref() {
        notifications.purge_organization(organization_id).await?;
    }
    if let Some(agent) = state.siblings.agent.as_ref() {
        agent.purge_organization(organization_id).await?;
    }
    Ok(())
}

/// The longest notifications' project purge may take. The transfer's accept
/// calls it inside its transaction, holding two organizations' locks, so it
/// cannot be allowed the console's whole ten seconds.
const PROJECT_PURGE_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// Notifications' teardown for one project — its connectors, their
/// deliveries, its feed and any handshake in flight — before it is handed
/// to another organization, whose events must never post into the old
/// one's channels, and after one is deleted. No notifications module is
/// nothing to purge; a refusal or a purge over budget is an `Err`, which the
/// transfer treats as a refusal to move. The agent needs no call: it finds
/// what it holds of a project under an organization that no longer has it,
/// and removes it, itself.
pub(crate) async fn purge_project_connectors(
    state: &AppState,
    project_id: &ProjectId,
) -> Result<(), TelmoniError> {
    let Some(notifications) = state.siblings.notifications.as_ref() else {
        return Ok(());
    };
    match tokio::time::timeout(
        PROJECT_PURGE_BUDGET,
        notifications.purge_project(project_id),
    )
    .await
    {
        Ok(done) => done.map(|_| ()),
        Err(_) => Err(TelmoniError::Internal("the project purge timed out".into())),
    }
}

/// The purge hook, for what the deployment holds of the organization outside
/// this database. No hook is done, since nothing outside holds the
/// organization; an error stops the tail before the row can go.
async fn purge_hook(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    let Some(hook) = state.siblings.purge_hook.as_ref() else {
        return Ok(());
    };
    hook.purge_organization(organization_id).await
}

/// The request's tail, and the sweep's until it lands: the hook purges, then
/// it is recorded so the sweep stops retrying. Idempotent on both sides.
pub(crate) async fn purge_hook_and_record(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    purge_hook(state, organization_id).await?;
    let mut tx = organization_scope(&state.db, organization_id).await?;
    organizations::record_hook_purged(&mut tx, organization_id).await?;
    tx.commit().await?;
    Ok(())
}

/// The organization's lifecycle status, read in its own scope.
async fn status_of(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<Option<OrganizationStatus>, TelmoniError> {
    let mut tx = organization_scope(&state.db, organization_id).await?;
    let status = organizations::status(&mut tx, organization_id).await?;
    tx.commit().await?;
    Ok(status)
}

/// What the finalize decides on — status, ripe, hook purge recorded — read in
/// the organization's own scope.
async fn finalize_standing_of(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<Option<(OrganizationStatus, bool, bool)>, TelmoniError> {
    let mut tx = organization_scope(&state.db, organization_id).await?;
    let standing = organizations::finalize_standing(&mut tx, organization_id).await?;
    tx.commit().await?;
    Ok(standing)
}

/// The finalize's purges in strict order, `?` on each, so a failure stops the
/// tail before the next step and before the hard delete.
async fn finalize_purges(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    purge_organization_siblings(state, organization_id).await?;
    purge_hook(state, organization_id).await
}

/// The sweep's interim step for a pending organization inside its wait:
/// the hook purge the request could not land. Refused for an organization
/// that is not pending; done for one already gone.
pub async fn purge_pending_organization(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<PurgeOutcome, TelmoniError> {
    match status_of(state, organization_id).await? {
        None => return Ok(PurgeOutcome::AlreadyGone),
        Some(OrganizationStatus::PendingDeletion) => {}
        Some(_) => return Ok(PurgeOutcome::NotPending),
    }
    purge_hook(state, organization_id).await?;
    let mut tx = organization_scope(&state.db, organization_id).await?;
    let recorded = organizations::record_hook_purged(&mut tx, organization_id).await?;
    tx.commit().await?;
    if recorded {
        return Ok(PurgeOutcome::Purged);
    }
    // Nothing to record: a request's tail recorded it first, or the row is
    // gone. The step answers what stands, not what it set out to do.
    Ok(match status_of(state, organization_id).await? {
        None => PurgeOutcome::AlreadyGone,
        Some(OrganizationStatus::PendingDeletion) => PurgeOutcome::Purged,
        Some(_) => PurgeOutcome::NotPending,
    })
}

/// The finalize: every sibling purge, then the hard delete. Its roster,
/// invitations, projects, keys and deletion codes cascade; the people on the
/// roster are untouched. Refused before `erase_after` (see the module doc),
/// which the sweep's listing already respects.
pub async fn finalize_organization(
    state: &AppState,
    organization_id: &OrganizationId,
) -> Result<FinalizeOutcome, TelmoniError> {
    let hook_recorded = match finalize_standing_of(state, organization_id).await? {
        None => return Ok(FinalizeOutcome::AlreadyGone),
        Some((status, _, _)) if status != OrganizationStatus::PendingDeletion => {
            return Ok(FinalizeOutcome::NotPending);
        }
        Some((_, false, _)) => return Ok(FinalizeOutcome::TooSoon),
        Some((_, true, hook_recorded)) => hook_recorded,
    };

    // ⚠ **A hook purge nobody has landed goes first.** The request's failed
    // and the wait passed before the interim step could retry it; run behind
    // notifications here, a notifications failure would hold it back for as
    // long as it lasted, with whatever the hook stops — a subscription
    // charging, say — running all the while. Recorded, so the listing stops
    // owing it.
    if !hook_recorded {
        purge_hook_and_record(state, organization_id).await?;
    }
    // Notifications for the first time, the hook for the second: whatever
    // landed on a sibling since the mark — a request past its authorization
    // when the mark went down — goes with this.
    finalize_purges(state, organization_id).await?;

    let mut tx = organization_scope(&state.db, organization_id).await?;
    // ⚠ **Under the lock, and only a row still pending and ripe goes.** The
    // standing was read before the purges, which take a sibling round trip
    // each; whatever the row became in between, an unconditional DELETE
    // would take it anyway. This matches nothing then, and the step answers
    // what stands.
    locks::lock_organization(&mut tx, organization_id).await?;
    if !organizations::delete_ripe(&mut tx, organization_id).await? {
        tx.rollback().await?;
        return Ok(match finalize_standing_of(state, organization_id).await? {
            None => FinalizeOutcome::AlreadyGone,
            Some((OrganizationStatus::PendingDeletion, false, _)) => FinalizeOutcome::TooSoon,
            Some(_) => FinalizeOutcome::NotPending,
        });
    }

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id,
            in_project: None,
            actor: Actor::Service("auth"),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": "deletion_finalize" })),
        },
    )
    .await?;

    tx.commit().await?;

    tracing::info!(organization_id = %organization_id, "organization deletion finalized (hard delete)");
    Ok(FinalizeOutcome::Finalized)
}

/// Ask notifications to rewrite every notice that names the person — "A
/// former member joined the project" in place of their name — on every feed
/// they were ever announced to. No notifications module is nothing to
/// rewrite; a failure stops the erasure before the identity goes, and the
/// sweep retries, because a name left in a feed after the person is gone is
/// the deletion not kept.
async fn redact_person_notices(state: &AppState, user_id: &UserId) -> Result<(), TelmoniError> {
    let Some(notifications) = state.siblings.notifications.as_ref() else {
        return Ok(());
    };
    notifications.redact_person(user_id).await?;
    Ok(())
}

/// What the agent holds of a person: their conversations, and their address
/// and name where other people's answers quoted them, in `organizations`.
/// No agent is nothing to erase.
async fn erase_in_agent(
    state: &AppState,
    user_id: &UserId,
    person: &identities::Person,
    organizations: &[OrganizationId],
) -> Result<(), TelmoniError> {
    let Some(agent) = state.siblings.agent.as_ref() else {
        return Ok(());
    };
    agent
        .erase_person(
            user_id,
            &person.email,
            person.display_name.as_deref(),
            organizations,
        )
        .await?;
    Ok(())
}

/// Erase a person whose account deletion was confirmed: the identity
/// provider's user FIRST (never wipe local rows for a person who still exists
/// upstream), then the notices that named them, then what the agent holds
/// of them, then every membership they hold, each audited on that
/// organization's chain, then the notices and the agent once more, then the
/// identity. Shared by the inline tail and the sweep, so the two cannot drift.
///
/// ⚠ **Refuses a person who has not asked to go** — the sweep reaches it for
/// anybody its listing names, and a bug there must not erase a live person —
/// and one who still owns an organization, whose deletion must finish first.
pub async fn erase_person(
    state: &AppState,
    user_id: &UserId,
) -> Result<ErasureOutcome, TelmoniError> {
    let mut tx = person_scope(&state.db, user_id).await?;
    let person = identities::get(&mut tx, user_id).await?;
    tx.commit().await?;
    let Some(person) = person else {
        tracing::info!(user_id = %user_id, "erasure found no identity; nothing to do");
        return Ok(ErasureOutcome::AlreadyGone);
    };
    if person.deletion_requested_at.is_none() {
        return Err(TelmoniError::Internal(format!(
            "{user_id} has not asked to delete their account — refusing to erase a live person"
        )));
    }

    let mut mtx = maintenance_scope(&state.db, AuthLane).await?;
    let held = organization_members::memberships_of_person(&mut mtx, user_id).await?;
    let owned = held
        .iter()
        .find(|(_, role)| *role == OrganizationRole::Owner)
        .map(|(organization, _)| organization.clone());
    let owned_status = match &owned {
        Some(organization) => organizations::status(&mut mtx, organization).await?,
        None => None,
    };
    mtx.commit().await?;
    if let Some(organization) = owned {
        // ⚠ **An owned organization that is still active will never be
        // deleted**: its deletion was not requested with the account, so the
        // sweep would wait on it forever. Account deletion and ownership
        // accept both take the person's lock to rule this out; seeing it
        // means that broke, and it needs a person.
        if owned_status == Some(OrganizationStatus::Active) {
            return Err(TelmoniError::Internal(format!(
                "{user_id} is being erased but owns the active organization {organization}, \
                 whose deletion was never requested"
            )));
        }
        // The expected wait: the organization's own tail finishes first.
        return Err(AuthError::Conflict(format!(
            "{user_id} still owns {organization}; its deletion finishes first, and the sweep \
             retries this erasure after it"
        ))
        .into());
    }

    state
        .account_holder(user_id)
        .await?
        .delete_user(user_id)
        .await?;

    // The notices that named them, first: the agent's index reads the feed
    // again after its erase below, and must find them already rewritten.
    redact_person_notices(state, user_id).await?;
    // ⚠ **The agent before the memberships go**: the organizations they are
    // in are where their name is scrubbed, and the agent keeps them, so the
    // erase after the removals below, or a retry, still knows them.
    let mut organizations: Vec<OrganizationId> = held
        .iter()
        .map(|(organization, _)| organization.clone())
        .collect();
    erase_in_agent(state, user_id, &person, &organizations).await?;

    let mut mtx = maintenance_scope(&state.db, AuthLane).await?;
    locks::lock_person(&mut mtx, user_id).await?;
    let held = organization_members::memberships_of_person(&mut mtx, user_id).await?;
    if let Some((organization, _)) = held
        .iter()
        .find(|(_, role)| *role == OrganizationRole::Owner)
    {
        return Err(TelmoniError::Internal(format!(
            "{user_id} came to own {organization} while being erased"
        )));
    }
    // Every row this removes: the organization whose chain records it, the
    // project when it was a seat, and the role it held.
    let mut removed: Vec<(OrganizationId, Option<ProjectId>, String)> = Vec::new();
    for (organization, role) in &held {
        locks::lock_organization(&mut mtx, organization).await?;
        let seats = members::memberships_in_organization(&mut mtx, organization, user_id).await?;
        for seat in seats {
            if members::remove(&mut mtx, &seat.project_id, user_id).await? {
                removed.push((
                    organization.clone(),
                    Some(seat.project_id),
                    seat.role.to_string(),
                ));
            }
        }
        if organization_members::remove(&mut mtx, organization, user_id).await? {
            removed.push((organization.clone(), None, role.to_string()));
        }
    }
    // A seat outside any organization they belong to should not exist (the
    // accept enrols every seat holder), but the RESTRICT below would refuse
    // the whole erasure on one, so they are removed and audited too.
    for seat in members::memberships_of(&mut mtx, user_id).await? {
        if members::remove(&mut mtx, &seat.project_id, user_id).await? {
            removed.push((
                seat.organization_id,
                Some(seat.project_id),
                seat.role.to_string(),
            ));
        }
    }
    for (organization, in_project, role) in &removed {
        emit_audit(
            &mut mtx,
            AuditEvent {
                organization_id: organization,
                in_project: in_project.as_ref(),
                actor: Actor::Service("deletion-saga"),
                action: AuditAction::Deleted,
                resource_kind: TelmoniResourceKind::Member,
                resource_id: Some(user_id.as_str()),
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({ "kind": "account_deletion", "role": role })),
            },
        )
        .await?;
    }
    // Their address on the invitations they accepted, which would otherwise
    // stand for as long as the sending organization does. One they never
    // accepted is the sender's to revoke, and it expires.
    let forgotten = invites::forget_accepted_by(&mut mtx, user_id).await?;
    mtx.commit().await?;
    tracing::info!(user_id = %user_id, invitations = forgotten, "accepted invitations deleted");

    // Again, before the identity: a notice that named them since the first
    // pass, while they were still a member, is rewritten too (the pass is
    // idempotent). A retry after a failure here finds the memberships gone
    // and the identity still pending, and takes it from this step.
    redact_person_notices(state, user_id).await?;
    // And the agent again, now that nothing it reads can name them: a turn
    // that read them from the roster before it changed, still answering when
    // the first erase ran, has its answer scrubbed here or withheld when it
    // lands, and a notice indexed between the two passes goes too. Every
    // organization a seat was removed from counts, a stray one's included.
    let reached = held
        .iter()
        .map(|(organization, _)| organization)
        .chain(removed.iter().map(|(organization, _, _)| organization));
    for organization in reached {
        if !organizations.contains(organization) {
            organizations.push(organization.clone());
        }
    }
    erase_in_agent(state, user_id, &person, &organizations).await?;

    // The identity last. Sessions and codes cascade; a membership that landed
    // since the removals above makes this fail (RESTRICT), and the sweep
    // retries.
    let mut tx = person_scope(&state.db, user_id).await?;
    identities::delete(&mut tx, user_id).await?;
    tx.commit().await?;

    tracing::info!(user_id = %user_id, organizations = held.len(), "person erased");
    Ok(ErasureOutcome::Erased)
}

/// The people whose account deletion was confirmed and whose erasure the
/// sweep still owes, oldest request first. Across every person, so the
/// maintenance lane.
pub async fn pending_people(
    state: &AppState,
    limit: i64,
) -> Result<Vec<(UserId, DateTime<Utc>)>, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let rows = identities::list_pending_deletion(&mut tx, limit.clamp(1, 500)).await?;
    tx.commit().await?;
    Ok(rows)
}

/// Run an organization's inline tail and answer the deletion lane: 202
/// either way, since the row waits for `erase_after`; `purged` says whether
/// the hook purge landed in this request, and `erase_after` when the row
/// goes.
pub(crate) async fn deletion_response(
    state: &AppState,
    organization: &OrganizationId,
    erase_after: DateTime<Utc>,
) -> (StatusCode, Json<serde_json::Value>) {
    let purged = matches!(
        run_inline_tail(state, organization).await,
        TailOutcome::Purged
    );
    (
        StatusCode::ACCEPTED,
        Json(json!({ "purged": purged, "erase_after": erase_after })),
    )
}

/// Run an account's tails inline, within one budget, and answer: 200 when
/// the person is gone, 202 when the sweeps finish it.
///
/// A person who owned an organization is always a 202: its hook purge runs
/// here, but its row waits out the finalize grace, and their erasure waits
/// for the row (`erase_person`). That is the expected order, not a failure.
pub(crate) async fn account_deletion_response(
    state: &AppState,
    user_id: &UserId,
    owned: &[OrganizationId],
) -> (StatusCode, Json<serde_json::Value>) {
    let budget = std::time::Duration::from_millis(state.config.deletion_tail_budget_ms);
    let tails = async {
        for organization in owned {
            purge_hook_and_record(state, organization).await?;
        }
        if !owned.is_empty() {
            return Ok::<bool, TelmoniError>(false);
        }
        erase_person(state, user_id).await.map(|_| true)
    };
    let deleted = match tokio::time::timeout(budget, tails).await {
        Ok(Ok(deleted)) => deleted,
        Ok(Err(e)) => {
            tracing::warn!(user_id = %user_id, error = %e,
                "inline account deletion failed; the sweeps finish it");
            false
        }
        Err(_) => {
            tracing::warn!(user_id = %user_id, budget_ms = state.config.deletion_tail_budget_ms,
                "inline account deletion hit its budget; the sweeps finish it");
            false
        }
    };
    let status = if deleted {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    (status, Json(json!({ "deleted": deleted })))
}
