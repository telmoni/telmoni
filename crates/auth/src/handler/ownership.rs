//! Handing an organization over — `/internal/organization/owner-transfer`.
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
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationId, OrganizationRole, TelmoniError,
    TelmoniResourceKind, UserId,
};

use crate::{
    AppState,
    db::{identities, locks, members, organization_members, organizations},
    handler::{organization_of, organization_role_or_forbidden, parse_user_id},
};

/// Body of `POST /internal/organization/owner-transfer`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfferRequest {
    /// The admin to offer the organization to — their user id, as the roster
    /// lists it.
    pub member_id: String,
}

/// A transaction on the organization, holding its lock, with the caller's role
/// read under it. `with_person` takes the caller's own lock first, for the one
/// lane that changes what they own.
///
/// ⚠ **Membership is checked before the lock is taken, and again under it.**
/// The organization's id arrives from the caller, and a lock is a queue: a
/// stranger naming this organization must not be able to stand in it, holding
/// a pooled connection while its roster waits.
async fn locked<'a>(
    state: &'a AppState,
    organization: &OrganizationId,
    user_id: &UserId,
    with_person: bool,
) -> Result<(Scoped<'a, Organization>, OrganizationRole), TelmoniError> {
    let mut tx = organization_scope(&state.db, organization).await?;
    organization_role_or_forbidden(&mut tx, organization, user_id).await?;
    if with_person {
        locks::lock_person(&mut tx, user_id).await?;
    }
    locks::lock_organization(&mut tx, organization).await?;
    let role = organization_role_or_forbidden(&mut tx, organization, user_id).await?;
    Ok((tx, role))
}

/// Whether a caller holding `role` may offer `organization` to `target`,
/// checked in the order a refusal names: only its owner offers it, only once
/// it has a name, and only to one of its admins. Answers the name, for the
/// mail.
///
/// ⚠ **Only a named organization.** The offer's mail and notice call the
/// organization by its name, and an unnamed one has nothing to be called by
/// — nor anyone in it but its owner, since the invite lanes hold the same
/// line. A name can be changed but never cleared, so one checked here is
/// still there at accept.
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
    let Some(name) = organizations::get(tx, organization)
        .await?
        .and_then(|o| o.name)
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
    else {
        return Err(AuthError::Conflict(
            "name the organization before handing it over — the offer calls it by its name, \
             and it has none yet"
                .into(),
        )
        .into());
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

/// `POST /internal/organization/owner-transfer` — the owner offers the
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

    // `locked` refuses an organization being deleted, under its lock.
    let (mut tx, role) = locked(&state, &organization, &user_id, false).await?;
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
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "ownership_offered",
                "to": target.as_str(),
                "replaces": replaced.as_ref().map(UserId::as_str),
            })),
        },
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

/// `DELETE /internal/organization/owner-transfer` — the owner withdraws the
/// offer, and the admin who held it is mailed.
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;

    let (mut tx, role) = locked(&state, &organization, &user_id, false).await?;
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
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": "ownership_offer_cancelled", "to": holder.as_str() })),
        },
    )
    .await?;
    let holder_email = identities::contact(&mut tx, &holder)
        .await?
        .map(|c| c.email);
    let owner = identities::contact(&mut tx, &user_id).await?;
    let name = organizations::get(&mut tx, &organization)
        .await?
        .and_then(|o| o.name);
    tx.commit().await?;

    if let (Some(to), Some(owner)) = (&holder_email, owner) {
        let label = crate::identity::organization_label(name.as_deref());
        if let Err(e) = state
            .mailer
            .send_ownership_offer_withdrawn(to, &owner.display(), &label)
            .await
        {
            // The withdrawal stands without the mail: their console no longer
            // lists the offer.
            tracing::warn!(organization_id = %organization, error = %e, "ownership withdrawal mail failed");
        }
    }

    Ok(Json(json!({
        "offeredTo": holder,
        "offeredToEmail": holder_email,
        "ownerOrganizationId": organization,
    })))
}

/// `POST /internal/organization/owner-transfer/decline` — the admin holding
/// the offer turns it down, and the owner is mailed.
pub async fn decline(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let user_id = principal.user_id;

    let (mut tx, _) = locked(&state, &organization, &user_id, false).await?;
    if !organization_members::decline_offer(&mut tx, &organization, &user_id).await? {
        return Err(AuthError::NotFound("you hold no offer for this organization".into()).into());
    }
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": "ownership_offer_declined" })),
        },
    )
    .await?;
    let owner = match organization_members::owner_of(&mut tx, &organization).await? {
        Some(owner) => identities::contact(&mut tx, &owner).await?,
        None => None,
    };
    let admin = identities::contact(&mut tx, &user_id).await?;
    let name = organizations::get(&mut tx, &organization)
        .await?
        .and_then(|o| o.name);
    tx.commit().await?;

    if let (Some(owner), Some(admin)) = (owner, admin) {
        let label = crate::identity::organization_label(name.as_deref());
        if let Err(e) = state
            .mailer
            .send_ownership_declined(&owner.email, &admin.display(), &label)
            .await
        {
            // The decline stands without the mail: the owner's roster no longer
            // lists the offer.
            tracing::warn!(organization_id = %organization, error = %e, "ownership declined mail failed");
        }
    }

    Ok(Json(json!({ "ownerOrganizationId": organization })))
}

/// `POST /internal/organization/owner-transfer/accept` — the admin holding a
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

    let (tx, role) = locked(&state, &organization, &user_id, true).await?;
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
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Organization,
            resource_id: Some(organization.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "ownership_transferred",
                "from": previous.as_ref().map(UserId::as_str),
                "to": user_id.as_str(),
            })),
        },
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
                AuditEvent {
                    organization_id: &organization,
                    in_project: Some(&seat.project_id),
                    actor: Actor::User(user_id.as_str()),
                    action: AuditAction::Deleted,
                    resource_kind: TelmoniResourceKind::Member,
                    resource_id: Some(user_id.as_str()),
                    request_id: None,
                    ip_address: None,
                    user_agent: None,
                    metadata: Some(json!({
                        "kind": "seat_folded_into_ownership",
                        "role": seat.role.to_string(),
                    })),
                },
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
        .and_then(|o| o.name);
    tx.commit().await?;

    if let (Some(previous), Some(new_owner)) = (previous_contact, new_owner) {
        let label = crate::identity::organization_label(name.as_deref());
        if let Err(e) = state
            .mailer
            .send_ownership_accepted(&previous.email, &new_owner.display(), &label)
            .await
        {
            // The transfer stands without the mail: the previous owner's
            // console shows them as an admin from their next page.
            tracing::warn!(organization_id = %organization, error = %e, "ownership accepted mail failed");
        }
    }

    tracing::info!(organization_id = %organization, to = %user_id, "ownership transferred");
    Ok(Json(json!({
        "ownerOrganizationId": organization,
        "previousOwner": previous,
    })))
}
