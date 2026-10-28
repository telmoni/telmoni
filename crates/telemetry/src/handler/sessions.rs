//! The console's session lanes, under `Resource::Trace`, which every role on
//! a project holds: `GET /internal/v1/telemetry/sessions`, a page of the
//! list as the trace list's filters narrow it, with its total;
//! `/session-facets` and `/session-export`, beside it rather than under it,
//! where a session's id would be read as one; and `/sessions/{session_id}`,
//! one session's traces in the order they ran, each with the spans a layout
//! pinned by name (`?pinned=`): a replay in metadata, never a message. Each
//! reads the spans of the project `x-project-id` names and no other.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::extract::{Json, Query};
use telmoni_shared::{
    AuthError, ProjectId, SessionSort, SortDirection, TelmoniError, TraceSort, ViewKind,
};

use crate::contract::{NAME_MAX, SESSION_ID_MAX};
use crate::handler::traces::{
    Facets, Reading, download, export_rows, facets_page, list_page, parse_days, parse_limit,
    read_failed, reading, values,
};
use crate::handler::views::LayoutOptions;
use crate::sessions::{SessionRow, Sessions};
use crate::store::Projects;
use crate::traces::{self, PINNED_MAX, PinnedRow, TraceFilter, TraceRow, Traces, Window};
use crate::{AppState, db};

/// A session's own page's query string, every value text until it is
/// checked: the window its traces are read in, and which spans each trace
/// answers under it — the span names a layout pinned, `pinned` given again
/// for each, or `view`, the layout to read them from.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionQuery {
    /// How many days back.
    pub days: Option<String>,
    /// The most traces to answer.
    pub limit: Option<String>,
    /// The span names whose spans each trace answers beside its sums.
    pub pinned: Vec<String>,
    /// The layout whose pinned names to read with: a saved layout's id, or
    /// `project` for the project's own.
    pub view: Option<String>,
}

/// Which spans a session's traces answer under them: the names given, or
/// the names a layout pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pinning {
    Named(Vec<String>),
    Layout(Layout),
}

/// The layout a session is read with: the saved one a link names, where it
/// is one of the project's, else the project's own; or the project's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Saved(Uuid),
    Project,
}

impl SessionQuery {
    /// The query string's pairs, read.
    ///
    /// # Errors
    ///
    /// A parameter the lane does not take, never echoed, or one given twice
    /// that is not `pinned`.
    pub fn read(pairs: Vec<(String, String)>) -> Result<Self, TelmoniError> {
        let mut query = Self::default();
        for (name, value) in pairs {
            let once = match name.as_str() {
                "pinned" => {
                    query.pinned.push(value);
                    continue;
                }
                "days" => &mut query.days,
                "limit" => &mut query.limit,
                "view" => &mut query.view,
                _ => {
                    return Err(TelmoniError::BadRequest(
                        "the lane takes no parameter by that name".into(),
                    ));
                }
            };
            if once.replace(value).is_some() {
                return Err(TelmoniError::BadRequest(format!(
                    "{name} is given more than once"
                )));
            }
        }
        Ok(query)
    }

    /// The window and the pinning, each value checked: a name trimmed and
    /// once, and at most [`PINNED_MAX`] of them; a layout named by its id,
    /// or as the project's own, and never beside names.
    ///
    /// # Errors
    ///
    /// A value that is not what its parameter takes, named by the parameter
    /// and never echoed.
    pub fn into_parts(self) -> Result<(Window, Pinning), TelmoniError> {
        let pinning = match self.view {
            Some(view) => {
                if !self.pinned.is_empty() {
                    return Err(TelmoniError::BadRequest(
                        "a session is read with a layout or with pinned names, not both".into(),
                    ));
                }
                Pinning::Layout(match view.as_str() {
                    "project" => Layout::Project,
                    id => Layout::Saved(id.parse().map_err(|_| {
                        TelmoniError::BadRequest(
                            "view names a saved layout by its id, or the project's own as project"
                                .into(),
                        )
                    })?),
                })
            }
            None => {
                let pinned = values("pinned", &self.pinned, NAME_MAX)?;
                if pinned.len() > PINNED_MAX {
                    return Err(TelmoniError::BadRequest(format!(
                        "pinned names at most {PINNED_MAX} spans"
                    )));
                }
                Pinning::Named(pinned)
            }
        };
        Ok((
            Window {
                days: parse_days(self.days.as_deref())?,
                limit: parse_limit(self.limit.as_deref())?,
            },
            pinning,
        ))
    }
}

/// What the page's lane answers.
#[derive(Debug, Serialize)]
pub struct SessionList {
    /// The page's sessions, in the order asked.
    pub sessions: Vec<SessionRow>,
    /// How many days back the page read.
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
    pub sort: SessionSort,
    /// And its direction.
    pub dir: SortDirection,
}

/// A download's header row: the page's fields, in its order.
const CSV_HEADER: [&str; 9] = [
    "session_id",
    "user_id",
    "traces",
    "errors",
    "steps",
    "input_tokens",
    "output_tokens",
    "first_seen",
    "last_seen",
];

impl SessionRow {
    /// The row a download writes for it, under [`CSV_HEADER`], its times as
    /// RFC 3339.
    fn cells(&self) -> [String; 9] {
        let time = |t: &DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Micros, true);
        [
            self.session_id.clone(),
            self.user_id.clone(),
            self.traces.to_string(),
            self.errors.to_string(),
            self.steps.to_string(),
            self.input_tokens.to_string(),
            self.output_tokens.to_string(),
            time(&self.first_seen),
            time(&self.last_seen),
        ]
    }
}

/// What one session's lane answers: its traces in the order they ran.
#[derive(Debug, Serialize)]
pub struct SessionDetail {
    pub session_id: String,
    /// The session's runs, earliest first, each summed as the trace list sums
    /// it, with the spans pinned under it.
    pub traces: Vec<SessionTrace>,
    /// The span names the traces' pinned spans were read by.
    pub pinned: Vec<String>,
    /// How many days back the lane read.
    pub days: u16,
    /// The most traces it would answer.
    pub limit: u32,
    /// Whether the window held more of the session's traces than the limit.
    pub more: bool,
}

/// One trace of a session's page: the list's sums, and the spans a layout
/// pinned by name, in the order they started.
#[derive(Debug, Serialize)]
pub struct SessionTrace {
    #[serde(flatten)]
    pub trace: TraceRow,
    pub pinned: Vec<PinnedRow>,
}

/// `GET /internal/v1/telemetry/sessions` — a page of the project's sessions
/// in the window, filtered and sorted as the query says, and how many the
/// filter admits: two reads at once, as the trace list's are.
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<SessionList>, TelmoniError> {
    let listed = list_page::<Sessions>(&state, &headers, pairs).await?;
    Ok(Json(SessionList {
        more: listed.more(),
        sessions: listed.rows,
        days: listed.days,
        limit: listed.paging.limit,
        page: listed.paging.page,
        total: listed.total,
        sort: listed.sort,
        dir: listed.dir,
    }))
}

/// `GET /internal/v1/telemetry/session-facets` — each filter's values among
/// the project's runs in the window, and how many sessions hold a run
/// carrying each under every other filter.
pub async fn get_session_facets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Facets>, TelmoniError> {
    Ok(Json(
        facets_page::<Sessions>(&state, &headers, pairs).await?,
    ))
}

/// `GET /internal/v1/telemetry/session-export` — the project's sessions the
/// filter admits, sorted as asked, as a file: up to
/// [`EXPORT_MAX`](crate::traces::EXPORT_MAX) of them.
pub async fn export_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, TelmoniError> {
    let (rows, format) = export_rows::<Sessions>(&state, &headers, pairs, Reading::Export).await?;
    download(format, &rows, &CSV_HEADER, SessionRow::cells, "session")
}

/// The names a layout pins: the saved layout a link names, where it is one
/// of the project's; else the project's own; else none. A layout saved by
/// an older console that no longer reads pins none, as its page draws it.
async fn layout_pins(
    state: &AppState,
    project_id: &ProjectId,
    layout: Layout,
) -> Result<Vec<String>, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let layouts = db::views(&mut tx, project_id, ViewKind::Layout).await?;
    let own = db::session_layout(&mut tx, project_id).await?;
    tx.commit().await?;
    let saved = |id: Uuid| layouts.iter().find(|row| row.id == id);
    let chosen = match layout {
        Layout::Saved(id) => saved(id).or_else(|| own.and_then(saved)),
        Layout::Project => own.and_then(saved),
    };
    Ok(chosen
        .and_then(|row| serde_json::from_value::<LayoutOptions>(row.query.clone()).ok())
        .map(|options| options.pinned)
        .unwrap_or_default())
}

/// `GET /internal/v1/telemetry/sessions/{session_id}` — one session's traces in
/// the window, earliest first, each with the spans `pinned` names under it,
/// or those the layout `view` names pins; `404` for a session the project's
/// runs in the window did not belong to, which is also what another
/// project's session id gets.
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<SessionDetail>, TelmoniError> {
    let project_id = reading(&state, &headers).await?;
    let projects = Projects::one(project_id.clone());
    let session_id = session_id.trim().to_owned();
    if session_id.is_empty() || session_id.chars().count() > SESSION_ID_MAX {
        return Err(TelmoniError::BadRequest(format!(
            "a session is named by its id, up to {SESSION_ID_MAX} characters"
        )));
    }
    let (window, pinning) = SessionQuery::read(pairs)?.into_parts()?;
    let pinned = match pinning {
        Pinning::Named(names) => names,
        Pinning::Layout(layout) => layout_pins(&state, &project_id, layout).await?,
    };
    let filter = TraceFilter {
        session_ids: vec![session_id.clone()],
        days: window.days,
        ..TraceFilter::default()
    };
    // One past the limit, so the answer can say whether there were more.
    let mut rows = traces::list::<Traces>(
        &state.store,
        &projects,
        &filter,
        (TraceSort::Started, SortDirection::Asc),
        window.limit + 1,
        0,
    )
    .await
    .map_err(read_failed)?;
    if rows.is_empty() {
        return Err(AuthError::NotFound(
            "no session with that id among this project's runs in the window".into(),
        )
        .into());
    }
    let more = rows.len() > window.limit as usize;
    rows.truncate(window.limit as usize);
    let runs: Vec<Uuid> = rows.iter().map(|row| row.run_id).collect();
    let mut spans: HashMap<Uuid, Vec<PinnedRow>> = HashMap::new();
    for row in traces::pinned(&state.store, &projects, &runs, &pinned, window.days)
        .await
        .map_err(read_failed)?
    {
        spans.entry(row.run_id).or_default().push(row);
    }
    Ok(Json(SessionDetail {
        session_id,
        traces: rows
            .into_iter()
            .map(|row| SessionTrace {
                pinned: spans.remove(&row.run_id).unwrap_or_default(),
                trace: row,
            })
            .collect(),
        pinned,
        days: window.days,
        limit: window.limit,
        more,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::traces::TraceQuery;
    use crate::traces::Listing;
    use crate::traces::{DEFAULT_DAYS, DEFAULT_LIMIT};

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn an_empty_query_is_the_latest_first_over_a_week() {
        let query = TraceQuery::read(Vec::new(), Reading::List).expect("read");
        assert_eq!(query.filter().expect("the default").days, DEFAULT_DAYS);
        assert_eq!(query.paging().expect("the default").limit, DEFAULT_LIMIT);
        assert_eq!(
            query.sort_for::<Sessions>().expect("the default"),
            (SessionSort::LastSeen, SortDirection::Desc)
        );
    }

    #[test]
    fn each_column_has_its_own_first_direction() {
        for column in SessionSort::all() {
            let query = TraceQuery::read(pairs(&[("sort", &column.to_string())]), Reading::List)
                .expect("read");
            assert_eq!(
                query.sort_for::<Sessions>().expect("a column"),
                (column, Sessions::first_direction(column))
            );
        }
        assert_eq!(
            Sessions::first_direction(SessionSort::Session),
            SortDirection::Asc
        );
        assert_eq!(
            Sessions::first_direction(SessionSort::LastSeen),
            SortDirection::Desc
        );
    }

    /// A sort the page does not take, a trace list's among them, is refused
    /// naming the parameter, never the value.
    #[test]
    fn a_sort_the_page_does_not_take_is_refused_without_an_echo() {
        for given in ["started", "spend<script>"] {
            let query = TraceQuery::read(pairs(&[("sort", given)]), Reading::List).expect("read");
            let detail = query
                .sort_for::<Sessions>()
                .expect_err(given)
                .to_problem_details()
                .detail
                .unwrap_or_default();
            assert!(detail.starts_with("sort is one of session"), "{detail}");
            assert!(!detail.contains("script"), "{detail}");
        }
    }

    /// A download's row is the page's fields under the header's names.
    #[test]
    fn a_downloads_row_is_the_pages_fields_under_the_headers_names() {
        let at = DateTime::from_timestamp_millis(1_791_721_800_000).expect("a time");
        let session = SessionRow {
            session_id: "thread, 42".into(),
            user_id: "user-a".into(),
            traces: 3,
            errors: 1,
            steps: 7,
            input_tokens: 300,
            output_tokens: 30,
            first_seen: at,
            last_seen: at,
        };
        let cells = session.cells();
        assert_eq!(cells.len(), CSV_HEADER.len());
        assert_eq!(cells[0], "thread, 42");
        assert_eq!(cells[5], "300");
        assert_eq!(cells[7], "2026-10-11T12:30:00.000000Z");
    }

    /// A session's page reads its window and the names pinned, each trimmed
    /// and once, or the layout to pin by, and refuses a parameter it does
    /// not take, one given twice, a layout beside names, or more names than
    /// a trace shows — naming the parameter, never the value.
    #[test]
    fn a_sessions_page_reads_its_window_and_the_names_pinned() {
        let (window, pinning) = SessionQuery::read(pairs(&[
            ("days", "30"),
            ("pinned", " plan "),
            ("pinned", "plan"),
            ("pinned", ""),
            ("pinned", "search_docs"),
        ]))
        .expect("read")
        .into_parts()
        .expect("checked");
        assert_eq!(window.days, 30);
        assert_eq!(window.limit, DEFAULT_LIMIT);
        assert_eq!(
            pinning,
            Pinning::Named(vec!["plan".into(), "search_docs".into()])
        );
        let (window, pinning) = SessionQuery::read(Vec::new())
            .expect("read")
            .into_parts()
            .expect("the default");
        assert_eq!(window, Window::default());
        assert_eq!(pinning, Pinning::Named(Vec::new()));
        let (_, pinning) = SessionQuery::read(pairs(&[("view", "project")]))
            .expect("read")
            .into_parts()
            .expect("the project's own");
        assert_eq!(pinning, Pinning::Layout(Layout::Project));
        let id = Uuid::new_v4();
        let (_, pinning) = SessionQuery::read(pairs(&[("view", &id.to_string())]))
            .expect("read")
            .into_parts()
            .expect("a saved layout");
        assert_eq!(pinning, Pinning::Layout(Layout::Saved(id)));

        let names: Vec<String> = (0..=PINNED_MAX)
            .map(|i| format!("step-{i}<script>"))
            .collect();
        let nine: Vec<(&str, &str)> = names.iter().map(|name| ("pinned", name.as_str())).collect();
        let long = "p".repeat(NAME_MAX + 1);
        for query in [
            vec![("sort<script>", "started")],
            vec![("days", "7"), ("days", "30")],
            vec![("days", "0<script>")],
            vec![("pinned", long.as_str())],
            vec![("view", "nope<script>")],
            vec![("view", "project"), ("pinned", "plan")],
            nine,
        ] {
            let refused = SessionQuery::read(pairs(&query))
                .and_then(SessionQuery::into_parts)
                .expect_err("refused")
                .to_problem_details();
            assert_eq!(refused.status, 400, "{query:?}");
            assert!(!refused.detail.unwrap_or_default().contains("script"));
        }
    }
}
