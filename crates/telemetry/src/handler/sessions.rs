//! The console's session lanes, under `Resource::Trace`, which every role on
//! a project holds: `GET /internal/v1/telemetry/sessions`, a page of the
//! sessions list as the trace list's filters narrow it, with its total and
//! each session's own scores; `/sessions/facets`, each filter's values and
//! how many sessions hold a trace carrying each; and `/sessions/export`, the
//! rows as a file. Each reads the spans of the project `x-project-id` names
//! and no other.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use telmoni_shared::extract::{Json, Query};
use telmoni_shared::{SessionSort, SortDirection, TelmoniError};

use crate::AppState;
use crate::handler::traces::{
    Facets, download, export_rows, facets_page, list_page, read_failed, reading,
};
use crate::scores::Summaries;
use crate::sessions::{SessionRow, Sessions};
use crate::store::Projects;
use crate::traces::ListSort;

/// What the list answers.
#[derive(Debug, Serialize)]
pub struct SessionList {
    /// The page's sessions, in the order asked.
    pub sessions: Vec<SessionRow>,
    /// How many days back the list read.
    pub days: u16,
    /// The most a page holds.
    pub limit: u32,
    /// Which page this is, from 1.
    pub page: u32,
    /// How many sessions the filter admits in the window, on every page.
    pub total: u64,
    /// Whether a page after this one holds more.
    pub more: bool,
    /// The sort it is in.
    pub sort: ListSort<SessionSort>,
    /// And its direction.
    pub dir: SortDirection,
    /// Each session's own scores by name, keyed by its id as stored: those
    /// given to the session, never its traces'.
    pub scores: Summaries,
}

/// A download's header row: the list's fields, in its order.
const CSV_HEADER: [&str; 14] = [
    "session_id",
    "user_id",
    "agents",
    "traces",
    "failed_traces",
    "input_tokens",
    "output_tokens",
    "spend",
    "duration_ms",
    "first_seen",
    "last_seen",
    "last_ended",
    "last_status",
    "tags",
];

impl SessionRow {
    /// The row a download writes for it, under [`CSV_HEADER`]: its times as
    /// RFC 3339, and its agents and tags each one cell, `; ` between them.
    fn cells(&self) -> [String; 14] {
        let time = |t: &DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Micros, true);
        [
            self.session_id.clone(),
            self.user_id.clone(),
            self.agents.join("; "),
            self.traces.to_string(),
            self.failed_traces.to_string(),
            self.input_tokens.to_string(),
            self.output_tokens.to_string(),
            self.spend.to_string(),
            self.duration_ms.to_string(),
            time(&self.first_seen),
            time(&self.last_seen),
            self.last_ended.to_string(),
            self.last_status.to_string(),
            self.tags.join("; "),
        ]
    }
}

/// `GET /internal/v1/telemetry/sessions` — a page of the project's sessions
/// in the window, filtered and sorted as the query says, how many the filter
/// admits, and the page's sessions' own scores.
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<SessionList>, TelmoniError> {
    let project_id = reading(&state, &headers).await?;
    let listed = list_page::<Sessions>(&state, &project_id, pairs).await?;
    let sessions: Vec<String> = listed.rows.iter().map(|s| s.session_id.clone()).collect();
    let scores = crate::scores::of_sessions(&state.store, &Projects::one(project_id), &sessions)
        .await
        .map_err(read_failed)?;
    Ok(Json(SessionList {
        more: listed.more(),
        sessions: listed.rows,
        days: listed.days,
        limit: listed.paging.limit,
        page: listed.paging.page,
        total: listed.total,
        sort: listed.sort,
        dir: listed.dir,
        scores,
    }))
}

/// `GET /internal/v1/telemetry/sessions/facets` — each filter's values among
/// the project's traces in the window, and how many sessions hold a trace
/// carrying each under every other filter.
pub async fn get_facets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Facets>, TelmoniError> {
    Ok(Json(
        facets_page::<Sessions>(&state, &headers, pairs).await?,
    ))
}

/// `GET /internal/v1/telemetry/sessions/export` — the project's sessions the
/// filter admits, sorted as asked, as a file: up to
/// [`EXPORT_MAX`](crate::traces::EXPORT_MAX) of them.
pub async fn export_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, TelmoniError> {
    let (rows, format, _) = export_rows::<Sessions>(&state, &headers, pairs).await?;
    download(format, &rows, &CSV_HEADER, SessionRow::cells, "session")
}

#[cfg(test)]
mod tests {
    use super::*;
    use telmoni_shared::SpanStatus;
    use telmoni_shared::csv;

    /// A download's row is the list's fields under the header's names.
    #[test]
    fn a_downloads_row_is_the_lists_fields_under_the_headers_names() {
        let session = SessionRow {
            session_id: "ab".repeat(16),
            user_id: String::new(),
            agents: vec!["support-bot".into(), "triage".into()],
            traces: 4,
            failed_traces: 1,
            input_tokens: 10,
            output_tokens: 5,
            spend: 0.25,
            duration_ms: 1_500,
            first_seen: DateTime::from_timestamp_millis(1_791_721_800_000).expect("a time"),
            last_seen: DateTime::from_timestamp_millis(1_791_721_801_500).expect("a time"),
            last_ended: true,
            last_status: SpanStatus::Error,
            tags: vec!["repo:telmoni".into()],
        };
        let mut out = Vec::new();
        csv::row(&mut out, &session.cells());
        let line = String::from_utf8(out).expect("UTF-8");
        assert_eq!(
            line,
            "abababababababababababababababab,,support-bot; triage,4,1,10,5,0.25,1500,\
             2026-10-11T12:30:00.000000Z,2026-10-11T12:30:01.500000Z,true,error,repo:telmoni\r\n"
        );
        let json = serde_json::to_value(&session).expect("JSON");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        let mut header = CSV_HEADER.to_vec();
        header.sort_unstable();
        keys.sort_unstable();
        assert_eq!(keys, header, "the header names the JSON's every field");
    }
}
