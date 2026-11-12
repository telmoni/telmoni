//! The sessions list's reads, through the one query module
//! ([`crate::store`]): the sessions a project's traces belonged to in a
//! window of days, by the id the application gave each (`session_id`, sent
//! as `gen_ai.conversation.id`), as stored. A session is admitted by any one
//! trace the trace list's filters admit, and summed over all of its traces
//! in the window; a trace that named no session belongs to none. A score
//! given to the session itself admits it as one on a trace of it does. A
//! session's traces are the trace list's own read, narrowed to it.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};

use telmoni_shared::{SessionSort, SortDirection, SpanStatus};

use crate::store::text;
// The trace's sums by their own names, which `trace_sums!` expands to.
use crate::traces::{
    Listing, admits_all, facets_sql, scored, scores_asked, spans_in_window, trace_carries,
    trace_status, trace_sums,
};

/// One session as the list reads it: its traces in the window, summed.
#[derive(Debug, Clone, PartialEq, Row, Serialize, Deserialize)]
pub struct SessionRow {
    /// The session, as stored.
    pub session_id: String,
    /// The end user it was with, as stored; empty when none was sent.
    pub user_id: String,
    /// The agents its traces ran, each once, sorted.
    pub agents: Vec<String>,
    /// Its traces that started in the window, those still running among them.
    pub traces: u64,
    /// Those that ended in error.
    pub failed_traces: u64,
    /// Its traces' model calls' input tokens, summed.
    pub input_tokens: u64,
    /// And their output tokens.
    pub output_tokens: u64,
    /// What its traces' model calls cost in US dollars, each as the trace
    /// list prices it.
    pub spend: f64,
    /// How long its traces ran, summed, in milliseconds: never the time
    /// between them.
    pub duration_ms: i64,
    /// When its first trace in the window started.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub first_seen: DateTime<Utc>,
    /// When its last trace in the window ended: what the list sorts by first.
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub last_seen: DateTime<Utc>,
    /// Whether the agent's own span of the trace that started last has arrived.
    pub last_ended: bool,
    /// How that trace ended.
    #[serde(deserialize_with = "text::deserialize")]
    pub last_status: SpanStatus,
    /// Its traces' tags as `key:value`, each once, sorted.
    pub tags: Vec<String>,
}

/// Whether the score asked for admits a trace of a session: one on the trace
/// or a span of it, as the trace list reads it, or one given to its session.
macro_rules! session_scored {
    () => {
        concat!(
            "(",
            scored!(),
            " OR trace_session_id IN (SELECT session_id ",
            scores_asked!(),
            " AND target = 'session'))"
        )
    };
}

/// Every trace of the window, a row each, flagged (`admitted`) where the
/// filters admit it, the score asked for among them: what is summed over all
/// of a session's traces and admitted by any one of them.
macro_rules! traces_flagged {
    () => {
        concat!(
            "SELECT ",
            trace_sums!(),
            ", toUInt8(",
            admits_all!(),
            " AND ",
            session_scored!(),
            ") AS admitted ",
            spans_in_window!(),
            "GROUP BY trace_id"
        )
    };
}

/// The window's traces folded by `$key`, a sum each trace carries: a row for
/// each value of it that holds a trace the filter admits, summed over every
/// trace of its in the window, and none for the traces that carry no value.
/// `$own` is what the list sums beside them. The sums take names no column
/// has, `$as` before each, for the reason the trace list's do.
macro_rules! summed_by {
    ($key:literal, $as:literal, $own:literal) => {
        concat!(
            "SELECT ",
            $key,
            " AS ",
            $as,
            "_id, ",
            $own,
            "arraySort(groupUniqArray(trace_agent)) AS ",
            $as,
            "_agents, count() AS ",
            $as,
            "_traces, countIf(trace_status = 'error') AS ",
            $as,
            "_failed, sum(input_total) AS ",
            $as,
            "_input, sum(output_total) AS ",
            $as,
            "_output, ",
            $as,
            "_input + ",
            $as,
            "_output AS ",
            $as,
            "_tokens, sum(spent) AS ",
            $as,
            "_spend, sum(duration_ms) AS ",
            $as,
            "_duration, min(first_started_at) AS ",
            $as,
            "_first, max(last_ended_at) AS ",
            $as,
            "_last, argMax(trace_ended, first_started_at) AS ",
            $as,
            "_last_ended, argMax(trace_status, first_started_at) AS ",
            $as,
            "_last_status, arraySort(groupUniqArrayArray(tag_pairs)) AS ",
            $as,
            "_tag_pairs FROM (",
            traces_flagged!(),
            ") WHERE ",
            $key,
            " != '' GROUP BY ",
            $key,
            " HAVING max(admitted) = 1"
        )
    };
}

/// A page of `$rows`, what [`summed_by!`] folded, with one `ORDER BY`, so
/// every (column, direction) is a literal: the query module takes nothing
/// else. The outer SELECT gives the sums the row's names, `$own` the list's
/// own among them.
macro_rules! summed_page {
    ($as:literal, $own:literal, $rows:expr, $order:expr) => {
        concat!(
            "SELECT ",
            $as,
            "_id, ",
            $own,
            $as,
            "_agents AS agents, ",
            $as,
            "_traces AS traces, ",
            $as,
            "_failed AS failed_traces, ",
            $as,
            "_input AS input_tokens, ",
            $as,
            "_output AS output_tokens, ",
            $as,
            "_spend AS spend, ",
            $as,
            "_duration AS duration_ms, ",
            $as,
            "_first AS first_seen, ",
            $as,
            "_last AS last_seen, ",
            $as,
            "_last_ended AS last_ended, ",
            $as,
            "_last_status AS last_status, ",
            $as,
            "_tag_pairs AS tags FROM (",
            $rows,
            ") ORDER BY ",
            $order,
            ", last_seen DESC, ",
            $as,
            "_id LIMIT {limit:UInt32} OFFSET {offset:UInt64}"
        )
    };
}

pub(crate) use session_scored;
pub(crate) use summed_by;
pub(crate) use summed_page;
pub(crate) use traces_flagged;

/// The sessions that hold a trace the filter admits, a row each, with the
/// end user any trace of theirs named.
macro_rules! sessions {
    () => {
        summed_by!(
            "trace_session_id",
            "session",
            "anyIf(trace_user_id, trace_user_id != '') AS session_user, "
        )
    };
}

/// The list's SQL for one `ORDER BY`.
macro_rules! list_sql {
    ($order:expr) => {
        summed_page!("session", "session_user AS user_id, ", sessions!(), $order)
    };
}

/// The list's SQL for one sort: a literal per column and direction.
const fn list_sql(sort: SessionSort, direction: SortDirection) -> &'static str {
    use SortDirection::{Asc, Desc};
    match (sort, direction) {
        (SessionSort::LastSeen, Asc) => list_sql!("last_seen ASC"),
        (SessionSort::LastSeen, Desc) => list_sql!("last_seen DESC"),
        (SessionSort::FirstSeen, Asc) => list_sql!("first_seen ASC"),
        (SessionSort::FirstSeen, Desc) => list_sql!("first_seen DESC"),
        (SessionSort::Traces, Asc) => list_sql!("traces ASC"),
        (SessionSort::Traces, Desc) => list_sql!("traces DESC"),
        (SessionSort::FailedTraces, Asc) => list_sql!("failed_traces ASC"),
        (SessionSort::FailedTraces, Desc) => list_sql!("failed_traces DESC"),
        (SessionSort::Tokens, Asc) => list_sql!("session_tokens ASC"),
        (SessionSort::Tokens, Desc) => list_sql!("session_tokens DESC"),
        (SessionSort::Spend, Asc) => list_sql!("spend ASC"),
        (SessionSort::Spend, Desc) => list_sql!("spend DESC"),
        (SessionSort::Duration, Asc) => list_sql!("duration_ms ASC"),
        (SessionSort::Duration, Desc) => list_sql!("duration_ms DESC"),
    }
}

/// How many sessions the filter admits: the list's total, a second read
/// beside its page, since a page past the last holds no row to carry it.
const COUNT_SQL: &str = concat!("SELECT count() AS total FROM (", sessions!(), ")");

/// The list's facets, each counting the sessions that hold a trace carrying
/// a value. The score asked for is read off each trace summed, as the list
/// reads it, never off the spans, where it would count scored traces alone.
const FACETS_SQL: &str = facets_sql!(
    "uniqExact(trace_session_id)",
    spans_in_window!(),
    concat!("AND trace_session_id != '' AND ", session_scored!(), " ")
);

/// The sessions list: a session a row, the one last heard from first.
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

    /// The latest and the most first; the earliest started first.
    fn first_direction(sort: SessionSort) -> SortDirection {
        match sort {
            SessionSort::FirstSeen => SortDirection::Asc,
            SessionSort::LastSeen
            | SessionSort::Traces
            | SessionSort::FailedTraces
            | SessionSort::Tokens
            | SessionSort::Spend
            | SessionSort::Duration => SortDirection::Desc,
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
                    SessionSort::LastSeen => "last_seen",
                    SessionSort::FirstSeen => "first_seen",
                    SessionSort::Traces => "traces",
                    SessionSort::FailedTraces => "failed_traces",
                    SessionSort::Tokens => "session_tokens",
                    SessionSort::Spend => "spend",
                    SessionSort::Duration => "duration_ms",
                };
                let order = format!("ORDER BY {column} {}", direction.to_string().to_uppercase());
                assert!(sql.contains(&order), "{sort} {direction}: {sql}");
                for param in [
                    "{days:UInt16}",
                    "{agents:Array(String)}",
                    "{prompt_versions:Array(String)}",
                    "{limit:UInt32}",
                    "{offset:UInt64}",
                ] {
                    assert!(sql.contains(param), "{sort} {direction} lacks {param}");
                }
            }
        }
    }

    /// ⚠ A session is admitted by any one trace and summed over all of them:
    /// the filters flag a trace, never drop it before the sums.
    #[test]
    fn a_session_is_admitted_by_one_trace_and_summed_over_all() {
        let sql = list_sql(SessionSort::LastSeen, SortDirection::Desc);
        for sql in [sql, COUNT_SQL] {
            assert!(sql.contains("HAVING max(admitted) = 1"), "{sql}");
            assert!(
                !sql.contains("GROUP BY trace_id HAVING"),
                "a trace the filters refuse is still summed: {sql}"
            );
            assert!(
                sql.contains("WHERE trace_session_id != ''"),
                "a trace with no session belongs to none"
            );
            // The score asked for flags a trace as any filter does: among the
            // spans' conditions it would sum a session's scored traces alone.
            let (flagged, spans) = sql
                .split_once(") AS admitted ")
                .expect("the filters flag each trace");
            assert!(flagged.contains("FROM telemetry.scores FINAL"), "{sql}");
            let read = spans.split("GROUP BY trace_id").next().unwrap_or_default();
            assert!(
                read.contains(PROJECTS_PARAM) && !read.contains("telemetry.scores"),
                "{sql}"
            );
            // A score given to the session flags each of its traces too.
            assert!(
                flagged.contains("OR trace_session_id IN (SELECT session_id FROM telemetry.scores")
                    && flagged.ends_with("AND target = 'session'))"),
                "{sql}"
            );
        }
    }

    /// The facets count sessions, under the trace list's filters and none
    /// for the traces that named no session; the score asked for is read off
    /// each trace summed, as the list reads it, a session's own with it.
    #[test]
    fn the_facets_count_sessions() {
        assert!(FACETS_SQL.contains(PROJECTS_PARAM));
        assert!(FACETS_SQL.contains("uniqExact(trace_session_id) AS listed"));
        assert!(FACETS_SQL.contains("{tags:Array(String)}"));
        let (spans, kept) = FACETS_SQL
            .split_once("GROUP BY trace_id) ")
            .expect("each trace summed");
        assert!(!spans.contains("telemetry.scores"), "{FACETS_SQL}");
        assert!(
            kept.contains("AND trace_session_id != '' AND (({score:String} = '' OR trace_id IN (")
                && kept.contains("AND target = 'session')) GROUP BY facet, value"),
            "{FACETS_SQL}"
        );
    }

    /// The sorts that put the most or the latest first say so, and no tag
    /// sorts the list.
    #[test]
    fn each_sort_runs_its_own_way_first_and_no_tag_sorts_the_list() {
        assert_eq!(
            Sessions::first_direction(SessionSort::FirstSeen),
            SortDirection::Asc
        );
        for sort in SessionSort::all() {
            if sort != SessionSort::FirstSeen {
                assert_eq!(
                    Sessions::first_direction(sort),
                    SortDirection::Desc,
                    "{sort}"
                );
            }
        }
        assert!(Sessions::tag_sql(SortDirection::Asc).is_none());
    }
}
