//! The users page's read, through the one query module ([`crate::store`]):
//! every end user the project's runs were for in a window of days, as
//! stored — hashed under the project's key unless it keeps ids plain — with
//! what those runs came to. One aggregation over the spans within retention,
//! since the rollups name no user, bounded by the window and never the
//! whole table; a run whose spans carry no user is nobody's and is left
//! out.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::{SortDirection, UserSort};

use crate::store::{Projects, Store};
use crate::traces::Window;

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
    /// Input and output, summed: what the page sorts by first.
    pub tokens: u64,
    /// When their first run in the window started.
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub first_seen: DateTime<Utc>,
    /// When their last run in the window ended.
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub last_seen: DateTime<Utc>,
}

/// The page's SQL with one `ORDER BY`, so every (column, direction) is a
/// literal: the query module takes nothing else. Two levels, as the trace
/// list's: ClickHouse resolves a SELECT alias before a column of the same
/// name anywhere in the query, so the sums take names no column has and the
/// outer SELECT gives them the row's.
macro_rules! list_sql {
    ($order:literal) => {
        concat!(
            "SELECT user_id, traces, errors, steps, input_total AS input_tokens, ",
            "output_total AS output_tokens, tokens, first_seen, last_seen ",
            "FROM (SELECT user_id, uniqExact(run_id) AS traces, ",
            "uniqExactIf(run_id, status = 'error') AS errors, ",
            "countIf(kind = 'step') AS steps, ",
            "sum(input_tokens) AS input_total, sum(output_tokens) AS output_total, ",
            "input_total + output_total AS tokens, ",
            "min(started_at) AS first_seen, max(ended_at) AS last_seen ",
            "FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
            "AND user_id != '' ",
            "GROUP BY user_id) ",
            "ORDER BY ",
            $order,
            ", last_seen DESC, user_id ",
            "LIMIT {limit:UInt32}"
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
        (UserSort::Tokens, Asc) => list_sql!("tokens ASC"),
        (UserSort::Tokens, Desc) => list_sql!("tokens DESC"),
        (UserSort::FirstSeen, Asc) => list_sql!("first_seen ASC"),
        (UserSort::FirstSeen, Desc) => list_sql!("first_seen DESC"),
        (UserSort::LastSeen, Asc) => list_sql!("last_seen ASC"),
        (UserSort::LastSeen, Desc) => list_sql!("last_seen DESC"),
    }
}

/// The users of `projects` in `window`, sorted as asked: up to
/// `window.limit` of them.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn list(
    store: &Store,
    projects: &Projects,
    window: Window,
    sort: UserSort,
    direction: SortDirection,
) -> clickhouse::error::Result<Vec<UserRow>> {
    store
        .read(projects, list_sql(sort, direction))
        .param("days", window.days)
        .param("limit", window.limit)
        .fetch_all()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PROJECTS_PARAM;

    /// Every sort is one literal that names its projects, carries no
    /// settings, orders by its own column first, and binds the window.
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
                    UserSort::Tokens => "tokens",
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
                for param in ["{days:UInt16}", "{limit:UInt32}"] {
                    assert!(sql.contains(param), "{sort} {direction} lacks {param}");
                }
                assert!(
                    sql.contains("user_id != ''"),
                    "a run with no user is nobody's"
                );
            }
        }
    }
}
