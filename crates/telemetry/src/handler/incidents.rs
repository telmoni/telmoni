//! A project's incidents, under [`Resource::Alert`] as their alerts are: `GET
//! /internal/v1/telemetry/incidents`, those not resolved first, then the
//! newest, or with `?trace=` those whose notices showed that trace, and
//! `/incidents/{id}`; `POST /incidents/{id}/acknowledge` and `/resolve`,
//! which any member of the project sends, each filed in the organization's
//! audit chain; an Acknowledge pressed on its Slack notice takes the same
//! path. Acknowledging one stops its reminders; resolving one by hand
//! is told as its recovery is, and an alert still past its threshold opens
//! nothing new until it recovers and breaches again, or rises from the
//! warning resolved to an alert. Each is answered with the changes its
//! alert's or monitor's agents made in the day before it opened
//! ([`crate::changes`]), read as it is answered, so one filed late is named.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::IncidentActed;
use telmoni_shared::{
    AuditAction, AuthError, IncidentStatus, ProjectId, TelmoniError, TelmoniResourceKind,
};

use crate::AppState;
use crate::changes::{self, Opened, Scope};
use crate::handler::chain;
use crate::rules::incidents::{self, IncidentRow};

/// The most incidents the list answers.
pub const INCIDENTS_MAX: i64 = 200;

/// An incident as the lanes answer it: its row, and what its agents changed
/// to in the [`changes::BEFORE_INCIDENT`] before it opened.
#[derive(Debug, Serialize)]
pub struct IncidentAnswer {
    #[serde(flatten)]
    pub row: IncidentRow,
    /// The newest first, [`changes::INCIDENT_CHANGES_MAX`] at most; none
    /// where they could not be read, which is not that nothing changed.
    pub changes: Option<Vec<changes::Change>>,
    /// Whether it had more than are named.
    pub changes_more: bool,
}

/// What the list of incidents answers.
#[derive(Debug, Serialize)]
pub struct IncidentList {
    pub incidents: Vec<IncidentAnswer>,
}

/// The list's query: none, or the trace whose incidents its page links.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncidentQuery {
    #[serde(default)]
    pub trace: Option<String>,
}

fn incident_id(given: &str) -> Result<Uuid, TelmoniError> {
    given.parse().map_err(|_| not_found())
}

fn not_found() -> TelmoniError {
    AuthError::NotFound("no incident with that id in this project".into()).into()
}

/// Each incident's changes, by its id, in two reads however many there are:
/// their alerts' filters, a monitor's agent being on its row, then the changes.
async fn changes_of(
    state: &AppState,
    project_id: &ProjectId,
    rows: &[IncidentRow],
) -> Result<HashMap<Uuid, (Vec<changes::Change>, bool)>, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let alerts: Vec<Uuid> = rows.iter().filter_map(|row| row.rule_id).collect();
    let filters: HashMap<Uuid, Scope> = crate::rules::db::filters_of(&mut tx, project_id, &alerts)
        .await?
        .into_iter()
        .map(|(id, filters)| (id, Scope::of_alert(&filters.0)))
        .collect();
    let opened: Vec<Opened> = rows
        .iter()
        .filter_map(|row| {
            let scope = match row.owner()? {
                incidents::Owner::Rule(id) => filters.get(&id)?.clone(),
                incidents::Owner::Monitor(_) => Scope::of_monitor(row.agent.as_deref()?),
            };
            Some(Opened {
                incident_id: row.id,
                opened_at: row.opened_at,
                scope,
            })
        })
        .collect();
    let around = changes::around_incidents(&mut tx, project_id, &opened).await?;
    tx.commit().await?;
    Ok(around)
}

/// Each of `rows`' changes by its id; none, logged, where they cannot be
/// read, since an incident is answered all the same.
async fn changes_read(
    state: &AppState,
    project_id: &ProjectId,
    rows: &[IncidentRow],
) -> Option<HashMap<Uuid, (Vec<changes::Change>, bool)>> {
    match changes_of(state, project_id, rows).await {
        Ok(around) => Some(around),
        Err(error) => {
            tracing::warn!(
                project_id = %project_id,
                error = %error,
                "telemetry: the changes before a project's incidents could not be read; \
                 they are answered without"
            );
            None
        }
    }
}

/// `row` with what was read of its changes: none where nothing was.
fn answer(
    row: IncidentRow,
    around: Option<&mut HashMap<Uuid, (Vec<changes::Change>, bool)>>,
) -> IncidentAnswer {
    let (changes, changes_more) = match around {
        Some(around) => {
            let (changes, more) = around.remove(&row.id).unwrap_or_default();
            (Some(changes), more)
        }
        None => (None, false),
    };
    IncidentAnswer {
        row,
        changes,
        changes_more,
    }
}

/// `rows` as the lanes answer them, each with its changes.
async fn with_changes(
    state: &AppState,
    project_id: &ProjectId,
    rows: Vec<IncidentRow>,
) -> Vec<IncidentAnswer> {
    let mut around = changes_read(state, project_id, &rows).await;
    rows.into_iter()
        .map(|row| answer(row, around.as_mut()))
        .collect()
}

/// One incident as the lanes answer it.
async fn answered(state: &AppState, project_id: &ProjectId, row: IncidentRow) -> IncidentAnswer {
    let mut around = changes_read(state, project_id, std::slice::from_ref(&row)).await;
    answer(row, around.as_mut())
}

/// `GET /internal/v1/telemetry/incidents` — the project's incidents, those
/// not resolved first, then the newest; with `?trace=`, those whose notices
/// showed that trace, the newest first; [`INCIDENTS_MAX`] at most.
pub async fn list_incidents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<IncidentQuery>,
) -> Result<Json<IncidentList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    Ok(Json(incident_list(&state, project_id, query).await?))
}

/// The project's incidents, as the console's lane and `/v1` answer them.
pub(crate) async fn incident_list(
    state: &AppState,
    project_id: &ProjectId,
    query: IncidentQuery,
) -> Result<IncidentList, TelmoniError> {
    let trace = match query.trace.as_deref() {
        None => None,
        Some(given) => Some(super::traces::trace_id_of(given).map_err(|_| {
            TelmoniError::BadRequest("trace is a trace's id, 32 hex characters".into())
        })?),
    };
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = match trace {
        Some(trace) => incidents::showing(&mut tx, project_id, &trace, INCIDENTS_MAX).await?,
        None => incidents::incidents(&mut tx, project_id, INCIDENTS_MAX).await?,
    };
    tx.commit().await?;
    Ok(IncidentList {
        incidents: with_changes(state, project_id, rows).await,
    })
}

/// `POST /internal/v1/telemetry/incidents/{id}/acknowledge` — one of the
/// project's incidents taken by the person, where it is open: its reminders
/// stop. One taken already, or resolved, is answered as it stands.
pub async fn acknowledge_incident(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<IncidentAnswer>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = incident_id(&id)?;
    let (row, _) = acknowledge(&state, &acting, id).await?;
    Ok(Json(answered(&state, project_id, row).await))
}

/// One of the project's incidents taken by the person `acting` names, from
/// the console or a press of its notice's button alike: answers it, and
/// whether this took it.
pub(crate) async fn acknowledge(
    state: &AppState,
    acting: &Acting,
    id: Uuid,
) -> Result<(IncidentRow, bool), TelmoniError> {
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let taken = chain::write(&state.db, acting, project_id, "incident", async |tx| {
        moved(state, acting, tx, id, Take::Acknowledge).await
    })
    .await?;
    tracing::info!(project_id = %project_id, incident_id = %id, "telemetry: an incident acknowledged");
    Ok(taken)
}

/// What a press of an incident notice's Acknowledge came to.
pub(crate) async fn acknowledged(
    state: &AppState,
    acting: &Acting,
    id: Uuid,
) -> Result<IncidentActed, TelmoniError> {
    let (row, taken) = acknowledge(state, acting, id).await?;
    Ok(if taken {
        IncidentActed::Acknowledged
    } else if row.status == IncidentStatus::Resolved {
        IncidentActed::ResolvedAlready
    } else {
        IncidentActed::AcknowledgedAlready
    })
}

/// What a member does to an incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Take {
    Acknowledge,
    Resolve,
}

impl Take {
    /// The status it leaves the incident at.
    const fn status(self) -> IncidentStatus {
        match self {
            Self::Acknowledge => IncidentStatus::Acknowledged,
            Self::Resolve => IncidentStatus::Resolved,
        }
    }
}

/// An incident taken or resolved by the member, under the organization's
/// chain lock: the role checked on auth's second answer, and the move made
/// and filed, where this made it. Answers the incident, and whether it moved.
async fn moved(
    state: &AppState,
    acting: &Acting,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    id: Uuid,
    take: Take,
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
    let (row, moved) = match take {
        Take::Resolve => incidents::resolve(tx, project_id, id, &acting.user_id).await?,
        Take::Acknowledge => incidents::acknowledge(tx, project_id, id, &acting.user_id).await?,
    };
    let row = row.ok_or_else(not_found)?;
    if moved {
        let change = chain::Change {
            action: AuditAction::Updated,
            kind: TelmoniResourceKind::Incident,
            id: id.to_string(),
            metadata: match row.owner() {
                Some(incidents::Owner::Monitor(monitor_id)) => json!({
                    "agent": row.agent,
                    "monitor_id": monitor_id,
                    "status": take.status(),
                }),
                _ => json!({
                    "alert": row.rule_name,
                    "rule_id": row.rule_id,
                    "status": take.status(),
                }),
            },
        };
        chain::file(tx, acting, project_id, change).await?;
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
) -> Result<Json<IncidentAnswer>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = incident_id(&id)?;
    let (row, resolved) = chain::write(&state.db, &acting, project_id, "incident", async |tx| {
        moved(&state, &acting, tx, id, Take::Resolve).await
    })
    .await?;
    if resolved {
        tracing::info!(project_id = %project_id, incident_id = %id, "telemetry: an incident resolved by hand");
        crate::notify::resolved_by_hand(&state, &row).await;
    }
    Ok(Json(answered(&state, project_id, row).await))
}

/// `GET /internal/v1/telemetry/incidents/{id}` — one of the project's
/// incidents, as the console reads the one a trace links past a full list.
pub async fn get_incident(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<IncidentAnswer>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    Ok(Json(incident(&state, project_id, &id).await?))
}

/// One of the project's incidents, as the console's lane and `/v1` answer it.
pub(crate) async fn incident(
    state: &AppState,
    project_id: &ProjectId,
    id: &str,
) -> Result<IncidentAnswer, TelmoniError> {
    let id = incident_id(id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let row = incidents::one(&mut tx, project_id, id)
        .await?
        .ok_or_else(not_found)?;
    tx.commit().await?;
    Ok(answered(state, project_id, row).await)
}
