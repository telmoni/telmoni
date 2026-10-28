//! The users page's reads, through the one query module ([`crate::store`]):
//! the end users the project's runs were for in a window of days, as stored
//! — hashed under the project's key unless it keeps ids plain — each summed
//! over all of their runs, admitted by any run the trace list's filters
//! admit, with its page, total, facets and download. The spans within the
//! window are read, since the rollups name no user; a run with no user is
//! nobody's.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::{SortDirection, UserSort};

use crate::traces::{Listing, facets_sql, run_rows};

/// One user as the page reads it: a user id's runs, summed.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct UserRow {
    /// The end user, as stored.
    pub user_id: String,
    /// How many runs were theirs in the window.
    pub traces: u64,
    /// How many of those runs had a span that failed.
    pub errors: u64,
    /// How many steps their runs took, summed.
    pub steps: u64,
    /// Their model calls' input tokens, summed.
    pub input_tokens: u64,
    /// Their model calls' output tokens, summed.
    pub output_tokens: u64,
    /// When their first run in the window started.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub first_seen: DateTime<Utc>,
    /// When their last run in the window ended.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub last_seen: DateTime<Utc>,
}

/// The users one of whose runs the filter admits, a row each, summed over
/// every run of theirs in the window. The sums take names no column has, for
/// the reason the trace list's do.
macro_rules! users {
    () => {
        concat!(
            "SELECT run_user_id AS user_id, ",
            "count() AS user_traces, countIf(errors > 0) AS user_errors, ",
            "sum(steps) AS user_steps, sum(input_total) AS user_input, ",
            "sum(output_total) AS user_output, user_input + user_output AS user_tokens, ",
            "min(first_started_at) AS user_first, max(last_ended_at) AS user_last ",
            "FROM (",
            run_rows!(),
            ") WHERE run_user_id != '' ",
            "GROUP BY run_user_id ",
            "HAVING max(admitted) = 1"
        )
    };
}

/// The page's SQL with one `ORDER BY`, so every (column, direction) is a
/// literal: the query module takes nothing else. The outer SELECT gives the
/// sums the row's names.
macro_rules! list_sql {
    ($order:literal) => {
        concat!(
            "SELECT user_id, user_traces AS traces, user_errors AS errors, ",
            "user_steps AS steps, user_input AS input_tokens, user_output AS output_tokens, ",
            "user_first AS first_seen, user_last AS last_seen ",
            "FROM (",
            users!(),
            ") ORDER BY ",
            $order,
            ", last_seen DESC, user_id ",
            "LIMIT {limit:UInt32} OFFSET {offset:UInt64}"
        )
    };
}

/// The page's SQL for one sort: a literal per column and direction.
const fn list_sql(sort: UserSort, direction: SortDirection) -> &'static str {
    use SortDirection::{Asc, Desc};
    match (sort, direction) {
        (UserSort::User, Asc) => list_sql!("user_id ASC"),
        (UserSort::User, Desc) => list_sql!("user_id DESC"),
        (UserSort::Traces, Asc) => list_sql!("traces ASC"),
        (UserSort::Traces, Desc) => list_sql!("traces DESC"),
        (UserSort::Errors, Asc) => list_sql!("errors ASC"),
        (UserSort::Errors, Desc) => list_sql!("errors DESC"),
        (UserSort::Steps, Asc) => list_sql!("steps ASC"),
        (UserSort::Steps, Desc) => list_sql!("steps DESC"),
        (UserSort::InputTokens, Asc) => list_sql!("input_tokens ASC"),
        (UserSort::InputTokens, Desc) => list_sql!("input_tokens DESC"),
        (UserSort::OutputTokens, Asc) => list_sql!("output_tokens ASC"),
        (UserSort::OutputTokens, Desc) => list_sql!("output_tokens DESC"),
        (UserSort::Tokens, Asc) => list_sql!("user_tokens ASC"),
        (UserSort::Tokens, Desc) => list_sql!("user_tokens DESC"),
        (UserSort::FirstSeen, Asc) => list_sql!("first_seen ASC"),
        (UserSort::FirstSeen, Desc) => list_sql!("first_seen DESC"),
        (UserSort::LastSeen, Asc) => list_sql!("last_seen ASC"),
        (UserSort::LastSeen, Desc) => list_sql!("last_seen DESC"),
    }
}

/// How many users the filter admits: the page's total, a second read beside
/// it, since a page past the last holds no row to carry it.
const COUNT_SQL: &str = concat!("SELECT count() AS total FROM (", users!(), ")");

/// The page's facets, each counting the users one of whose runs carries a
/// value.
const FACETS_SQL: &str = facets_sql!("uniqExact(run_user_id)", "AND run_user_id != '' ");

/// The users page: a user a row, summed over their runs; the most tokens
/// first.
pub(crate) struct Users;

impl Listing for Users {
    type Row = UserRow;
    type Sort = UserSort;
    const SORTS: &'static [UserSort] = &UserSort::all();
    const FALLBACK: UserSort = UserSort::Tokens;
    const COUNT_SQL: &'static str = COUNT_SQL;
    const FACETS_SQL: &'static str = FACETS_SQL;

    fn list_sql(sort: UserSort, direction: SortDirection) -> &'static str {
        list_sql(sort, direction)
    }

    /// The most and the latest first; ids in the alphabet's order, and the
    /// earliest seen first.
    fn first_direction(sort: UserSort) -> SortDirection {
        match sort {
            UserSort::User | UserSort::FirstSeen => SortDirection::Asc,
            UserSort::Traces
            | UserSort::Errors
            | UserSort::Steps
            | UserSort::InputTokens
            | UserSort::OutputTokens
            | UserSort::Tokens
            | UserSort::LastSeen => SortDirection::Desc,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PROJECTS_PARAM;

    /// Every sort is one literal that names its projects, carries no
    /// settings, orders by its own column first, and binds the window, the
    /// filters and the page.
    #[test]
    fn every_sort_is_a_literal_naming_its_projects_and_its_column() {
        for sort in UserSort::all() {
            for direction in SortDirection::all() {
                let sql = list_sql(sort, direction);
                assert!(sql.contains(PROJECTS_PARAM), "{sort} {direction}");
                assert!(!sql.to_ascii_lowercase().contains(" settings "));
                let column = match sort {
                    UserSort::User => "user_id",
                    UserSort::Traces => "traces",
                    UserSort::Errors => "errors",
                    UserSort::Steps => "steps",
                    UserSort::InputTokens => "input_tokens",
                    UserSort::OutputTokens => "output_tokens",
                    UserSort::Tokens => "user_tokens",
                    UserSort::FirstSeen => "first_seen",
                    UserSort::LastSeen => "last_seen",
                };
                let order = format!(
                    "ORDER BY {column} {}",
                    match direction {
                        SortDirection::Asc => "ASC",
                        SortDirection::Desc => "DESC",
                    }
                );
                assert!(sql.contains(&order), "{sort} {direction}: {sql}");
                for param in [
                    "{days:UInt16}",
                    "{agents:Array(String)}",
                    "{prompts:Array(String)}",
                    "{limit:UInt32}",
                    "{offset:UInt64}",
                ] {
                    assert!(sql.contains(param), "{sort} {direction} lacks {param}");
                }
                assert!(
                    sql.contains("run_user_id != ''"),
                    "a run with no user is nobody's"
                );
            }
        }
    }

    /// The total and the facets read the users the page lists, by the same
    /// filters, a user counted once whatever their runs carry.
    #[test]
    fn the_total_and_the_facets_count_users() {
        for sql in [COUNT_SQL, FACETS_SQL] {
            assert!(sql.contains(PROJECTS_PARAM));
            assert!(sql.contains("{tags:Array(String)}"), "{sql}");
            assert!(sql.contains("run_user_id != ''"), "{sql}");
        }
        assert!(COUNT_SQL.contains("HAVING max(admitted) = 1"));
        assert!(FACETS_SQL.contains("uniqExact(run_user_id) AS listed"));
    }
}
