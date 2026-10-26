//! Alerts, built as Langfuse's are: each a metric — a measure of the
//! project's spans of one kind, filtered by the fields the hourly totals
//! keep, folded over the last hour, day or week — against an alert threshold
//! and an optional warning one under one operator. Kept in `telemetry.rules`
//! ([`db`]), made, edited, paused and deleted by any member of the project
//! ([`crate::handler::rules`]), and each evaluated a minute after the last by
//! a loop every replica runs ([`evaluate`]). An organization's alerts,
//! paused ones counted, are held to its plan's number
//! ([`telmoni_shared::seam::Entitlements::alert_ceilings`]).

pub mod db;
pub mod evaluate;

use serde::{Deserialize, Serialize};

use telmoni_shared::{
    MissingData, RuleAggregation, RuleMeasure, RuleOperator, RuleWindow, SpanKind, SpanStatus,
    TelmoniError,
};

use crate::contract::{AGENT_MAX, ENVIRONMENT_MAX, MODEL_MAX, NAME_MAX, PROVIDER_MAX, RELEASE_MAX};
use crate::handler::traces::values;

/// The longest an alert's name is, in characters, as the table's CHECK has it.
pub const RULE_NAME_MAX: usize = 60;

/// The longest re-notification period, in minutes: a week.
pub const RENOTIFY_MAX_MINUTES: i32 = 10_080;

/// The spans an alert reads: those of its kind whose fields its filters
/// name, each filter any of its values. Only what the hourly totals keep:
/// they name no user, session, tag or prompt, so nor does an alert.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuleFilters {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agent: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub model: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub provider: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<SpanStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub release: Vec<String>,
}

impl RuleFilters {
    /// The filters as the trace list's checks leave them: each value
    /// trimmed, given once and under its field's cap.
    fn checked(self) -> Result<Self, TelmoniError> {
        let mut status = self.status;
        status.dedup();
        Ok(Self {
            agent: values("agent", &self.agent, AGENT_MAX)?,
            model: values("model", &self.model, MODEL_MAX)?,
            provider: values("provider", &self.provider, PROVIDER_MAX)?,
            tool: values("tool", &self.tool, NAME_MAX)?,
            status,
            environment: values("environment", &self.environment, ENVIRONMENT_MAX)?,
            release: values("release", &self.release, RELEASE_MAX)?,
        })
    }
}

/// An alert as a person writes it: a new one, or one saved over.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleInput {
    pub name: String,
    /// The spans it reads: runs, steps, model calls or tool calls.
    pub kind: SpanKind,
    pub measure: RuleMeasure,
    pub aggregation: RuleAggregation,
    #[serde(default)]
    pub filters: RuleFilters,
    pub window: RuleWindow,
    pub operator: RuleOperator,
    pub alert_threshold: f64,
    #[serde(default)]
    pub warning_threshold: Option<f64>,
    #[serde(default)]
    pub missing_data: MissingData,
    /// How often a severity held is told again, in minutes; never if none.
    #[serde(default)]
    pub renotify_minutes: Option<i32>,
}

/// An alert checked whole, as the table keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub name: String,
    pub kind: SpanKind,
    pub measure: RuleMeasure,
    pub aggregation: RuleAggregation,
    pub filters: RuleFilters,
    pub window: RuleWindow,
    pub operator: RuleOperator,
    pub alert_threshold: f64,
    pub warning_threshold: Option<f64>,
    pub missing_data: MissingData,
    pub renotify_minutes: Option<i32>,
}

impl RuleInput {
    /// The alert, or why it cannot be one: a name of 1 to [`RULE_NAME_MAX`]
    /// characters; a kind of span ours names; an aggregation its measure
    /// folds ([`folds`]); finite thresholds, a warning one met before the
    /// alert's as the operator reads them; and re-notification from a minute
    /// to a week.
    pub fn checked(self) -> Result<Rule, TelmoniError> {
        let bad = |why: &str| TelmoniError::BadRequest(why.to_owned());
        let name = self.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > RULE_NAME_MAX {
            return Err(bad(&format!(
                "an alert's name is 1 to {RULE_NAME_MAX} characters"
            )));
        }
        if self.kind == SpanKind::Span {
            return Err(bad("an alert reads runs, steps, model calls or tool calls"));
        }
        if !folds(self.measure, self.aggregation) {
            return Err(bad(&format!(
                "{} is not folded by {}: count takes count; failures count or avg; duration \
                 avg, sum or a percentile; tokens avg or sum",
                self.measure, self.aggregation
            )));
        }
        if self.measure == RuleMeasure::Tokens && self.kind != SpanKind::ModelCall {
            return Err(bad("tokens are counted on model calls alone"));
        }
        if !self.alert_threshold.is_finite() {
            return Err(bad("the alert threshold is a finite number"));
        }
        if let Some(warning) = self.warning_threshold {
            if !warning.is_finite() {
                return Err(bad("the warning threshold is a finite number"));
            }
            if !warns_first(self.operator, warning, self.alert_threshold) {
                return Err(bad(
                    "the warning threshold is met before the alert threshold: below it for \
                     > and ≥, above it for < and ≤, and none for = and ≠",
                ));
            }
        }
        if let Some(minutes) = self.renotify_minutes
            && !(1..=RENOTIFY_MAX_MINUTES).contains(&minutes)
        {
            return Err(bad(&format!(
                "re-notification is every 1 to {RENOTIFY_MAX_MINUTES} minutes, or never"
            )));
        }
        Ok(Rule {
            name,
            kind: self.kind,
            measure: self.measure,
            aggregation: self.aggregation,
            filters: self.filters.checked()?,
            window: self.window,
            operator: self.operator,
            alert_threshold: self.alert_threshold,
            warning_threshold: self.warning_threshold,
            missing_data: self.missing_data,
            renotify_minutes: self.renotify_minutes,
        })
    }
}

/// Whether `aggregation` folds `measure`: a count of spans; failures counted
/// or as a share; durations averaged, summed or at a percentile; tokens
/// averaged or summed, since the hourly totals keep their sum alone.
#[must_use]
pub const fn folds(measure: RuleMeasure, aggregation: RuleAggregation) -> bool {
    use RuleAggregation as A;
    match measure {
        RuleMeasure::Count => matches!(aggregation, A::Count),
        RuleMeasure::Failures => matches!(aggregation, A::Count | A::Avg),
        RuleMeasure::Duration => !matches!(aggregation, A::Count),
        RuleMeasure::Tokens => matches!(aggregation, A::Avg | A::Sum),
    }
}

/// Whether a value climbing toward the alert threshold meets `warning`
/// first, as `operator` compares: an equality has no way there.
fn warns_first(operator: RuleOperator, warning: f64, alert: f64) -> bool {
    match operator {
        RuleOperator::Gt | RuleOperator::Gte => warning < alert,
        RuleOperator::Lt | RuleOperator::Lte => warning > alert,
        RuleOperator::Eq | RuleOperator::Ne => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> RuleInput {
        RuleInput {
            name: " Slow model calls ".into(),
            kind: SpanKind::ModelCall,
            measure: RuleMeasure::Duration,
            aggregation: RuleAggregation::P95,
            filters: RuleFilters {
                model: vec![" claude-sonnet-5-5 ".into(), "claude-sonnet-5-5".into()],
                ..RuleFilters::default()
            },
            window: RuleWindow::Hour,
            operator: RuleOperator::Gt,
            alert_threshold: 5_000.0,
            warning_threshold: Some(3_000.0),
            missing_data: MissingData::Zero,
            renotify_minutes: None,
        }
    }

    #[test]
    fn an_alert_is_kept_as_its_checks_leave_it() {
        let rule = input().checked().expect("an alert");
        assert_eq!(rule.name, "Slow model calls");
        assert_eq!(rule.filters.model, ["claude-sonnet-5-5"]);
    }

    /// ⚠ Each measure folds only as the hourly totals can answer it.
    #[test]
    fn a_measure_is_folded_only_as_the_totals_can_answer() {
        for aggregation in RuleAggregation::all() {
            assert_eq!(
                folds(RuleMeasure::Count, aggregation),
                aggregation == RuleAggregation::Count
            );
            assert_eq!(
                folds(RuleMeasure::Duration, aggregation),
                aggregation != RuleAggregation::Count
            );
            assert_eq!(
                folds(RuleMeasure::Tokens, aggregation),
                matches!(aggregation, RuleAggregation::Avg | RuleAggregation::Sum)
            );
        }
        let refused = RuleInput {
            aggregation: RuleAggregation::Count,
            ..input()
        };
        assert!(refused.checked().is_err());
    }

    #[test]
    fn a_warning_is_met_before_the_alert() {
        assert!(
            RuleInput {
                warning_threshold: Some(6_000.0),
                ..input()
            }
            .checked()
            .is_err()
        );
        assert!(
            RuleInput {
                operator: RuleOperator::Lt,
                alert_threshold: 1.0,
                warning_threshold: Some(2.0),
                ..input()
            }
            .checked()
            .is_ok()
        );
        assert!(
            RuleInput {
                operator: RuleOperator::Eq,
                ..input()
            }
            .checked()
            .is_err(),
            "an equality takes no warning"
        );
    }

    #[test]
    fn what_cannot_be_an_alert_is_refused() {
        for refused in [
            RuleInput {
                name: " ".into(),
                ..input()
            },
            RuleInput {
                name: "x".repeat(RULE_NAME_MAX + 1),
                ..input()
            },
            RuleInput {
                kind: SpanKind::Span,
                ..input()
            },
            RuleInput {
                alert_threshold: f64::NAN,
                ..input()
            },
            RuleInput {
                renotify_minutes: Some(0),
                ..input()
            },
            RuleInput {
                renotify_minutes: Some(RENOTIFY_MAX_MINUTES + 1),
                ..input()
            },
            RuleInput {
                kind: SpanKind::Run,
                measure: RuleMeasure::Tokens,
                aggregation: RuleAggregation::Sum,
                warning_threshold: None,
                ..input()
            },
        ] {
            assert!(refused.checked().is_err());
        }
    }
}
