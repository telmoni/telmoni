//! `GET /internal/v1/projects` — every project the caller can open, and
//! `POST /internal/v1/projects` — one more of them. Creating is an ORGANIZATION
//! act: there is no project to bind yet, so the gate is the organization role.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use serde_json::json;

use telmoni_shared::OrganizationAction;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::{Json, Path};
use telmoni_shared::person_token::Principal;
use telmoni_shared::slug::Slug;
use telmoni_shared::{AuditAction, AuthError, ProjectId, TelmoniError, TelmoniResourceKind};

use super::answer::Projects;
use super::authorize_organization;
use crate::AppState;
use crate::db::{AuthLane, projects};

/// The longest a project name may be. The column is unconstrained, so this is
/// the only thing enforcing it.
pub const MAX_PROJECT_NAME: usize = 100;

/// A submitted project name as a project carries it: shared by create and
/// rename so the two cannot drift. It is printed in mail subjects, so it is
/// held to what an organization's name is.
fn validate_project_name(raw: &str) -> Result<String, TelmoniError> {
    crate::identity::name(raw, "project", MAX_PROJECT_NAME)
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

/// `GET /internal/v1/projects` — the projects the caller can open in the
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
    Ok(Json(Projects {
        projects: project_list,
    }))
}

/// `GET /internal/v1/projects/everywhere` — every project the caller can open, in
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
    Ok(Json(Projects {
        projects: project_list,
    }))
}

/// Request payload for creating a project.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProjectRequest {
    /// What to call it. Trimmed and length-checked by `validate_project_name`.
    pub name: String,
}

/// `POST /internal/v1/projects` — add a project to the acting organization.
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
    if let Err(refused) = authorize_organization(acting.role, OrganizationAction::CreateProjects) {
        acting.tx.commit().await?;
        return Err(refused);
    }
    // The creator's role on the new project is the one their organization role
    // gives them on every project in it. The gate above admits only roles that
    // give one, so a `None` is a broken rule, refused before anything is written.
    let Some(role) = telmoni_shared::rbac::project_role(Some(acting.role), None) else {
        return Err(TelmoniError::Internal(
            "an organization role that may create a project holds no role on it".into(),
        ));
    };

    // On the organization's lock, as a project landing here by transfer
    // takes it to pick a free name.
    crate::db::locks::lock_organization(&mut acting.tx, &organization_id).await?;
    let project_id = telmoni_shared::ProjectId::new();
    let Some(slug) = projects::create(&mut acting.tx, &project_id, &organization_id, &name)
        .await
        .map_err(|e| name_taken(e, &name))?
    else {
        return Err(AuthError::Conflict("project id collision — retry".into()).into());
    };

    emit_audit(
        &mut acting.tx,
        AuditEvent::new(
            &organization_id,
            Actor::User(user_id.as_str()),
            AuditAction::Created,
            TelmoniResourceKind::Project,
        )
        .in_project(&project_id)
        .on(project_id.as_str())
        .details(json!({ "name": name })),
    )
    .await?;

    acting.tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(projects::ProjectSummary {
            id: project_id,
            slug,
            name,
            role,
        }),
    ))
}

/// `DELETE /internal/v1/projects/{project_id}` — destroy a project and everything that
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
/// that no longer exists — is purged again by notifications' hourly
/// retention sweep.
pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
) -> Result<impl IntoResponse, telmoni_shared::TelmoniError> {
    let user_id = principal.user_id;

    let acting = super::acting_project(&state, &project_id, &user_id).await?;
    super::authorize(
        acting.role,
        telmoni_shared::rbac::Verb::Delete,
        telmoni_shared::rbac::Resource::Project,
    )?;
    // The organization's lock before the cascade takes the seats, as a member's removal does.
    let acting = acting
        .hold(
            &project_id,
            &user_id,
            telmoni_shared::rbac::Verb::Delete,
            telmoni_shared::rbac::Resource::Project,
        )
        .await?;

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
        AuditEvent::new(
            &organization_id,
            Actor::User(user_id.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::Project,
        )
        .in_project(&project_id)
        .on(project_id.as_str())
        .details(json!({ "name": name })),
    )
    .await?;

    acting.tx.commit().await?;

    if let Err(e) = super::deletion::purge_project_connectors(
        &state,
        &project_id,
        super::deletion::Teardown::AfterDelete,
    )
    .await
    {
        tracing::warn!(project_id = %project_id, error = %e, "the connectors' project purge failed after the delete");
    }
    if let Err(e) = super::deletion::purge_project_telemetry(&state, &project_id).await {
        tracing::warn!(project_id = %project_id, error = %e, "telemetry's project purge failed after the delete");
    }
    Ok(Json(ProjectDeleted { id: project_id }))
}

/// A project deleted, by its id.
#[derive(Debug, Serialize)]
pub struct ProjectDeleted {
    pub id: ProjectId,
}

/// A project renamed: its name now, and the slug that follows it.
#[derive(Debug, Serialize)]
pub struct ProjectRenamed {
    pub name: String,
    pub slug: Slug,
}

/// Request payload for updating a project.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateProjectRequest {
    /// The new name for the project.
    pub name: String,
}

/// `PATCH /internal/v1/projects/{project_id}` — rename a project. Its slug moves
/// with the name, so the answer carries both and the console follows it.
pub async fn update_project(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<ProjectId>,
    principal: Principal,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<impl IntoResponse, telmoni_shared::TelmoniError> {
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
    let slug = projects::update_name(&mut acting.tx, &organization_id, &project_id, &trimmed)
        .await
        .map_err(|e| name_taken(e, &trimmed))?;
    let acting = acting.leave_owner_scope().await?;
    let Some(slug) = slug else {
        return Err(AuthError::NotFound("project not found".into()).into());
    };

    acting.tx.commit().await?;
    Ok(Json(ProjectRenamed {
        name: trimmed,
        slug,
    }))
}
