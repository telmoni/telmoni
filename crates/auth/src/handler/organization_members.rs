//! Organization-level members and invitations.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationRole, TelmoniError, TelmoniResourceKind,
};

use crate::{
    AppState,
    db::{AuthLane, identities, locks, members, organization_members, organizations},
    handler::{
        ActingOrganization, acting_organization, organization_of, organization_role_or_forbidden,
        parse_user_id,
    },
    model::InviteScope,
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
        OrganizationRole::Owner => Err(AuthError::BadRequest(
            "an organization changes owner by a transfer the new owner accepts — a member can be \
             made an admin or a member"
                .into(),
        )
        .into()),
    }
}

/// `GET /internal/organization/members` — list all organization members.
pub async fn list_organization_members(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    let ActingOrganization { mut tx, role } =
        acting_organization(&state, &organization, &actor).await?;

    if !role.can_view_org_members() {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "insufficient permissions to view organization members".into(),
        )
        .into());
    }

    let rows = organization_members::list_for_organization(&mut tx, &organization).await?;
    tx.commit().await?;

    Ok(Json(json!({ "members": rows })))
}

/// `PUT /internal/organization/members/{member_id}/role` — update a member's role.
pub async fn update_organization_member_role(
    State(state): State<Arc<AppState>>,
    Path(member_id): Path<String>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<UpdateOrganizationRoleRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;
    let member_id = parse_user_id(&member_id)?;
    let role = ensure_grantable(req.role)?;

    let ActingOrganization { mut tx, .. } =
        acting_organization(&state, &organization, &actor).await?;
    // The roles are read again under the lock: an ownership transfer may have
    // moved either of them since `acting_organization` looked.
    locks::lock_organization(&mut tx, &organization).await?;
    let caller_role = organization_role_or_forbidden(&mut tx, &organization, &actor).await?;

    if !caller_role.can_manage_org_members() {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "only an organization owner or admin can change organization member roles".into(),
        )
        .into());
    }
    if organization_members::role_on_organization(&mut tx, &organization, &member_id).await?
        == Some(OrganizationRole::Owner)
    {
        return Err(AuthError::BadRequest(
            "the owner's role changes only by handing the organization over".into(),
        )
        .into());
    }

    if !organization_members::update_role(&mut tx, &organization, &member_id, role).await? {
        tx.commit().await?;
        return Err(AuthError::NotFound("no such organization member".into()).into());
    }

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(member_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "role": role.to_string(),
                "scope": InviteScope::Organization,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    Ok(StatusCode::OK)
}

/// `DELETE /internal/organization/members/{member_id}` — remove an organization member.
pub async fn remove_organization_member(
    State(state): State<Arc<AppState>>,
    Path(member_id): Path<String>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;
    let member_id = parse_user_id(&member_id)?;

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
        return Err(AuthError::BadRequest(if leaving {
            "you own this organization — hand it to an admin before you leave".into()
        } else {
            "the owner cannot be removed".into()
        })
        .into());
    }

    if !leaving && !caller_role.can_manage_org_members() {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "only an organization owner or admin can remove organization members".into(),
        )
        .into());
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
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(member_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "scope": InviteScope::Organization,
                "kind": if leaving { "left" } else { "removed" },
            })),
        },
    )
    .await?;

    for pm in &project_memberships {
        if !members::remove(&mut tx, &pm.project_id, &member_id).await? {
            continue;
        }
        emit_audit(
            &mut tx,
            AuditEvent {
                organization_id: &organization,
                in_project: Some(&pm.project_id),
                actor: Actor::User(actor.as_str()),
                action: AuditAction::Deleted,
                resource_kind: TelmoniResourceKind::Member,
                resource_id: Some(member_id.as_str()),
                request_id: None,
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({
                    "kind": "organization_removal_cascade",
                    "role": pm.role.to_string(),
                })),
            },
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

/// `GET /internal/organization/invites` — list live organization invitations.
pub async fn list_organization_invites(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization = organization_of(&headers)?;
    let actor = principal.user_id;

    let ActingOrganization { mut tx, role } =
        acting_organization(&state, &organization, &actor).await?;

    if !role.can_view_org_members() {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "insufficient permissions to view organization invites".into(),
        )
        .into());
    }

    let rows = organization_members::list_invites(&mut tx, &organization).await?;
    tx.commit().await?;

    Ok(Json(json!({ "invites": rows })))
}

/// `POST /internal/organization/invites` — invite a person to the organization.
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

    if !caller_role.can_manage_org_members() {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "only an organization owner or admin can invite organization members".into(),
        )
        .into());
    }

    let flags = crate::db::flags::resolve_for_organization(&mut tx, &organization).await?;
    if !flags.is_on(telmoni_shared::Flag::Members) {
        tx.commit().await?;
        return Err(telmoni_shared::TenantError::FeatureOff {
            flag: telmoni_shared::Flag::Members,
        }
        .into());
    }

    // ⚠ Only a named organization invites. The invitation, the console and
    // the roster all show the person the organization's name, and an
    // organization has only its owner until it has one: the console asks for
    // the name before it opens, and this holds the same line for any client.
    if organizations::get(&mut tx, &organization)
        .await?
        .is_none_or(|o| o.name.is_none())
    {
        tx.commit().await?;
        return Err(AuthError::Conflict(
            "name the organization before inviting anyone to it".into(),
        )
        .into());
    }

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
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(invite_id.to_string().as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "email": email,
                "role": role.to_string(),
                "scope": InviteScope::Organization,
            })),
        },
    )
    .await?;
    // Who is inviting, and into what, for the mail — read here, where the
    // organization's scope admits its own roster's identities.
    let inviter = identities::contact(&mut tx, &actor).await?;
    let name = organizations::get(&mut tx, &organization)
        .await?
        .and_then(|o| o.name);
    tx.commit().await?;

    let link = format!(
        "{}/invite/{}",
        state.config.app_url.trim_end_matches('/'),
        raw_token
    );

    let label = crate::identity::organization_label(name.as_deref());
    let inviter = inviter.map_or_else(|| label.clone(), |c| c.display());
    if let Err(e) = state
        .mailer
        .send_member_invite(
            &email,
            &inviter,
            &label,
            crate::mailer::InvitedTo::Organization,
            &role.to_string(),
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

/// `DELETE /internal/organization/invites/{invite_id}` — revoke an organization invitation.
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

    if !caller_role.can_manage_org_members() {
        tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "only an organization owner or admin can revoke organization invitations".into(),
        )
        .into());
    }

    let Some(email) =
        organization_members::revoke_invite(&mut tx, &organization, invite_id).await?
    else {
        tx.commit().await?;
        return Err(AuthError::NotFound("no such invitation".into()).into());
    };

    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &organization,
            in_project: None,
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(invite_id.to_string().as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({
                "kind": "organization_invite_revoked",
                "scope": InviteScope::Organization,
            })),
        },
    )
    .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "inviteId": invite_id,
        "email": email,
        "ownerOrganizationId": organization,
    })))
}
