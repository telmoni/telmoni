//! A project's incidents, under [`Resource::Rule`] as their alerts are: `GET
//! /internal/v1/telemetry/incidents`, those not resolved first, then the
//! newest, or with `?run=` those whose notices showed that run, and
//! `/incidents/{id}` with its votes; `POST /incidents/{id}/vote`,
//! `/acknowledge` and `/resolve`, which any member of the project sends; and
//! `GET /precision`, the share of the week's votes calling its incidents
//! useful. Acknowledging one stops its reminders; resolving one by hand is
//! told as its recovery is, and an alert still past its threshold opens
//! nothing new until it recovers and breaches again.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{AuthError, IncidentStatus, Severity, TelmoniError, Verdict};

use crate::AppState;
use crate::rules::incidents::{self, IncidentRow};
use crate::rules::votes;

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
        .require_project(Verb::Read, Resource::Rule)?
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
        crate::notify::resolved_by_hand(&state, &row).await;
    }
    Ok(Json(Incident::from(row)))
}

/// One incident with its votes, as the vote page reads it.
#[derive(Debug, Serialize)]
pub struct IncidentVotes {
    #[serde(flatten)]
    pub incident: Incident,
    pub useful: i64,
    pub noise: i64,
    /// The asker's own word on it, if they gave one.
    pub yours: Option<Verdict>,
}

/// `GET /internal/v1/telemetry/incidents/{id}` — one of the project's
/// incidents, with its votes and the person's own.
pub async fn get_incident(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<IncidentVotes>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Rule)?
        .project_id;
    let id = incident_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let row = incidents::one(&mut tx, project_id, id)
        .await?
        .ok_or_else(not_found)?;
    let tally = votes::tally(&mut tx, project_id, id, &acting.user_id).await?;
    tx.commit().await?;
    Ok(Json(IncidentVotes {
        incident: Incident::from(row),
        useful: tally.useful,
        noise: tally.noise,
        yours: tally.yours,
    }))
}

/// A member's word on an incident.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoteInput {
    pub verdict: Verdict,
}

/// `POST /internal/v1/telemetry/incidents/{id}/vote` — the person's word on
/// one of the project's incidents, useful or noise, given or changed: a
/// `POST` alone, since the link a notice carries opens a page that asks, and
/// a link a mail scanner or a chat's preview follows must cast nothing.
pub async fn vote_incident(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VoteInput>,
) -> Result<Json<IncidentVotes>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Update, Resource::Rule)?
        .project_id;
    let id = incident_id(&id)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    if !votes::vote(&mut tx, project_id, id, &acting.user_id, input.verdict).await? {
        return Err(not_found());
    }
    let row = incidents::one(&mut tx, project_id, id)
        .await?
        .ok_or_else(not_found)?;
    let tally = votes::tally(&mut tx, project_id, id, &acting.user_id).await?;
    tx.commit().await?;
    tracing::info!(project_id = %project_id, incident_id = %id, verdict = %input.verdict, "telemetry: an incident voted on");
    Ok(Json(IncidentVotes {
        incident: Incident::from(row),
        useful: tally.useful,
        noise: tally.noise,
        yours: tally.yours,
    }))
}

/// How far back the precision reads, in days.
pub const PRECISION_DAYS: i64 = 7;

/// One alert's share useful.
#[derive(Debug, Serialize)]
pub struct RulePrecision {
    pub rule_id: Uuid,
    pub rule_name: String,
    pub useful: i64,
    pub noise: i64,
    /// Useful over every vote; none without a vote.
    pub ratio: Option<f64>,
}

/// What the precision answers: the project's share of votes calling its
/// incidents useful over the window, and each alert's.
#[derive(Debug, Serialize)]
pub struct Precision {
    pub days: i64,
    pub useful: i64,
    pub noise: i64,
    pub ratio: Option<f64>,
    pub rules: Vec<RulePrecision>,
}

/// Useful over every vote, none without one.
#[expect(
    clippy::cast_precision_loss,
    reason = "a vote count past 2^53 is no project's"
)]
fn ratio(useful: i64, noise: i64) -> Option<f64> {
    let votes = useful + noise;
    (votes > 0).then(|| useful as f64 / votes as f64)
}

/// `GET /internal/v1/telemetry/precision` — the share of the votes on the
/// project's incidents opened in the last [`PRECISION_DAYS`] that call them
/// useful, and each alert's: what heads its alerts, so the noisy ones are
/// retuned or deleted.
pub async fn get_precision(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Precision>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Rule)?
        .project_id;
    let since = Utc::now() - TimeDelta::days(PRECISION_DAYS);
    let mut tx = project_scope(&state.db, project_id).await?;
    let rules = votes::precision(&mut tx, project_id, since).await?;
    tx.commit().await?;
    let (useful, noise) = rules
        .iter()
        .fold((0, 0), |(u, n), rule| (u + rule.useful, n + rule.noise));
    Ok(Json(Precision {
        days: PRECISION_DAYS,
        useful,
        noise,
        ratio: ratio(useful, noise),
        rules: rules
            .into_iter()
            .map(|rule| RulePrecision {
                ratio: ratio(rule.useful, rule.noise),
                rule_id: rule.rule_id,
                rule_name: rule.rule_name,
                useful: rule.useful,
                noise: rule.noise,
            })
            .collect(),
    }))
}
