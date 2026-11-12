//! The console's end-user lanes, each over the spans of the project
//! `x-project-id` names and no other. Under `Resource::Trace`, which every
//! role on a project holds: `GET /internal/v1/telemetry/users`, a page of the
//! users list as the trace list's filters narrow it, with its total;
//! `/users/facets`, each filter's values and how many users hold a trace
//! carrying each; and `/users/export`, the rows as a file. A project
//! admin's: `POST /users/erase`, which erases one.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped};
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuditAction, ProjectId, Redacted, SortDirection, TelmoniError, TelmoniResourceKind, UserSort,
};

use crate::AppState;
use crate::contract::USER_ID_MAX;
use crate::erasure::{self, Erase};
use crate::handler::chain;
use crate::handler::traces::{Facets, download, export_rows, facets_page, list_page, reading};
use crate::hashing;
use crate::traces::ListSort;
use crate::users::{UserRow, Users};

/// What the list answers.
#[derive(Debug, Serialize)]
pub struct UserList {
    /// The page's end users, in the order asked.
    pub users: Vec<UserRow>,
    /// How many days back the list read.
    pub days: u16,
    /// The most a page holds.
    pub limit: u32,
    /// Which page this is, from 1.
    pub page: u32,
    /// How many users the filter admits in the window, on every page.
    pub total: u64,
    /// Whether a page after this one holds more.
    pub more: bool,
    /// The sort it is in.
    pub sort: ListSort<UserSort>,
    /// And its direction.
    pub dir: SortDirection,
}

/// A download's header row: the list's fields, in its order.
const CSV_HEADER: [&str; 14] = [
    "user_id",
    "sessions",
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

impl UserRow {
    /// The row a download writes for them, under [`CSV_HEADER`]: their times
    /// as RFC 3339, and their agents and tags each one cell, `; ` between them.
    fn cells(&self) -> [String; 14] {
        let time = |t: &DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Micros, true);
        [
            self.user_id.clone(),
            self.sessions.to_string(),
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

/// `GET /internal/v1/telemetry/users` — a page of the project's end users in
/// the window, filtered and sorted as the query says, and how many the
/// filter admits.
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<UserList>, TelmoniError> {
    let project_id = reading(&state, &headers).await?;
    let listed = list_page::<Users>(&state, &project_id, pairs).await?;
    Ok(Json(UserList {
        more: listed.more(),
        users: listed.rows,
        days: listed.days,
        limit: listed.paging.limit,
        page: listed.paging.page,
        total: listed.total,
        sort: listed.sort,
        dir: listed.dir,
    }))
}

/// `GET /internal/v1/telemetry/users/facets` — each filter's values among the
/// project's traces in the window, and how many users hold a trace carrying
/// each under every other filter.
pub async fn get_facets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Facets>, TelmoniError> {
    Ok(Json(facets_page::<Users>(&state, &headers, pairs).await?))
}

/// `GET /internal/v1/telemetry/users/export` — the project's end users the
/// filter admits, sorted as asked, as a file: up to
/// [`EXPORT_MAX`](crate::traces::EXPORT_MAX) of them.
pub async fn export_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, TelmoniError> {
    let (rows, format, _) = export_rows::<Users>(&state, &headers, pairs).await?;
    download(format, &rows, &CSV_HEADER, UserRow::cells, "user")
}

/// An end user to erase, by the id the project's SDK sent: never logged,
/// never kept; hashed as ingest hashed it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Erasure {
    /// The id as sent, byte for byte.
    pub user_id: Redacted,
}

/// What an erasure answers: how much of the end user went.
#[derive(Debug, Serialize)]
pub struct Erased {
    /// Their traces, marked deleted.
    pub traces: u64,
    /// Those traces' spans.
    pub spans: u64,
    /// The content fields those spans kept, deleted.
    pub fields: u64,
}

/// `POST /internal/v1/telemetry/users/erase` — one end user erased by a
/// project admin: every trace their id is on, as [`super::traces::delete_trace`]
/// deletes one, the request filed first under the hash, never the id, and
/// the receipt mailed after. An id on no trace is filed and answered too.
pub async fn erase_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(erasure): Json<Erasure>,
) -> Result<Json<Erased>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Delete, Resource::Trace)?
        .project_id;
    let sent = hashing::id_as_sent(erasure.user_id.expose(), "an end user's id", USER_ID_MAX)?;
    let hash = hashing::storage_for_project(&state, project_id)
        .await?
        .store(&sent);
    let found = erasure::find(&state, project_id, Erase::EndUser(&hash)).await?;
    let traces = found.traces.len() as u64;
    chain::write(
        &state.db,
        &acting,
        project_id,
        "end user erasure",
        async |tx| file_erasure(&state, &acting, project_id, tx, &hash, traces).await,
    )
    .await?;
    let spans = found.spans;
    let erase = Erase::EndUser(&hash);
    let fields = erasure::carry_out(&state, project_id, erase, found, &acting.user_id).await?;
    tracing::info!(project_id = %project_id, traces, spans, fields, "telemetry: an end user erased");
    Ok(Json(Erased {
        traces,
        spans,
        fields,
    }))
}

/// The erasure's request, filed under the organization's chain lock by the
/// hash alone, the role checked again on auth's second answer.
async fn file_erasure(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    hash: &str,
    traces: u64,
) -> Result<(), TelmoniError> {
    chain::held(
        state.auth.as_ref(),
        acting,
        tx,
        Verb::Delete,
        Resource::Trace,
    )
    .await?;
    emit_audit(
        tx,
        AuditEvent::new(
            &acting.organization_id,
            Actor::User(acting.user_id.as_str()),
            AuditAction::Deleted,
            TelmoniResourceKind::EndUser,
        )
        .in_project(project_id)
        .on(hash)
        .details(json!({ "traces": traces })),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use telmoni_shared::SpanStatus;
    use telmoni_shared::csv;

    /// A download's row is the list's fields under the header's names.
    #[test]
    fn a_downloads_row_is_the_lists_fields_under_the_headers_names() {
        let user = UserRow {
            user_id: "cd".repeat(16),
            sessions: 2,
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
        csv::row(&mut out, &user.cells());
        let line = String::from_utf8(out).expect("UTF-8");
        assert_eq!(
            line,
            "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd,2,support-bot; triage,4,1,10,5,0.25,1500,\
             2026-10-11T12:30:00.000000Z,2026-10-11T12:30:01.500000Z,true,error,repo:telmoni\r\n"
        );
        let json = serde_json::to_value(&user).expect("JSON");
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
