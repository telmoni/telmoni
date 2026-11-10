//! An agent's versions side by side: its releases, its models, or one
//! prompt's versions over a window of days, a row each of the traces that
//! carried it, read from the spans so a trace, its duration, its failure and
//! its spend are the trace list's own ([`crate::traces`]). A trace counts
//! under each value its spans in the chosen environment carry, and is
//! measured whole; one carrying none is the row with no value.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use telmoni_shared::ChangeKind;

use crate::store::{Projects, Read, Store};
use crate::traces::{TraceFilter, filtered, trace_carries, trace_status, trace_sums, window};

/// The most versions one answer names, the newest first; the row with no
/// value is beside them.
pub const VERSIONS_MAX: usize = 50;

/// The most scores a version's row shows: those given most often.
pub const SCORES_SHOWN: usize = 5;

/// The most environments or prompts the answer offers, the most traces first.
pub const CHOICES_MAX: u32 = 50;

/// The agent's traces with a span in the environment, a row each as the
/// trace list sums them, `members` the values those spans carry.
macro_rules! member_traces {
    ($members:expr, $present:expr) => {
        concat!(
            "SELECT ",
            trace_sums!(),
            ", ",
            $members,
            " AS members ",
            window!(),
            "AND agent = {agent:String} GROUP BY trace_id HAVING ",
            $present
        )
    };
}

/// The traces folded by value, the newest first seen first and the row with
/// no value last, kept beside the limit; no ended trace, no duration or spend.
macro_rules! versions_sql {
    ($members:expr, $present:expr) => {
        concat!(
            "SELECT value, count() AS traces, countIf(trace_status = 'error') AS failed_traces, ",
            "min(first_started_at) AS first_at, ",
            "if(countIf(trace_ended) > 0, quantileExactIf(0.5)(duration_ms, trace_ended), NULL) AS p50_ms, ",
            "if(countIf(trace_ended) > 0, quantileExactIf(0.95)(duration_ms, trace_ended), NULL) AS p95_ms, ",
            "if(countIf(trace_ended) > 0, quantileExactIf(0.5)(spent, trace_ended), NULL) AS median_spend ",
            "FROM (",
            member_traces!($members, $present),
            ") ARRAY JOIN if(empty(members), [''], members) AS value ",
            "GROUP BY value ORDER BY value = '', first_at DESC, value ",
            "LIMIT {max:UInt32} BY value = ''"
        )
    };
}

/// Each score's mean on the traces of each version named, a trace's score
/// read once a version however many of its spans carry it.
macro_rules! scores_sql {
    ($members:expr, $present:expr) => {
        concat!(
            "SELECT version, name, avg(value) AS mean, count() AS given ",
            "FROM (SELECT trace_id, arrayJoin(if(empty(members), [''], members)) AS version ",
            "FROM (SELECT trace_id, ",
            $members,
            " AS members ",
            window!(),
            "AND agent = {agent:String} GROUP BY trace_id HAVING ",
            $present,
            ")) AS carried INNER JOIN (",
            "SELECT trace_id, name, value FROM telemetry.scores FINAL ",
            "WHERE project_id IN {projects:Array(String)} AND is_deleted = 0 ",
            "AND data_type IN ('number', 'boolean') ",
            "AND received_at >= now64(6) - toIntervalDay({days:UInt16}) ",
            // The agent's traces alone, so the join holds no other's scores.
            "AND trace_id IN (SELECT trace_id ",
            window!(),
            "AND agent = {agent:String})",
            ") AS scored USING (trace_id) ",
            "WHERE has({versions:Array(String)}, version) ",
            "GROUP BY version, name ORDER BY version, name"
        )
    };
}

macro_rules! here {
    () => {
        "countIf(environment = {environment:String}) > 0"
    };
}

macro_rules! releases {
    () => {
        "groupUniqArrayIf(release, release != '' AND environment = {environment:String})"
    };
}

macro_rules! models {
    () => {
        "groupUniqArrayIf(model, kind = 'model_call' AND model != '' \
         AND environment = {environment:String})"
    };
}

/// The prompt's versions off the spans naming it, name and version read
/// apart so a colon in either joins no two prompts.
macro_rules! prompt_versions {
    () => {
        "groupUniqArrayIf(attributes['gen_ai.prompt.version'], \
         environment = {environment:String} \
         AND attributes['gen_ai.prompt.name'] = {prompt:String} \
         AND attributes['gen_ai.prompt.version'] != '')"
    };
}

macro_rules! prompted_here {
    () => {
        "countIf(environment = {environment:String} \
         AND attributes['gen_ai.prompt.name'] = {prompt:String}) > 0"
    };
}

const fn versions_sql(by: ChangeKind) -> &'static str {
    match by {
        ChangeKind::Release => versions_sql!(releases!(), here!()),
        ChangeKind::Model => versions_sql!(models!(), here!()),
        ChangeKind::Prompt => versions_sql!(prompt_versions!(), prompted_here!()),
    }
}

const fn scores_sql(by: ChangeKind) -> &'static str {
    match by {
        ChangeKind::Release => scores_sql!(releases!(), here!()),
        ChangeKind::Model => scores_sql!(models!(), here!()),
        ChangeKind::Prompt => scores_sql!(prompt_versions!(), prompted_here!()),
    }
}

/// The environments the agent's spans were sent from, the one with no name
/// among them, each with the traces holding a span of it.
const ENVIRONMENTS_SQL: &str = concat!(
    "SELECT environment AS value, uniqExact(trace_id) AS traces ",
    window!(),
    "AND agent = {agent:String} ",
    "GROUP BY value ORDER BY traces DESC, value LIMIT {max:UInt32}"
);

/// The prompts the agent's spans in the environment name, each with its traces.
const PROMPTS_SQL: &str = concat!(
    "SELECT attributes['gen_ai.prompt.name'] AS value, uniqExact(trace_id) AS traces ",
    window!(),
    "AND agent = {agent:String} AND environment = {environment:String} ",
    "AND attributes['gen_ai.prompt.name'] != '' ",
    "GROUP BY value ORDER BY traces DESC, value LIMIT {max:UInt32}"
);

/// Whose versions a read is of.
#[derive(Debug, Clone, Copy)]
pub struct Of<'a> {
    pub agent: &'a str,
    /// What its traces are split by: the words a change is kinded by.
    pub by: ChangeKind,
    pub environment: &'a str,
    /// The prompt, where the split is by one's versions; empty otherwise.
    pub prompt: &'a str,
}

/// An environment or a prompt the answer offers.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct Choice {
    /// Empty for the environment with no name.
    pub value: String,
    /// The agent's traces holding a span of it.
    pub traces: u64,
}

/// One version's traces, as ClickHouse answers them.
#[derive(Debug, Clone, PartialEq, Row, Deserialize)]
pub struct VersionRow {
    pub value: String,
    pub traces: u64,
    pub failed_traces: u64,
    #[serde(deserialize_with = "clickhouse::serde::chrono::datetime64::micros::deserialize")]
    pub first_at: DateTime<Utc>,
    pub p50_ms: Option<i64>,
    pub p95_ms: Option<i64>,
    pub median_spend: Option<f64>,
}

/// One score's mean on one version's traces.
#[derive(Debug, Clone, PartialEq, Row, Deserialize)]
pub struct ScoreRow {
    pub version: String,
    pub name: String,
    pub mean: f64,
    pub given: u64,
}

/// A score's mean on a version's traces, a yes or no's 1 or 0.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoreMean {
    pub name: String,
    pub mean: f64,
    /// The scores it is the mean of.
    pub count: u64,
}

/// One version over the window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Version {
    /// Empty for the traces that carried none.
    pub value: String,
    /// When the first of its traces in the window started.
    pub first_at: DateTime<Utc>,
    /// Its traces, those still running among them.
    pub traces: u64,
    /// Those that ended in error.
    pub failed_traces: u64,
    /// Its ended traces' median duration; none before one has ended.
    pub p50_ms: Option<i64>,
    /// And their 95th percentile.
    pub p95_ms: Option<i64>,
    /// The median of what its ended traces cost, in US dollars.
    pub median_spend: Option<f64>,
    /// Its mean of each score the answer names, in the answer's order.
    pub scores: Vec<ScoreMean>,
}

fn bound(read: Read, filter: &TraceFilter, of: &Of<'_>) -> Read {
    filtered(read, filter)
        .param("agent", of.agent)
        .param("environment", of.environment)
        .param("prompt", of.prompt)
}

/// The agent's environments in the window, the most traces first.
///
/// # Errors
///
/// ClickHouse did not answer.
pub async fn environments(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    agent: &str,
) -> clickhouse::error::Result<Vec<Choice>> {
    filtered(store.read(projects, ENVIRONMENTS_SQL), filter)
        .param("agent", agent)
        .param("max", CHOICES_MAX)
        .fetch_all()
        .await
}

/// The prompts the agent's traces name in the environment, the most traces first.
///
/// # Errors
///
/// ClickHouse did not answer.
pub async fn prompts(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    agent: &str,
    environment: &str,
) -> clickhouse::error::Result<Vec<Choice>> {
    filtered(store.read(projects, PROMPTS_SQL), filter)
        .param("agent", agent)
        .param("environment", environment)
        .param("max", CHOICES_MAX)
        .fetch_all()
        .await
}

/// The versions' rows, [`VERSIONS_MAX`] named ones at most, and whether the
/// window held more.
///
/// # Errors
///
/// ClickHouse did not answer.
pub async fn rows(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    of: &Of<'_>,
) -> clickhouse::error::Result<(Vec<VersionRow>, bool)> {
    let most = u32::try_from(VERSIONS_MAX).unwrap_or(u32::MAX);
    let mut rows: Vec<VersionRow> = bound(store.read(projects, versions_sql(of.by)), filter, of)
        .param("max", most.saturating_add(1))
        .fetch_all()
        .await?;
    let named = rows.iter().filter(|row| !row.value.is_empty()).count();
    let more = named > VERSIONS_MAX;
    if more {
        // The oldest named row is the last before the one with no value.
        rows.remove(VERSIONS_MAX);
    }
    Ok((rows, more))
}

/// Each score's mean on each of `rows`' versions.
///
/// # Errors
///
/// ClickHouse did not answer.
pub async fn score_means(
    store: &Store,
    projects: &Projects,
    filter: &TraceFilter,
    of: &Of<'_>,
    rows: &[VersionRow],
) -> clickhouse::error::Result<Vec<ScoreRow>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let versions: Vec<&str> = rows.iter().map(|row| row.value.as_str()).collect();
    bound(store.read(projects, scores_sql(of.by)), filter, of)
        .param("versions", versions)
        .fetch_all()
        .await
}

/// `rows` with their scores, and the scores the answer names: the
/// [`SCORES_SHOWN`] given most often across them, then by name.
#[must_use]
pub fn scored(rows: Vec<VersionRow>, means: Vec<ScoreRow>) -> (Vec<Version>, Vec<String>) {
    let mut given: HashMap<&str, u64> = HashMap::new();
    for mean in means.iter().filter(|mean| mean.mean.is_finite()) {
        *given.entry(mean.name.as_str()).or_default() += mean.given;
    }
    let mut names: Vec<(&str, u64)> = given.into_iter().collect();
    names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let names: Vec<String> = names
        .into_iter()
        .take(SCORES_SHOWN)
        .map(|(name, _)| name.to_owned())
        .collect();
    let versions = rows
        .into_iter()
        .map(|row| {
            let scores = names
                .iter()
                .filter_map(|name| {
                    means
                        .iter()
                        .find(|mean| {
                            mean.version == row.value && &mean.name == name && mean.mean.is_finite()
                        })
                        .map(|mean| ScoreMean {
                            name: mean.name.clone(),
                            mean: mean.mean,
                            count: mean.given,
                        })
                })
                .collect();
            Version {
                value: row.value,
                first_at: row.first_at,
                traces: row.traces,
                failed_traces: row.failed_traces,
                p50_ms: row.p50_ms,
                p95_ms: row.p95_ms,
                median_spend: row.median_spend,
                scores,
            }
        })
        .collect();
    (versions, names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(value: &str) -> VersionRow {
        VersionRow {
            value: value.into(),
            traces: 1,
            failed_traces: 0,
            first_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap_or_default(),
            p50_ms: None,
            p95_ms: None,
            median_spend: None,
        }
    }

    fn mean(version: &str, name: &str, mean: f64, given: u64) -> ScoreRow {
        ScoreRow {
            version: version.into(),
            name: name.into(),
            mean,
            given,
        }
    }

    #[test]
    fn the_scores_named_are_those_given_most_then_by_name() {
        let means = vec![
            mean("v1", "f", 1.0, 1),
            mean("v1", "e", 1.0, 1),
            mean("v1", "d", 1.0, 2),
            mean("v2", "c", 1.0, 2),
            mean("v1", "b", 1.0, 2),
            mean("v2", "a", 0.5, 2),
            mean("v1", "a", 0.25, 3),
        ];
        let (versions, names) = scored(vec![row("v2"), row("v1")], means);
        assert_eq!(names, ["a", "b", "c", "d", "e"], "`f` is the sixth");
        let named = |version: &Version| -> Vec<(String, f64, u64)> {
            version
                .scores
                .iter()
                .map(|s| (s.name.clone(), s.mean, s.count))
                .collect()
        };
        assert_eq!(
            versions.iter().map(named).collect::<Vec<_>>(),
            [
                vec![("a".to_owned(), 0.5, 2), ("c".to_owned(), 1.0, 2)],
                vec![
                    ("a".to_owned(), 0.25, 3),
                    ("b".to_owned(), 1.0, 2),
                    ("d".to_owned(), 1.0, 2),
                    ("e".to_owned(), 1.0, 1),
                ],
            ],
            "each version its own means, in the answer's order"
        );
    }

    #[test]
    fn a_mean_that_is_no_number_is_no_score() {
        let (versions, names) = scored(vec![row("v1")], vec![mean("v1", "a", f64::NAN, 2)]);
        assert!(names.is_empty());
        assert!(versions.iter().all(|v| v.scores.is_empty()));
    }
}
