//! Members: who else may see an organization's runs, one project at a time.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, AuthzError, OrganizationRole, Role, TelmoniError, TelmoniResourceKind,
    UserId,
};

use crate::{
    AppState,
    db::{AuthLane, members, organization_members},
    handler::{ActingProject, acting_project, authorize, parse_project_id, parse_user_id},
    model::AuditKind,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRoleRequest {
    pub role: Role,
}

/// `owner` is a role on the wire, but not one anyone can be GRANTED on a
/// project: a project's owner is its organization's owner.
pub(crate) fn grantable(role: Role) -> Result<Role, TelmoniError> {
    match role {
        Role::Admin | Role::Member => Ok(role),
        Role::Owner => Err(AuthError::BadRequest(
            "a seat is admin or member — the project's owner is its organization's owner".into(),
        )
        .into()),
    }
}

/// Whether `member` owns the organization that holds this project. Read under
/// the owning organization's scope, the only one its roster is visible in,
/// and handed back project-scoped.
async fn is_organization_owner<'a>(
    acting: ActingProject<'a>,
    member: &UserId,
) -> Result<(ActingProject<'a>, bool, Option<String>), TelmoniError> {
    let mut acting = acting.enter_owner_scope().await?;
    let role =
        organization_members::role_on_organization(&mut acting.tx, &acting.organization, member)
            .await?;
    let name =
        organization_members::member_display_name(&mut acting.tx, &acting.organization, member)
            .await?;
    let acting = acting.leave_owner_scope().await?;
    Ok((acting, role == Some(OrganizationRole::Owner), name))
}

/// `GET /internal/projects/{project_id}/members` — the roster.
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let actor = principal.user_id;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Read, Resource::Member)?;
    // ⚠ Under the owning organization as well as the project: the owner's row
    // and every person's address are the organization's to see, and a
    // project-only scope would join them against nothing.
    let mut acting = acting.enter_owner_scope().await?;
    let rows = members::list_for_project(&mut acting.tx, &project_id).await?;
    let acting = acting.leave_owner_scope().await?;
    acting.tx.commit().await?;

    Ok(Json(json!({ "members": rows })))
}

/// `PUT /internal/projects/{project_id}/members/{member_id}/role`.
pub async fn update_role(
    State(state): State<Arc<AppState>>,
    Path((project_id, member_id)): Path<(String, String)>,
    principal: Principal,
    Json(req): Json<UpdateRoleRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let member_id = parse_user_id(&member_id)?;
    let actor = principal.user_id;
    let role = grantable(req.role)?;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Update, Resource::Member)?;

    let (mut acting, owner, _) = is_organization_owner(acting, &member_id).await?;
    if owner {
        acting.tx.rollback().await?;
        return Err(AuthError::BadRequest("cannot modify the project owner's role".into()).into());
    }

    if !members::update_role(&mut acting.tx, &project_id, &member_id, role).await? {
        return Err(AuthError::NotFound("no such member".into()).into());
    }
    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &acting.organization,
            in_project: Some(&project_id),
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Updated,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(member_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "role": role })),
        },
    )
    .await?;
    acting.tx.commit().await?;

    tracing::info!(project_id = %project_id, member = %member_id, %role, "member role changed");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /internal/projects/{project_id}/members/{member_id}` — the owner
/// or an admin removes somebody.
pub async fn remove_member(
    State(state): State<Arc<AppState>>,
    Path((project_id, member_id)): Path<(String, String)>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let member_id = parse_user_id(&member_id)?;
    let actor = principal.user_id;

    let acting = acting_project(&state, &project_id, &actor).await?;
    authorize(acting.role, Verb::Delete, Resource::Member)?;

    let (mut acting, owner, leaver_name) = is_organization_owner(acting, &member_id).await?;
    if owner {
        acting.tx.rollback().await?;
        return Err(AuthError::BadRequest("cannot remove the project owner".into()).into());
    }

    if !members::remove(&mut acting.tx, &project_id, &member_id).await? {
        return Err(AuthError::NotFound("no such member".into()).into());
    }
    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &acting.organization,
            in_project: Some(&project_id),
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(member_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": AuditKind::RemovedByOwner })),
        },
    )
    .await?;
    let organization = acting.organization.clone();
    acting.tx.commit().await?;

    let name = leaver_name.as_deref().unwrap_or("A member");
    crate::notify::emit_member_left(&state, &project_id, &organization, &member_id, name).await;

    tracing::info!(project_id = %project_id, member = %member_id, "member removed");
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /internal/memberships/{project_id}` — the member gives up their
/// seat. Maintenance lane: the seat is a row they may only read.
pub async fn leave(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let project_id = parse_project_id(&project_id)?;
    let actor = principal.user_id;

    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let owner = crate::handler::get_project_organization(&mut tx, &project_id).await;
    if let Ok(ref organization) = owner
        && organization_members::role_on_organization(&mut tx, organization, &actor).await?
            == Some(OrganizationRole::Owner)
    {
        return Err(AuthzError::Forbidden(
            "you own this project — delete it instead of leaving it".into(),
        )
        .into());
    }

    let owner = owner?;
    let leaver_name = organization_members::member_display_name(&mut tx, &owner, &actor).await?;

    if !members::remove(&mut tx, &project_id, &actor).await? {
        return Err(AuthError::NotFound("you are not a member of this project".into()).into());
    }
    emit_audit(
        &mut tx,
        AuditEvent {
            organization_id: &owner,
            in_project: Some(&project_id),
            actor: Actor::User(actor.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Member,
            resource_id: Some(actor.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "kind": AuditKind::Left })),
        },
    )
    .await?;
    tx.commit().await?;

    let name = leaver_name.as_deref().unwrap_or("A member");
    crate::notify::emit_member_left(&state, &project_id, &owner, &actor, name).await;

    tracing::info!(project_id = %project_id, member = %actor, "member left");
    Ok(StatusCode::NO_CONTENT)
}
