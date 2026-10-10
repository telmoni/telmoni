//! `GET /internal/projects` — every project the caller can open, and
//! `POST /internal/projects` — one more of them. Creating is an ORGANIZATION
//! act: there is no project to bind yet, so the gate is the organization role.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, http::StatusCode, response::IntoResponse};
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{AuditAction, AuthError, AuthzError, TelmoniError, TelmoniResourceKind};

use crate::AppState;
use crate::db::{AuthLane, projects};

/// The longest a project name may be. The column is unconstrained, so this is
/// the only thing enforcing it.
pub(crate) const MAX_PROJECT_NAME: usize = 100;

/// Trim a submitted project name and refuse empty or overlong ones. Shared by
/// create and rename so the two cannot drift, and counted in CHARACTERS, not
/// bytes.
fn validate_project_name(raw: &str) -> Result<&str, TelmoniError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AuthError::BadRequest("project name cannot be empty".into()).into());
    }
    if trimmed.chars().count() > MAX_PROJECT_NAME {
        return Err(AuthError::BadRequest(format!(
            "project name too long (max {MAX_PROJECT_NAME} characters)"
        ))
        .into());
    }
    Ok(trimmed)
}

/// The one constraint a project NAME can trip — one name per organization,
/// case-insensitively — as a 409 that names the name, not a 500.
fn name_taken(e: sqlx::Error, name: &str) -> TelmoniError {
    match e {
        sqlx::Error::Database(ref db)
            if db.constraint() == Some("projects_organization_name_key") =>
        {
            AuthError::Conflict(format!(
                "a project named \"{name}\" already exists in this organization"
            ))
            .into()
        }
        other => other.into(),
    }
}

/// `GET /internal/projects` — the projects the caller can open in the
/// organization named by `x-organization-id`, by name.
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, telmoni_shared::TelmoniError> {
    let user_id = principal.user_id;
    let organization_id = super::organization_of(&headers)?;

    // ⚠ **An empty list stays empty.** Projects are created only when
    // explicitly requested, so an organization with no projects remains empty.
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let project_list =
        projects::list_for_organization_user(&mut tx, &organization_id, &user_id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "projects": project_list })))
}

/// `GET /internal/projects/everywhere` — every project the caller can open, in
/// every organization. Maintenance lane, because the question spans
/// organizations and there is no one GUC to bind.
pub async fn list_projects_everywhere(
    State(state): State<Arc<AppState>>,
    principal: Principal,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = principal.user_id;
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let project_list = projects::list_everywhere_for_user(&mut tx, &user_id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "projects": project_list })))
}

/// Request payload for creating a project.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProjectRequest {
    /// What to call it. Trimmed and length-checked by `validate_project_name`.
    pub name: String,
}

/// `POST /internal/projects` — add a project to the acting organization.
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
    Json(req): Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization_id = super::organization_of(&headers)?;
    let user_id = principal.user_id;
    let name = validate_project_name(&req.name)?;

    let mut acting = super::acting_organization(&state, &organization_id, &user_id).await?;
    if !acting.role.can_create_projects() {
        acting.tx.commit().await?;
        return Err(AuthzError::Forbidden(
            "only an organization owner or admin can create projects".into(),
        )
        .into());
    }

    // On the organization's lock, as a project landing here by transfer
    // takes it to pick a free name.
    crate::db::locks::lock_organization(&mut acting.tx, &organization_id).await?;
    let project_id = telmoni_shared::ProjectId::new();
    let Some(slug) = projects::create(&mut acting.tx, &project_id, &organization_id, name)
        .await
        .map_err(|e| name_taken(e, name))?
    else {
        return Err(AuthError::Conflict("project id collision — retry".into()).into());
    };

    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &organization_id,
            in_project: Some(&project_id),
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::Project,
            resource_id: Some(project_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "name": name })),
        },
    )
    .await?;

    acting.tx.commit().await?;

    let role = if acting.role == telmoni_shared::OrganizationRole::Owner {
        telmoni_shared::Role::Owner
    } else {
        telmoni_shared::Role::Admin
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": project_id, "slug": slug, "name": name, "role": role })),
    ))
}

/// `DELETE /internal/projects/{project_id}` — destroy a project and everything that
/// hangs off it here: seats, invitations and API keys cascade.
///
/// **The audit chain stays, and must**: `in_project` has no foreign key, so a
/// destruction does not erase its own record.
///
/// ⚠️ **OWNER ONLY, tighter than create**: what an organization IS belongs to
/// the owner. The last project may go; `/console` handles an organization
/// with none.
///
/// Notifications keys its rows on `project_id` with no cross-schema foreign
/// key, so its purge lane is called once the row is gone, best effort: what a
/// failed purge leaves — sealed vendor grants and webhook secrets on a project
/// that no longer exists — is reached by nothing until the organization's own
/// purge, which sweeps it.
pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    principal: Principal,
) -> Result<impl IntoResponse, telmoni_shared::TelmoniError> {
    let project_id = super::parse_project_id(&project_id)?;
    let user_id = principal.user_id;

    let acting = super::acting_project(&state, &project_id, &user_id).await?;
    super::authorize(
        acting.role,
        telmoni_shared::rbac::Verb::Delete,
        telmoni_shared::rbac::Resource::Project,
    )?;

    let organization_id = acting.organization.clone();

    // The delete first, answering the name the audit row records: two reads
    // and a write were three round trips for one.
    let mut acting = acting.enter_owner_scope().await?;
    let name = projects::delete(&mut acting.tx, &organization_id, &project_id).await?;
    let mut acting = acting.leave_owner_scope().await?;
    let Some(name) = name else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };

    emit_audit(
        &mut acting.tx,
        AuditEvent {
            organization_id: &organization_id,
            in_project: Some(&project_id),
            actor: Actor::User(user_id.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::Project,
            resource_id: Some(project_id.as_str()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "name": name })),
        },
    )
    .await?;

    acting.tx.commit().await?;

    if let Err(e) = super::deletion::purge_project_connectors(&state, &project_id).await {
        tracing::warn!(project_id = %project_id, error = %e, "the connectors' project purge failed after the delete");
    }
    if let Err(e) = super::deletion::purge_project_telemetry(&state, &project_id).await {
        tracing::warn!(project_id = %project_id, error = %e, "telemetry's project purge failed after the delete");
    }
    Ok(Json(json!({ "status": "deleted", "id": project_id })))
}

/// Request payload for updating a project.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateProjectRequest {
    /// The new name for the project.
    pub name: String,
}

/// `PATCH /internal/projects/{project_id}` — rename a project. Its slug moves
/// with the name, so the answer carries both and the console follows it.
pub async fn update_project(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    principal: Principal,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<impl IntoResponse, telmoni_shared::TelmoniError> {
    let project_id = super::parse_project_id(&project_id)?;
    let user_id = principal.user_id;
    let trimmed = validate_project_name(&req.name)?;

    let acting = super::acting_project(&state, &project_id, &user_id).await?;
    super::authorize(
        acting.role,
        telmoni_shared::rbac::Verb::Update,
        telmoni_shared::rbac::Resource::Project,
    )?;

    let organization_id = acting.organization.clone();
    let mut acting = acting.enter_owner_scope().await?;
    // On the organization's lock, as a create takes it: the new name's slug is
    // picked against the organization's other projects.
    crate::db::locks::lock_organization(&mut acting.tx, &organization_id).await?;
    let slug = projects::update_name(&mut acting.tx, &organization_id, &project_id, trimmed)
        .await
        .map_err(|e| name_taken(e, trimmed))?;
    let acting = acting.leave_owner_scope().await?;
    let Some(slug) = slug else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };

    acting.tx.commit().await?;
    Ok(Json(
        json!({ "status": "ok", "name": trimmed, "slug": slug }),
    ))
}
