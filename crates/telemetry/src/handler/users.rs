//! The console's user lanes, under `Resource::Trace`, which every role on a
//! project holds: `GET /internal/v1/telemetry/users`, a page of the end users
//! as stored, as the trace list's filters narrow it, with its total; and
//! `/users/facets` and `/users/export`. Each reads the spans of the project
//! `x-project-id` names and no other.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use telmoni_shared::extract::{Json, Query};
use telmoni_shared::{SortDirection, TelmoniError, UserSort};

use crate::AppState;
use crate::handler::traces::{Facets, Reading, download, export_rows, facets_page, list_page};
use crate::users::{UserRow, Users};

/// What the page's lane answers.
#[derive(Debug, Serialize)]
pub struct UserList {
    /// The page's users, in the order asked.
    pub users: Vec<UserRow>,
    /// How many days back the page read.
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
    pub sort: UserSort,
    /// And its direction.
    pub dir: SortDirection,
}

/// A download's header row: the page's fields, in its order.
const CSV_HEADER: [&str; 9] = [
    "user_id",
    "traces",
    "errors",
    "steps",
    "input_tokens",
    "output_tokens",
    "duration_ms",
    "first_seen",
    "last_seen",
];

impl UserRow {
    /// The row a download writes for them, under [`CSV_HEADER`], the times as
    /// RFC 3339.
    fn cells(&self) -> [String; 9] {
        let time = |t: &DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Micros, true);
        [
            self.user_id.clone(),
            self.traces.to_string(),
            self.errors.to_string(),
            self.steps.to_string(),
            self.input_tokens.to_string(),
            self.output_tokens.to_string(),
            self.duration_ms.to_string(),
            time(&self.first_seen),
            time(&self.last_seen),
        ]
    }
}

/// `GET /internal/v1/telemetry/users` — a page of the project's users in the
/// window, filtered and sorted as the query says, and how many the filter
/// admits: two reads at once, as the trace list's are.
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<UserList>, TelmoniError> {
    let listed = list_page::<Users>(&state, &headers, pairs).await?;
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
/// project's runs in the window, and how many users have a run carrying each
/// under every other filter.
pub async fn get_user_facets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<Facets>, TelmoniError> {
    Ok(Json(facets_page::<Users>(&state, &headers, pairs).await?))
}

/// `GET /internal/v1/telemetry/users/export` — the project's users the filter
/// admits, sorted as asked, as a file: up to [`EXPORT_MAX`](crate::traces::EXPORT_MAX)
/// of them.
pub async fn export_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Response, TelmoniError> {
    let (rows, format) = export_rows::<Users>(&state, &headers, pairs, Reading::Export).await?;
    download(format, &rows, &CSV_HEADER, UserRow::cells, "user")
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
    fn an_empty_query_is_the_most_spent_on_first_over_a_week() {
        let query = TraceQuery::read(Vec::new(), Reading::List).expect("read");
        assert_eq!(query.filter().expect("the default").days, DEFAULT_DAYS);
        assert_eq!(query.paging().expect("the default").limit, DEFAULT_LIMIT);
        assert_eq!(
            query.sort_for::<Users>().expect("the default"),
            (UserSort::Tokens, SortDirection::Desc)
        );
    }

    #[test]
    fn each_column_has_its_own_first_direction() {
        for column in UserSort::all() {
            let query = TraceQuery::read(pairs(&[("sort", &column.to_string())]), Reading::List)
                .expect("read");
            assert_eq!(
                query.sort_for::<Users>().expect("a column"),
                (column, Users::first_direction(column))
            );
        }
        assert_eq!(Users::first_direction(UserSort::User), SortDirection::Asc);
        assert_eq!(
            Users::first_direction(UserSort::FirstSeen),
            SortDirection::Asc
        );
        assert_eq!(
            Users::first_direction(UserSort::LastSeen),
            SortDirection::Desc
        );
    }

    /// A value a parameter does not take is refused by the parameter's name,
    /// and never echoed.
    #[test]
    fn a_bad_value_names_the_parameter_and_echoes_nothing() {
        for (given, parameter) in [
            (("sort", "spend<script>"), "sort"),
            (("sort", "started"), "sort"),
            (("dir", "sideways"), "dir"),
            (("days", "0"), "days"),
            (("limit", "501"), "limit"),
            (("page", "1001"), "page"),
        ] {
            let query = TraceQuery::read(pairs(&[given]), Reading::List).expect("read");
            let refused = query
                .filter()
                .and_then(|_| query.sort_for::<Users>())
                .and_then(|_| {
                    query
                        .paging()
                        .map(|_| (UserSort::Tokens, SortDirection::Desc))
                })
                .expect_err("refused");
            let problem = refused.to_problem_details();
            assert_eq!(problem.status, 400);
            let detail = problem.detail.expect("a detail");
            assert!(detail.starts_with(parameter), "{detail}");
            assert!(!detail.contains("script") && !detail.contains("sideways"));
        }
    }

    /// A download's row is the page's fields under the header's names.
    #[test]
    fn a_downloads_row_is_the_pages_fields_under_the_headers_names() {
        let at = DateTime::from_timestamp_millis(1_791_721_800_000).expect("a time");
        let user = UserRow {
            user_id: "user-a".into(),
            traces: 2,
            errors: 1,
            steps: 2,
            input_tokens: 300,
            output_tokens: 30,
            duration_ms: 1_250,
            first_seen: at,
            last_seen: at,
        };
        let cells = user.cells();
        assert_eq!(cells.len(), CSV_HEADER.len());
        assert_eq!(cells[0], "user-a");
        assert_eq!(cells[4], "300");
        assert_eq!(cells[6], "1250");
        assert_eq!(cells[7], "2026-10-11T12:30:00.000000Z");
    }
}
