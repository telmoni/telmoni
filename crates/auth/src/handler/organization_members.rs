//! Organization-level members and invitations.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::OrganizationAction;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::{Json, Path};
use telmoni_shared::person_token::Principal;
use telmoni_shared::{
    AuditAction, AuthError, OrganizationRole, TelmoniError, TelmoniResourceKind, UserId,
};

use super::authorize_organization;
use crate::{
    AppState,
    db::{AuthLane, identities, locks, members, organization_members, organizations},
    handler::{
        ActingOrganization, acting_organization,
        invite::{admit_invitation, ceiling_of, roster_ceiling},
        organization_of, organization_role_or_forbidden,
    },
    model::{AuditKind, InviteScope},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganizationRoleRequest {
    pub role: OrganizationRole,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganizationInviteRequest {
    pub email: String,
    pub role: OrganizationRole,
}

/// Grantable check: only Admin and Member may be granted. Owner is never
/// granted — it moves only by a transfer the new owner accepts.
fn ensure_grantable(role: OrganizationRole) -> Result<OrganizationRole, TelmoniError> {
    match role {
        OrganizationRole::Admin | OrganizationRole::Member => Ok(role),
        OrganizationRole::Owner => Err(TelmoniError::BadRequest(
            "an organization changes owner by a transfer the new owner accepts — a member can be \
             made an admin or a member"
                .into(),
        )),
    }
}

/// `GET /internal/v1/organization/members` — list all organization members.
pub async fn list_organization_members(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    let ActingOrganization { mut tx, role } =
        acting_organization(&state, &organization, &actor).await?;

    if let Err(refused) = authorize_organization(role, OrganizationAction::ViewMembers) {
        tx.commit().await?;
        return Err(refused);
    }

    let rows = organization_members::list_for_organization(&mut tx, &organization).await?;
    tx.commit().await?;
    // Read once the roster's connection is back, never beside it.
    let ceiling = roster_ceiling(&state, &organization).await?;

    Ok(Json(json!({ "members": rows, "ceiling": ceiling })))
}

/// `PUT /internal/v1/organization/members/{member_id}/role` — update a member's role.
pub async fn update_organization_member_role(
    State(state): State<Arc<AppState>>,
    Path(member_id): Path<UserId>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<UpdateOrganizationRoleRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;
    let role = ensure_grantable(req.role)?;

    let ActingOrganization { mut tx, .. } =
        acting_organization(&state, &organization, &actor).await?;
    // The roles are read again under the lock: an ownership transfer may have
    // moved either of them since `acting_organization` looked.
    locks::lock_organization(&mut tx, &organization).await?;
    let caller_role = organization_role_or_forbidden(&mut tx, &organization, &actor).await?;

    if let Err(refused) = authorize_organization(caller_role, OrganizationAction::ManageMembers) {
        tx.commit().await?;
        return Err(refused);
    }
    if organization_members::role_on_organization(&mut tx, &organization, &member_id).await?
        == Some(OrganizationRole::Owner)
    {
        return Err(TelmoniError::BadRequest(
            "the owner's role changes only by handing the organization over".into(),
        ));
    }

    if !organization_members::update_role(&mut tx, &organization, &member_id, role).await? {
        tx.commit().await?;
        return Err(AuthError::NotFound("no such organization member".into()).into());
    }

    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(actor.as_str()),
            AuditAction::Updated,
            TelmoniResourceKind::Member,
        )
        .on(member_id.as_str())
        .details(json!({
            "role": role,
            "scope": InviteScope::Organization,
        })),
    )
    .await?;
    tx.commit().await?;

    Ok(StatusCode::OK)
}

/// `DELETE /internal/v1/organization/members/{member_id}` — remove an organization member.
pub async fn remove_organization_member(
    State(state): State<Arc<AppState>>,
    Path(member_id): Path<UserId>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    // The maintenance lane because leaving deletes a row the leaver may only
    // read. Membership before the lock, so a stranger is refused at once
    // rather than holding a connection in the organization's queue; and again
    // under it, because a transfer may have made the member (or the caller)
    // the owner a moment ago.
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    organization_role_or_forbidden(&mut tx, &organization, &actor).await?;
    locks::lock_organization(&mut tx, &organization).await?;
    let caller_role = organization_role_or_forbidden(&mut tx, &organization, &actor).await?;

    let leaving = member_id == actor;

    if organization_members::role_on_organization(&mut tx, &organization, &member_id).await?
        == Some(OrganizationRole::Owner)
    {
        return Err(TelmoniError::BadRequest(if leaving {
            "you own this organization — hand it to an admin before you leave".into()
        } else {
            "the owner cannot be removed".into()
        }));
    }

    if !leaving
        && let Err(refused) = authorize_organization(caller_role, OrganizationAction::ManageMembers)
    {
        tx.commit().await?;
        return Err(refused);
    }

    let leaver_name =
        organization_members::member_display_name(&mut tx, &organization, &member_id).await?;

    let project_memberships =
        members::memberships_in_organization(&mut tx, &organization, &member_id).await?;

    if !organization_members::remove(&mut tx, &organization, &member_id).await? {
        tx.commit().await?;
        return Err(AuthError::NotFound("no such organization member".into()).into());
    }

    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(actor.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::Member,
        )
        .on(member_id.as_str())
        .details(json!({
            "scope": InviteScope::Organization,
            "kind": if leaving { AuditKind::Left } else { AuditKind::Removed },
        })),
    )
    .await?;

    for pm in &project_memberships {
        if !members::remove(&mut tx, &pm.project_id, &member_id).await? {
            continue;
        }
        emit_audit(
            &mut tx,
            AuditEvent::new(
                &organization,
                Actor::User(actor.as_str()),
                AuditAction::Deleted,
                TelmoniResourceKind::Member,
            )
            .in_project(&pm.project_id)
            .on(member_id.as_str())
            .details(json!({
                "kind": AuditKind::OrganizationRemovalCascade,
                "role": pm.role,
            })),
        )
        .await?;
    }

    tx.commit().await?;

    let name = leaver_name.as_deref().unwrap_or("A member");
    for pm in &project_memberships {
        crate::notify::emit_member_left(&state, &pm.project_id, &organization, &member_id, name)
            .await;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /internal/v1/organization/invites` — list live organization invitations.
pub async fn list_organization_invites(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    let ActingOrganization { mut tx, role } =
        acting_organization(&state, &organization, &actor).await?;

    if let Err(refused) = authorize_organization(role, OrganizationAction::ViewMembers) {
        tx.commit().await?;
        return Err(refused);
    }

    let rows = organization_members::list_invites(&mut tx, &organization).await?;
    tx.commit().await?;

    Ok(Json(json!({ "invites": rows })))
}

/// `POST /internal/v1/organization/invites` — invite a person to the organization.
pub async fn create_organization_invite(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<CreateOrganizationInviteRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;
    let role = ensure_grantable(req.role)?;
    let email = crate::identity::validate_email(&req.email)?;

    let ActingOrganization {
        mut tx,
        role: caller_role,
    } = acting_organization(&state, &organization, &actor).await?;

    if let Err(refused) = authorize_organization(caller_role, OrganizationAction::ManageMembers) {
        tx.commit().await?;
        return Err(refused);
    }

    let flags = crate::db::flags::resolve_for_organization(&mut tx, &organization).await?;
    if !flags.is_on(telmoni_shared::Flag::Members) {
        tx.commit().await?;
        return Err(telmoni_shared::TenantError::FeatureOff {
            flag: telmoni_shared::Flag::Members,
        }
        .into());
    }
    // A paused organization takes no new members, and one at its ceiling
    // promises no further seat.
    let ceiling = ceiling_of(&state, &organization).await?;
    admit_invitation(&state, &organization, ceiling, &email).await?;

    // The invitation's mail calls the organization by its name.
    let Some(name) = organizations::get(&mut tx, &organization)
        .await?
        .map(|o| o.name)
    else {
        tx.commit().await?;
        return Err(AuthError::NotFound("organization not found".into()).into());
    };

    // Everyone on the roster, the owner included: an invitation to somebody
    // already in could only fail at accept, and an owner's would have been
    // the one that demoted them before accepts stopped rewriting roles.
    if organization_members::address_on_roster(&mut tx, &organization, &email).await? {
        tx.commit().await?;
        return Err(AuthError::Conflict("that address is already a member".into()).into());
    }

    organization_members::revoke_live_for_email(&mut tx, &organization, &email).await?;

    let raw_token = Uuid::new_v4().to_string();
    let token_hash = telmoni_shared::digest::sha256_hex(raw_token.as_bytes());
    let expires_at = Utc::now() + Duration::days(crate::handler::invite::INVITE_TTL_DAYS);

    let invite_id = organization_members::create_invite(
        &mut tx,
        &organization,
        &email,
        role,
        &token_hash,
        &actor,
        expires_at,
    )
    .await?;

    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(actor.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Member,
        )
        .on(invite_id.to_string().as_str())
        .details(json!({
            "email": email,
            "role": role,
            "scope": InviteScope::Organization,
        })),
    )
    .await?;
    // Who is inviting, and into what, for the mail — read here, where the
    // organization's scope admits its own roster's identities.
    let inviter = identities::contact(&mut tx, &actor).await?;
    tx.commit().await?;

    let link = format!(
        "{}/invite/{}",
        state.config.app_url.trim_end_matches('/'),
        raw_token
    );

    let label = name;
    let inviter = inviter.map_or_else(|| label.clone(), |c| c.display());
    if let Err(e) = state
        .mailer
        .send_member_invite(
            &email,
            &inviter,
            &label,
            crate::mailer::InvitedTo::Organization(role),
            &link,
        )
        .await
    {
        tracing::warn!(invite = %invite_id, error = %e, "organization invitation mail failed");
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": invite_id,
            "link": link,
            "expiresAt": expires_at,
            "ownerOrganizationId": organization,
        })),
    ))
}

/// `DELETE /internal/v1/organization/invites/{invite_id}` — revoke an organization invitation.
pub async fn revoke_organization_invite(
    State(state): State<Arc<AppState>>,
    Path(invite_id): Path<Uuid>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    let ActingOrganization {
        mut tx,
        role: caller_role,
    } = acting_organization(&state, &organization, &actor).await?;

    if let Err(refused) = authorize_organization(caller_role, OrganizationAction::ManageMembers) {
        tx.commit().await?;
        return Err(refused);
    }

    let Some(email) =
        organization_members::revoke_invite(&mut tx, &organization, invite_id).await?
    else {
        tx.commit().await?;
        return Err(AuthError::NotFound("no such invitation".into()).into());
    };

    emit_audit(
        &mut tx,
        AuditEvent::new(
            &organization,
            Actor::User(actor.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::Member,
        )
        .on(invite_id.to_string().as_str())
        .details(json!({
            "kind": AuditKind::OrganizationInviteRevoked,
            "scope": InviteScope::Organization,
        })),
    )
    .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "inviteId": invite_id,
        "email": email,
        "ownerOrganizationId": organization,
    })))
}
