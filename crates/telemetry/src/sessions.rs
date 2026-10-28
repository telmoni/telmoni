//! The sessions page's reads, through the one query module
//! ([`crate::store`]): the sessions the project's runs belonged to in a
//! window of days, by the id the application gave it (`session_id`, sent as
//! `gen_ai.conversation.id`), each summed over all of its runs — admitted by
//! any run the trace list's filters admit — with its page, total, facets and
//! download. A run with no session id belongs to none. A session's own page
//! is the trace list's read for it (`crate::traces::list`).

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::{SessionSort, SortDirection};

use crate::traces::{Listing, facets_sql, run_rows};

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
    /// When its first run in the window started.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub first_seen: DateTime<Utc>,
    /// When its last run in the window ended: what the page sorts by first.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub last_seen: DateTime<Utc>,
}

/// The sessions that hold a run the filter admits, a row each, summed over
/// every run of theirs in the window. The sums take names no column has, for
/// the reason the trace list's do.
macro_rules! sessions {
    () => {
        concat!(
            "SELECT run_session_id AS session_id, ",
            "anyIf(run_user_id, run_user_id != '') AS session_user, ",
            "count() AS session_traces, countIf(errors > 0) AS session_errors, ",
            "sum(steps) AS session_steps, sum(input_total) AS session_input, ",
            "sum(output_total) AS session_output, ",
            "session_input + session_output AS session_tokens, ",
            "min(first_started_at) AS session_first, max(last_ended_at) AS session_last ",
            "FROM (",
            run_rows!(),
            ") WHERE run_session_id != '' ",
            "GROUP BY run_session_id ",
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
            "SELECT session_id, session_user AS user_id, session_traces AS traces, ",
            "session_errors AS errors, session_steps AS steps, ",
            "session_input AS input_tokens, session_output AS output_tokens, ",
            "session_first AS first_seen, ",
            "session_last AS last_seen ",
            "FROM (",
            sessions!(),
            ") ORDER BY ",
            $order,
            ", last_seen DESC, session_id ",
            "LIMIT {limit:UInt32} OFFSET {offset:UInt64}"
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
        (SessionSort::InputTokens, Asc) => list_sql!("input_tokens ASC"),
        (SessionSort::InputTokens, Desc) => list_sql!("input_tokens DESC"),
        (SessionSort::OutputTokens, Asc) => list_sql!("output_tokens ASC"),
        (SessionSort::OutputTokens, Desc) => list_sql!("output_tokens DESC"),
        (SessionSort::Tokens, Asc) => list_sql!("session_tokens ASC"),
        (SessionSort::Tokens, Desc) => list_sql!("session_tokens DESC"),
        (SessionSort::FirstSeen, Asc) => list_sql!("first_seen ASC"),
        (SessionSort::FirstSeen, Desc) => list_sql!("first_seen DESC"),
        (SessionSort::LastSeen, Asc) => list_sql!("last_seen ASC"),
        (SessionSort::LastSeen, Desc) => list_sql!("last_seen DESC"),
    }
}

/// How many sessions the filter admits: the page's total, a second read
/// beside it, since a page past the last holds no row to carry it.
const COUNT_SQL: &str = concat!("SELECT count() AS total FROM (", sessions!(), ")");

/// The page's facets, each counting the sessions that hold a run carrying a
/// value.
const FACETS_SQL: &str = facets_sql!("uniqExact(run_session_id)", "AND run_session_id != '' ");

/// The sessions page: a session a row, summed over its runs; the one last
/// heard from first.
pub(crate) struct Sessions;

impl Listing for Sessions {
    type Row = SessionRow;
    type Sort = SessionSort;
    const SORTS: &'static [SessionSort] = &SessionSort::all();
    const FALLBACK: SessionSort = SessionSort::LastSeen;
    const COUNT_SQL: &'static str = COUNT_SQL;
    const FACETS_SQL: &'static str = FACETS_SQL;

    fn list_sql(sort: SessionSort, direction: SortDirection) -> &'static str {
        list_sql(sort, direction)
    }

    /// The most and the latest first; ids in the alphabet's order, and the
    /// earliest seen first.
    fn first_direction(sort: SessionSort) -> SortDirection {
        match sort {
            SessionSort::Session | SessionSort::User | SessionSort::FirstSeen => SortDirection::Asc,
            SessionSort::Traces
            | SessionSort::Errors
            | SessionSort::Steps
            | SessionSort::InputTokens
            | SessionSort::OutputTokens
            | SessionSort::Tokens
            | SessionSort::LastSeen => SortDirection::Desc,
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
                    SessionSort::InputTokens => "input_tokens",
                    SessionSort::OutputTokens => "output_tokens",
                    SessionSort::Tokens => "session_tokens",
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
                    sql.contains("run_session_id != ''"),
                    "a run with no session id belongs to none"
                );
            }
        }
    }

    /// The total and the facets read the sessions the page lists, by the
    /// same filters, a session counted once whatever its runs carry.
    #[test]
    fn the_total_and_the_facets_count_sessions() {
        for sql in [COUNT_SQL, FACETS_SQL] {
            assert!(sql.contains(PROJECTS_PARAM));
            assert!(sql.contains("{tags:Array(String)}"), "{sql}");
            assert!(sql.contains("run_session_id != ''"), "{sql}");
        }
        assert!(COUNT_SQL.contains("HAVING max(admitted) = 1"));
        assert!(FACETS_SQL.contains("uniqExact(run_session_id) AS listed"));
    }
}
