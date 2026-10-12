//! The sessions page's read, through the one query module
//! ([`crate::store`]): every session the project's runs belonged to in a
//! window of days, by the id the application gave it (`session_id`, sent as
//! `gen_ai.conversation.id`), with the end user it was with and what its runs
//! came to. One aggregation over the spans within retention, bounded by the
//! window and never the whole table; a run whose spans carry no session id
//! belongs to none and is left out. A session's own page is its traces in the
//! order they ran, which is the trace list read for it
//! ([`crate::traces::list`]).

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::{SessionSort, SortDirection};

use crate::store::{Projects, Store};
use crate::traces::Window;

/// One session as the page reads it: its runs, summed.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct SessionRow {
    /// The session, by the id the application set.
    pub session_id: String,
    /// The end user it was with, as stored; empty when none was sent.
    pub user_id: String,
    /// How many runs it holds in the window.
    pub traces: u64,
    /// How many of those runs had a span that failed.
    pub errors: u64,
    /// How many steps its runs took, summed.
    pub steps: u64,
    /// Its model calls' input tokens, summed.
    pub input_tokens: u64,
    /// Its model calls' output tokens, summed.
    pub output_tokens: u64,
    /// Input and output, summed.
    pub tokens: u64,
    /// When its first run in the window started.
    #[serde(with = "clickhouse::serde::chrono::datetime64::micros")]
    pub first_seen: DateTime<Utc>,
    /// When its last run in the window ended: what the page sorts by first.
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
            "SELECT session_id, run_user_id AS user_id, traces, errors, steps, ",
            "input_total AS input_tokens, output_total AS output_tokens, tokens, ",
            "first_seen, last_seen ",
            "FROM (SELECT session_id, ",
            "anyIf(user_id, user_id != '') AS run_user_id, ",
            "uniqExact(run_id) AS traces, ",
            "uniqExactIf(run_id, status = 'error') AS errors, ",
            "countIf(kind = 'step') AS steps, ",
            "sumIf(input_tokens, kind = 'model_call') AS input_total, ",
            "sumIf(output_tokens, kind = 'model_call') AS output_total, ",
            "input_total + output_total AS tokens, ",
            "min(started_at) AS first_seen, max(ended_at) AS last_seen ",
            "FROM telemetry.spans ",
            "WHERE project_id IN {projects:Array(String)} ",
            "AND received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
            "AND session_id != '' ",
            "GROUP BY session_id) ",
            "ORDER BY ",
            $order,
            ", last_seen DESC, session_id ",
            "LIMIT {limit:UInt32}"
        )
    };
}

/// The page's SQL for one sort: a literal per column and direction.
const fn list_sql(sort: SessionSort, direction: SortDirection) -> &'static str {
    use SortDirection::{Asc, Desc};
    match (sort, direction) {
        (SessionSort::Session, Asc) => list_sql!("session_id ASC"),
        (SessionSort::Session, Desc) => list_sql!("session_id DESC"),
        (SessionSort::User, Asc) => list_sql!("user_id ASC"),
        (SessionSort::User, Desc) => list_sql!("user_id DESC"),
        (SessionSort::Traces, Asc) => list_sql!("traces ASC"),
        (SessionSort::Traces, Desc) => list_sql!("traces DESC"),
        (SessionSort::Errors, Asc) => list_sql!("errors ASC"),
        (SessionSort::Errors, Desc) => list_sql!("errors DESC"),
        (SessionSort::Steps, Asc) => list_sql!("steps ASC"),
        (SessionSort::Steps, Desc) => list_sql!("steps DESC"),
        (SessionSort::Tokens, Asc) => list_sql!("tokens ASC"),
        (SessionSort::Tokens, Desc) => list_sql!("tokens DESC"),
        (SessionSort::FirstSeen, Asc) => list_sql!("first_seen ASC"),
        (SessionSort::FirstSeen, Desc) => list_sql!("first_seen DESC"),
        (SessionSort::LastSeen, Asc) => list_sql!("last_seen ASC"),
        (SessionSort::LastSeen, Desc) => list_sql!("last_seen DESC"),
    }
}

/// The sessions of `projects` in `window`, sorted as asked: up to
/// `window.limit` of them.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn list(
    store: &Store,
    projects: &Projects,
    window: Window,
    sort: SessionSort,
    direction: SortDirection,
) -> clickhouse::error::Result<Vec<SessionRow>> {
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
        for sort in SessionSort::all() {
            for direction in SortDirection::all() {
                let sql = list_sql(sort, direction);
                assert!(sql.contains(PROJECTS_PARAM), "{sort} {direction}");
                assert!(!sql.to_ascii_lowercase().contains(" settings "));
                let column = match sort {
                    SessionSort::Session => "session_id",
                    SessionSort::User => "user_id",
                    SessionSort::Traces => "traces",
                    SessionSort::Errors => "errors",
                    SessionSort::Steps => "steps",
                    SessionSort::Tokens => "tokens",
                    SessionSort::FirstSeen => "first_seen",
                    SessionSort::LastSeen => "last_seen",
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
                    sql.contains("session_id != ''"),
                    "a run with no session id belongs to none"
                );
            }
        }
    }
}
