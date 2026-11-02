//! `GET /internal/v1/telemetry/usage` — what an organization has spent of
//! its plan's month, for its owner and admins: the span units since the
//! month began, the content it holds and the data out sent today, each
//! against what the plan holds. A binary that sells nothing answers none.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::Serialize;

use telmoni_shared::TelmoniError;
use telmoni_shared::extract::Json;

use crate::{AppState, capacity, ceilings};

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
    })))
}
