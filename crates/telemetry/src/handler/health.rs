//! An organization's health, on its Overview: its incidents still open on the
//! projects the person can open, which auth answers, read under the
//! organization's binding and narrowed to those projects here.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Serialize;

use telmoni_shared::db::tenant_session::organization_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb, can};
use telmoni_shared::seam::ReadableProject;
use telmoni_shared::{AuthzError, ProjectId, TelmoniError};

use crate::AppState;
use crate::handler::incidents::INCIDENTS_MAX;
use crate::rules::incidents;
use crate::rules::incidents::IncidentRow;

/// The project an open incident is on, as the Overview names and links it.
#[derive(Debug, Serialize)]
pub struct HealthProject {
    pub id: ProjectId,
    pub slug: String,
    pub name: String,
}

/// One open incident, with its project.
#[derive(Debug, Serialize)]
pub struct HealthIncident {
    pub project: HealthProject,
    #[serde(flatten)]
    pub incident: IncidentRow,
}

/// What the Overview reads: no incident at all when all is well.
#[derive(Debug, Serialize)]
pub struct Health {
    pub incidents: Vec<HealthIncident>,
}

/// `GET /internal/v1/telemetry/organizations/{id}/health` — the acting
/// organization's open incidents on the projects the person may read alerts
/// on, the newest first, [`INCIDENTS_MAX`] at most.
pub async fn organization_health(
    State(state): State<Arc<AppState>>,
    Path(organization): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Health>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    if organization != acting.organization_id.as_str() {
        return Err(AuthzError::Forbidden("not the acting organization".into()).into());
    }
    let projects: HashMap<ProjectId, ReadableProject> = state
        .auth
        .readable_projects(&acting)
        .await?
        .into_iter()
        .filter(|project| can(project.role, Verb::Read, Resource::Alert))
        .map(|project| (project.project_id.clone(), project))
        .collect();
    if projects.is_empty() {
        return Ok(Json(Health {
            incidents: Vec::new(),
        }));
    }
    let ids: Vec<ProjectId> = projects.keys().cloned().collect();
    let mut tx = organization_scope(&state.db, &acting.organization_id).await?;
    let rows = incidents::open_in(&mut tx, &acting.organization_id, &ids, INCIDENTS_MAX).await?;
    tx.commit().await?;
    Ok(Json(Health {
        incidents: rows
            .into_iter()
            .filter_map(|row| {
                let project = projects.get(&row.project_id)?;
                Some(HealthIncident {
                    project: HealthProject {
                        id: project.project_id.clone(),
                        slug: project.slug.clone(),
                        name: project.name.clone(),
                    },
                    incident: row,
                })
            })
            .collect(),
    }))
}
