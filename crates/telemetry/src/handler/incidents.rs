//! A project's incidents, under [`Resource::Rule`] as their alerts are: `GET
//! /internal/v1/telemetry/incidents`, those not resolved first, then the
//! newest; and `POST /incidents/{id}/acknowledge` and `/resolve`, which any
//! member of the project sends. Acknowledging one stops its reminders;
//! resolving one by hand is told as its recovery is, and an alert still past
//! its threshold opens nothing new until it recovers and breaches again.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuthError, IncidentStatus, Severity, TelmoniError};

use crate::AppState;
use crate::rules::incidents::{self, IncidentRow};

/// The most incidents the list answers.
pub const INCIDENTS_MAX: i64 = 200;

/// One incident, as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct Incident {
    pub id: Uuid,
    pub rule_id: Uuid,
    /// Its alert's name.
    pub rule_name: String,
    pub status: IncidentStatus,
    pub severity: Severity,
    /// The value it last breached, or recovered, at.
    pub value: Option<f64>,
    pub opened_at: DateTime<Utc>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    /// Who took it, by id.
    pub acknowledged_by: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
    /// Who resolved it, by id; none where its alert recovered.
    pub resolved_by: Option<String>,
    /// How often it breached again within an hour of a recovery.
    pub reopened: i32,
}

impl From<IncidentRow> for Incident {
    fn from(row: IncidentRow) -> Self {
        Self {
            id: row.id,
            rule_id: row.rule_id,
            rule_name: row.rule_name,
            status: row.status,
            severity: row.severity,
            value: row.value,
            opened_at: row.opened_at,
            acknowledged_at: row.acknowledged_at,
            acknowledged_by: row.acknowledged_by,
            resolved_at: row.resolved_at,
            resolved_by: row.resolved_by,
            reopened: row.reopened,
        }
    }
}

/// What the list of incidents answers.
#[derive(Debug, Serialize)]
pub struct IncidentList {
    pub incidents: Vec<Incident>,
}

fn incident_id(given: &str) -> Result<Uuid, TelmoniError> {
    given.parse().map_err(|_| not_found())
}

fn not_found() -> TelmoniError {
    AuthError::NotFound("no incident with that id in this project".into()).into()
}

/// `GET /internal/v1/telemetry/incidents` — the project's incidents, those
/// not resolved first, then the newest, [`INCIDENTS_MAX`] at most.
pub async fn list_incidents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<IncidentList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Rule)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = incidents::incidents(&mut tx, project_id, INCIDENTS_MAX).await?;
    tx.commit().await?;
    Ok(Json(IncidentList {
        incidents: rows.into_iter().map(Incident::from).collect(),
    }))
}

/// `POST /internal/v1/telemetry/incidents/{id}/acknowledge` — one of the
/// project's incidents taken by the person, where it is open: its reminders
/// stop. One taken already, or resolved, is answered as it stands.
pub async fn acknowledge_incident(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Incident>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Rule)?
        .project_id;
    let id = incident_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let row = incidents::acknowledge(&mut tx, project_id, id, &acting.user_id)
        .await?
        .ok_or_else(not_found)?;
    tx.commit().await?;
    tracing::info!(project_id = %project_id, incident_id = %id, "telemetry: an incident acknowledged");
    Ok(Json(Incident::from(row)))
}

/// `POST /internal/v1/telemetry/incidents/{id}/resolve` — one of the
/// project's incidents resolved by the person, told as a recovery is. One
/// resolved already is answered as it stands, and told nothing again.
pub async fn resolve_incident(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Incident>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Rule)?
        .project_id;
    let id = incident_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let (row, resolved) = incidents::resolve(&mut tx, project_id, id, &acting.user_id).await?;
    let row = row.ok_or_else(not_found)?;
    tx.commit().await?;
    if resolved {
        tracing::info!(project_id = %project_id, incident_id = %id, "telemetry: an incident resolved by hand");
        crate::notify::resolved_by_hand(
            &state,
            &row.organization_id,
            project_id,
            id,
            row.rule_id,
            &row.rule_name,
            row.severity,
        )
        .await;
    }
    Ok(Json(Incident::from(row)))
}
