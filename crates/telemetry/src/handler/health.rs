//! An organization's health and spend, on its Overview: its incidents still
//! open on the projects the person can open, which auth answers, read under
//! the organization's binding and narrowed to those projects here; and, to
//! its owners and admins, what each project spent this period.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Serialize;

use telmoni_shared::db::tenant_session::organization_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb, can};
use telmoni_shared::seam::ReadableProject;
use telmoni_shared::slug::Slug;
use telmoni_shared::{AuthzError, ProjectId, TelmoniError};

use crate::AppState;
use crate::handler::incidents::INCIDENTS_MAX;
use crate::rules::incidents;
use crate::rules::incidents::IncidentRow;

/// The project an open incident is on, as the Overview names and links it.
#[derive(Debug, Serialize)]
pub struct HealthProject {
    pub id: ProjectId,
    pub slug: Slug,
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

/// One project's spend this period, against its budget where it set one.
#[derive(Debug, Serialize)]
pub struct ProjectSpend {
    pub project: HealthProject,
    /// In US dollars, each call reported or priced.
    pub spent: f64,
    /// Its calls with neither a cost nor a price, which `spent` leaves out.
    pub unpriced_calls: u64,
    /// The alert threshold of its budget, a running cost alert summed over the
    /// month with no filter; none where it set none.
    pub budget: Option<f64>,
    /// What its budget read of the calendar month when last evaluated, which
    /// a plan's period need not match; none until it is.
    pub budget_spent: Option<f64>,
}

/// What the Overview's spend answers: the period and each project with a
/// model call in it, the costliest first.
#[derive(Debug, Serialize)]
pub struct Spend {
    pub period_start: chrono::DateTime<chrono::Utc>,
    pub period_end: chrono::DateTime<chrono::Utc>,
    pub total: f64,
    pub projects: Vec<ProjectSpend>,
}

/// The period an organization's spend is read over: its plan's month where a
/// plan holds it, else the calendar month in UTC.
async fn period(
    state: &AppState,
    acting: &telmoni_shared::acting::Acting,
) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
    if let Some(capacity) = state
        .capacities
        .of(state.entitlements.as_ref(), &acting.organization_id)
        .await
    {
        return (capacity.period_start, capacity.period_end);
    }
    let now = chrono::Utc::now();
    let month = crate::rules::evaluate::Window::of(now, telmoni_shared::RuleWindow::Month);
    let end = month
        .from
        .checked_add_months(chrono::Months::new(1))
        .unwrap_or(now);
    (month.from, end)
}

/// `GET /internal/v1/telemetry/organizations/{id}/spend` — what each of the
/// acting organization's projects spent this period, to its owners and
/// admins alone, each against its budget.
pub async fn organization_spend(
    State(state): State<Arc<AppState>>,
    Path(organization): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Spend>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    if organization != acting.organization_id.as_str() {
        return Err(AuthzError::Forbidden("not the acting organization".into()).into());
    }
    acting.require_organization_admin()?;
    let projects: HashMap<ProjectId, ReadableProject> = state
        .auth
        .readable_projects(&acting)
        .await?
        .into_iter()
        .map(|project| (project.project_id.clone(), project))
        .collect();
    let (period_start, period_end) = period(&state, &acting).await;
    let Some(read) = crate::store::Projects::of(projects.keys().cloned()) else {
        return Ok(Json(Spend {
            period_start,
            period_end,
            total: 0.0,
            projects: Vec::new(),
        }));
    };
    let spent = crate::spend::projects(
        &state,
        &read,
        period_start,
        chrono::Utc::now().min(period_end),
    )
    .await
    .map_err(|unread| match unread {
        crate::spend::Unread::Store(error) => crate::store::failed(
            &error,
            "an organization's spend",
            "the span store did not answer; try again",
        ),
        crate::spend::Unread::Db(error) => error.into(),
    })?;
    let ids: Vec<ProjectId> = projects.keys().cloned().collect();
    let mut tx = organization_scope(&state.db, &acting.organization_id).await?;
    // Each project's tightest running budget, with its own month so far as last read.
    let budgets: Vec<(String, f64, Option<f64>)> = sqlx::query_as(
        "SELECT DISTINCT ON (project_id) project_id, alert_threshold, value
           FROM telemetry.rules
          WHERE organization_id = $1 AND project_id = ANY($2) AND paused IS NULL
            AND kind = 'run' AND measure = 'cost' AND aggregation = 'sum'
            AND lookback = 'month' AND filters = '{}'::jsonb
          ORDER BY project_id, alert_threshold",
    )
    .bind(&acting.organization_id)
    .bind(&ids)
    .fetch_all(tx.conn())
    .await?;
    tx.commit().await?;
    let budgets: HashMap<String, (f64, Option<f64>)> = budgets
        .into_iter()
        .map(|(project, threshold, value)| (project, (threshold, value)))
        .collect();
    let by_id: HashMap<&str, &ReadableProject> = projects
        .values()
        .map(|project| (project.project_id.as_str(), project))
        .collect();
    let rows: Vec<ProjectSpend> = spent
        .into_iter()
        .filter_map(|row| {
            let project = by_id.get(row.project_id.as_str())?;
            Some(ProjectSpend {
                project: HealthProject {
                    id: project.project_id.clone(),
                    slug: project.slug.clone(),
                    name: project.name.clone(),
                },
                spent: row.spent,
                unpriced_calls: row.unpriced_calls,
                budget: budgets.get(&row.project_id).map(|budget| budget.0),
                budget_spent: budgets.get(&row.project_id).and_then(|budget| budget.1),
            })
        })
        .collect();
    Ok(Json(Spend {
        period_start,
        period_end,
        total: rows.iter().map(|row| row.spent).sum(),
        projects: rows,
    }))
}
