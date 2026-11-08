//! `GET /internal/v1/telemetry/limits` — how an organization divides its
//! pool among its projects, read by every member: the plan's numbers, and
//! each project they can open with its caps and what it has used of each.
//! `PUT /internal/v1/telemetry/limits` — one project's caps set, on its
//! scope, by the organization's owners and admins alone, each at most the
//! plan's and audited ([`crate::project_limits`]).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::Utc;
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{Binding, Scoped, maintenance_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::{AuditAction, TelmoniError, TelmoniResourceKind};

use super::chain;
use crate::db::TelemetryLane;
use crate::project_limits::{self, Caps};
use crate::store::{Projects, seconds};
use crate::{AppState, capacity};

/// Each project's span units since `from`.
const UNITS_SQL: &str = concat!(
    "SELECT h.project_id AS project_id, toUInt64(sum(h.units)) AS units ",
    "FROM telemetry.spans_hourly AS h ",
    "WHERE h.project_id IN {projects:Array(String)} AND h.hour >= {from:DateTime('UTC')} ",
    "GROUP BY project_id"
);

/// One project's span units since a moment, as the hourly totals count them.
#[derive(Debug, Row, Deserialize)]
struct ProjectUnits {
    project_id: String,
    units: u64,
}

/// What the plan holds of each limit a project may be capped on; `None`
/// where nothing is sold, or the plan holds no number of it.
#[derive(Debug, Default, Serialize)]
pub struct PlanLimits {
    pub spans: Option<u64>,
    pub content_bytes: Option<u64>,
    pub alerts: Option<u32>,
    pub alert_deliveries_per_day: Option<u32>,
}

/// What a project has used of each limit: its span units this month, none
/// where the span store did not answer, the content it holds, its alerts,
/// monitors and objectives, and its alert deliveries today where capped.
#[derive(Debug, Default, Serialize)]
pub struct Used {
    pub spans: Option<u64>,
    pub content_bytes: u64,
    pub alerts: u64,
    pub monitors: u64,
    pub objectives: u64,
    pub alert_deliveries_today: Option<u64>,
}

/// One project, its caps and what it has used.
#[derive(Debug, Serialize)]
pub struct ProjectShare {
    pub id: String,
    pub name: String,
    pub caps: Caps,
    pub used: Used,
}

/// The organization's division of its pool.
#[derive(Debug, Serialize)]
pub struct Limits {
    pub plan: PlanLimits,
    pub projects: Vec<ProjectShare>,
}

/// The plan's numbers, as the entitlements seam answers them.
async fn plan_limits(
    state: &AppState,
    organization_id: &telmoni_shared::OrganizationId,
) -> PlanLimits {
    let capacity = state
        .capacities
        .of(state.entitlements.as_ref(), organization_id)
        .await;
    let alerts =
        telmoni_shared::seam::alert_ceilings(state.entitlements.as_ref(), organization_id).await;
    PlanLimits {
        spans: capacity.as_ref().map(|c| c.spans.limit),
        content_bytes: capacity.as_ref().map(|c| c.content_bytes),
        alerts: alerts.as_ref().map(|a| a.alerts),
        alert_deliveries_per_day: alerts.as_ref().map(|a| a.deliveries_per_day),
    }
}

/// A count by project among `projects`, as `sql` groups it.
async fn counts<B: Binding>(
    tx: &mut Scoped<'_, B>,
    sql: &'static str,
    projects: &[String],
) -> sqlx::Result<HashMap<String, i64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(sql)
        .bind(projects)
        .fetch_all(tx.conn())
        .await?;
    Ok(rows.into_iter().collect())
}

/// `GET /internal/v1/telemetry/limits`, every member's.
pub async fn get_limits(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Limits>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let organization_id = acting.organization_id.clone();
    let plan = plan_limits(&state, &organization_id).await;
    let readable = state.auth.readable_projects(&acting).await?;
    let ids: Vec<String> = readable
        .iter()
        .map(|project| project.project_id.to_string())
        .collect();

    let mut lane = maintenance_scope(&state.db, TelemetryLane).await?;
    let caps = project_limits::caps_in(&mut lane, &organization_id).await?;
    let alerts = counts(
        &mut lane,
        "SELECT project_id, count(*) FROM telemetry.rules
          WHERE project_id = ANY($1) AND objective IS NULL GROUP BY project_id",
        &ids,
    )
    .await?;
    let objectives = counts(
        &mut lane,
        "SELECT project_id, count(*) FROM telemetry.rules
          WHERE project_id = ANY($1) AND objective IS NOT NULL GROUP BY project_id",
        &ids,
    )
    .await?;
    let monitors = counts(
        &mut lane,
        "SELECT project_id, count(*) FROM telemetry.monitors
          WHERE project_id = ANY($1) GROUP BY project_id",
        &ids,
    )
    .await?;
    let content = counts(
        &mut lane,
        "SELECT project_id, sum(bytes)::bigint FROM (
             SELECT project_id, bytes FROM telemetry.plain_content WHERE project_id = ANY($1)
             UNION ALL
             SELECT project_id, bytes FROM telemetry.media WHERE project_id = ANY($1)
         ) held GROUP BY project_id",
        &ids,
    )
    .await?;
    lane.commit().await?;

    let period = capacity::project_period(&state, &organization_id, Utc::now()).await;
    // The page stands without the span store: its units are said unknown.
    let units: Option<HashMap<String, u64>> =
        match Projects::of(readable.iter().map(|project| project.project_id.clone())) {
            Some(projects) => {
                let from = chrono::DateTime::from_timestamp(period, 0).unwrap_or_default();
                match state
                    .store
                    .read(&projects, UNITS_SQL)
                    .param("from", seconds(from))
                    .fetch_all::<ProjectUnits>()
                    .await
                {
                    Ok(rows) => Some(
                        rows.into_iter()
                            .map(|row| (row.project_id, row.units))
                            .collect(),
                    ),
                    Err(error) => {
                        tracing::warn!(organization_id = %organization_id, error = %error,
                            "telemetry: the projects' units could not be read for their limits");
                        None
                    }
                }
            }
            None => Some(HashMap::new()),
        };

    let today = Utc::now().date_naive();
    let mut connection = state.redis.connection().await;
    let mut projects = Vec::with_capacity(readable.len());
    for project in readable {
        let id = project.project_id.to_string();
        let caps = caps.get(&id).copied().unwrap_or_default();
        let count =
            |of: &HashMap<String, i64>| of.get(&id).map_or(0, |n| u64::try_from(*n).unwrap_or(0));
        let alert_deliveries_today = match (&mut connection, caps.alert_deliveries_per_day) {
            (Some(connection), Some(_)) => {
                let key = format!("telemetry:project-alert-deliveries:{id}:{today}");
                let spent: Result<Option<u64>, redis::RedisError> =
                    redis::AsyncCommands::get(connection, &key).await;
                spent.ok().map(Option::unwrap_or_default)
            }
            _ => None,
        };
        projects.push(ProjectShare {
            used: Used {
                spans: units
                    .as_ref()
                    .map(|units| units.get(&id).copied().unwrap_or(0)),
                content_bytes: count(&content),
                alerts: count(&alerts),
                monitors: count(&monitors),
                objectives: count(&objectives),
                alert_deliveries_today,
            },
            id,
            name: project.name,
            caps,
        });
    }
    Ok(Json(Limits { plan, projects }))
}

/// The bounds a cap is held to: none below nothing, and none above the
/// plan's number, where it holds one.
fn bounded(what: &str, cap: Option<i64>, most: Option<u64>) -> Result<(), TelmoniError> {
    let Some(cap) = cap else {
        return Ok(());
    };
    if cap < 0 {
        return Err(TelmoniError::BadRequest(format!(
            "a project's cap on {what} is a number of none or more"
        )));
    }
    if let Some(most) = most
        && u64::try_from(cap).unwrap_or(0) > most
    {
        return Err(TelmoniError::BadRequest(format!(
            "a project's cap on {what} is at most the plan's, {most}"
        )));
    }
    Ok(())
}

/// `PUT /internal/v1/telemetry/limits` — the scoped project's caps, set
/// whole by the organization's owners and admins; none of them unset.
pub async fn put_limits(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(caps): Json<Caps>,
) -> Result<Json<Caps>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    acting.require_organization_admin()?;
    let project_id = acting
        .project
        .as_ref()
        .ok_or_else(|| TelmoniError::BadRequest("name the project its caps are set on".into()))?
        .project_id
        .clone();
    let plan = plan_limits(&state, &acting.organization_id).await;
    bounded("spans a month", caps.spans, plan.spans)?;
    bounded("content held", caps.content_bytes, plan.content_bytes)?;
    bounded(
        "alerts",
        caps.alerts.map(i64::from),
        plan.alerts.map(u64::from),
    )?;
    bounded("monitors", caps.monitors.map(i64::from), None)?;
    bounded(
        "objectives",
        caps.objectives.map(i64::from),
        plan.alerts.map(u64::from),
    )?;
    bounded(
        "alert deliveries a day",
        caps.alert_deliveries_per_day.map(i64::from),
        plan.alert_deliveries_per_day.map(u64::from),
    )?;

    let auth = &*state.auth;
    chain::write(&state.db, &acting, &project_id, "limits", async |tx| {
        chain::hold(auth, &acting, tx)
            .await?
            .require_organization_admin()?;
        // Filed, so the purge sweep, which pages the settings rows, finds a
        // project that holds caps and nothing else; and its caps hold.
        crate::db::ensure_settings(tx, &project_id, &acting.organization_id).await?;
        let before =
            project_limits::set_caps(tx, &project_id, &acting.organization_id, &caps).await?;
        if before != caps {
            emit_audit(
                tx,
                AuditEvent::new(
                    &acting.organization_id,
                    Actor::User(acting.user_id.as_str()),
                    AuditAction::Updated,
                    TelmoniResourceKind::Project,
                )
                .in_project(&project_id)
                .on(project_id.as_str())
                .details(json!({ "limits": { "from": before, "to": caps } })),
            )
            .await?;
        }
        Ok(())
    })
    .await?;
    state.project_caps.forget(&project_id);
    // Nothing counted the project while it had no span cap: its month so far
    // counts from now on. Failing that, the hourly pass does it.
    if caps.spans.is_some()
        && let Err(error) =
            capacity::raise_project_spans(&state, &acting.organization_id, &project_id).await
    {
        tracing::warn!(project_id = %project_id, error = %error,
            "telemetry: a capped project's month could not be counted yet");
    }
    tracing::info!(project_id = %project_id, "telemetry: a project's caps set");
    Ok(Json(caps))
}
