//! A project's monitors, under [`Resource::Alert`] as its alerts are: `GET`
//! and `POST /internal/v1/telemetry/monitors`, `GET`, `PUT` and `DELETE
//! /monitors/{id}`, and `POST /monitors/{id}/pause` and `/resume`, every one
//! open to any member of the project. A monitor is one agent's schedule,
//! checked whole ([`MonitorInput::checked`]), and where the loop last judged
//! it ([`crate::monitors`]). Every change a member makes to one is filed in
//! the organization's audit chain.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuditAction, AuthError, ProjectId, TelmoniError, TelmoniResourceKind};

use crate::AppState;
use crate::db;
use crate::handler::chain::{self, Change};
use crate::handler::rules::AlertLink;
use crate::monitors::{self, Monitor, MonitorInput, MonitorRow};
use crate::rules::Link;
use crate::rules::incidents::{self, IncidentRow, Owner};

/// The unique index a second monitor of one agent in a project runs into.
const AGENT_TAKEN: &str = "monitors_project_agent_idx";

/// One monitor, as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct MonitorAnswer {
    #[serde(flatten)]
    pub monitor: MonitorRow,
    /// The connectors its notices go to.
    pub connectors: Vec<AlertLink>,
}

impl MonitorAnswer {
    fn of(monitor: MonitorRow, links: Vec<Link>, muted: &[Uuid]) -> Self {
        let connectors = links
            .into_iter()
            .map(|link| AlertLink {
                muted: muted.contains(&link.connection_id),
                link,
            })
            .collect();
        Self {
            monitor,
            connectors,
        }
    }
}

/// What the list of monitors answers.
#[derive(Debug, Serialize)]
pub struct MonitorList {
    /// The project's monitors, by agent.
    pub monitors: Vec<MonitorAnswer>,
}

fn monitor_id(given: &str) -> Result<Uuid, TelmoniError> {
    given.parse().map_err(|_| not_found())
}

fn not_found() -> TelmoniError {
    AuthError::NotFound("no monitor with that id in this project".into()).into()
}

/// A write the unique index refused: the agent has a monitor already.
fn agent_taken(error: sqlx::Error) -> TelmoniError {
    let taken = error
        .as_database_error()
        .and_then(|db| db.constraint())
        .is_some_and(|constraint| constraint == AGENT_TAKEN);
    if taken {
        return AuthError::Conflict("this project has a monitor of that agent already".into())
            .into();
    }
    error.into()
}

/// The connectors five failed deliveries in a row muted for the monitor;
/// none, logged, where notifications cannot say.
async fn muted(state: &AppState, project_id: &ProjectId, id: Uuid) -> Vec<Uuid> {
    match state
        .notifications
        .muted(project_id, &Owner::Monitor(id).route_key())
        .await
    {
        Ok(muted) => muted,
        Err(error) => {
            tracing::warn!(
                project_id = %project_id,
                monitor_id = %id,
                error = %error,
                "telemetry: which of a monitor's connectors are muted could not be read"
            );
            Vec::new()
        }
    }
}

/// The organization's chain lock taken in `tx`, and `verb` on the project's
/// alerts and monitors checked on auth's answer under it.
async fn held(
    state: &AppState,
    acting: &Acting,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    verb: Verb,
) -> Result<ProjectId, TelmoniError> {
    let project_id = acting
        .require_project(verb, Resource::Alert)?
        .project_id
        .clone();
    chain::held(state.auth.as_ref(), acting, tx, verb, Resource::Alert).await?;
    Ok(project_id)
}

/// `GET /internal/v1/telemetry/monitors` — the project's monitors, by agent.
pub async fn list_monitors(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<MonitorList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = monitors::monitors(&mut tx, project_id).await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let links = monitors::links(&mut tx, project_id, &ids).await?;
    tx.commit().await?;
    let mut linked: HashMap<Uuid, Vec<Link>> = HashMap::with_capacity(rows.len());
    for (id, link) in links {
        linked.entry(id).or_default().push(link);
    }
    // One read of every monitor's muted connectors, not one a row.
    let keys: Vec<String> = ids
        .iter()
        .map(|id| Owner::Monitor(*id).route_key())
        .collect();
    let mut muted = match state.notifications.muted_routes(project_id, &keys).await {
        Ok(muted) => muted,
        Err(error) => {
            tracing::warn!(
                project_id = %project_id,
                error = %error,
                "telemetry: which of the monitors' connectors are muted could not be read"
            );
            HashMap::new()
        }
    };
    let monitors = rows
        .into_iter()
        .zip(&keys)
        .map(|(row, key)| {
            let own = linked.remove(&row.id).unwrap_or_default();
            let muted = muted.remove(key).unwrap_or_default();
            MonitorAnswer::of(row, own, &muted)
        })
        .collect();
    Ok(Json(MonitorList { monitors }))
}

/// `GET /internal/v1/telemetry/monitors/{id}` — one of the project's monitors.
pub async fn get_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MonitorAnswer>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let id = monitor_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let row = monitors::monitor(&mut tx, project_id, id)
        .await?
        .ok_or_else(not_found)?;
    let links = monitors::links(&mut tx, project_id, &[id]).await?;
    tx.commit().await?;
    let muted = muted(&state, project_id, id).await;
    Ok(Json(MonitorAnswer::of(
        row,
        links.into_iter().map(|(_, link)| link).collect(),
        &muted,
    )))
}

/// `POST /internal/v1/telemetry/monitors` — a new monitor of one of the
/// project's agents, `new` until that agent's next run; the project's
/// settings row, which the purges find its monitors by, made with it.
pub async fn create_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<MonitorInput>,
) -> Result<(StatusCode, Json<MonitorAnswer>), TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Create, Resource::Alert)?
        .project_id
        .clone();
    let monitor = input.checked()?;
    let row = chain::write(&state.db, &acting, &project_id, "monitor", async |tx| {
        save(&state, &acting, tx, &monitor).await
    })
    .await?;
    tracing::info!(project_id = %project_id, monitor_id = %row.id, "telemetry: a monitor made");
    Ok((
        StatusCode::CREATED,
        Json(MonitorAnswer::of(row, monitor.connectors, &[])),
    ))
}

/// The save itself, under the organization's chain lock: the role checked
/// on auth's second answer, the settings row made or re-filed, and the
/// monitor written and filed.
async fn save(
    state: &AppState,
    acting: &Acting,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    monitor: &Monitor,
) -> Result<MonitorRow, TelmoniError> {
    let project_id = &held(state, acting, tx, Verb::Create).await?;
    db::ensure_settings(tx, project_id, &acting.organization_id).await?;
    let row = monitors::insert(tx, project_id, &acting.organization_id, monitor)
        .await
        .map_err(agent_taken)?;
    monitors::write_links(tx, project_id, row.id, &monitor.connectors).await?;
    let change = Change {
        action: AuditAction::Created,
        kind: TelmoniResourceKind::Monitor,
        id: row.id,
        metadata: json!({ "agent": row.agent }),
    };
    chain::file(tx, acting, project_id, change).await?;
    Ok(row)
}

/// `PUT /internal/v1/telemetry/monitors/{id}` — one of the project's
/// monitors saved over whole: its schedule, grace and connectors. Its agent
/// is the one it was made for; another agent is another monitor.
pub async fn update_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<MonitorInput>,
) -> Result<Json<MonitorAnswer>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = monitor_id(&id)?;
    let monitor = input.checked()?;
    let row = chain::write(&state.db, &acting, project_id, "monitor", async |tx| {
        held(&state, &acting, tx, Verb::Update).await?;
        let before = monitors::monitor(tx, project_id, id)
            .await?
            .ok_or_else(not_found)?;
        if before.agent != monitor.agent {
            return Err(TelmoniError::BadRequest(
                "a monitor's agent is the one it was made for; make another for another agent"
                    .into(),
            ));
        }
        let next_due = monitors::due_after_edit(&before, &monitor)
            .map_err(|fault| TelmoniError::BadRequest(fault.detail().to_owned()))?;
        let row = monitors::update(tx, project_id, id, &monitor, next_due)
            .await
            .map_err(agent_taken)?
            .ok_or_else(not_found)?;
        monitors::write_links(tx, project_id, id, &monitor.connectors).await?;
        let changed: Vec<&str> = [
            (
                "schedule",
                (before.every_seconds, &before.cron, &before.time_zone)
                    != (row.every_seconds, &row.cron, &row.time_zone),
            ),
            ("grace_seconds", before.grace_seconds != row.grace_seconds),
        ]
        .into_iter()
        .filter_map(|(field, moved)| moved.then_some(field))
        .collect();
        let change = Change {
            action: AuditAction::Updated,
            kind: TelmoniResourceKind::Monitor,
            id,
            metadata: json!({ "agent": row.agent, "changed": changed }),
        };
        chain::file(tx, &acting, project_id, change).await?;
        Ok(row)
    })
    .await?;
    tracing::info!(project_id = %project_id, monitor_id = %id, "telemetry: a monitor saved over");
    let muted = muted(&state, project_id, id).await;
    Ok(Json(MonitorAnswer::of(row, monitor.connectors, &muted)))
}

/// `DELETE /internal/v1/telemetry/monitors/{id}` — one of the project's
/// monitors, gone with its incidents and windows; its open incident is told
/// resolved, as a member's resolution is, to the connectors it linked.
pub async fn delete_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Delete, Resource::Alert)?
        .project_id;
    let id = monitor_id(&id)?;
    let (open, links): (Option<IncidentRow>, Vec<Link>) =
        chain::write(&state.db, &acting, project_id, "monitor", async |tx| {
            held(&state, &acting, tx, Verb::Delete).await?;
            let row = monitors::monitor(tx, project_id, id)
                .await?
                .ok_or_else(not_found)?;
            let open = incidents::open_of(tx, Owner::Monitor(id)).await?;
            let links = monitors::links(tx, project_id, &[id])
                .await?
                .into_iter()
                .map(|(_, link)| link)
                .collect();
            if !monitors::delete(tx, project_id, id).await? {
                return Err(not_found());
            }
            let change = Change {
                action: AuditAction::Deleted,
                kind: TelmoniResourceKind::Monitor,
                id,
                metadata: json!({ "agent": row.agent }),
            };
            chain::file(tx, &acting, project_id, change).await?;
            Ok((open, links))
        })
        .await?;
    tracing::info!(project_id = %project_id, monitor_id = %id, "telemetry: a monitor deleted");
    if let Some(open) = open {
        crate::notify::deleted_with(&state, &open, links).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /internal/v1/telemetry/monitors/{id}/pause` — one of the project's
/// monitors paused: not judged, and telling nothing, until it is resumed.
pub async fn pause_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MonitorAnswer>, TelmoniError> {
    paused(state, headers, id, false).await
}

/// `POST /internal/v1/telemetry/monitors/{id}/resume` — one of the project's
/// paused monitors `new` again, its next run due after its agent's next start.
pub async fn resume_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MonitorAnswer>, TelmoniError> {
    paused(state, headers, id, true).await
}

/// A pause or a resume, filed where it changed the monitor; one paused, or
/// running, already is answered as it stands.
async fn paused(
    state: Arc<AppState>,
    headers: HeaderMap,
    id: String,
    resume: bool,
) -> Result<Json<MonitorAnswer>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id = monitor_id(&id)?;
    let (row, links) = chain::write(&state.db, &acting, project_id, "monitor", async |tx| {
        held(&state, &acting, tx, Verb::Update).await?;
        let before = monitors::monitor(tx, project_id, id)
            .await?
            .ok_or_else(not_found)?;
        let row = monitors::pause(tx, project_id, id, resume)
            .await?
            .ok_or_else(not_found)?;
        if before.state != row.state {
            let change = Change {
                action: AuditAction::Updated,
                kind: TelmoniResourceKind::Monitor,
                id,
                metadata: json!({
                    "agent": row.agent,
                    "state": { "from": before.state, "to": row.state },
                }),
            };
            chain::file(tx, &acting, project_id, change).await?;
        }
        Ok((row, monitors::links(tx, project_id, &[id]).await?))
    })
    .await?;
    tracing::info!(project_id = %project_id, monitor_id = %id, resume, "telemetry: a monitor paused or resumed");
    let muted = muted(&state, project_id, id).await;
    Ok(Json(MonitorAnswer::of(
        row,
        links.into_iter().map(|(_, link)| link).collect(),
        &muted,
    )))
}
