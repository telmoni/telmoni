//! Handing a project over — `/internal/projects/{project_id}/transfer`.
//!
//! A project's owner is its organization's owner, so handing a project over
//! means moving it into an organization the new owner owns. The owner offers
//! it to one of the project's ADMINS; the admin accepts it into an
//! organization they own, or declines; the owner can withdraw it. On accept
//! the project row changes organization in one transaction: its API keys
//! follow (`api_tokens_project_fkey` cascades), its seats and invitations stay
//! on it and every seat holder is enrolled on the new organization's roster
//! as a project invitation would enrol them, the acceptor's seat is folded
//! into the ownership they now hold, and the previous owner is seated as an
//! admin — the mirror of the organization handover, where the previous owner
//! stays on as an admin. Its connectors do
//! not come with it: they are the old organization's vendor grants and webhook
//! secrets, purged in notifications before the move commits; the agent finds
//! what it holds of the project under the old organization and removes it
//! within the hour. The audit chains
//! stay where they are — the old organization's keeps the project's history
//! and the new one's starts with its arrival — because a chain cannot be
//! spliced. A withdrawal, a replacement and an answer are mailed to whoever
//! the offer leaves waiting, as the organization's are.
//!
//! One live offer per project, dead after `OFFER_TTL_DAYS`. It lives on the
//! admin's seat (`transfer_offered_at`), so removing them, their leaving, or
//! demoting them ends it with no bookkeeping of its own.
//!
//! ⚠ **Offer, withdraw and decline take the owning organization's lock and
//! read the roles again under it; accept takes the caller's own lock, then
//! both organizations' in id order** — the order account deletion takes them
//! in — and re-reads every role and status under them (see `db::locks`).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, maintenance_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationId, OrganizationRole, OrganizationStatus,
    ProjectId, Role, TelmoniError, TelmoniResourceKind, UserId,
};

use crate::{
    AppState,
    db::{AuthLane, identities, locks, members, organization_members, organizations, projects},
    handler::{
        ActingProject, BEING_DELETED, acting_project, parse_organization_id, parse_project_id,
        parse_user_id, projects::MAX_PROJECT_NAME, refuse_unless_active,
    },
};

/// Body of `POST /internal/projects/{project_id}/transfer`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfferRequest {
    /// The admin to offer the project to — their user id, as the roster
    /// lists it.
    pub member_id: String,
}

/// Body of `POST /internal/projects/{project_id}/transfer/accept`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptRequest {
    /// Which organization of the caller's the project lands in. Optional for
    /// somebody who owns exactly one; required, and refused unless theirs,
    /// for somebody who owns several.
    #[serde(default)]
    pub organization_id: Option<String>,
}

/// The project's transaction with its owning organization bound and locked,
/// and the caller's role on the project read again under the lock.
///
/// ⚠ **Membership is checked before the lock is taken, and again under it.**
/// The project's id arrives from the caller, and a lock is a queue: a
/// stranger naming this project must not be able to stand in it, holding a
/// pooled connection while its organization's roster waits.
async fn locked<'a>(
    state: &'a AppState,
    project_id: &ProjectId,
    user_id: &UserId,
) -> Result<(ActingProject<'a, ProjectAndOrganization>, Role), TelmoniError> {
    let acting = acting_project(state, project_id, user_id).await?;
    let mut acting = acting.enter_owner_scope().await?;
    let organization = acting.organization.clone();
    locks::lock_organization(&mut acting.tx, &organization).await?;
    let on_organization =
        organization_members::role_on_organization(&mut acting.tx, &organization, user_id).await?;
    let seat = members::role_on(&mut acting.tx, project_id, user_id).await?;
    let Some(role) = telmoni_shared::rbac::project_role(on_organization, seat) else {
        return Err(AuthzError::Forbidden("you are not a member of this project".into()).into());
    };
    refuse_unless_active(&mut acting.tx, &organization).await?;
    Ok((acting, role))
}

/// A name ending in a plain number from 2 up, as `"Payments 2"`: the part
/// before the number, and the number. `"Project 007"`, `"Project 1"` and
/// `"Project +5"` are not.
fn numbered(name: &str) -> Option<(&str, u32)> {
    let (base, digits) = name.rsplit_once(' ')?;
    let n: u32 = digits.parse().ok()?;
    let base = base.trim_end();
    (n >= 2 && n.to_string() == digits && !base.is_empty()).then_some((base, n))
}

/// The names a project may land under, best first: its own, then numbered
/// ones, each cut short to fit `MAX_PROJECT_NAME`. Two organizations name
/// their projects alike all the time — "Web", "API", "Payments" — so refusing
/// a clash would refuse the ordinary handover.
///
/// The numbers are `"{name} 2"`, `"{name} 3"`, … unless `family` names the
/// series the name already belongs to: `"Payments 2"` arriving where
/// "Payments" is continues as `"Payments 3"`. The caller only
/// passes a family whose base the destination holds, so `"Project 2024"`
/// becomes `"Project 2024 2"`, not a different year.
///
/// `held` is how many projects the destination holds. The numbered names end
/// in different numbers, so they differ from each other, and at most one
/// equals the name itself (a full-length name ending in " 2"): of the
/// `held + 2` names, one is free.
fn landing_names(wanted: &str, held: usize, family: Option<(&str, u32)>) -> Vec<String> {
    let (base, first) = family.map_or((wanted, 2), |(base, n)| (base, u64::from(n) + 1));
    std::iter::once(wanted.to_owned())
        .chain((first..).take(held + 1).map(|n| {
            let suffix = format!(" {n}");
            let room = MAX_PROJECT_NAME.saturating_sub(suffix.chars().count());
            let cut: String = base.chars().take(room).collect();
            format!("{}{suffix}", cut.trim_end())
        }))
        .collect()
}

/// The refusal an accept can still trip on the destination's name index, as
/// a 409 naming the project, not a 500: the destination's lock keeps creates
/// out, but a rename does not take it. It does not name the organization: the
/// page it is read on names it already.
fn name_taken_in(e: sqlx::Error, name: &str) -> TelmoniError {
    match e {
        sqlx::Error::Database(ref db)
            if db.constraint() == Some("projects_organization_name_key") =>
        {
            AuthError::Conflict(format!(
                "a project named \"{name}\" already exists in the organization it would land \
                 in — rename one of them first"
            ))
            .into()
        }
        other => other.into(),
    }
}

async fn notify_project_offer(
    state: &AppState,
    project_id: &ProjectId,
    project_name: &str,
    organization_label: &str,
    owner_display: &str,
    recipient_email: Option<&str>,
    withdrawn_from_email: Option<&str>,
) {
    let link = format!(
        "{}/account/notifications",
        state.config.app_url.trim_end_matches('/')
    );
    // Side by side: each can wait out the mail client's timeout, and the
    // console abandons the whole request at ten seconds.
    let (offered, withdrawn) = tokio::join!(
        async {
            match recipient_email {
                Some(to) => {
                    state
                        .mailer
                        .send_project_offer(
                            to,
                            owner_display,
                            project_name,
                            organization_label,
                            &link,
                        )
                        .await
                }
                None => Ok(()),
            }
        },
        async {
            match withdrawn_from_email {
                Some(to) => {
                    state
                        .mailer
                        .send_project_offer_withdrawn(to, owner_display, project_name)
                        .await
                }
                None => Ok(()),
            }
        },
    );
    // Either stands without its mail: the recipient's console lists the
    // offer, and the replaced holder's no longer does.
    if let Err(e) = offered {
        tracing::warn!(project_id = %project_id, error = %e, "project offer mail failed");
    }
    if let Err(e) = withdrawn {
        tracing::warn!(project_id = %project_id, error = %e, "project offer withdrawal mail failed");
    }
}

/// `POST /internal/projects/{project_id}/transfer` — the owner offers the
/// project to one of its admins. Withdraws any offer already out, because
/// there is only ever one, and mails the admin who held it.
///
/// ⚠ **Nothing here asks whether the admin is deleting their account**, for
/// the reason the organization's offer does not: that is theirs, and a
/// refusal naming it would tell the owner. Accept refuses them under their
/// own lock.
pub async fn offer(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    principal: Principal,
    Json(req): Json<OfferRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let user_id = principal.user_id;
    let target = parse_user_id(&req.member_id)?;
    if target == user_id {
        return Err(AuthError::BadRequest("you already own this project".into()).into());
    }

    let (mut acting, role) = locked(&state, &project_id, &user_id).await?;
    if role != Role::Owner {
        return Err(AuthzError::Forbidden(
            "only the project's owner — its organization's owner — can hand it over".into(),
        )
        .into());
    }
    match members::role_on(&mut acting.tx, &project_id, &target).await? {
        None => return Err(AuthError::NotFound("no such project member".into()).into()),
        Some(Role::Admin) => {}
        Some(_) => {
            return Err(AuthError::Conflict(
                "only an admin can be offered the project — make them an admin first".into(),
            )
            .into());
        }
    }
    let Some(standing) = projects::standing(&mut acting.tx, &project_id).await? else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };
    let organization = acting.organization.clone();

    // Somebody else's live offer is withdrawn by this one, and they are told:
    // the chain names them, the console refreshes their notifications, and
    // they are mailed.
    let replaced = members::live_project_offer_holder(&mut acting.tx, &project_id)
        .await?
        .filter(|holder| *holder != target);
    members::clear_project_offers(&mut acting.tx, &project_id).await?;
    if !members::offer_project_to(&mut acting.tx, &project_id, &target).await? {
        return Err(AuthError::Conflict("that member is no longer an admin".into()).into());
    }
    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &organization,
            in_project: Some(&project_id),
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Project,
            resource_id: Some(project_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "project_offered",
                "to": target.as_str(),
                "replaces": replaced.as_ref().map(UserId::as_str),
            })),
        },
    )
    .await?;

    // Every seat holder is on the owning organization's roster, which is what
    // its scope may see of `auth.identities`.
    let recipient = identities::contact(&mut acting.tx, &target).await?;
    let owner = identities::contact(&mut acting.tx, &user_id).await?;
    let withdrawn_from_email = match &replaced {
        Some(holder) => identities::contact(&mut acting.tx, holder)
            .await?
            .map(|c| c.email),
        None => None,
    };
    let Some(organization_label) = organization_members::label_parts(&mut acting.tx, &organization)
        .await?
        .map(|parts| parts.name)
    else {
        return Err(AuthError::NotFound("organization not found".into()).into());
    };
    acting.tx.commit().await?;

    let expires_at = Utc::now() + Duration::days(i64::from(members::OFFER_TTL_DAYS));
    // For the console's live notices and the mail: from the roster, never from
    // the browser that asked.
    let recipient_email = recipient.map(|r| r.email);
    if let Some(owner) = owner.map(|o| o.display()) {
        notify_project_offer(
            &state,
            &project_id,
            &standing.name,
            &organization_label,
            &owner,
            recipient_email.as_deref(),
            withdrawn_from_email.as_deref(),
        )
        .await;
    }

    tracing::info!(project_id = %project_id, to = %target, "project offered");
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

/// `DELETE /internal/projects/{project_id}/transfer` — the owner withdraws
/// the offer, and the admin who held it is mailed.
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let user_id = principal.user_id;

    let (mut acting, role) = locked(&state, &project_id, &user_id).await?;
    if role != Role::Owner {
        return Err(AuthzError::Forbidden(
            "only the project's owner can withdraw its offer".into(),
        )
        .into());
    }
    let Some(holder) = members::live_project_offer_holder(&mut acting.tx, &project_id).await?
    else {
        return Err(AuthError::NotFound("there is no offer to withdraw".into()).into());
    };
    let organization = acting.organization.clone();
    members::clear_project_offers(&mut acting.tx, &project_id).await?;
    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &organization,
            in_project: Some(&project_id),
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Project,
            resource_id: Some(project_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": "project_offer_cancelled", "to": holder.as_str() })),
        },
    )
    .await?;
    let holder_email = identities::contact(&mut acting.tx, &holder)
        .await?
        .map(|c| c.email);
    let owner = identities::contact(&mut acting.tx, &user_id).await?;
    let standing = projects::standing(&mut acting.tx, &project_id).await?;
    acting.tx.commit().await?;

    if let (Some(to), Some(owner), Some(standing)) = (&holder_email, owner, standing)
        && let Err(e) = state
            .mailer
            .send_project_offer_withdrawn(to, &owner.display(), &standing.name)
            .await
    {
        // The withdrawal stands without the mail: their console no longer
        // lists the offer.
        tracing::warn!(project_id = %project_id, error = %e, "project offer withdrawal mail failed");
    }

    Ok(Json(json!({
        "offeredTo": holder,
        "offeredToEmail": holder_email,
        "ownerOrganizationId": organization,
    })))
}

/// `POST /internal/projects/{project_id}/transfer/decline` — the admin
/// holding the offer turns it down, and the owner is mailed.
pub async fn decline(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let user_id = principal.user_id;

    let (mut acting, _) = locked(&state, &project_id, &user_id).await?;
    if !members::decline_project_offer(&mut acting.tx, &project_id, &user_id).await? {
        return Err(AuthError::NotFound("you hold no offer for this project".into()).into());
    }
    let organization = acting.organization.clone();
    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &organization,
            in_project: Some(&project_id),
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Project,
            resource_id: Some(project_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": "project_offer_declined" })),
        },
    )
    .await?;
    let owner = match organization_members::owner_of(&mut acting.tx, &organization).await? {
        Some(owner) => identities::contact(&mut acting.tx, &owner).await?,
        None => None,
    };
    let admin = identities::contact(&mut acting.tx, &user_id).await?;
    let standing = projects::standing(&mut acting.tx, &project_id).await?;
    acting.tx.commit().await?;

    if let (Some(owner), Some(admin), Some(standing)) = (owner, admin, standing)
        && let Err(e) = state
            .mailer
            .send_project_declined(&owner.email, &admin.display(), &standing.name)
            .await
    {
        // The decline stands without the mail: the owner's roster no longer
        // lists the offer.
        tracing::warn!(project_id = %project_id, error = %e, "project declined mail failed");
    }

    Ok(Json(json!({ "ownerOrganizationId": organization })))
}

struct TransferAudits<'a> {
    source: &'a OrganizationId,
    destination: &'a OrganizationId,
    project_id: &'a ProjectId,
    user_id: &'a UserId,
    previous: &'a UserId,
    project_name: &'a str,
    /// The name it was offered under, when a clash in the destination
    /// renamed it on the way in.
    renamed_from: Option<&'a str>,
    seated: bool,
    enrolled: bool,
    holders_enrolled: &'a [&'a members::Seat],
}

async fn emit_project_transfer_audits(
    conn: &mut sqlx::PgConnection,
    audits: TransferAudits<'_>,
) -> Result<(), TelmoniError> {
    // Both chains, the source's first. Every other writer of two chains takes
    // the same membership locks this holds, so the chain locks cannot cycle.
    emit_audit(
        conn,
        AuditEvent {
            organization_id: audits.source,
            in_project: Some(audits.project_id),
            actor: Actor::User(audits.user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Project,
            resource_id: Some(audits.project_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "project_transferred",
                "to_organization": audits.destination.as_str(),
                "to": audits.user_id.as_str(),
                "from": audits.previous.as_str(),
            })),
        },
    )
    .await?;
    emit_audit(
        conn,
        AuditEvent {
            organization_id: audits.destination,
            in_project: Some(audits.project_id),
            actor: Actor::User(audits.user_id.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Project,
            resource_id: Some(audits.project_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some({
                let mut received = json!({
                    "kind": "project_received",
                    "from_organization": audits.source.as_str(),
                    "from": audits.previous.as_str(),
                    "to": audits.user_id.as_str(),
                    "name": audits.project_name,
                });
                // Only when it happened: the console's audit table prints
                // metadata whole, and a `null` on every handover is noise.
                if let (Some(renamed_from), Some(fields)) =
                    (audits.renamed_from, received.as_object_mut())
                {
                    fields.insert("renamed_from".into(), json!(renamed_from));
                }
                received
            }),
        },
    )
    .await?;
    // The seat the new owner held is folded into the ownership.
    emit_audit(
        conn,
        AuditEvent {
            organization_id: audits.destination,
            in_project: Some(audits.project_id),
            actor: Actor::User(audits.user_id.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(audits.user_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "seat_folded_into_ownership",
                "role": Role::Admin.to_string(),
            })),
        },
    )
    .await?;
    if audits.seated {
        emit_audit(
            conn,
            AuditEvent {
                organization_id: audits.destination,
                in_project: Some(audits.project_id),
                actor: Actor::User(audits.user_id.as_str()),
                action: AuditAction::Created,
                resource_kind: TelmoniResourceKind::Member,
                resource_id: Some(audits.previous.as_str()),
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({
                    "kind": "previous_owner_seated",
                    "role": Role::Admin.to_string(),
                })),
            },
        )
        .await?;
    }
    if audits.enrolled {
        emit_audit(
            conn,
            AuditEvent {
                organization_id: audits.destination,
                in_project: None,
                actor: Actor::User(audits.user_id.as_str()),
                action: AuditAction::Created,
                resource_kind: TelmoniResourceKind::Member,
                resource_id: Some(audits.previous.as_str()),
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({
                    "kind": "previous_owner_enrolled",
                    "role": OrganizationRole::Member.to_string(),
                })),
            },
        )
        .await?;
    }
    for seat in audits.holders_enrolled {
        emit_audit(
            conn,
            AuditEvent {
                organization_id: audits.destination,
                in_project: None,
                actor: Actor::User(audits.user_id.as_str()),
                action: AuditAction::Created,
                resource_kind: TelmoniResourceKind::Member,
                resource_id: Some(seat.user_id.as_str()),
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({
                    "kind": "seat_holder_enrolled",
                    "role": OrganizationRole::Member.to_string(),
                    "seat": seat.role.to_string(),
                })),
            },
        )
        .await?;
    }
    Ok(())
}

fn resolve_destination_organization(
    named: Option<OrganizationId>,
    owned: &[OrganizationId],
    source: &OrganizationId,
) -> Result<OrganizationId, TelmoniError> {
    let destination = match (named, owned) {
        (Some(named), owned) => {
            if !owned.contains(&named) {
                return Err(AuthzError::Forbidden(
                    "you do not own that organization — a project lands only in one of yours"
                        .into(),
                )
                .into());
            }
            named
        }
        (None, [only]) => only.clone(),
        (None, []) => {
            return Err(AuthError::Conflict(
                "you own no organization to take the project into".into(),
            )
            .into());
        }
        (None, _) => {
            return Err(AuthError::BadRequest(
                "you own several organizations — name the one to take the project into".into(),
            )
            .into());
        }
    };
    if destination == *source {
        return Err(
            AuthError::BadRequest("the project is already in that organization".into()).into(),
        );
    }
    Ok(destination)
}

async fn lock_transfer_organizations(
    conn: &mut sqlx::PgConnection,
    source: &OrganizationId,
    destination: &OrganizationId,
) -> Result<(), TelmoniError> {
    // Both organizations' locks, by id — the order every lane that takes
    // several uses, so two accepts crossing each other cannot deadlock.
    let (first, second) = if source.as_str() <= destination.as_str() {
        (source, destination)
    } else {
        (destination, source)
    };
    locks::lock_organization(conn, first).await?;
    locks::lock_organization(conn, second).await?;
    Ok(())
}

/// `POST /internal/projects/{project_id}/transfer/accept` — the admin holding
/// a live offer takes the project into an organization they own. The
/// previous owner keeps a seat on it, as an admin, and is mailed.
///
/// The one lane that writes across two tenants, so it runs in the
/// maintenance lane: `tenant_isolation` on `auth.projects` would refuse the
/// moved row under either organization's binding. Everything it decides on is
/// read again under the locks, and the seat is folded and the offer spent in
/// one statement whose `WHERE` re-checks the offer is still live; if that
/// matches nothing the transaction rolls back.
///
/// ⚠ **The caller's own lock first, then both organizations' by id** — the
/// order account deletion takes them in. Without the caller's, an accept
/// could land after their account deletion read what they own, and leave a
/// project in an organization whose owner can never sign in to hand it on.
/// Under the lock, somebody on their way out is refused.
///
/// ⚠ **Notifications' purge runs inside the transaction, before the commit,
/// and a purge that fails moves nothing.** The project's connectors are the
/// old organization's Slack and Discord grants and webhook secrets; a project
/// arriving with them would post the new owner's events into the old owner's
/// channels. Only a purge that answered 2xx is followed by the move.
pub async fn accept(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    principal: Principal,
    Json(req): Json<AcceptRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let user_id = principal.user_id;
    let named = req
        .organization_id
        .as_deref()
        .map(parse_organization_id)
        .transpose()?;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;

    // Membership before any lock: the project's id arrives from the caller,
    // and a stranger must not stand in a queue holding a pooled connection.
    let Some(standing) = projects::standing(&mut tx, &project_id).await? else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };
    let source = standing.organization_id.clone();
    if organization_members::role_on_organization(&mut tx, &source, &user_id).await?
        == Some(OrganizationRole::Owner)
    {
        return Err(AuthError::BadRequest("you already own this project".into()).into());
    }
    if members::role_on(&mut tx, &project_id, &user_id)
        .await?
        .is_none()
    {
        return Err(AuthzError::Forbidden("you are not a member of this project".into()).into());
    }

    locks::lock_person(&mut tx, &user_id).await?;
    let Some(person) = identities::get(&mut tx, &user_id).await? else {
        return Err(AuthError::NotFound("sign in again to accept this offer".into()).into());
    };
    if person.deletion_requested_at.is_some() {
        return Err(AuthError::Conflict(
            "your account is being deleted — it cannot take a project on".into(),
        )
        .into());
    }

    let owned = organization_members::active_organizations_owned_by(&mut tx, &user_id).await?;
    let destination = resolve_destination_organization(named, &owned, &source)?;

    lock_transfer_organizations(&mut tx, &source, &destination).await?;

    // Under the locks, everything again.
    if organizations::status(&mut tx, &source).await? != Some(OrganizationStatus::Active) {
        return Err(AuthzError::Forbidden(BEING_DELETED.to_string()).into());
    }
    if organizations::status(&mut tx, &destination).await? != Some(OrganizationStatus::Active) {
        return Err(AuthzError::Forbidden("that organization is being deleted".into()).into());
    }
    if organization_members::role_on_organization(&mut tx, &destination, &user_id).await?
        != Some(OrganizationRole::Owner)
    {
        return Err(AuthError::Conflict("you no longer own that organization".into()).into());
    }
    let Some(previous) = organization_members::owner_of(&mut tx, &source).await? else {
        return Err(AuthError::Conflict(
            "the project's organization is changing hands — try again in a moment".into(),
        )
        .into());
    };
    // What the mails call the destination by.
    let Some(destination_label) = organization_members::label_parts(&mut tx, &destination)
        .await?
        .map(|parts| parts.name)
    else {
        return Err(AuthError::NotFound("organization not found".into()).into());
    };

    if !members::fold_offered_seat(&mut tx, &project_id, &user_id).await? {
        return Err(AuthError::Conflict(
            "this offer is no longer open — it was withdrawn, it lapsed, or you are no longer \
             an admin of the project"
                .into(),
        )
        .into());
    }
    let held = usize::try_from(projects::count_in(&mut tx, &destination).await?).unwrap_or(0);
    let family = match numbered(&standing.name) {
        Some((base, n)) => projects::holds_name(&mut tx, &destination, base)
            .await?
            .then_some((base, n)),
        None => None,
    };
    let candidates = landing_names(&standing.name, held, family);
    // `None` cannot happen (see `landing_names`); the move's own refusal
    // covers it if it ever does.
    let name = projects::first_free_name(&mut tx, &destination, &candidates)
        .await?
        .unwrap_or_else(|| standing.name.clone());
    let Some(slug) =
        projects::move_to_organization(&mut tx, &project_id, &source, &destination, &name)
            .await
            .map_err(|e| name_taken_in(e, &name))?
    else {
        return Err(AuthError::Conflict(
            "the project is no longer where it was offered from".into(),
        )
        .into());
    };
    let organization_slug = organizations::slug_of(&mut tx, &destination).await?;
    // The seats that stay, read before the previous owner's is added so
    // theirs is recorded under its own kind: the new owner's is folded already.
    let staying = members::seats_on(&mut tx, &project_id).await?;
    // The previous owner keeps a seat, and with it a row on the destination's
    // roster, as a project invitation would give them: every seat holder is
    // on the owning organization's roster, or `/me` would list a seat in an
    // organization they cannot switch to and the console could never open it.
    let seated =
        members::insert_if_absent(&mut tx, &project_id, &previous, Role::Admin, &user_id).await?;
    let enrolled = organization_members::add_member(
        &mut tx,
        &destination,
        &previous,
        OrganizationRole::Member,
        &user_id,
    )
    .await?;
    // Every other seat holder is enrolled the same way.
    let mut holders_enrolled = Vec::new();
    for seat in &staying {
        if organization_members::add_member(
            &mut tx,
            &destination,
            &seat.user_id,
            OrganizationRole::Member,
            &user_id,
        )
        .await?
        {
            holders_enrolled.push(seat);
        }
    }

    emit_project_transfer_audits(
        &mut tx,
        TransferAudits {
            source: &source,
            destination: &destination,
            project_id: &project_id,
            user_id: &user_id,
            previous: &previous,
            project_name: &name,
            renamed_from: (name != standing.name).then_some(standing.name.as_str()),
            seated,
            enrolled,
            holders_enrolled: &holders_enrolled,
        },
    )
    .await?;

    // The previous owner and the new one, for the mail: the lane may read both.
    let previous_contact = identities::get(&mut tx, &previous).await?;
    let new_owner = crate::identity::display_for(person.display_name.as_deref(), &person.email);

    crate::handler::deletion::purge_project_connectors(&state, &project_id).await?;
    tx.commit().await?;

    if let Some(previous_contact) = previous_contact
        && let Err(e) = state
            .mailer
            .send_project_accepted(
                &previous_contact.email,
                &new_owner,
                &standing.name,
                &destination_label,
            )
            .await
    {
        // The transfer stands without the mail: the previous owner's console
        // shows the project under its new organization from their next page.
        tracing::warn!(project_id = %project_id, error = %e, "project accepted mail failed");
    }

    tracing::info!(
        project_id = %project_id,
        from = %source,
        to = %destination,
        "project transferred"
    );
    Ok(Json(json!({
        "projectId": project_id,
        "slug": slug,
        "organizationId": destination,
        "organizationSlug": organization_slug,
        "previousOrganizationId": source,
        "previousOwner": previous,
        "name": name,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_own_name_comes_first_then_the_numbers_in_order() {
        assert_eq!(
            landing_names("Default Project", 1, None),
            ["Default Project", "Default Project 2", "Default Project 3"]
        );
        assert_eq!(
            landing_names("Payments", 0, None),
            ["Payments", "Payments 2"]
        );
    }

    #[test]
    fn a_plain_trailing_number_from_two_up_is_a_series() {
        assert_eq!(numbered("Default Project 2"), Some(("Default Project", 2)));
        assert_eq!(numbered("Project 2024"), Some(("Project", 2024)));
        for not_numbered in [
            "Default Project",
            "Project 1",
            "Project 0",
            "Project 007",
            "Project +5",
            "Project -3",
            "Project 2.5",
            " 2",
            "2",
            "Project 99999999999",
        ] {
            assert_eq!(numbered(not_numbered), None, "{not_numbered:?}");
        }
    }

    /// "Default Project 2" carries on its series instead of growing a second
    /// number.
    #[test]
    fn a_name_in_a_series_continues_it() {
        let wanted = "Default Project 2";
        assert_eq!(
            landing_names(wanted, 2, numbered(wanted)),
            [
                "Default Project 2",
                "Default Project 3",
                "Default Project 4",
                "Default Project 5"
            ]
        );
        assert_eq!(
            landing_names("Project 2024", 0, None),
            ["Project 2024", "Project 2024 2"]
        );
        let top = format!("Project {}", u32::MAX);
        assert_eq!(
            landing_names(&top, 0, numbered(&top))[1],
            format!("Project {}", u64::from(u32::MAX) + 1)
        );
    }

    /// The guarantee the accept leans on: more distinct names than the
    /// destination holds, even when the name itself is one of the numbered
    /// ones and must not be counted twice.
    #[test]
    fn there_is_always_one_more_distinct_name_than_the_destination_holds() {
        let full_length_two = format!("{} 2", "x".repeat(MAX_PROJECT_NAME - 2));
        let full_length_series = format!("{} 99", "x".repeat(MAX_PROJECT_NAME - 3));
        for wanted in [
            "Default Project",
            "Default Project 2",
            full_length_two.as_str(),
            full_length_series.as_str(),
        ] {
            for family in [None, numbered(wanted)] {
                for held in [0, 1, 9, 10, 99] {
                    let names = landing_names(wanted, held, family);
                    let distinct: std::collections::HashSet<String> =
                        names.iter().map(|n| n.to_lowercase()).collect();
                    assert!(
                        distinct.len() > held,
                        "{wanted:?} in {family:?} with {held} held"
                    );
                }
            }
        }
    }

    #[test]
    fn every_name_fits_and_a_long_one_gives_up_characters_for_its_number() {
        let wanted = "x".repeat(MAX_PROJECT_NAME);
        for name in landing_names(&wanted, 120, None) {
            assert!(name.chars().count() <= MAX_PROJECT_NAME, "{name}");
        }
        assert_eq!(
            landing_names(&wanted, 0, None)[1],
            format!("{} 2", "x".repeat(MAX_PROJECT_NAME - 2))
        );
        let series = format!("{} 99", "x".repeat(MAX_PROJECT_NAME - 3));
        assert_eq!(
            landing_names(&series, 0, numbered(&series))[1],
            format!("{} 100", "x".repeat(MAX_PROJECT_NAME - 4))
        );
        let multibyte = "é".repeat(MAX_PROJECT_NAME);
        assert_eq!(
            landing_names(&multibyte, 0, None)[1].chars().count(),
            MAX_PROJECT_NAME
        );
    }

    /// A cut that lands on a space would leave two before the number.
    #[test]
    fn a_cut_short_name_does_not_end_in_a_double_space() {
        let wanted = format!("{} y", "x".repeat(MAX_PROJECT_NAME - 3));
        assert_eq!(
            landing_names(&wanted, 0, None)[1],
            format!("{} 2", "x".repeat(MAX_PROJECT_NAME - 3))
        );
    }
}
