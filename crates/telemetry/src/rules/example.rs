//! The run a notice shows, picked from the spans its alert counted over its
//! window: the latest that failed for failures, the slowest for a duration,
//! the one with the most tokens for tokens, the one that spent the most for
//! a cost, the one scored lowest or highest for a score, as it crosses, the
//! one with the most steps, the longest loop or the longest quiet, else the
//! latest; and that run as its spans so far tell it, its spend among them.
//! Names and counts alone, never a content field.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::Deserialize;
use uuid::Uuid;

use telmoni_shared::{RuleMeasure, RuleOperator};

use crate::AppState;
use crate::rules::db::RuleRow;
use crate::rules::evaluate::{self, Window};
use crate::spend::{self, Unread};
use crate::store::{Projects, micros};

/// Which of the window's spans a notice shows the run of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    LatestFailed,
    Slowest,
    MostTokens,
    MostSpent,
    LowestScored,
    HighestScored,
    MostSteps,
    LongestRepeat,
    Stalest,
    Latest,
}

impl Pick {
    /// The span that best shows an alert on `measure` crossing as `operator`
    /// compares: a score's lowest where it falls below, its highest above.
    #[must_use]
    pub const fn of(measure: RuleMeasure, operator: RuleOperator) -> Self {
        match measure {
            RuleMeasure::Score if matches!(operator, RuleOperator::Gt | RuleOperator::Gte) => {
                Self::HighestScored
            }
            RuleMeasure::Failures | RuleMeasure::Burn => Self::LatestFailed,
            RuleMeasure::Duration => Self::Slowest,
            RuleMeasure::Tokens => Self::MostTokens,
            RuleMeasure::Cost => Self::MostSpent,
            RuleMeasure::Score => Self::LowestScored,
            RuleMeasure::Steps => Self::MostSteps,
            RuleMeasure::Repeats => Self::LongestRepeat,
            RuleMeasure::Stalled => Self::Stalest,
            RuleMeasure::Count => Self::Latest,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::LatestFailed => "latest_failed",
            Self::Slowest => "slowest",
            Self::MostTokens => "most_tokens",
            Self::MostSpent => "most_spent",
            Self::LowestScored => "lowest_scored",
            Self::HighestScored => "highest_scored",
            Self::MostSteps => "most_steps",
            Self::LongestRepeat => "longest_repeat",
            Self::Stalest => "stalest",
            Self::Latest => "latest",
        }
    }
}

/// A loop as a notice tells it: the agent and tool of the longest stretch,
/// its length, and how many runs of that agent called that tool past each threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repeated {
    pub agent: String,
    pub tool: String,
    pub times: u64,
    pub past_alert: u64,
    pub past_warning: u64,
}

/// A run as a notice tells it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExampleRun {
    pub run_id: Uuid,
    pub pick: Pick,
    /// Its last step's name, none before its first.
    pub last_step: Option<String>,
    /// Its own error's type, else that of its latest span that failed.
    pub error_type: Option<String>,
    pub steps: u64,
    /// Its tokens in and out.
    pub tokens: u64,
    /// What it spent, in US dollars ([`spend::run`]).
    pub spent: f64,
    /// The provider and model of each name its calls with neither a cost
    /// nor a price carry.
    pub unpriced: Vec<(String, String)>,
    /// Whether its own span has arrived, past which its counts are whole.
    pub ended: bool,
    /// The loop it shows, for an alert on repeats.
    pub repeated: Option<Repeated>,
}

/// The run of the window's span the pick names, over the filters the
/// alert's read of `spans` takes.
const PICK_SQL: &str = concat!(
    "SELECT run_id FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} ",
    "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
    "AND kind = {kind:String} ",
    crate::rules::spans_admitted!(),
    "AND ({pick:String} != 'latest_failed' OR status = 'error') ",
    "ORDER BY multiIf(",
    "{pick:String} = 'slowest', dateDiff('microsecond', started_at, ended_at), ",
    "{pick:String} = 'most_tokens', toInt64(input_tokens + output_tokens), ",
    "toInt64(0)) DESC, received_at DESC ",
    "LIMIT 1"
);

/// The run scored lowest, or highest, by the score alert's name over its
/// window, of the scores a run's or its spans', each as its last row has it.
const SCORED_SQL: &str = concat!(
    "SELECT run AS run_id FROM (SELECT argMax(run_id, received_at) AS run, ",
    "argMax(value, received_at) AS v, argMax(is_deleted, received_at) AS gone ",
    "FROM telemetry.scores ",
    "WHERE project_id IN {projects:Array(String)} AND name = {name:String} ",
    "AND data_type IN ('number', 'boolean') ",
    "AND received_at >= {from:DateTime64(6, 'UTC')} AND received_at < {to:DateTime64(6, 'UTC')} ",
    "GROUP BY score_id) ",
    "WHERE gone = 0 AND run != toUUID('00000000-0000-0000-0000-000000000000') ",
    "ORDER BY if({pick:String} = 'highest_scored', -v, v) ASC LIMIT 1"
);

/// The run of the window with the most steps, the latest of those tied.
const MOST_STEPS_SQL: &str = concat!(
    "SELECT run_id FROM (",
    evaluate::run_steps!(),
    ") ORDER BY n DESC, last DESC LIMIT 1"
);

/// The run quiet longest of those a stall alert counts.
const STALEST_SQL: &str = concat!(
    "SELECT run_id FROM (",
    evaluate::stalled_runs!(),
    ") ORDER BY n DESC, last DESC LIMIT 1"
);

/// The run that called one tool the most times in a row, the latest of those
/// tied, with its agent and tool, and how many runs of that agent called
/// that tool in a row at least as often as each threshold, as `strict` compares.
const REPEATED_SQL: &str = concat!(
    "SELECT argMax(run_id, (n, last)) AS run_id, agent, tool, max(n) AS times, ",
    "countIf(if({strict:Bool}, n > {alert:Float64}, n >= {alert:Float64})) AS past_alert, ",
    "countIf(if({strict:Bool}, n > {warning:Float64}, n >= {warning:Float64})) ",
    "AS past_warning FROM (",
    evaluate::run_repeats!(),
    ") WHERE n > 0 GROUP BY agent, tool ORDER BY times DESC, max(last) DESC LIMIT 1"
);

/// One run's spans so far, summed.
const RUN_SQL: &str = concat!(
    "SELECT argMaxIf(name, ended_at, kind = 'step') AS last_step, ",
    "countIf(kind = 'step') AS steps, ",
    "sum(input_tokens + output_tokens) AS tokens, ",
    "anyIf(error_type, kind = 'run' AND error_type != '') AS run_error, ",
    "argMaxIf(error_type, ended_at, error_type != '') AS latest_error, ",
    "countIf(kind = 'run') AS runs ",
    "FROM telemetry.spans ",
    "WHERE project_id IN {projects:Array(String)} AND run_id = {run:UUID}"
);

#[derive(Row, Deserialize)]
struct Picked {
    #[serde(with = "clickhouse::serde::uuid")]
    run_id: Uuid,
}

#[derive(Row, Deserialize)]
struct RepeatedRow {
    #[serde(with = "clickhouse::serde::uuid")]
    run_id: Uuid,
    agent: String,
    tool: String,
    times: u64,
    past_alert: u64,
    past_warning: u64,
}

#[derive(Row, Deserialize)]
struct Summed {
    last_step: String,
    steps: u64,
    tokens: u64,
    run_error: String,
    latest_error: String,
    runs: u64,
}

/// The run a notice of `rule`'s shows, over its window ending at `now`;
/// none where the window holds no span the pick admits.
///
/// # Errors
///
/// ClickHouse, or Postgres for the prices, did not answer.
pub async fn example(
    state: &AppState,
    rule: &RuleRow,
    now: DateTime<Utc>,
) -> Result<Option<ExampleRun>, Unread> {
    let window = Window::of(now, rule.definition.window);
    let filters = &rule.definition.filters;
    // A latency objective's runs miss it by running long.
    let pick = match rule.definition.objective {
        Some(objective) if objective.latency_ms.is_some() => Pick::Slowest,
        _ => Pick::of(rule.definition.measure, rule.definition.operator),
    };
    let projects = Projects::one(rule.project_id.clone());
    let mut repeated = None;
    let run_id = if pick == Pick::MostSpent {
        let spent = spend::runs(state, &projects, window, filters).await?;
        (spent.runs > 0).then_some(spent.costliest)
    } else if matches!(pick, Pick::MostSteps | Pick::Stalest) {
        let sql = if pick == Pick::MostSteps {
            MOST_STEPS_SQL
        } else {
            STALEST_SQL
        };
        let picked: Option<Picked> = evaluate::by_run(state, rule, now, sql)
            .fetch_optional()
            .await?;
        picked.map(|Picked { run_id }| run_id)
    } else if pick == Pick::LongestRepeat {
        let definition = &rule.definition;
        let row: Option<RepeatedRow> = evaluate::by_run(state, rule, now, REPEATED_SQL)
            .param("strict", definition.operator == RuleOperator::Gt)
            .param("alert", definition.alert_threshold)
            .param(
                "warning",
                definition
                    .warning_threshold
                    .unwrap_or(definition.alert_threshold),
            )
            .fetch_optional()
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        repeated = Some(Repeated {
            agent: row.agent,
            tool: row.tool,
            times: row.times,
            past_alert: row.past_alert,
            past_warning: row.past_warning,
        });
        Some(row.run_id)
    } else if matches!(pick, Pick::LowestScored | Pick::HighestScored) {
        let picked: Option<Picked> = state
            .store
            .read(&projects, SCORED_SQL)
            .param(
                "name",
                rule.definition.score_name.as_deref().unwrap_or_default(),
            )
            .param("pick", pick.as_str())
            .param("from", micros(window.from))
            .param("to", micros(window.to))
            .fetch_optional()
            .await?;
        picked.map(|Picked { run_id }| run_id)
    } else {
        let read = state
            .store
            .read(&projects, PICK_SQL)
            .param("from", micros(window.from))
            .param("to", micros(window.to))
            .param("kind", rule.definition.kind.to_string())
            .param("pick", pick.as_str());
        let picked: Option<Picked> = filters.bind(read).fetch_optional().await?;
        picked.map(|Picked { run_id }| run_id)
    };
    let Some(run_id) = run_id else {
        return Ok(None);
    };
    let summed: Summed = state
        .store
        .read(&projects, RUN_SQL)
        .param("run", run_id)
        .fetch_one()
        .await?;
    let spent = spend::run(state, &projects, run_id).await?;
    let named = |text: String| (!text.is_empty()).then_some(text);
    Ok(Some(ExampleRun {
        run_id,
        pick,
        last_step: named(summed.last_step),
        error_type: named(summed.run_error).or_else(|| named(summed.latest_error)),
        steps: summed.steps,
        tokens: summed.tokens,
        spent: spent.spent,
        unpriced: spent.unpriced,
        ended: summed.runs > 0,
        repeated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each measure picks the span that shows it, and the read names each pick.
    #[test]
    fn each_measure_picks_the_span_that_shows_it() {
        assert_eq!(
            Pick::of(RuleMeasure::Failures, RuleOperator::Lt),
            Pick::LatestFailed
        );
        assert_eq!(
            Pick::of(RuleMeasure::Duration, RuleOperator::Lt),
            Pick::Slowest
        );
        assert_eq!(
            Pick::of(RuleMeasure::Tokens, RuleOperator::Lt),
            Pick::MostTokens
        );
        assert_eq!(
            Pick::of(RuleMeasure::Cost, RuleOperator::Lt),
            Pick::MostSpent
        );
        assert_eq!(
            Pick::of(RuleMeasure::Score, RuleOperator::Lt),
            Pick::LowestScored
        );
        assert_eq!(
            Pick::of(RuleMeasure::Score, RuleOperator::Gte),
            Pick::HighestScored
        );
        assert_eq!(
            Pick::of(RuleMeasure::Duration, RuleOperator::Gt),
            Pick::Slowest
        );
        assert_eq!(Pick::of(RuleMeasure::Count, RuleOperator::Lt), Pick::Latest);
        assert_eq!(
            Pick::of(RuleMeasure::Steps, RuleOperator::Gt),
            Pick::MostSteps
        );
        assert_eq!(
            Pick::of(RuleMeasure::Repeats, RuleOperator::Gte),
            Pick::LongestRepeat
        );
        assert_eq!(
            Pick::of(RuleMeasure::Stalled, RuleOperator::Gt),
            Pick::Stalest
        );
        for pick in [Pick::LatestFailed, Pick::Slowest, Pick::MostTokens] {
            assert!(PICK_SQL.contains(&format!("'{}'", pick.as_str())));
        }
    }
}
