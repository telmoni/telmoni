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
pub mod deliveries;
pub mod evaluate;
pub mod example;
pub mod incidents;
pub mod mutes;

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Datelike as _, TimeDelta, Timelike as _, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::{
    MissingData, RuleAggregation, RuleLevel, RuleMeasure, RuleOperator, RuleWindow, SpanKind,
    SpanStatus, TelmoniError,
};

use crate::contract::{
    AGENT_MAX, ENVIRONMENT_MAX, MODEL_MAX, NAME_MAX, PROVIDER_MAX, RELEASE_MAX, UTC_OFFSET_RANGE,
};
use crate::handler::traces::values;
use crate::store::Read;

/// The longest an alert's name is, in characters, as the table's CHECK has it.
pub const RULE_NAME_MAX: usize = 60;

/// The longest re-notification period, in minutes: a week.
pub const RENOTIFY_MAX_MINUTES: i32 = 10_080;

/// The most connectors one alert links.
pub const LINKS_MAX: usize = 20;

/// The hours a linked connector hears an alert's notices in: on `days`, 1
/// Monday to 7 Sunday, from `from` up to `to`, each `HH:MM` at `utc_offset`
/// minutes east of UTC; `to` before `from` runs past midnight into the next
/// day, and `to` equal to `from` is the whole day. A fixed offset, so a
/// clock change moves the hours by its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hours {
    pub days: Vec<u8>,
    pub from: TimeOfDay,
    pub to: TimeOfDay,
    #[serde(default)]
    pub utc_offset: i16,
}

impl Hours {
    /// The hours as the lane keeps them: days 1 to 7, each once and in
    /// order, and an offset UTC's own range holds; a time is one by its type.
    fn checked(mut self) -> Result<Self, TelmoniError> {
        let bad = |why: &str| TelmoniError::BadRequest(why.to_owned());
        self.days.sort_unstable();
        self.days.dedup();
        if self.days.is_empty() || self.days.iter().any(|d| !(1..=7).contains(d)) {
            return Err(bad(
                "hours name days 1 (Monday) to 7 (Sunday), one at least",
            ));
        }
        if !UTC_OFFSET_RANGE.contains(&self.utc_offset) {
            return Err(bad("an offset from UTC is -720 to 840 minutes"));
        }
        Ok(self)
    }

    /// Whether `at` falls in these hours.
    #[must_use]
    pub fn hold(&self, at: DateTime<Utc>) -> bool {
        let (from, to) = (u32::from(self.from.0), u32::from(self.to.0));
        let local = at + TimeDelta::minutes(i64::from(self.utc_offset));
        let day = u8::try_from(local.weekday().number_from_monday()).unwrap_or(0);
        let yesterday = if day == 1 { 7 } else { day.saturating_sub(1) };
        let minute = local.hour() * 60 + local.minute();
        let on = |day: u8| self.days.contains(&day);
        match from.cmp(&to) {
            std::cmp::Ordering::Equal => on(day),
            std::cmp::Ordering::Less => on(day) && (from..to).contains(&minute),
            std::cmp::Ordering::Greater => {
                (on(day) && minute >= from) || (on(yesterday) && minute < to)
            }
        }
    }
}

/// Minutes into a day, read and written as `HH:MM`: a time that does not
/// parse never reaches the hours, so none is held open to every hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeOfDay(u16);

impl FromStr for TimeOfDay {
    type Err = &'static str;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (hour, minute) = text.split_once(':').ok_or("a time of day, HH:MM")?;
        if hour.len() != 2 || minute.len() != 2 {
            return Err("a time of day, HH:MM");
        }
        let (hour, minute): (u16, u16) = (
            hour.parse().map_err(|_| "a time of day, HH:MM")?,
            minute.parse().map_err(|_| "a time of day, HH:MM")?,
        );
        (hour < 24 && minute < 60)
            .then_some(Self(hour * 60 + minute))
            .ok_or("a time of day, HH:MM")
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A connector an alert links: which of its notices it hears, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub connection_id: Uuid,
    #[serde(default)]
    pub level: RuleLevel,
    #[serde(default)]
    pub hours: Option<Hours>,
}

impl Link {
    /// Whether this connector hears a notice of an incident at `severity`,
    /// told at `at`.
    #[must_use]
    pub fn hears(&self, severity: telmoni_shared::Severity, at: DateTime<Utc>) -> bool {
        self.level.hears(severity) && self.in_hours(at)
    }

    /// Whether its hours hold at `at`, as every hour does where it has none.
    #[must_use]
    pub fn in_hours(&self, at: DateTime<Utc>) -> bool {
        self.hours.as_ref().is_none_or(|hours| hours.hold(at))
    }

    /// Whether it hears an incident's end, told at `at`: as one of `told_to`,
    /// which heard the incident, whatever holds notices back; else at the
    /// severity it ended at, in its hours and outside a window (`muted`).
    #[must_use]
    pub fn hears_end(
        &self,
        severity: telmoni_shared::Severity,
        told_to: &[Uuid],
        muted: bool,
        at: DateTime<Utc>,
    ) -> bool {
        told_to.contains(&self.connection_id) || (!muted && self.hears(severity, at))
    }
}

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

/// The filters on `telemetry.spans` a run's cost takes too: all but its
/// tools and statuses. Bound by [`RuleFilters::bind_calls`].
macro_rules! calls_admitted {
    () => {
        concat!(
            "AND (empty({agents:Array(String)}) OR agent IN {agents:Array(String)}) ",
            "AND (empty({models:Array(String)}) OR model IN {models:Array(String)}) ",
            "AND (empty({providers:Array(String)}) OR provider IN {providers:Array(String)}) ",
            "AND (empty({environments:Array(String)}) OR environment IN {environments:Array(String)}) ",
            "AND (empty({releases:Array(String)}) OR release IN {releases:Array(String)}) ",
        )
    };
}

/// Every filter an alert takes, on `telemetry.spans`. Bound by
/// [`RuleFilters::bind`].
macro_rules! spans_admitted {
    () => {
        concat!(
            $crate::rules::calls_admitted!(),
            "AND (empty({tools:Array(String)}) OR (kind = 'tool_call' AND name IN {tools:Array(String)})) ",
            "AND (empty({statuses:Array(String)}) OR status IN {statuses:Array(String)}) ",
        )
    };
}

pub(crate) use {calls_admitted, spans_admitted};

impl RuleFilters {
    /// `read` bound to the filters [`calls_admitted!`] names.
    pub(crate) fn bind_calls(&self, read: Read) -> Read {
        read.param("agents", &self.agent)
            .param("models", &self.model)
            .param("providers", &self.provider)
            .param("environments", &self.environment)
            .param("releases", &self.release)
    }

    /// `read` bound to every filter [`spans_admitted!`] names.
    pub(crate) fn bind(&self, read: Read) -> Read {
        let statuses: Vec<String> = self.status.iter().map(SpanStatus::to_string).collect();
        self.bind_calls(read)
            .param("tools", &self.tool)
            .param("statuses", statuses)
    }

    /// The filters as the trace list's checks leave them: each value
    /// trimmed, given once and under its field's cap.
    fn checked(self) -> Result<Self, TelmoniError> {
        let mut status = Vec::new();
        for given in self.status {
            if !status.contains(&given) {
                status.push(given);
            }
        }
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
    /// The project's connectors its notices go to; none, the feed alone.
    #[serde(default)]
    pub connectors: Vec<Link>,
}

/// What an alert is: the eleven fields the table keeps, a person writes and
/// the lanes answer, spelled once. The row and the answer embed it.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Definition {
    pub name: String,
    /// The spans it reads: runs, steps, model calls or tool calls.
    pub kind: SpanKind,
    pub measure: RuleMeasure,
    pub aggregation: RuleAggregation,
    #[sqlx(json)]
    pub filters: RuleFilters,
    /// The column is `lookback`.
    #[sqlx(rename = "lookback")]
    pub window: RuleWindow,
    pub operator: RuleOperator,
    pub alert_threshold: f64,
    pub warning_threshold: Option<f64>,
    pub missing_data: MissingData,
    /// How often a severity held is told again, in minutes; never if none.
    pub renotify_minutes: Option<i32>,
}

/// An alert checked whole, as the table keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub definition: Definition,
    /// The project's connectors its notices go to; none, the feed alone.
    pub connectors: Vec<Link>,
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
        if !self.kind.alertable() {
            return Err(bad("an alert reads runs, steps, model calls or tool calls"));
        }
        if !self.measure.folds(self.aggregation) {
            return Err(bad(&format!(
                "{} is not folded by {}: count takes count; failures count or avg; duration \
                 avg, sum or a percentile; tokens avg or sum; cost avg, sum or max",
                self.measure, self.aggregation
            )));
        }
        if !self.measure.measured_on(self.kind) {
            return Err(bad(match self.measure {
                RuleMeasure::Tokens => "tokens are counted on model calls alone",
                _ => "cost is counted per run, from its model calls",
            }));
        }
        if self.measure == RuleMeasure::Cost {
            // Read from the calls as they land, before the run's own status is known.
            if !(self.filters.status.is_empty() && self.filters.tool.is_empty()) {
                return Err(bad(
                    "a run's cost is read from its model calls as they land: filter it by \
                     agent, model, provider, environment or release",
                ));
            }
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
        if self.connectors.len() > LINKS_MAX {
            return Err(bad(&format!("an alert links up to {LINKS_MAX} connectors")));
        }
        let mut connectors: Vec<Link> = Vec::with_capacity(self.connectors.len());
        for link in self.connectors {
            if connectors
                .iter()
                .any(|kept| kept.connection_id == link.connection_id)
            {
                return Err(bad("an alert links a connector once"));
            }
            connectors.push(Link {
                hours: link.hours.map(Hours::checked).transpose()?,
                ..link
            });
        }
        Ok(Rule {
            connectors,
            definition: Definition {
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
            },
        })
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
            connectors: Vec::new(),
        }
    }

    /// A status given again, wherever in the list, is kept once.
    #[test]
    fn a_status_given_again_is_kept_once() {
        let mut given = input();
        given.filters.status = vec![
            SpanStatus::Error,
            SpanStatus::Ok,
            SpanStatus::Error,
            SpanStatus::Ok,
        ];
        let rule = given.checked().expect("an alert");
        assert_eq!(
            rule.definition.filters.status,
            [SpanStatus::Error, SpanStatus::Ok]
        );
    }

    #[test]
    fn an_alert_is_kept_as_its_checks_leave_it() {
        let rule = input().checked().expect("an alert");
        assert_eq!(rule.definition.name, "Slow model calls");
        assert_eq!(rule.definition.filters.model, ["claude-sonnet-5-5"]);
    }

    /// ⚠ Each measure folds only as the hourly totals can answer it.
    #[test]
    fn a_measure_is_folded_only_as_the_totals_can_answer() {
        for aggregation in RuleAggregation::all() {
            assert_eq!(
                RuleMeasure::Count.folds(aggregation),
                aggregation == RuleAggregation::Count
            );
            assert_eq!(
                RuleMeasure::Duration.folds(aggregation),
                !matches!(aggregation, RuleAggregation::Count | RuleAggregation::Max)
            );
            assert_eq!(
                RuleMeasure::Tokens.folds(aggregation),
                matches!(aggregation, RuleAggregation::Avg | RuleAggregation::Sum)
            );
            assert_eq!(
                RuleMeasure::Cost.folds(aggregation),
                matches!(
                    aggregation,
                    RuleAggregation::Avg | RuleAggregation::Sum | RuleAggregation::Max
                )
            );
        }
        let refused = RuleInput {
            aggregation: RuleAggregation::Count,
            ..input()
        };
        assert!(refused.checked().is_err());
    }

    /// ⚠ Hours hold on their days, at their offset, and run past midnight
    /// when they end before they start.
    #[test]
    fn hours_hold_on_their_days_at_their_offset() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .expect("an instant")
                .with_timezone(&Utc)
        };
        let time = |text: &str| text.parse::<TimeOfDay>().expect("a time of day");
        for refused in ["24:00", "9:00", "09:60", "0900", ""] {
            assert!(refused.parse::<TimeOfDay>().is_err(), "{refused}");
        }
        assert_eq!(time("09:05").to_string(), "09:05");
        let office = Hours {
            days: vec![1, 2, 3, 4, 5],
            from: time("09:00"),
            to: time("18:00"),
            utc_offset: 60,
        }
        .checked()
        .expect("hours");
        // Monday 2026-10-26, 08:30 UTC is 09:30 at UTC+1.
        assert!(office.hold(at("2026-10-26T08:30:00Z")));
        assert!(!office.hold(at("2026-10-26T07:30:00Z")));
        assert!(!office.hold(at("2026-10-25T08:30:00Z")), "a Sunday");
        let night = Hours {
            days: vec![5],
            from: time("22:00"),
            to: time("06:00"),
            utc_offset: 0,
        };
        assert!(night.hold(at("2026-10-30T23:00:00Z")), "Friday night");
        assert!(
            night.hold(at("2026-10-31T05:00:00Z")),
            "Saturday before dawn"
        );
        assert!(!night.hold(at("2026-10-31T07:00:00Z")));
        for refused in [
            Hours {
                days: vec![],
                ..night.clone()
            },
            Hours {
                days: vec![8],
                ..night.clone()
            },
            Hours {
                utc_offset: 900,
                ..night.clone()
            },
        ] {
            assert!(refused.checked().is_err());
        }
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
            RuleInput {
                measure: RuleMeasure::Cost,
                aggregation: RuleAggregation::Max,
                warning_threshold: None,
                ..input()
            },
        ] {
            assert!(refused.checked().is_err());
        }
    }

    /// The cost cap reads each run's calls as they land, so it filters as they do.
    #[test]
    fn the_cost_cap_is_a_runs_cost_filtered_as_its_calls_are() {
        let cap = || RuleInput {
            kind: SpanKind::Run,
            measure: RuleMeasure::Cost,
            aggregation: RuleAggregation::Max,
            filters: RuleFilters {
                agent: vec!["support-bot".into()],
                model: vec!["gpt-4o".into()],
                ..RuleFilters::default()
            },
            alert_threshold: 5.0,
            warning_threshold: Some(2.5),
            ..input()
        };
        assert!(cap().checked().is_ok());
        for refused in [
            RuleFilters {
                status: vec![SpanStatus::Error],
                ..RuleFilters::default()
            },
            RuleFilters {
                tool: vec!["search_docs".into()],
                ..RuleFilters::default()
            },
        ] {
            assert!(
                RuleInput {
                    filters: refused,
                    ..cap()
                }
                .checked()
                .is_err()
            );
        }
    }
}
