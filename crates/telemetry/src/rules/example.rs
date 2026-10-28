//! The run a notice shows, picked from the spans its alert counted over its
//! window: the latest that failed for failures, the slowest for a duration,
//! the one with the most tokens for tokens, the one that spent the most for
//! a cost, else the latest; and that run as its spans so far tell it, its
//! spend among them. Names and counts alone, never a content field.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::Deserialize;
use uuid::Uuid;

use telmoni_shared::RuleMeasure;

use crate::AppState;
use crate::rules::db::RuleRow;
use crate::rules::evaluate::Window;
use crate::spend::{self, Unread};
use crate::store::{Projects, micros};

/// Which of the window's spans a notice shows the run of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    LatestFailed,
    Slowest,
    MostTokens,
    MostSpent,
    Latest,
}

impl Pick {
    /// The span that best shows an alert on `measure` crossing.
    #[must_use]
    pub const fn of(measure: RuleMeasure) -> Self {
        match measure {
            RuleMeasure::Failures => Self::LatestFailed,
            RuleMeasure::Duration => Self::Slowest,
            RuleMeasure::Tokens => Self::MostTokens,
            RuleMeasure::Cost => Self::MostSpent,
            RuleMeasure::Count => Self::Latest,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::LatestFailed => "latest_failed",
            Self::Slowest => "slowest",
            Self::MostTokens => "most_tokens",
            Self::MostSpent => "most_spent",
            Self::Latest => "latest",
        }
    }
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
    let window = Window::ending(now, rule.lookback.duration());
    let filters = &rule.filters.0;
    let pick = Pick::of(rule.measure);
    let projects = Projects::one(rule.project_id.clone());
    let run_id = if pick == Pick::MostSpent {
        let spent = spend::runs(state, &projects, window, filters).await?;
        (spent.runs > 0).then_some(spent.costliest)
    } else {
        let read = state
            .store
            .read(&projects, PICK_SQL)
            .param("from", micros(window.from))
            .param("to", micros(window.to))
            .param("kind", rule.kind.to_string())
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
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each measure picks the span that shows it, and the read names each pick.
    #[test]
    fn each_measure_picks_the_span_that_shows_it() {
        assert_eq!(Pick::of(RuleMeasure::Failures), Pick::LatestFailed);
        assert_eq!(Pick::of(RuleMeasure::Duration), Pick::Slowest);
        assert_eq!(Pick::of(RuleMeasure::Tokens), Pick::MostTokens);
        assert_eq!(Pick::of(RuleMeasure::Cost), Pick::MostSpent);
        assert_eq!(Pick::of(RuleMeasure::Count), Pick::Latest);
        for pick in [Pick::LatestFailed, Pick::Slowest, Pick::MostTokens] {
            assert!(PICK_SQL.contains(&format!("'{}'", pick.as_str())));
        }
    }
}
