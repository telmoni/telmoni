//! `GET /internal/v1/telemetry/users` — the users page, under `Resource::Trace`,
//! which every role on a project holds: every end user the project's
//! runs were for in a window of days, as stored, with what those runs came
//! to, sorted as the URL says. Reads the spans of the project `x-project-id`
//! names and no other: the store names that project on every read, twice.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use telmoni_shared::extract::{Json, Query};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::{SortDirection, TelmoniError, UserSort};

use crate::AppState;
use crate::handler::traces::{parse_days, parse_limit, read_failed};
use crate::store::Projects;
use crate::traces::Window;
use crate::users::{self, UserRow};

/// The page's query string, every value text until it is checked, so an
/// empty value asks for the default and a value that is not what its
/// parameter takes is a 400 that echoes none of it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsersQuery {
    /// The column to sort by ([`UserSort`]).
    #[serde(default)]
    pub sort: String,
    /// Which way ([`SortDirection`]); the column's own default when unset.
    #[serde(default)]
    pub dir: String,
    /// How many days back.
    #[serde(default)]
    pub days: String,
    /// The most users to answer.
    #[serde(default)]
    pub limit: String,
}

impl UsersQuery {
    /// The window and the sort the query asks for, each value checked.
    ///
    /// # Errors
    ///
    /// A value that is not what its parameter takes, named by the parameter
    /// and never echoed.
    pub fn into_parts(self) -> Result<(Window, UserSort, SortDirection), TelmoniError> {
        let sort = match self.sort.trim() {
            "" => UserSort::Tokens,
            given => given.parse::<UserSort>().map_err(|_| {
                TelmoniError::BadRequest(
                    "sort is one of user, traces, errors, steps, tokens, first_seen, last_seen"
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
pub const fn default_direction(sort: UserSort) -> SortDirection {
    match sort {
        UserSort::User | UserSort::FirstSeen => SortDirection::Asc,
        UserSort::Traces
        | UserSort::Errors
        | UserSort::Steps
        | UserSort::Tokens
        | UserSort::LastSeen => SortDirection::Desc,
    }
}

/// What the page's lane answers.
#[derive(Debug, Serialize)]
pub struct UserList {
    /// The users, in the order asked.
    pub users: Vec<User>,
    /// How many days back the page read.
    pub days: u16,
    /// The most it would answer.
    pub limit: u32,
    /// Whether the window held more users than the limit.
    pub more: bool,
    /// The sort it is in.
    pub sort: UserSort,
    /// And its direction.
    pub dir: SortDirection,
}

/// One user of the page: a user id's runs, summed ([`UserRow`]).
#[derive(Debug, Serialize)]
pub struct User {
    pub user_id: String,
    pub traces: u64,
    pub errors: u64,
    pub steps: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

impl From<UserRow> for User {
    fn from(row: UserRow) -> Self {
        Self {
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

/// `GET /internal/v1/telemetry/users` — the project's users in the window,
/// sorted as the query says, up to its limit, and whether the window held
/// more.
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UsersQuery>,
) -> Result<Json<UserList>, TelmoniError> {
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
    let rows = users::list(&state.store, &Projects::one(project_id), read, sort, dir)
        .await
        .map_err(read_failed)?;
    let more = rows.len() > window.limit as usize;
    Ok(Json(UserList {
        users: rows
            .into_iter()
            .take(window.limit as usize)
            .map(User::from)
            .collect(),
        days: window.days,
        limit: window.limit,
        more,
        sort,
        dir,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traces::{DEFAULT_DAYS, DEFAULT_LIMIT};

    #[test]
    fn an_empty_query_is_the_most_spent_on_first_over_a_week() {
        let (window, sort, dir) = UsersQuery::default().into_parts().expect("the default");
        assert_eq!(window.days, DEFAULT_DAYS);
        assert_eq!(window.limit, DEFAULT_LIMIT);
        assert_eq!(sort, UserSort::Tokens);
        assert_eq!(dir, SortDirection::Desc);
    }

    #[test]
    fn each_column_has_its_own_first_direction() {
        for sort in UserSort::all() {
            let (_, parsed, dir) = UsersQuery {
                sort: sort.to_string(),
                ..UsersQuery::default()
            }
            .into_parts()
            .expect("a column");
            assert_eq!(parsed, sort);
            assert_eq!(dir, default_direction(sort));
        }
        assert_eq!(default_direction(UserSort::User), SortDirection::Asc);
        assert_eq!(default_direction(UserSort::FirstSeen), SortDirection::Asc);
        assert_eq!(default_direction(UserSort::LastSeen), SortDirection::Desc);
    }

    /// A value a parameter does not take is refused by the parameter's name,
    /// and never echoed.
    #[test]
    fn a_bad_value_names_the_parameter_and_echoes_nothing() {
        for (query, parameter) in [
            (
                UsersQuery {
                    sort: "spend<script>".into(),
                    ..UsersQuery::default()
                },
                "sort",
            ),
            (
                UsersQuery {
                    dir: "sideways".into(),
                    ..UsersQuery::default()
                },
                "dir",
            ),
            (
                UsersQuery {
                    days: "0".into(),
                    ..UsersQuery::default()
                },
                "days",
            ),
            (
                UsersQuery {
                    limit: "501".into(),
                    ..UsersQuery::default()
                },
                "limit",
            ),
        ] {
            let problem = query
                .into_parts()
                .expect_err("refused")
                .to_problem_details();
            assert_eq!(problem.status, 400);
            let detail = problem.detail.expect("a detail");
            assert!(detail.starts_with(parameter), "{detail}");
            assert!(!detail.contains("script") && !detail.contains("sideways"));
        }
    }
}
