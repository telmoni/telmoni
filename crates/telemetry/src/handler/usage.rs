//! `GET /internal/v1/telemetry/usage` — what an organization has spent of
//! its plan's month, for its owner and admins: the span units since the
//! month began, the content it holds and the data out sent today, each
//! against what the plan holds, and each project's span units a UTC day from
//! the hourly totals. A binary that sells nothing answers none.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::TelmoniError;
use telmoni_shared::extract::Json;

use crate::store::{Projects, seconds};
use crate::{AppState, capacity, ceilings};

/// Each project's span units a UTC day from `from`, oldest first.
const DAYS_SQL: &str = concat!(
    "SELECT toString(toDate(h.hour)) AS day, h.project_id AS project_id, ",
    "toUInt64(sum(h.units)) AS units FROM telemetry.spans_hourly AS h ",
    "WHERE h.project_id IN {projects:Array(String)} AND h.hour >= {from:DateTime('UTC')} ",
    "GROUP BY day, project_id HAVING units > 0 ORDER BY day, project_id"
);

/// One project's span units on one UTC day, as the hourly totals count them.
#[derive(Debug, Row, Deserialize, Serialize)]
pub struct DayUnits {
    /// `YYYY-MM-DD`.
    pub day: String,
    pub project_id: String,
    pub units: u64,
}

/// A project the days name, with the name a page labels it by.
#[derive(Debug, Serialize)]
pub struct UsageProject {
    pub id: String,
    pub name: String,
}

/// How much of one thing, against what the plan holds.
#[derive(Debug, Serialize)]
pub struct Spent {
    pub used: u64,
    pub limit: u64,
}

/// The organization's month, as its plan holds it.
#[derive(Debug, Serialize)]
pub struct Usage {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub spans: Spent,
    pub content_held: Spent,
    /// Today, UTC, where the plan holds a day's data out.
    pub data_out_today: Option<Spent>,
    /// Each project's units a day since the period began, a day with none left out.
    pub days: Vec<DayUnits>,
    /// The projects `days` names.
    pub projects: Vec<UsageProject>,
}

/// `GET /internal/v1/telemetry/usage`, the owner's and admins' alone.
pub async fn get_usage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Option<Usage>>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let organization_id = &acting.require_organization_admin()?.organization_id;
    let Some(held) =
        telmoni_shared::seam::capacity(state.entitlements.as_ref(), organization_id).await
    else {
        return Ok(Json(None));
    };
    let spans = capacity::spans_used(&state, organization_id, &held, false).await?;
    let content = capacity::content_held(&state, organization_id).await?;
    let data_out = ceilings::data_out_today(&state, organization_id).await;
    let readable = state.auth.readable_projects(&acting).await?;
    let days: Vec<DayUnits> =
        match Projects::of(readable.iter().map(|project| project.project_id.clone())) {
            Some(projects) => state
                .store
                .read(&projects, DAYS_SQL)
                .param("from", seconds(held.period_start))
                .fetch_all()
                .await
                .map_err(|error| {
                    crate::store::failed(
                        &error,
                        "a month's units by day",
                        "the span store did not answer; try again",
                    )
                })?,
            None => Vec::new(),
        };
    let projects = readable
        .into_iter()
        .filter(|project| {
            days.iter()
                .any(|day| day.project_id == project.project_id.as_str())
        })
        .map(|project| UsageProject {
            id: project.project_id.to_string(),
            name: project.name,
        })
        .collect();
    Ok(Json(Some(Usage {
        period_start: held.period_start,
        period_end: held.period_end,
        spans: Spent {
            used: spans,
            limit: held.spans.limit,
        },
        content_held: Spent {
            used: content,
            limit: held.content_bytes,
        },
        data_out_today: data_out.map(|(used, limit)| Spent { used, limit }),
        days,
        projects,
    })))
}
