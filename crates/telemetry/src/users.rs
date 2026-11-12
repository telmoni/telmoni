//! The users list's reads, through the one query module ([`crate::store`]):
//! the end users a project's traces were for in a window of days, by the id
//! the application gave each (`user_id`), as stored. It is the sessions
//! list's fold by another key ([`crate::sessions`]): a user is admitted by
//! any one trace the trace list's filters admit, and summed over all of
//! their traces in the window; a trace that named no user belongs to none.
//! A score admits a user where it is on a trace of theirs or was given to a
//! session of theirs; a user has none of their own. A user's sessions and
//! traces are those lists' own reads, narrowed to them.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::{SortDirection, SpanStatus, UserSort};

use crate::sessions::{session_scored, summed_by, summed_page, traces_flagged};
use crate::store::text;
// The trace's sums by their own names, which `traces_flagged!` expands to.
use crate::traces::{
    Listing, admits_all, facets_sql, scored, scores_asked, spans_in_window, trace_carries,
    trace_status, trace_sums,
};

/// One end user as the list reads them: their traces in the window, summed.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct UserRow {
    /// The end user, as stored.
    pub user_id: String,
    /// The sessions their traces named in the window, each once.
    pub sessions: u64,
    /// The agents their traces ran, each once, sorted.
    pub agents: Vec<String>,
    /// Their traces that started in the window, those still running among them.
    pub traces: u64,
    /// Those that ended in error.
    pub failed_traces: u64,
    /// Their traces' model calls' input tokens, summed.
    pub input_tokens: u64,
    /// And their output tokens.
    pub output_tokens: u64,
    /// What their traces' model calls cost in US dollars, each as the trace
    /// list prices it.
    pub spend: f64,
    /// How long their traces ran, summed, in milliseconds: never the time
    /// between them.
    pub duration_ms: i64,
    /// When their first trace in the window started.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub first_seen: DateTime<Utc>,
    /// When their last trace in the window ended: what the list sorts by first.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub last_seen: DateTime<Utc>,
    /// Whether the agent's own span of the trace that started last has arrived.
    pub last_ended: bool,
    /// How that trace ended.
    #[serde(deserialize_with = "text::deserialize")]
    pub last_status: SpanStatus,
    /// Their traces' tags as `key:value`, each once, sorted.
    pub tags: Vec<String>,
}

/// The end users that hold a trace the filter admits, a row each, with how
/// many sessions their traces named.
macro_rules! users {
    () => {
        summed_by!(
            "trace_user_id",
            "user",
            "uniqExactIf(trace_session_id, trace_session_id != '') AS user_sessions, "
        )
    };
}

/// The list's SQL for one `ORDER BY`.
macro_rules! list_sql {
    ($order:expr) => {
        summed_page!("user", "user_sessions AS sessions, ", users!(), $order)
    };
}

/// The list's SQL for one sort: a literal per column and direction.
const fn list_sql(sort: UserSort, direction: SortDirection) -> &'static str {
    use SortDirection::{Asc, Desc};
    match (sort, direction) {
        (UserSort::LastSeen, Asc) => list_sql!("last_seen ASC"),
        (UserSort::LastSeen, Desc) => list_sql!("last_seen DESC"),
        (UserSort::FirstSeen, Asc) => list_sql!("first_seen ASC"),
        (UserSort::FirstSeen, Desc) => list_sql!("first_seen DESC"),
        (UserSort::Sessions, Asc) => list_sql!("sessions ASC"),
        (UserSort::Sessions, Desc) => list_sql!("sessions DESC"),
        (UserSort::Traces, Asc) => list_sql!("traces ASC"),
        (UserSort::Traces, Desc) => list_sql!("traces DESC"),
        (UserSort::FailedTraces, Asc) => list_sql!("failed_traces ASC"),
        (UserSort::FailedTraces, Desc) => list_sql!("failed_traces DESC"),
        (UserSort::Tokens, Asc) => list_sql!("user_tokens ASC"),
        (UserSort::Tokens, Desc) => list_sql!("user_tokens DESC"),
        (UserSort::Spend, Asc) => list_sql!("spend ASC"),
        (UserSort::Spend, Desc) => list_sql!("spend DESC"),
        (UserSort::Duration, Asc) => list_sql!("duration_ms ASC"),
        (UserSort::Duration, Desc) => list_sql!("duration_ms DESC"),
    }
}

/// How many users the filter admits: the list's total, a second read beside
/// its page, since a page past the last holds no row to carry it.
const COUNT_SQL: &str = concat!("SELECT count() AS total FROM (", users!(), ")");

/// The list's facets, each counting the users that hold a trace carrying a
/// value, the score asked for read off each trace summed, as the list reads it.
const FACETS_SQL: &str = facets_sql!(
    "uniqExact(trace_user_id)",
    spans_in_window!(),
    concat!("AND trace_user_id != '' AND ", session_scored!(), " ")
);

/// The users list: an end user a row, the one last heard from first.
pub(crate) struct Users;

impl Listing for Users {
    type Row = UserRow;
    type Sort = UserSort;
    const SORTS: &'static [UserSort] = &UserSort::all();
    const FALLBACK: UserSort = UserSort::LastSeen;
    const COUNT_SQL: &'static str = COUNT_SQL;
    const FACETS_SQL: &'static str = FACETS_SQL;

    fn list_sql(sort: UserSort, direction: SortDirection) -> &'static str {
        list_sql(sort, direction)
    }

    /// The latest and the most first; the earliest started first.
    fn first_direction(sort: UserSort) -> SortDirection {
        match sort {
            UserSort::FirstSeen => SortDirection::Asc,
            UserSort::LastSeen
            | UserSort::Sessions
            | UserSort::Traces
            | UserSort::FailedTraces
            | UserSort::Tokens
            | UserSort::Spend
            | UserSort::Duration => SortDirection::Desc,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PROJECTS_PARAM;

    /// Every sort is one literal that names its projects, orders by its own
    /// column first, and binds the window, the filters and the page.
    #[test]
    fn every_sort_is_a_literal_naming_its_projects_and_its_column() {
        for sort in UserSort::all() {
            for direction in SortDirection::all() {
                let sql = list_sql(sort, direction);
                assert!(sql.contains(PROJECTS_PARAM), "{sort} {direction}");
                assert!(!sql.to_ascii_lowercase().contains(" settings "));
                let column = match sort {
                    UserSort::LastSeen => "last_seen",
                    UserSort::FirstSeen => "first_seen",
                    UserSort::Sessions => "sessions",
                    UserSort::Traces => "traces",
                    UserSort::FailedTraces => "failed_traces",
                    UserSort::Tokens => "user_tokens",
                    UserSort::Spend => "spend",
                    UserSort::Duration => "duration_ms",
                };
                let order = format!("ORDER BY {column} {}", direction.to_string().to_uppercase());
                assert!(sql.contains(&order), "{sort} {direction}: {sql}");
                assert!(sql.ends_with(
                    ", last_seen DESC, user_id LIMIT {limit:UInt32} OFFSET {offset:UInt64}"
                ));
                for param in [
                    "{days:UInt16}",
                    "{users:Array(String)}",
                    "{tags:Array(String)}",
                ] {
                    assert!(sql.contains(param), "{sort} {direction} lacks {param}");
                }
            }
        }
    }

    /// ⚠ A user is admitted by any one trace and summed over all of them, a
    /// trace that named none in no row, and their sessions counted once each.
    #[test]
    fn a_user_is_admitted_by_one_trace_and_summed_over_all() {
        let sql = list_sql(UserSort::LastSeen, SortDirection::Desc);
        for sql in [sql, COUNT_SQL] {
            assert!(
                sql.contains(
                    "WHERE trace_user_id != '' GROUP BY trace_user_id HAVING max(admitted) = 1"
                ),
                "{sql}"
            );
            assert!(
                !sql.contains("GROUP BY trace_id HAVING"),
                "a trace the filters refuse is still summed: {sql}"
            );
            assert!(
                sql.contains(
                    "uniqExactIf(trace_session_id, trace_session_id != '') AS user_sessions"
                ),
                "{sql}"
            );
        }
        assert!(
            sql.starts_with("SELECT user_id, user_sessions AS sessions, user_agents AS agents, ")
        );
    }

    /// The facets count users, under the trace list's filters and none for
    /// the traces that named no user.
    #[test]
    fn the_facets_count_users() {
        assert!(FACETS_SQL.contains(PROJECTS_PARAM));
        assert!(FACETS_SQL.contains("uniqExact(trace_user_id) AS listed"));
        assert!(
            FACETS_SQL
                .contains("AND trace_user_id != '' AND (({score:String} = '' OR trace_id IN (")
        );
    }

    /// The first trace sorts oldest first, every other column the most or
    /// the latest, and no tag sorts the list.
    #[test]
    fn each_sort_runs_its_own_way_first_and_no_tag_sorts_the_list() {
        for sort in UserSort::all() {
            let first = if sort == UserSort::FirstSeen {
                SortDirection::Asc
            } else {
                SortDirection::Desc
            };
            assert_eq!(Users::first_direction(sort), first, "{sort}");
        }
        assert!(Users::tag_sql(SortDirection::Asc).is_none());
    }
}
