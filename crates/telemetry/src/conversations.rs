//! The conversations page's read, through the one query module
//! ([`crate::store`]): every conversation the project's runs belonged to in a
//! window of days, by the id the application gave it
//! (`gen_ai.conversation.id`), with the end user it was with and what its
//! runs came to. One aggregation over the spans within retention, bounded by
//! the window and never the whole table; a run whose spans carry no
//! conversation belongs to none and is left out. A conversation's own page is
//! its traces in the order they ran, which is the trace list read for it
//! ([`crate::traces::list`]).

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::{ConversationSort, SortDirection};

use crate::store::{Projects, Store};
use crate::traces::Window;

/// One conversation as the page reads it: its runs, summed.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct ConversationRow {
    /// The conversation, as the application set it.
    pub conversation_id: String,
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
            "SELECT conversation_id, run_user_id AS user_id, traces, errors, steps, ",
            "input_total AS input_tokens, output_total AS output_tokens, tokens, ",
            "first_seen, last_seen ",
            "FROM (SELECT conversation_id, ",
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
            "AND conversation_id != '' ",
            "GROUP BY conversation_id) ",
            "ORDER BY ",
            $order,
            ", last_seen DESC, conversation_id ",
            "LIMIT {limit:UInt32}"
        )
    };
}

/// The page's SQL for one sort: a literal per column and direction.
const fn list_sql(sort: ConversationSort, direction: SortDirection) -> &'static str {
    use SortDirection::{Asc, Desc};
    match (sort, direction) {
        (ConversationSort::Conversation, Asc) => list_sql!("conversation_id ASC"),
        (ConversationSort::Conversation, Desc) => list_sql!("conversation_id DESC"),
        (ConversationSort::User, Asc) => list_sql!("user_id ASC"),
        (ConversationSort::User, Desc) => list_sql!("user_id DESC"),
        (ConversationSort::Traces, Asc) => list_sql!("traces ASC"),
        (ConversationSort::Traces, Desc) => list_sql!("traces DESC"),
        (ConversationSort::Errors, Asc) => list_sql!("errors ASC"),
        (ConversationSort::Errors, Desc) => list_sql!("errors DESC"),
        (ConversationSort::Steps, Asc) => list_sql!("steps ASC"),
        (ConversationSort::Steps, Desc) => list_sql!("steps DESC"),
        (ConversationSort::Tokens, Asc) => list_sql!("tokens ASC"),
        (ConversationSort::Tokens, Desc) => list_sql!("tokens DESC"),
        (ConversationSort::FirstSeen, Asc) => list_sql!("first_seen ASC"),
        (ConversationSort::FirstSeen, Desc) => list_sql!("first_seen DESC"),
        (ConversationSort::LastSeen, Asc) => list_sql!("last_seen ASC"),
        (ConversationSort::LastSeen, Desc) => list_sql!("last_seen DESC"),
    }
}

/// The conversations of `projects` in `window`, sorted as asked: up to
/// `window.limit` of them.
///
/// # Errors
///
/// ClickHouse did not answer, or refused the read.
pub async fn list(
    store: &Store,
    projects: &Projects,
    window: Window,
    sort: ConversationSort,
    direction: SortDirection,
) -> clickhouse::error::Result<Vec<ConversationRow>> {
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
        for sort in ConversationSort::all() {
            for direction in SortDirection::all() {
                let sql = list_sql(sort, direction);
                assert!(sql.contains(PROJECTS_PARAM), "{sort} {direction}");
                assert!(!sql.to_ascii_lowercase().contains(" settings "));
                let column = match sort {
                    ConversationSort::Conversation => "conversation_id",
                    ConversationSort::User => "user_id",
                    ConversationSort::Traces => "traces",
                    ConversationSort::Errors => "errors",
                    ConversationSort::Steps => "steps",
                    ConversationSort::Tokens => "tokens",
                    ConversationSort::FirstSeen => "first_seen",
                    ConversationSort::LastSeen => "last_seen",
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
                    sql.contains("conversation_id != ''"),
                    "a run with no conversation belongs to none"
                );
            }
        }
    }
}
