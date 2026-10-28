//! A project's incidents, under [`Resource::Alert`] as their alerts are: `GET
//! /internal/v1/telemetry/incidents`, those not resolved first, then the
//! newest, or with `?run=` those whose notices showed that run, and
//! `/incidents/{id}`; `POST /incidents/{id}/acknowledge` and `/resolve`,
//! which any member of the project sends, each filed in the organization's
//! audit chain. Acknowledging one stops its reminders; resolving one by hand
//! is told as its recovery is, and an alert still past its threshold opens
//! nothing new until it recovers and breaches again, or rises from the
//! warning resolved to an alert.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, AuthError, IncidentStatus, Severity, TelmoniError, TelmoniResourceKind,
};

use crate::AppState;
use crate::handler::chain::{self, Change};
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
    /// The connections its notices were queued to; every incident is in
    /// the project's feed besides.
    pub told_to: Vec<Uuid>,
    /// The runs its notices showed, oldest first.
    pub runs: Vec<Uuid>,
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
            told_to: row.told_to,
            runs: row.runs,
        }
    }
}

/// What the list of incidents answers.
#[derive(Debug, Serialize)]
pub struct IncidentList {
    pub incidents: Vec<Incident>,
}

/// The list's query: none, or the run whose incidents a trace's page links.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentQuery {
    #[serde(default)]
    pub run: Option<String>,
}

fn incident_id(given: &str) -> Result<Uuid, TelmoniError> {
    given.parse().map_err(|_| not_found())
}

fn not_found() -> TelmoniError {
    AuthError::NotFound("no incident with that id in this project".into()).into()
}

/// `GET /internal/v1/telemetry/incidents` — the project's incidents, those
/// not resolved first, then the newest; with `?run=`, those whose notices
/// showed that run, the newest first; [`INCIDENTS_MAX`] at most.
pub async fn list_incidents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<IncidentQuery>,
) -> Result<Json<IncidentList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let run = match query.run.as_deref() {
        None => None,
        Some(given) => Some(
            given
                .parse::<Uuid>()
                .map_err(|_| TelmoniError::BadRequest("run is a run's id, a UUID".into()))?,
        ),
    };
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = match run {
        Some(run) => incidents::showing(&mut tx, project_id, run, INCIDENTS_MAX).await?,
        None => incidents::incidents(&mut tx, project_id, INCIDENTS_MAX).await?,
    };
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
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = incident_id(&id)?;
    let (row, _) = chain::write(&state.db, &acting, project_id, "incident", async |tx| {
        moved(
            &state,
            &acting,
            &headers,
            tx,
            id,
            IncidentStatus::Acknowledged,
        )
        .await
    })
    .await?;
    tracing::info!(project_id = %project_id, incident_id = %id, "telemetry: an incident acknowledged");
    Ok(Json(Incident::from(row)))
}

/// An incident taken or resolved by the member, under the organization's
/// chain lock: the role checked on auth's second answer, and the move made
/// and filed, where this made it. Answers the incident, and whether it moved.
async fn moved(
    state: &AppState,
    acting: &Acting,
    headers: &HeaderMap,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    id: Uuid,
    to: IncidentStatus,
) -> Result<(IncidentRow, bool), TelmoniError> {
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    chain::held(
        state.auth.as_ref(),
        acting,
        tx,
        Verb::Update,
        Resource::Alert,
    )
    .await?;
    let (row, moved) = if to == IncidentStatus::Resolved {
        incidents::resolve(tx, project_id, id, &acting.user_id).await?
    } else {
        incidents::acknowledge(tx, project_id, id, &acting.user_id).await?
    };
    let row = row.ok_or_else(not_found)?;
    if moved {
        let change = Change {
            action: AuditAction::Updated,
            kind: TelmoniResourceKind::Incident,
            id,
            metadata: json!({ "alert": row.rule_name, "rule_id": row.rule_id, "status": to }),
        };
        chain::file(tx, acting, project_id, headers, change).await?;
    }
    Ok((row, moved))
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
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = incident_id(&id)?;
    let (row, resolved) = chain::write(&state.db, &acting, project_id, "incident", async |tx| {
        moved(&state, &acting, &headers, tx, id, IncidentStatus::Resolved).await
    })
    .await?;
    if resolved {
        tracing::info!(project_id = %project_id, incident_id = %id, "telemetry: an incident resolved by hand");
        crate::notify::resolved_by_hand(&state, &row).await;
    }
    Ok(Json(Incident::from(row)))
}

/// `GET /internal/v1/telemetry/incidents/{id}` — one of the project's
/// incidents, as the console reads the one a trace links past a full list.
pub async fn get_incident(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Incident>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let id = incident_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let row = incidents::one(&mut tx, project_id, id)
        .await?
        .ok_or_else(not_found)?;
    tx.commit().await?;
    Ok(Json(Incident::from(row)))
}
