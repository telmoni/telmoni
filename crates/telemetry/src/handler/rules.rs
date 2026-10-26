//! A project's alerts, under [`Resource::Rule`]: `GET` and `POST
//! /internal/v1/telemetry/rules`, and `GET`, `PUT` and `DELETE
//! /rules/{id}`, every one open to any member of the project, as Langfuse
//! lets members make alerts. An alert is its definition, checked whole
//! ([`RuleInput::checked`]), and its last evaluation, which the loop writes
//! ([`crate::rules::evaluate`]). One past its organization's plan, paused
//! ones counted, is refused when it is made: a `402` in the plan's own
//! words. No change to an alert is audited: it reads what the project's
//! spans already say, and changes what nobody may see.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::AlertCeilings;
use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam;
use telmoni_shared::{
    AuthError, MissingData, ProjectId, RuleAggregation, RuleMeasure, RuleOperator, RuleWindow,
    Severity, SpanKind, TelmoniError,
};

use crate::AppState;
use crate::db;
use crate::handler::chain;
use crate::rules::db::{self as rules, RuleRow};
use crate::rules::{Rule, RuleFilters, RuleInput};

/// The unique index a second alert of one name in a project, in any case,
/// runs into.
const NAME_TAKEN: &str = "rules_project_name_idx";

/// One alert, as the lanes answer it.
#[derive(Debug, Serialize)]
pub struct Alert {
    pub id: Uuid,
    pub name: String,
    pub kind: SpanKind,
    pub measure: RuleMeasure,
    pub aggregation: RuleAggregation,
    pub filters: RuleFilters,
    pub window: RuleWindow,
    pub operator: RuleOperator,
    pub alert_threshold: f64,
    pub warning_threshold: Option<f64>,
    pub missing_data: MissingData,
    pub renotify_minutes: Option<i32>,
    /// Who paused it, `person` or `plan`; none while it runs.
    pub paused: Option<String>,
    pub paused_at: Option<DateTime<Utc>>,
    /// Its value at its last evaluation: none before the first, or where
    /// the window held no span its filters admit.
    pub value: Option<f64>,
    pub evaluated_at: Option<DateTime<Utc>>,
    /// Where it stands, and since when: `paused` while it is.
    pub severity: Severity,
    pub severity_since: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<RuleRow> for Alert {
    fn from(row: RuleRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            kind: row.kind,
            measure: row.measure,
            aggregation: row.aggregation,
            filters: row.filters.0,
            window: row.lookback,
            operator: row.operator,
            alert_threshold: row.alert_threshold,
            warning_threshold: row.warning_threshold,
            missing_data: row.missing_data,
            renotify_minutes: row.renotify_minutes,
            paused: row.paused,
            paused_at: row.paused_at,
            value: row.value,
            evaluated_at: row.evaluated_at,
            severity: row.severity,
            severity_since: row.severity_since,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// What the list of alerts answers.
#[derive(Debug, Serialize)]
pub struct AlertList {
    /// The project's alerts, by name.
    pub rules: Vec<Alert>,
}

/// An alert's id from its path: a UUID, or an alert the project does not have.
fn rule_id(given: &str) -> Result<Uuid, TelmoniError> {
    given.parse().map_err(|_| not_found())
}

fn not_found() -> TelmoniError {
    AuthError::NotFound("no alert with that id in this project".into()).into()
}

/// A write the unique index refused: the project has an alert by that name.
/// Any other failure is the database's.
fn name_taken(error: sqlx::Error) -> TelmoniError {
    let taken = error
        .as_database_error()
        .and_then(|db| db.constraint())
        .is_some_and(|constraint| constraint == NAME_TAKEN);
    if taken {
        return AuthError::Conflict("this project has an alert by that name already".into()).into();
    }
    error.into()
}

/// `GET /internal/v1/telemetry/rules` — the project's alerts, by name.
pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<AlertList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Rule)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = rules::rules(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(AlertList {
        rules: rows.into_iter().map(Alert::from).collect(),
    }))
}

/// `GET /internal/v1/telemetry/rules/{id}` — one of the project's alerts.
pub async fn get_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Alert>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Rule)?
        .project_id;
    let id = rule_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let row = rules::rule(&mut tx, project_id, id).await?;
    tx.commit().await?;
    row.map(|row| Json(Alert::from(row))).ok_or_else(not_found)
}

/// `POST /internal/v1/telemetry/rules` — a new alert of the project. Its
/// plan's number is read first, outside any lock, and held to under the
/// organization's chain lock, so two alerts made at once in two of its
/// projects never both take its last place; the project's settings row,
/// which the purges find its alerts by, is made with it where there is none.
pub async fn create_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<RuleInput>,
) -> Result<(StatusCode, Json<Alert>), TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Create, Resource::Rule)?
        .project_id
        .clone();
    let rule = input.checked()?;
    let ceilings = seam::alert_ceilings(state.entitlements.as_ref(), &acting.organization_id).await;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match save(
        &state,
        &acting,
        &project_id,
        &mut tx,
        &rule,
        ceilings.as_ref(),
    )
    .await
    {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "alert");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, rule_id = %row.id, "telemetry: an alert made");
    Ok((StatusCode::CREATED, Json(Alert::from(row))))
}

/// The save itself, under the organization's chain lock: the role checked
/// on auth's second answer, the settings row made or re-filed, the
/// organization's alerts counted against its plan, and the alert written.
async fn save(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    rule: &Rule,
    ceilings: Option<&AlertCeilings>,
) -> Result<RuleRow, TelmoniError> {
    let now = chain::hold(state.auth.as_ref(), acting, project_id, tx).await?;
    now.require_project(Verb::Create, Resource::Rule)?;
    db::ensure_settings(tx, project_id, &acting.organization_id).await?;
    if let Some(ceilings) = ceilings {
        let held = rules::organization_count(tx, &acting.organization_id).await?;
        ceilings.admit(u64::try_from(held).unwrap_or(0))?;
    }
    rules::insert(
        tx,
        project_id,
        &acting.organization_id,
        Uuid::new_v4(),
        rule,
    )
    .await
    .map_err(name_taken)
}

/// `PUT /internal/v1/telemetry/rules/{id}` — one of the project's alerts,
/// saved over whole; its last evaluation is forgotten, since it answered
/// another question, and the loop evaluates it again within a minute.
pub async fn update_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RuleInput>,
) -> Result<Json<Alert>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Rule)?
        .project_id;
    let id = rule_id(&id)?;
    let rule = input.checked()?;
    let mut tx = chain::scoped(&state.db, &acting, project_id).await?;
    let row = rules::update(&mut tx, project_id, id, &rule)
        .await
        .map_err(name_taken)?
        .ok_or_else(not_found)?;
    tx.commit().await?;
    tracing::info!(project_id = %project_id, rule_id = %id, "telemetry: an alert saved over");
    Ok(Json(Alert::from(row)))
}

/// `DELETE /internal/v1/telemetry/rules/{id}` — one of the project's
/// alerts, gone.
pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Delete, Resource::Rule)?
        .project_id;
    let id = rule_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    if !rules::delete(&mut tx, project_id, id).await? {
        return Err(not_found());
    }
    tx.commit().await?;
    tracing::info!(project_id = %project_id, rule_id = %id, "telemetry: an alert deleted");
    Ok(StatusCode::NO_CONTENT)
}
