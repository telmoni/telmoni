//! Handing an organization over — `/internal/v1/organization/owner-transfer`.
//!
//! The owner offers the organization to one of its ADMINS; the admin accepts or
//! declines; the owner can withdraw it. On accept the old owner becomes an
//! admin (and may then leave) and the admin becomes the owner, in one
//! transaction. The API keys, the projects and the name stay with the
//! organization: nothing but the two roster rows moves. A withdrawal,
//! a replacement and an answer are mailed to whoever the offer leaves waiting,
//! because the console reaches them only in an open tab. An offer ended any
//! other way (a demotion, a removal, the organization's deletion, seven days
//! passing) mails nobody.
//!
//! One live offer per organization, dead after `OFFER_TTL_DAYS`. It lives on
//! the admin's roster row (`transfer_offered_at`), so removing them, their
//! leaving, or demoting them ends it with no bookkeeping of its own.
//!
//! A deployment with an ownership hook ([`crate::Siblings::ownership_hook`])
//! has a say on the admin coming to own it, asked before the offer and again
//! before the accept, and is told of the accept once it commits.
//!
//! ⚠ **Every lane here takes the organization's lock and reads the roles again
//! under it.** Accept races removal, demotion, a second offer and deletion;
//! without the lock a removal blocked behind an accept would re-check its WHERE
//! against the new owner's row and act on them (see `db::locks`).

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{Organization, Scoped, organization_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::seam::OwnershipChange;
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationId, OrganizationRole, TelmoniError,
    TelmoniResourceKind, UserId,
};

use crate::{
    AppState,
    db::{identities, locks, members, organization_members, organizations},
    handler::{organization_of, organization_role_or_forbidden, parse_user_id},
    model::AuditKind,
};

/// Body of `POST /internal/v1/organization/owner-transfer`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfferRequest {
    /// The admin to offer the organization to — their user id, as the roster
    /// lists it.
    pub member_id: String,
}

/// The longest auth waits on the deployment's ownership hook, asked or told.
/// The console abandons the whole request at ten seconds.
const HOOK_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

/// The deployment's say on whether `recipient` may come to own the
/// organization ([`telmoni_shared::seam::OwnershipHook`]): no hook is a yes,
/// and so is one that does not answer within [`HOOK_BUDGET`], logged, since
/// nothing it guards costs more than its outage lasts. Asked before any lock:
/// the hook reads auth back on auth's own pool.
async fn may_take_ownership(
    state: &AppState,
    organization: &OrganizationId,
    recipient: &UserId,
) -> Result<(), TelmoniError> {
    let Some(hook) = state.siblings.ownership_hook.as_ref() else {
        return Ok(());
    };
    match tokio::time::timeout(
        HOOK_BUDGET,
        hook.may_take_ownership(organization, recipient),
    )
    .await
    {
        Ok(answer) => answer,
        Err(_) => {
            tracing::warn!(organization_id = %organization,
                "the ownership hook did not answer in time; the change is let through");
            Ok(())
        }
    }
}

/// The deployment's say on whether `founder` may found another organization
/// at their asking ([`telmoni_shared::seam::OwnershipHook::may_found`]): no
/// hook is a yes, and so is one that does not answer within [`HOOK_BUDGET`],
/// logged, since a founding let through costs no more than a second
/// organization. Asked before any lock, as every ask of the hook is.
pub(crate) async fn may_found(state: &AppState, founder: &UserId) -> Result<(), TelmoniError> {
    let Some(hook) = state.siblings.ownership_hook.as_ref() else {
        return Ok(());
    };
    match tokio::time::timeout(HOOK_BUDGET, hook.may_found(founder)).await {
        Ok(answer) => answer,
        Err(_) => {
            tracing::warn!(user_id = %founder,
                "the ownership hook did not answer in time; the founding is let through");
            Ok(())
        }
    }
}

/// Tell the deployment's ownership hook of a change that has committed. It
/// stands whatever the hook answers, so a refusal or a silence past
/// [`HOOK_BUDGET`] is logged and nothing more: the hook's own catching up
/// covers a change it missed.
pub(crate) async fn tell_ownership_hook(state: &AppState, change: OwnershipChange) {
    let Some(hook) = state.siblings.ownership_hook.as_ref() else {
        return;
    };
    let organization = match &change {
        OwnershipChange::Founded {
            organization_id, ..
        }
        | OwnershipChange::HandedOver {
            organization_id, ..
        } => organization_id.clone(),
    };
    match tokio::time::timeout(HOOK_BUDGET, hook.ownership_changed(change)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(organization_id = %organization, error = %e,
                "the ownership hook failed on a change; it catches up on its own");
        }
        Err(_) => {
            tracing::warn!(organization_id = %organization,
                "the ownership hook did not answer in time; it catches up on its own");
        }
    }
}

/// Whether the caller holds the organization's live offer, read before any
/// lock and without one, for deciding whether the hook is asked at all: only
/// somebody who could take the organization is shown its answer. A stranger
/// is refused here as `locked` would refuse them.
async fn holds_the_offer(
    state: &AppState,
    organization: &OrganizationId,
    user_id: &UserId,
) -> Result<bool, TelmoniError> {
    let mut tx = organization_scope(&state.db, organization).await?;
    organization_role_or_forbidden(&mut tx, organization, user_id).await?;
    let holder = organization_members::live_offer_holder(&mut tx, organization).await?;
    tx.commit().await?;
    Ok(holder.as_ref() == Some(user_id))
}

/// Which locks [`locked`] takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lock {
    /// The organization's alone.
    Organization,
    /// The caller's own first, then the organization's: for the one lane that
    /// changes what they own.
    PersonThenOrganization,
}

/// A transaction on the organization, holding its lock, with the caller's role
/// read under it, and the caller's own lock too when `lock` asks for it.
///
/// ⚠ **Membership is checked before the lock is taken, and again under it.**
/// The organization's id arrives from the caller, and a lock is a queue: a
/// stranger naming this organization must not be able to stand in it, holding
/// a pooled connection while its roster waits.
async fn locked<'a>(
    state: &'a AppState,
    organization: &OrganizationId,
    user_id: &UserId,
    lock: Lock,
) -> Result<(Scoped<'a, Organization>, OrganizationRole), TelmoniError> {
    let mut tx = organization_scope(&state.db, organization).await?;
    organization_role_or_forbidden(&mut tx, organization, user_id).await?;
    if lock == Lock::PersonThenOrganization {
        locks::lock_person(&mut tx, user_id).await?;
    }
    locks::lock_organization(&mut tx, organization).await?;
    let role = organization_role_or_forbidden(&mut tx, organization, user_id).await?;
    Ok((tx, role))
}

/// Whether a caller holding `role` may offer `organization` to `target`,
/// checked in the order a refusal names: only its owner offers it, and only to
/// one of its admins. Answers the name, for the mail.
async fn offerable(
    tx: &mut Scoped<'_, Organization>,
    role: OrganizationRole,
    organization: &OrganizationId,
    target: &UserId,
) -> Result<String, TelmoniError> {
    if role != OrganizationRole::Owner {
        return Err(
            AuthzError::Forbidden("only the organization's owner can hand it over".into()).into(),
        );
    }
    let Some(name) = organizations::get(tx, organization).await?.map(|o| o.name) else {
        return Err(AuthError::NotFound("organization not found".into()).into());
    };
    match organization_members::role_on_organization(tx, organization, target).await? {
        None => Err(AuthError::NotFound("no such organization member".into()).into()),
        Some(OrganizationRole::Admin) => Ok(name),
        Some(_) => Err(AuthError::Conflict(
            "only an admin can be offered the organization — make them an admin first".into(),
        )
        .into()),
    }
}

/// `POST /internal/v1/organization/owner-transfer` — the owner offers the
/// organization to an admin. Withdraws any offer already out, because there is
/// only ever one, and mails the admin who held it.
///
/// ⚠ **Nothing here asks whether the admin is deleting their account.** That
/// is theirs: it lives in `auth.accounts`, which no organization may read, and
/// a refusal naming it would tell the owner. Such an offer can never be taken
/// up — accept refuses somebody leaving, under their own lock — and their
/// erasure removes the roster row it lives on once the finalize grace
/// (`deletion::FINALIZE_GRACE_SECONDS`) and the next sweep have passed.
pub async fn offer(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<OfferRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;
    let target = parse_user_id(&req.member_id)?;
    if target == user_id {
        return Err(AuthError::BadRequest("you already own this organization".into()).into());
    }

    // The deployment's say on the admin coming to own it, before any lock,
    // and asked only for its owner offering it to one of its admins: anybody
    // else is refused below by auth's own rules, and learns nothing from it.
    if state.siblings.ownership_hook.is_some() {
        let mut tx = organization_scope(&state.db, &organization).await?;
        let role = organization_role_or_forbidden(&mut tx, &organization, &user_id).await?;
        let target_role =
            organization_members::role_on_organization(&mut tx, &organization, &target).await?;
        tx.commit().await?;
        if role == OrganizationRole::Owner && target_role == Some(OrganizationRole::Admin) {
            may_take_ownership(&state, &organization, &target).await?;
        }
    }

    // `locked` refuses an organization being deleted, under its lock.
    let (mut tx, role) = locked(&state, &organization, &user_id, Lock::Organization).await?;
    let name = offerable(&mut tx, role, &organization, &target).await?;

    // Somebody else's live offer is withdrawn by this one, and they are told:
    // the chain names them, the console refreshes their notifications, and
    // they are mailed.
    let replaced = organization_members::live_offer_holder(&mut tx, &organization)
        .await?
        .filter(|holder| *holder != target);
    organization_members::clear_offers(&mut tx, &organization).await?;
    if !organization_members::offer_to(&mut tx, &organization, &target).await? {
        return Err(AuthError::Conflict("that member is no longer an admin".into()).into());
    }
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Organization,
        )
        .on(organization.as_str())
        .details(json!({
            "kind": AuditKind::OwnershipOffered,
            "to": target.as_str(),
            "replaces": replaced.as_ref().map(UserId::as_str),
        })),
    )
    .await?;

    // All three are on the roster, which is what the organization's scope may
    // see of `auth.identities`.
    let recipient = identities::contact(&mut tx, &target).await?;
    let owner = identities::contact(&mut tx, &user_id).await?;
    let withdrawn_from_email = match &replaced {
        Some(holder) => identities::contact(&mut tx, holder).await?.map(|c| c.email),
        None => None,
    };
    tx.commit().await?;

    let expires_at = Utc::now() + Duration::days(i64::from(organization_members::OFFER_TTL_DAYS));
    // For the console's live notices and the mail: from the roster, never from
    // the browser that asked.
    let recipient_email = recipient.map(|r| r.email);
    if let Some(owner) = owner.map(|o| o.display()) {
        let link = format!(
            "{}/account/notifications",
            state.config.app_url.trim_end_matches('/')
        );
        // Side by side: each can wait out the mail client's timeout, and the
        // console abandons the whole request at ten seconds.
        let (offered, withdrawn) = tokio::join!(
            async {
                match &recipient_email {
                    Some(to) => {
                        state
                            .mailer
                            .send_ownership_offer(to, &owner, &name, &link)
                            .await
                    }
                    None => Ok(()),
                }
            },
            async {
                match &withdrawn_from_email {
                    Some(to) => {
                        state
                            .mailer
                            .send_ownership_offer_withdrawn(to, &owner, &name)
                            .await
                    }
                    None => Ok(()),
                }
            },
        );
        // Either stands without its mail: the recipient's console lists the
        // offer, and the replaced holder's no longer does.
        if let Err(e) = offered {
            tracing::warn!(organization_id = %organization, error = %e, "ownership offer mail failed");
        }
        if let Err(e) = withdrawn {
            tracing::warn!(organization_id = %organization, error = %e, "ownership withdrawal mail failed");
        }
    }

    tracing::info!(organization_id = %organization, to = %target, "ownership offered");
    Ok((
        StatusCode::OK,
        Json(json!({
            "offeredTo": target,
            "offeredToEmail": recipient_email,
            "withdrawnFromEmail": withdrawn_from_email,
            "expiresAt": expires_at,
            "ownerOrganizationId": organization,
        })),
    ))
}

/// `DELETE /internal/v1/organization/owner-transfer` — the owner withdraws the
/// offer, and the admin who held it is mailed.
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;

    let (mut tx, role) = locked(&state, &organization, &user_id, Lock::Organization).await?;
    if role != OrganizationRole::Owner {
        return Err(AuthzError::Forbidden(
            "only the organization's owner can withdraw its offer".into(),
        )
        .into());
    }
    let Some(holder) = organization_members::live_offer_holder(&mut tx, &organization).await?
    else {
        return Err(AuthError::NotFound("there is no offer to withdraw".into()).into());
    };
    organization_members::clear_offers(&mut tx, &organization).await?;
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Organization,
        )
        .on(organization.as_str())
        .details(json!({
            "kind": AuditKind::OwnershipOfferCancelled,
            "to": holder.as_str(),
        })),
    )
    .await?;
    let holder_email = identities::contact(&mut tx, &holder)
        .await?
        .map(|c| c.email);
    let owner = identities::contact(&mut tx, &user_id).await?;
    let name = organizations::get(&mut tx, &organization)
        .await?
        .map(|o| o.name);
    tx.commit().await?;

    if let (Some(to), Some(owner), Some(name)) = (&holder_email, owner, name)
        && let Err(e) = state
            .mailer
            .send_ownership_offer_withdrawn(to, &owner.display(), &name)
            .await
    {
        // The withdrawal stands without the mail: their console no longer
        // lists the offer.
        tracing::warn!(organization_id = %organization, error = %e, "ownership withdrawal mail failed");
    }

    Ok(Json(json!({
        "offeredTo": holder,
        "offeredToEmail": holder_email,
        "ownerOrganizationId": organization,
    })))
}

/// `POST /internal/v1/organization/owner-transfer/decline` — the admin holding
/// the offer turns it down, and the owner is mailed.
pub async fn decline(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;

    let (mut tx, _) = locked(&state, &organization, &user_id, Lock::Organization).await?;
    if !organization_members::decline_offer(&mut tx, &organization, &user_id).await? {
        return Err(AuthError::NotFound("you hold no offer for this organization".into()).into());
    }
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Organization,
        )
        .on(organization.as_str())
        .details(json!({ "kind": AuditKind::OwnershipOfferDeclined })),
    )
    .await?;
    let owner = match organization_members::owner_of(&mut tx, &organization).await? {
        Some(owner) => identities::contact(&mut tx, &owner).await?,
        None => None,
    };
    let admin = identities::contact(&mut tx, &user_id).await?;
    let name = organizations::get(&mut tx, &organization)
        .await?
        .map(|o| o.name);
    tx.commit().await?;

    if let (Some(owner), Some(admin), Some(name)) = (owner, admin, name)
        && let Err(e) = state
            .mailer
            .send_ownership_declined(&owner.email, &admin.display(), &name)
            .await
    {
        // The decline stands without the mail: the owner's roster no longer
        // lists the offer.
        tracing::warn!(organization_id = %organization, error = %e, "ownership declined mail failed");
    }

    Ok(Json(json!({ "ownerOrganizationId": organization })))
}

/// `POST /internal/v1/organization/owner-transfer/accept` — the admin holding a
/// live offer becomes the owner, and the owner becomes an admin and is mailed:
/// what only the owner decides is the new owner's now.
///
/// Two statements because a partial unique index cannot be deferred: demote
/// whoever owns it now, then promote the caller and spend the offer in one
/// UPDATE whose WHERE re-checks everything — still an admin, the offer still
/// live, the organization still active. If that matches nothing the
/// transaction rolls back and the demotion with it.
///
/// ⚠ **The caller's own lock first, then the organization's** — the order
/// account deletion takes them in. Without it an accept could land after the
/// caller's account deletion read what they own, leaving a shared organization
/// with an owner who can never sign in to hand it on and whose erasure can
/// never finish. Under the lock, somebody on their way out is refused.
pub async fn accept(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;

    // The deployment's say, before any lock, asked only for whoever holds the
    // live offer; the offer itself is checked again under the locks.
    if state.siblings.ownership_hook.is_some()
        && holds_the_offer(&state, &organization, &user_id).await?
    {
        may_take_ownership(&state, &organization, &user_id).await?;
    }

    let (tx, role) = locked(
        &state,
        &organization,
        &user_id,
        Lock::PersonThenOrganization,
    )
    .await?;
    if role == OrganizationRole::Owner {
        return Err(AuthError::BadRequest("you already own this organization".into()).into());
    }
    // ⚠ The acceptor's own scope as well as the organization's: whether they
    // are deleting their account is in `auth.accounts`, which no
    // organization may read, and under the organization's scope alone the
    // row is invisible and reads as "not leaving".
    let mut tx = tx.bind_person(&user_id).await?;
    if identities::get(&mut tx, &user_id)
        .await?
        .is_none_or(|p| p.deletion_requested_at.is_some())
    {
        return Err(AuthError::Conflict(
            "your account is being deleted — it cannot take an organization on".into(),
        )
        .into());
    }

    let previous = organization_members::demote_owner(&mut tx, &organization, &user_id).await?;
    if !organization_members::promote_offer_holder(&mut tx, &organization, &user_id).await? {
        tx.rollback().await?;
        return Err(AuthError::Conflict(
            "this offer is no longer open — it was withdrawn, it lapsed, or the organization \
             is being deleted"
                .into(),
        )
        .into());
    }
    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(user_id.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Organization,
        )
        .on(organization.as_str())
        .details(json!({
            "kind": AuditKind::OwnershipTransferred,
            "from": previous.as_ref().map(UserId::as_str),
            "to": user_id.as_str(),
        })),
    )
    .await?;

    // ⚠ **Seats the new owner held are folded into the ownership.** The owner
    // row is Owner on every project, and every seat lane refuses to touch the
    // owner, so a seat left behind could be neither changed nor left — and if
    // they hand the organization on, it would outrank the admin fallback and
    // cap them at whatever it said. Their seats are visible to them under
    // `app.user_id`; each is removed under its own project's scope.
    let seats = members::memberships_in_organization(&mut tx, &organization, &user_id).await?;
    for seat in &seats {
        let mut ptx = tx.bind_project(&seat.project_id).await?;
        if members::remove(&mut ptx, &seat.project_id, &user_id).await? {
            emit_audit(
                &mut ptx,
                AuditEvent::new(
                    &organization,
                    Actor::User(user_id.as_str()),
                    AuditAction::Deleted,
                    TelmoniResourceKind::Member,
                )
                .in_project(&seat.project_id)
                .on(user_id.as_str())
                .details(json!({
                    "kind": AuditKind::SeatFoldedIntoOwnership,
                    "role": seat.role,
                })),
            )
            .await?;
        }
        tx = ptx.clear_project().await?;
    }
    let previous_contact = match &previous {
        Some(previous) => identities::contact(&mut tx, previous).await?,
        None => None,
    };
    let new_owner = identities::contact(&mut tx, &user_id).await?;
    let name = organizations::get(&mut tx, &organization)
        .await?
        .map(|o| o.name);
    tx.commit().await?;

    tell_ownership_hook(
        &state,
        OwnershipChange::HandedOver {
            organization_id: organization.clone(),
            from: previous.clone(),
            to: user_id.clone(),
        },
    )
    .await;

    if let (Some(previous), Some(new_owner), Some(name)) = (previous_contact, new_owner, name)
        && let Err(e) = state
            .mailer
            .send_ownership_accepted(&previous.email, &new_owner.display(), &name)
            .await
    {
        // The transfer stands without the mail: the previous owner's console
        // shows them as an admin from their next page.
        tracing::warn!(organization_id = %organization, error = %e, "ownership accepted mail failed");
    }

    tracing::info!(organization_id = %organization, to = %user_id, "ownership transferred");
    Ok(Json(json!({
        "ownerOrganizationId": organization,
        "previousOwner": previous,
    })))
}
