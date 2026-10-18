//! The console's session lanes, under `Resource::Trace`, which every role on
//! a project holds: `GET /internal/telemetry/sessions`, the page — every
//! session the project's runs belonged to in a window of days, with the end
//! user it was with and what its runs came to, sorted as the URL says — and
//! `GET /internal/telemetry/sessions/{session_id}`, one session's traces
//! in the order they ran, each with the spans a layout pinned by name
//! (`?pinned=`): a replay in metadata, never a message, since content is
//! not metadata. Each reads the spans of the project `x-project-id` names
//! and no other.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{
    AuthError, SessionSort, SortDirection, SpanKind, SpanStatus, TelmoniError, TraceSort,
};

use crate::AppState;
use crate::contract::{NAME_MAX, SESSION_ID_MAX};
use crate::handler::traces::{Trace, parse_days, parse_limit, read_failed, values};
use crate::sessions::{self, SessionRow};
use crate::store::Projects;
use crate::traces::{self, PINNED_MAX, PinnedRow, TraceFilter, Window};

/// The page's query string, every value text until it is checked, so an
/// empty value asks for the default and a value that is not what its
/// parameter takes is a 400 that echoes none of it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionsQuery {
    /// The column to sort by ([`SessionSort`]).
    #[serde(default)]
    pub sort: String,
    /// Which way ([`SortDirection`]); the column's own default when unset.
    #[serde(default)]
    pub dir: String,
    /// How many days back.
    #[serde(default)]
    pub days: String,
    /// The most sessions to answer.
    #[serde(default)]
    pub limit: String,
}

impl SessionsQuery {
    /// The window and the sort the query asks for, each value checked.
    ///
    /// # Errors
    ///
    /// A value that is not what its parameter takes, named by the parameter
    /// and never echoed.
    pub fn into_parts(self) -> Result<(Window, SessionSort, SortDirection), TelmoniError> {
        let sort = match self.sort.trim() {
            "" => SessionSort::LastSeen,
            given => given.parse::<SessionSort>().map_err(|_| {
                TelmoniError::BadRequest(
                    "sort is one of session, user, traces, errors, steps, tokens, first_seen, \
                     last_seen"
                        .into(),
                )
            })?,
        };
        let direction = match self.dir.trim() {
            "" => default_direction(sort),
            given => given
                .parse::<SortDirection>()
                .map_err(|_| TelmoniError::BadRequest("dir is asc or desc".into()))?,
        };
        Ok((
            Window {
                days: parse_days(&self.days)?,
                limit: parse_limit(&self.limit)?,
            },
            sort,
            direction,
        ))
    }
}

/// The way a column sorts when the URL names none: the most and the latest
/// first; ids in the alphabet's order, and the earliest seen first.
#[must_use]
pub const fn default_direction(sort: SessionSort) -> SortDirection {
    match sort {
        SessionSort::Session | SessionSort::User | SessionSort::FirstSeen => SortDirection::Asc,
        SessionSort::Traces
        | SessionSort::Errors
        | SessionSort::Steps
        | SessionSort::Tokens
        | SessionSort::LastSeen => SortDirection::Desc,
    }
}

/// A session's own page's query string, every value text until it is
/// checked: the window its traces are read in, and the span names a layout
/// pinned, `pinned` given again for each.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionQuery {
    /// How many days back.
    pub days: Option<String>,
    /// The most traces to answer.
    pub limit: Option<String>,
    /// The span names whose spans each trace answers beside its sums.
    pub pinned: Vec<String>,
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

    /// The window and the pinned names, each value checked: a name trimmed
    /// and once, and at most [`PINNED_MAX`] of them.
    ///
    /// # Errors
    ///
    /// A value that is not what its parameter takes, named by the parameter
    /// and never echoed.
    pub fn into_parts(self) -> Result<(Window, Vec<String>), TelmoniError> {
        let pinned = values("pinned", &self.pinned, NAME_MAX)?;
        if pinned.len() > PINNED_MAX {
            return Err(TelmoniError::BadRequest(format!(
                "pinned names at most {PINNED_MAX} spans"
            )));
        }
        Ok((
            Window {
                days: parse_days(self.days.as_deref().unwrap_or_default())?,
                limit: parse_limit(self.limit.as_deref().unwrap_or_default())?,
            },
            pinned,
        ))
    }
}

/// What the page's lane answers.
#[derive(Debug, Serialize)]
pub struct SessionList {
    /// The sessions, in the order asked.
    pub sessions: Vec<Session>,
    /// How many days back the page read.
    pub days: u16,
    /// The most it would answer.
    pub limit: u32,
    /// Whether the window held more sessions than the limit.
    pub more: bool,
    /// The sort it is in.
    pub sort: SessionSort,
    /// And its direction.
    pub dir: SortDirection,
}

/// One session of the page: its runs, summed ([`SessionRow`]).
#[derive(Debug, Serialize)]
pub struct Session {
    pub session_id: String,
    pub user_id: String,
    pub traces: u64,
    pub errors: u64,
    pub steps: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

impl From<SessionRow> for Session {
    fn from(row: SessionRow) -> Self {
        Self {
            session_id: row.session_id,
            user_id: row.user_id,
            traces: row.traces,
            errors: row.errors,
            steps: row.steps,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            first_seen: row.first_seen,
            last_seen: row.last_seen,
        }
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
    pub trace: Trace,
    pub pinned: Vec<PinnedSpan>,
}

/// One pinned span, as the page shows it under a trace that is shut.
#[derive(Debug, Serialize)]
pub struct PinnedSpan {
    pub span_id: String,
    pub parent_span_id: String,
    pub kind: SpanKind,
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub status: SpanStatus,
    pub error_type: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl From<PinnedRow> for PinnedSpan {
    fn from(row: PinnedRow) -> Self {
        Self {
            span_id: row.span_id,
            parent_span_id: row.parent_span_id,
            kind: row.kind,
            name: row.name,
            started_at: row.started_at,
            ended_at: row.ended_at,
            status: row.status,
            error_type: row.error_type,
            model: row.model,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
        }
    }
}

/// `GET /internal/telemetry/sessions` — the project's sessions in the window,
/// sorted as the query says, up to its limit, and whether the window held
/// more.
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SessionsQuery>,
) -> Result<Json<SessionList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Trace)?
        .project_id
        .clone();
    let (window, sort, dir) = query.into_parts()?;
    // One past the limit, so the answer can say whether there were more.
    let read = Window {
        limit: window.limit + 1,
        ..window
    };
    let rows = sessions::list(&state.store, &Projects::one(project_id), read, sort, dir)
        .await
        .map_err(read_failed)?;
    let more = rows.len() > window.limit as usize;
    Ok(Json(SessionList {
        sessions: rows
            .into_iter()
            .take(window.limit as usize)
            .map(Session::from)
            .collect(),
        days: window.days,
        limit: window.limit,
        more,
        sort,
        dir,
    }))
}

/// `GET /internal/telemetry/sessions/{session_id}` — one session's traces in
/// the window, earliest first, each with the spans `pinned` names under it;
/// `404` for a session the project's runs in the window did not belong to,
/// which is also what another project's session id gets.
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<SessionDetail>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Read, Resource::Trace)?
        .project_id
        .clone();
    let session_id = session_id.trim().to_owned();
    if session_id.is_empty() || session_id.chars().count() > SESSION_ID_MAX {
        return Err(TelmoniError::BadRequest(format!(
            "a session is named by its id, up to {SESSION_ID_MAX} characters"
        )));
    }
    let (window, pinned) = SessionQuery::read(pairs)?.into_parts()?;
    let projects = Projects::one(project_id);
    let filter = TraceFilter {
        session_ids: vec![session_id.clone()],
        days: window.days,
        ..TraceFilter::default()
    };
    // One past the limit, so the answer can say whether there were more.
    let mut rows = traces::list(
        &state.store,
        &projects,
        &filter,
        TraceSort::Started,
        SortDirection::Asc,
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
    let mut spans: HashMap<Uuid, Vec<PinnedSpan>> = HashMap::new();
    for row in traces::pinned(&state.store, &projects, &runs, &pinned, window.days)
        .await
        .map_err(read_failed)?
    {
        spans
            .entry(row.run_id)
            .or_default()
            .push(PinnedSpan::from(row));
    }
    Ok(Json(SessionDetail {
        session_id,
        traces: rows
            .into_iter()
            .map(|row| SessionTrace {
                pinned: spans.remove(&row.run_id).unwrap_or_default(),
                trace: Trace::from(row),
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
    use crate::traces::{DEFAULT_DAYS, DEFAULT_LIMIT};

    #[test]
    fn an_empty_query_is_the_latest_first_over_a_week() {
        let (window, sort, dir) = SessionsQuery::default().into_parts().expect("the default");
        assert_eq!(window.days, DEFAULT_DAYS);
        assert_eq!(window.limit, DEFAULT_LIMIT);
        assert_eq!(sort, SessionSort::LastSeen);
        assert_eq!(dir, SortDirection::Desc);
    }

    #[test]
    fn each_column_has_its_own_first_direction() {
        for sort in SessionSort::all() {
            let (_, parsed, dir) = SessionsQuery {
                sort: sort.to_string(),
                ..SessionsQuery::default()
            }
            .into_parts()
            .expect("a column");
            assert_eq!(parsed, sort);
            assert_eq!(dir, default_direction(sort));
        }
        assert_eq!(default_direction(SessionSort::Session), SortDirection::Asc);
        assert_eq!(
            default_direction(SessionSort::LastSeen),
            SortDirection::Desc
        );
    }

    fn pairs(query: &[(&str, &str)]) -> Vec<(String, String)> {
        query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// A session's page reads its window and the names pinned, each trimmed
    /// and once, and refuses a parameter it does not take, one given twice,
    /// or more names than a trace shows — naming the parameter, never the
    /// value.
    #[test]
    fn a_sessions_page_reads_its_window_and_the_names_pinned() {
        let (window, pinned) = SessionQuery::read(pairs(&[
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
        assert_eq!(pinned, ["plan", "search_docs"]);
        let (window, pinned) = SessionQuery::read(Vec::new())
            .expect("read")
            .into_parts()
            .expect("the default");
        assert_eq!(window, Window::default());
        assert!(pinned.is_empty());

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

    /// A value a parameter does not take is refused by the parameter's name,
    /// and never echoed.
    #[test]
    fn a_bad_value_names_the_parameter_and_echoes_nothing() {
        let refused = SessionsQuery {
            sort: "spend<script>".into(),
            ..SessionsQuery::default()
        }
        .into_parts()
        .expect_err("refused")
        .to_problem_details();
        assert_eq!(refused.status, 400);
        let detail = refused.detail.expect("a detail");
        assert!(detail.starts_with("sort"), "{detail}");
        assert!(!detail.contains("script"));
    }
}
