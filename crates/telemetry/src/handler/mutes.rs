//! A project's maintenance windows, under [`Resource::Alert`] as its alerts
//! are: `GET` and `POST /internal/v1/telemetry/mutes`, and `POST
//! /mutes/{id}/end`, each open to any member of the project. A window mutes
//! one alert, or every alert of the project, from its start — now, unless
//! given — to its end, at most thirty days on; ended early, it keeps who
//! ended it. A project keeps its newest [`MUTES_ENDED_KEPT`] ended, and each
//! window opened or ended early is filed in the organization's audit chain,
//! the record of who muted what.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuditAction, AuthError, TelmoniError, TelmoniResourceKind};

use crate::AppState;
use crate::db;
use crate::handler::chain::{self, Change};
use crate::rules::db as rules;
use crate::rules::mutes::{self, MUTE_DAYS_MAX, MuteRow};

/// The most windows the list answers.
pub const MUTES_MAX: i64 = 200;

/// The most ended windows a project keeps, the newest: as many as the list
/// could show, so none it keeps is out of reach.
pub const MUTES_ENDED_KEPT: i64 = MUTES_MAX;

/// The most windows a project holds that have not ended: fewer than the
/// list answers, so a window in force is always on the page to be ended.
pub const MUTES_UNENDED_MAX: i64 = 50;

/// A window as a person asks for one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MuteInput {
    /// The alert to mute; none, every alert of the project.
    #[serde(default)]
    pub rule_id: Option<Uuid>,
    /// When it starts; now, if none.
    #[serde(default)]
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: DateTime<Utc>,
}

/// A window, as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct Mute {
    pub id: Uuid,
    pub rule_id: Option<Uuid>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    /// Who muted it, by id.
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    /// Who ended it early, by id.
    pub ended_by: Option<String>,
}

impl From<MuteRow> for Mute {
    fn from(row: MuteRow) -> Self {
        Self {
            id: row.id,
            rule_id: row.rule_id,
            starts_at: row.starts_at,
            ends_at: row.ends_at,
            created_by: row.created_by,
            created_at: row.created_at,
            ended_by: row.ended_by,
        }
    }
}

/// What the list of windows answers.
#[derive(Debug, Serialize)]
pub struct MuteList {
    pub mutes: Vec<Mute>,
}

fn not_found() -> TelmoniError {
    AuthError::NotFound("no maintenance window with that id in this project".into()).into()
}

/// `GET /internal/v1/telemetry/mutes` — the project's windows, those not
/// ended first, then those ending last, [`MUTES_MAX`] at most.
pub async fn list_mutes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<MuteList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Alert)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = mutes::mutes(&mut tx, project_id, MUTES_MAX).await?;
    tx.commit().await?;
    Ok(Json(MuteList {
        mutes: rows.into_iter().map(Mute::from).collect(),
    }))
}

/// `POST /internal/v1/telemetry/mutes` — a window muting one of the
/// project's alerts, or all of them: ending after it starts and ahead of
/// now, and at most [`MUTE_DAYS_MAX`] days long.
pub async fn create_mute(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<MuteInput>,
) -> Result<(StatusCode, Json<Mute>), TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Create, Resource::Alert)?
        .project_id;
    let now = Utc::now();
    let starts_at = input.starts_at.unwrap_or(now);
    let bad = |why: &str| TelmoniError::BadRequest(why.to_owned());
    if input.ends_at <= starts_at || input.ends_at <= now {
        return Err(bad(
            "a maintenance window ends after it starts, and after now",
        ));
    }
    if input.ends_at - starts_at > TimeDelta::days(MUTE_DAYS_MAX) {
        return Err(bad(&format!(
            "a maintenance window is at most {MUTE_DAYS_MAX} days long"
        )));
    }
    let row = chain::write(
        &state.db,
        &acting,
        project_id,
        "maintenance window",
        async |tx| open(&state, &acting, tx, &input, starts_at).await,
    )
    .await?;
    tracing::info!(project_id = %project_id, mute_id = %row.id, "telemetry: a maintenance window made");
    Ok((StatusCode::CREATED, Json(Mute::from(row))))
}

/// The window itself, under the organization's chain lock: the role checked
/// on auth's second answer, the settings row made or re-filed under it, the
/// windows not ended counted against the cap, and the window written and
/// filed.
async fn open(
    state: &AppState,
    acting: &Acting,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    input: &MuteInput,
    starts_at: DateTime<Utc>,
) -> Result<MuteRow, TelmoniError> {
    let project_id = &acting
        .require_project(Verb::Create, Resource::Alert)?
        .project_id;
    chain::held(
        state.auth.as_ref(),
        acting,
        tx,
        Verb::Create,
        Resource::Alert,
    )
    .await?;
    // The settings row, by which the organization's erasure finds the window.
    db::ensure_settings(tx, project_id, &acting.organization_id).await?;
    if mutes::unended(tx, project_id).await? >= MUTES_UNENDED_MAX {
        return Err(AuthError::Conflict(format!(
            "a project holds at most {MUTES_UNENDED_MAX} maintenance windows that have not \
             ended; end one to set another"
        ))
        .into());
    }
    if let Some(rule_id) = input.rule_id
        && rules::rule(tx, project_id, rule_id).await?.is_none()
    {
        return Err(AuthError::NotFound("no alert with that id in this project".into()).into());
    }
    let row = mutes::insert(
        tx,
        project_id,
        input.rule_id,
        starts_at,
        input.ends_at,
        &acting.user_id,
    )
    .await?;
    mutes::prune(tx, project_id, MUTES_ENDED_KEPT).await?;
    let change = Change {
        action: AuditAction::Created,
        kind: TelmoniResourceKind::MaintenanceWindow,
        id: row.id,
        metadata: json!({
            "rule_id": row.rule_id,
            "starts_at": row.starts_at,
            "ends_at": row.ends_at,
        }),
    };
    chain::file(tx, acting, project_id, change).await?;
    Ok(row)
}

/// `POST /internal/v1/telemetry/mutes/{id}/end` — one of the project's
/// windows ended now, by the person; one ended already is answered as it
/// stands.
pub async fn end_mute(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Mute>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Alert)?
        .project_id;
    let id: Uuid = id.parse().map_err(|_| not_found())?;
    let row = chain::write(
        &state.db,
        &acting,
        project_id,
        "maintenance window",
        async |tx| ended(&state, &acting, tx, id).await,
    )
    .await?;
    Ok(Json(Mute::from(row)))
}

/// The end itself, under the organization's chain lock: the role checked on
/// auth's second answer, and the window ended and filed, where this ended it.
async fn ended(
    state: &AppState,
    acting: &Acting,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    id: Uuid,
) -> Result<MuteRow, TelmoniError> {
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
    let (row, ended) = mutes::end(tx, project_id, id, &acting.user_id).await?;
    let row = row.ok_or_else(not_found)?;
    if ended {
        mutes::prune(tx, project_id, MUTES_ENDED_KEPT).await?;
        let change = Change {
            action: AuditAction::Updated,
            kind: TelmoniResourceKind::MaintenanceWindow,
            id,
            metadata: json!({ "rule_id": row.rule_id, "ended_at": row.ends_at }),
        };
        chain::file(tx, acting, project_id, change).await?;
    }
    Ok(row)
}
