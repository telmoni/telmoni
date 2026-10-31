//! A project's privacy posture as its Privacy page reads it: `GET
//! /internal/v1/telemetry/privacy`, what its spans arrived with over the last
//! [`SENT_DAYS`] by the field inventory's rows, dropped content among them,
//! and how long its spans and the hourly totals that outlive them are kept.
//! Every role on the project `x-project-id` names reads it.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, Duration, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::TelmoniError;
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};

use crate::AppState;
use crate::retention::{ROLLUP_RETENTION_MONTHS, SPAN_RETENTION_DAYS};
use crate::store::{self, Projects, micros};

/// The days the page counts back over.
pub const SENT_DAYS: u32 = 30;

/// The spans received in the window, and how many carried each row: by the
/// partition's day and `received_at_idx`.
const SENT_SQL: &str = "SELECT count() AS spans,
            sumMap(sent, arrayWithConstant(length(sent), toUInt64(1))) AS sent
       FROM telemetry.spans
      WHERE project_id IN {projects:Array(String)}
        AND received_at >= {since:DateTime64(6, 'UTC')}";

#[derive(Row, Deserialize)]
struct Counted {
    spans: u64,
    sent: (Vec<String>, Vec<u64>),
}

/// What the lane answers.
#[derive(Debug, Serialize)]
pub struct Posture {
    /// [`SENT_DAYS`].
    pub days: u32,
    /// Where they begin, which the page's list of reads starts from too.
    pub since: DateTime<Utc>,
    /// The spans received in them.
    pub spans: u64,
    /// Each inventory row a span arrived with, most spans first.
    pub sent: Vec<Sent>,
    /// How long what the spans leave is kept.
    pub retention: Retention,
}

/// One inventory row, and the spans that arrived with it.
#[derive(Debug, Serialize)]
pub struct Sent {
    /// The row's `attribute`, as the inventory spells it.
    pub attribute: String,
    pub spans: u64,
}

/// The windows `telmoni-server rotate` holds, the same for every project.
#[derive(Debug, Serialize)]
pub struct Retention {
    /// Spans, and the content an `on` project keeps with them.
    pub span_days: i64,
    /// The hourly totals, which name no user and no tag.
    pub rollup_months: u32,
}

/// `GET /internal/v1/telemetry/privacy` — the project's posture.
pub async fn get_privacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Posture>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id;
    let since = Utc::now() - Duration::days(i64::from(SENT_DAYS));
    let counted: Counted = state
        .store
        .read(&Projects::one(project_id.clone()), SENT_SQL)
        .param("since", micros(since))
        .fetch_one()
        .await
        .map_err(|e| {
            store::failed(
                &e,
                "a project's privacy posture",
                "the span store did not answer; try again",
            )
        })?;
    let (attributes, spans) = counted.sent;
    let mut sent: Vec<Sent> = attributes
        .into_iter()
        .zip(spans)
        .map(|(attribute, spans)| Sent { attribute, spans })
        .collect();
    sent.sort_by(|a, b| {
        b.spans
            .cmp(&a.spans)
            .then_with(|| a.attribute.cmp(&b.attribute))
    });
    Ok(Json(Posture {
        days: SENT_DAYS,
        since,
        spans: counted.spans,
        sent,
        retention: Retention {
            span_days: SPAN_RETENTION_DAYS,
            rollup_months: ROLLUP_RETENTION_MONTHS,
        },
    }))
}
