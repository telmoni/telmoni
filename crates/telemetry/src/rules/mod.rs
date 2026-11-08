//! Alerts, built as Langfuse's are: each a metric — a measure of the
//! project's spans of one kind, filtered by the fields the hourly totals
//! keep, folded over the last hour, day or week, a run's cost over the month —
//! against an alert threshold and an optional warning one under one operator. Kept in `telemetry.rules`
//! ([`db`]), made, edited, paused and deleted by any member of the project
//! ([`crate::handler::rules`]), and each evaluated a minute after the last by
//! a loop every replica runs ([`evaluate`]). An organization's alerts,
//! paused ones counted, are held to its plan's number
//! ([`telmoni_shared::seam::Entitlements::alert_ceilings`]). An objective is
//! one alert too, on its burn ([`Objective`]): its fast window pages, its
//! slow one warns.

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
    MissingData, RuleAggregation, RuleBaseline, RuleLevel, RuleMeasure, RuleOperator, RuleWindow,
    SpanKind, SpanStatus, TelmoniError,
};

use crate::contract::{
    AGENT_MAX, ENVIRONMENT_MAX, MODEL_MAX, NAME_MAX, PROVIDER_MAX, RELEASE_MAX, UTC_OFFSET_RANGE,
};
use crate::handler::traces::values;
use crate::store::Read;

/// The longest an alert's name is, in characters, as the table's CHECK has it.
pub const RULE_NAME_MAX: usize = 60;

/// The longest score name a score alert reads, as a score's own.
pub const SCORE_NAME_MAX: usize = crate::scores::NAME_MAX;

/// The longest re-notification period, in minutes: a week.
pub const RENOTIFY_MAX_MINUTES: i32 = 10_080;

/// The most connectors one alert links.
pub const LINKS_MAX: usize = 20;

/// The burn an objective's last hour pages at: 2% of a 30 days' budget gone
/// in an hour.
pub const FAST_BURN: f64 = 14.4;

/// The burn its last six hours warn at: 5% of a 30 days' budget gone in them.
pub const SLOW_BURN: f64 = 6.0;

/// The slow window an objective's burn is read over too.
pub const SLOW_WINDOW: std::time::Duration = std::time::Duration::from_secs(6 * 3_600);

/// The days an objective's budget is counted over.
pub const OBJECTIVE_DAYS: [u16; 3] = [7, 28, 30];

/// The lowest and highest share an objective promises: past the highest its
/// budget is too small for any window to read.
pub const TARGET_RANGE: std::ops::RangeInclusive<f64> = 0.5..=0.9999;

/// The longest a latency objective's runs are given, in milliseconds: an hour.
pub const LATENCY_MAX_MS: u32 = 3_600_000;

/// What one agent's runs promise: that `target` of them, a share, succeed,
/// or with `latency_ms`, end inside it, over the last `days`. A run that
/// does not spends its error budget, `1 - target` of them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Objective {
    pub target: f64,
    #[serde(default)]
    pub latency_ms: Option<u32>,
    pub days: u16,
}

impl Objective {
    /// The objective as the table keeps it, or why it cannot be one.
    fn checked(self) -> Result<Self, TelmoniError> {
        let bad = |why: String| TelmoniError::BadRequest(why);
        if !TARGET_RANGE.contains(&self.target) {
            return Err(bad(format!(
                "an objective promises {}% to {}% of its runs",
                TARGET_RANGE.start() * 100.0,
                TARGET_RANGE.end() * 100.0
            )));
        }
        if self
            .latency_ms
            .is_some_and(|ms| !(1..=LATENCY_MAX_MS).contains(&ms))
        {
            return Err(bad(format!(
                "a latency objective's runs end inside 1 to {LATENCY_MAX_MS} ms"
            )));
        }
        if !OBJECTIVE_DAYS.contains(&self.days) {
            return Err(bad("an objective is counted over 7, 28 or 30 days".into()));
        }
        Ok(self)
    }

    /// The share of runs it lets miss: its error budget.
    #[must_use]
    pub fn budget(&self) -> f64 {
        1.0 - self.target
    }

    /// How fast `bad` of `runs` spend the budget: 1 spends it in exactly its
    /// days; none spent where there was no run.
    #[must_use]
    pub fn burn(&self, runs: u64, bad: u64) -> f64 {
        if runs == 0 {
            return 0.0;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a count past 2^53 runs is no objective's"
        )]
        let share = bad as f64 / runs as f64;
        share / self.budget()
    }
}

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
    /// What a retrieval searched: a retrieval alert's alone, as `tool` is a
    /// tool call alert's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<SpanStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub release: Vec<String>,
    /// Tags, each `key:value`, that a run carries where any of its spans
    /// does: a cost alert's alone, read from the spans that keep them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tag: Vec<String>,
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
/// [`RuleFilters::bind`]; `tools` holds a retrieval alert's sources.
macro_rules! spans_admitted {
    () => {
        concat!(
            $crate::rules::calls_admitted!(),
            "AND (empty({tools:Array(String)}) OR (kind IN ('tool_call', 'retrieval') ",
            "AND name IN {tools:Array(String)})) ",
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
            .param("tools", self.names())
            .param("statuses", statuses)
    }

    /// The names a span's own name is held to: a tool call's tools, or a
    /// retrieval's sources, which no alert holds together.
    pub(crate) fn names(&self) -> &[String] {
        if self.tool.is_empty() {
            &self.source
        } else {
            &self.tool
        }
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
            source: values("source", &self.source, NAME_MAX)?,
            status,
            environment: values("environment", &self.environment, ENVIRONMENT_MAX)?,
            release: values("release", &self.release, RELEASE_MAX)?,
            tag: crate::handler::traces::tag_values(&self.tag)?,
        })
    }
}

/// An alert as a person writes it: a new one, or one saved over.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleInput {
    pub name: String,
    /// The spans it reads: runs, steps, model calls, tool calls or retrievals.
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
    /// A score alert's score, by its name.
    #[serde(default)]
    pub score_name: Option<String>,
    /// What its value is read against: itself, or the window before.
    #[serde(default)]
    pub baseline: RuleBaseline,
    /// A burn alert's objective; none for any other.
    #[serde(default)]
    pub objective: Option<Objective>,
    /// The project's connectors its notices go to; none, the feed alone.
    #[serde(default)]
    pub connectors: Vec<Link>,
}

/// What an alert is: the fourteen fields the table keeps, a person writes and
/// the lanes answer, spelled once. The row and the answer embed it.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Definition {
    pub name: String,
    /// The spans it reads: runs, steps, model calls, tool calls or retrievals.
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
    /// A score alert's score, by its name; none for any other.
    pub score_name: Option<String>,
    /// What its value is read against: itself, or the window before.
    pub baseline: RuleBaseline,
    /// A burn alert's objective; none for any other.
    #[sqlx(json(nullable))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<Objective>,
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
    /// folds ([`RuleMeasure::folds`](telmoni_shared::RuleMeasure::folds)); finite thresholds, a warning one met before the
    /// alert's as the operator reads them; and re-notification from a minute
    /// to a week. A burn alert is its objective's: one agent's runs, and
    /// its thresholds and windows [`FAST_BURN`] and [`SLOW_BURN`]'s.
    pub fn checked(mut self) -> Result<Rule, TelmoniError> {
        let bad = |why: &str| TelmoniError::BadRequest(why.to_owned());
        match (self.measure, self.objective.take()) {
            (RuleMeasure::Burn, Some(objective)) => {
                if !(self.filters.agent.len() == 1
                    && RuleFilters {
                        agent: Vec::new(),
                        ..self.filters.clone()
                    } == RuleFilters::default())
                {
                    return Err(bad(
                        "an objective is one agent's: it takes that filter alone",
                    ));
                }
                self = Self {
                    kind: SpanKind::Run,
                    aggregation: RuleAggregation::Avg,
                    window: RuleWindow::Hour,
                    operator: RuleOperator::Gte,
                    alert_threshold: FAST_BURN,
                    warning_threshold: Some(SLOW_BURN),
                    missing_data: MissingData::Zero,
                    score_name: None,
                    baseline: RuleBaseline::None,
                    objective: Some(objective.checked()?),
                    ..self
                };
            }
            (RuleMeasure::Burn, None) => return Err(bad("a burn alert names its objective")),
            (_, Some(_)) => return Err(bad("an objective is a burn alert's alone")),
            (_, None) => {}
        }
        let name = self.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > RULE_NAME_MAX {
            return Err(bad(&format!(
                "an alert's name is 1 to {RULE_NAME_MAX} characters"
            )));
        }
        if !self.kind.alertable() {
            return Err(bad(
                "an alert reads runs, steps, model calls, tool calls or retrievals",
            ));
        }
        if !self.measure.folds(self.aggregation) {
            return Err(bad(&format!(
                "{} is not folded by {}: count takes count; failures count or avg; duration \
                 avg, sum or a percentile; tokens avg or sum; cost avg, sum or max; score \
                 count, avg, max or a percentile; burn avg; steps avg, max or a percentile; \
                 repeats max; stalled count",
                self.measure, self.aggregation
            )));
        }
        if !self.measure.measured_on(self.kind) {
            return Err(bad(match self.measure {
                RuleMeasure::Tokens => "tokens are counted on model calls alone",
                RuleMeasure::Score => "a score alert is a run's: it reads every score of its name",
                RuleMeasure::Burn => "an objective is a run's",
                RuleMeasure::Steps | RuleMeasure::Repeats | RuleMeasure::Stalled => {
                    "a run's steps, its repeats and a stall are read run by run"
                }
                _ => "cost is counted per run, from its model calls",
            }));
        }
        let filters = self.filters.checked()?;
        let score_name = self.score_name.map(|given| given.trim().to_owned());
        if self.measure == RuleMeasure::Score {
            if !score_name
                .as_deref()
                .is_some_and(|given| (1..=SCORE_NAME_MAX).contains(&given.chars().count()))
            {
                return Err(bad(&format!(
                    "a score alert names its score, 1 to {SCORE_NAME_MAX} characters"
                )));
            }
            // A score keeps no agent, model or status of its own.
            if filters != RuleFilters::default() {
                return Err(bad(
                    "a score alert reads every score of its name: it takes no filter",
                ));
            }
        } else if score_name.is_some() {
            return Err(bad("a score's name is a score alert's alone"));
        }
        // An empty window of scores is no score of 0, nor is a change read where
        // a window holds nothing a change of 0: either would open an incident
        // for what never happened.
        let counted =
            self.aggregation == RuleAggregation::Count || self.measure == RuleMeasure::Count;
        if self.missing_data == MissingData::Zero
            && !counted
            && (self.measure == RuleMeasure::Score || self.baseline == RuleBaseline::Previous)
        {
            return Err(bad(
                "a window with no score, or no change to read, is no 0: show it as no data, or \
                 keep the alert's last severity",
            ));
        }
        if self.baseline == RuleBaseline::Previous && self.window == RuleWindow::Month {
            return Err(bad(
                "a month so far has no window of its length before it: read the hour, day or \
                 week against the one before",
            ));
        }
        if self.measure == RuleMeasure::Stalled && self.window == RuleWindow::Month {
            return Err(bad(
                "a stall is a run quiet past the window: an hour, a day or a week",
            ));
        }
        let run_by_run = matches!(
            self.measure,
            RuleMeasure::Steps | RuleMeasure::Repeats | RuleMeasure::Stalled
        );
        // Read as its spans land, before the run's own status is known.
        if run_by_run && !filters.status.is_empty() {
            return Err(bad(
                "a run's steps, its repeats and a stall are read as its spans land: filter \
                 them by agent, environment or release",
            ));
        }
        if self.measure == RuleMeasure::Cost {
            // Read from the calls as they land, before the run's own status is known.
            if !(filters.status.is_empty() && filters.tool.is_empty()) {
                return Err(bad(
                    "a run's cost is read from its model calls as they land: filter it by \
                     agent, model, provider, environment, release or tag",
                ));
            }
        } else if !filters.tag.is_empty() {
            // The hourly totals other measures read keep no tags.
            return Err(bad("a tag narrows a cost alert or a budget alone"));
        }
        // A field its spans never carry would hold the alert at nothing for ever.
        let calls = self.kind == SpanKind::ModelCall || self.measure == RuleMeasure::Cost;
        if !calls && !(filters.model.is_empty() && filters.provider.is_empty()) {
            return Err(bad(
                "model and provider filter model calls, or a run's cost by its calls",
            ));
        }
        if self.kind != SpanKind::ToolCall && !filters.tool.is_empty() {
            return Err(bad("tool filters tool calls alone"));
        }
        if self.kind != SpanKind::Retrieval && !filters.source.is_empty() {
            return Err(bad("source filters retrievals alone"));
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
        let connectors = checked_links(self.connectors, "an alert")?;
        Ok(Rule {
            connectors,
            definition: Definition {
                name,
                kind: self.kind,
                measure: self.measure,
                aggregation: self.aggregation,
                filters,
                window: self.window,
                operator: self.operator,
                alert_threshold: self.alert_threshold,
                warning_threshold: self.warning_threshold,
                missing_data: self.missing_data,
                renotify_minutes: self.renotify_minutes,
                score_name,
                baseline: self.baseline,
                objective: self.objective,
            },
        })
    }
}

/// The connectors an alert or a monitor (`whose`) links, as the table keeps
/// them: [`LINKS_MAX`] at most, each once, its hours checked.
pub(crate) fn checked_links(links: Vec<Link>, whose: &str) -> Result<Vec<Link>, TelmoniError> {
    let bad = |why: String| TelmoniError::BadRequest(why);
    if links.len() > LINKS_MAX {
        return Err(bad(format!("{whose} links up to {LINKS_MAX} connectors")));
    }
    let mut connectors: Vec<Link> = Vec::with_capacity(links.len());
    for link in links {
        if connectors
            .iter()
            .any(|kept| kept.connection_id == link.connection_id)
        {
            return Err(bad(format!("{whose} links a connector once")));
        }
        connectors.push(Link {
            hours: link.hours.map(Hours::checked).transpose()?,
            ..link
        });
    }
    Ok(connectors)
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
            score_name: None,
            baseline: RuleBaseline::None,
            objective: None,
            connectors: Vec::new(),
        }
    }

    /// An objective's alert takes its thresholds and window from its burn,
    /// whatever was sent, one agent alone, and checks the objective.
    #[test]
    fn an_objective_is_one_agents_burn_at_the_fixed_thresholds() {
        let objective = Objective {
            target: 0.99,
            latency_ms: None,
            days: 30,
        };
        let burn = |filters: RuleFilters, objective: Option<Objective>| RuleInput {
            measure: RuleMeasure::Burn,
            filters,
            objective,
            ..input()
        };
        let one = || RuleFilters {
            agent: vec!["support-bot".into()],
            ..RuleFilters::default()
        };
        let kept = burn(one(), Some(objective))
            .checked()
            .expect("an objective");
        let definition = &kept.definition;
        assert_eq!(
            (definition.kind, definition.window, definition.operator),
            (SpanKind::Run, RuleWindow::Hour, RuleOperator::Gte)
        );
        assert_eq!(
            (definition.alert_threshold, definition.warning_threshold),
            (FAST_BURN, Some(SLOW_BURN))
        );
        assert_eq!(definition.objective, Some(objective));
        assert!(burn(one(), None).checked().is_err(), "names its objective");
        assert!(
            burn(RuleFilters::default(), Some(objective))
                .checked()
                .is_err()
        );
        let two = RuleFilters {
            agent: vec!["a".into(), "b".into()],
            ..RuleFilters::default()
        };
        assert!(burn(two, Some(objective)).checked().is_err(), "one agent");
        let released = RuleFilters {
            release: vec!["v2".into()],
            ..one()
        };
        assert!(
            burn(released, Some(objective)).checked().is_err(),
            "that filter alone"
        );
        for refused in [
            Objective {
                target: 1.0,
                ..objective
            },
            Objective {
                target: 0.4,
                ..objective
            },
            Objective {
                days: 14,
                ..objective
            },
            Objective {
                latency_ms: Some(0),
                ..objective
            },
        ] {
            assert!(burn(one(), Some(refused)).checked().is_err(), "{refused:?}");
        }
        let given = RuleInput {
            objective: Some(objective),
            ..input()
        };
        assert!(given.checked().is_err(), "a burn alert's alone");
    }

    /// A burn is the share missed over the share allowed, none without runs.
    #[test]
    fn a_burn_is_the_share_missed_over_the_budget() {
        let objective = Objective {
            target: 0.99,
            latency_ms: None,
            days: 30,
        };
        assert!((objective.burn(1_000, 10) - 1.0).abs() < 1e-9);
        assert!((objective.burn(1_000, 144) - 14.4).abs() < 1e-9);
        assert!(objective.burn(0, 0).abs() < f64::EPSILON);
    }

    /// A score alert names its score and takes no filter, a run's; a name
    /// is a score alert's alone; and a month has no window before it.
    #[test]
    fn a_score_alert_names_its_score_and_a_baseline_a_trailing_window() {
        let score = |name: Option<&str>| RuleInput {
            kind: SpanKind::Run,
            measure: RuleMeasure::Score,
            aggregation: RuleAggregation::Avg,
            filters: RuleFilters::default(),
            operator: RuleOperator::Lt,
            alert_threshold: 0.5,
            warning_threshold: Some(0.7),
            score_name: name.map(str::to_owned),
            missing_data: MissingData::NoData,
            ..input()
        };
        let kept = score(Some(" relevance ")).checked().expect("a score alert");
        assert_eq!(kept.definition.score_name.as_deref(), Some("relevance"));
        assert!(score(None).checked().is_err(), "names its score");
        assert!(score(Some("  ")).checked().is_err());
        assert!(
            score(Some(&"n".repeat(SCORE_NAME_MAX + 1)))
                .checked()
                .is_err()
        );
        let filtered = RuleInput {
            filters: RuleFilters {
                agent: vec!["bot".into()],
                ..RuleFilters::default()
            },
            ..score(Some("relevance"))
        };
        assert!(filtered.checked().is_err(), "a score keeps no agent");
        let of_calls = RuleInput {
            kind: SpanKind::ModelCall,
            ..score(Some("relevance"))
        };
        assert!(of_calls.checked().is_err(), "a run's");
        let summed = RuleInput {
            aggregation: RuleAggregation::Sum,
            ..score(Some("relevance"))
        };
        assert!(summed.checked().is_err(), "a sum of scores says nothing");
        let named = RuleInput {
            score_name: Some("relevance".into()),
            ..input()
        };
        assert!(named.checked().is_err(), "a name is a score alert's alone");
        let dropped = RuleInput {
            baseline: RuleBaseline::Previous,
            window: RuleWindow::Day,
            operator: RuleOperator::Lt,
            alert_threshold: -20.0,
            warning_threshold: Some(-10.0),
            ..score(Some("relevance"))
        };
        assert!(
            dropped.clone().checked().is_ok(),
            "a drop of a fifth against the day before"
        );
        let monthly = RuleInput {
            window: RuleWindow::Month,
            ..dropped
        };
        assert!(
            monthly.checked().is_err(),
            "a month has no window of its length before it"
        );
        let zeroed = RuleInput {
            missing_data: MissingData::Zero,
            ..score(Some("relevance"))
        };
        assert!(
            zeroed.checked().is_err(),
            "an empty window of scores is no score of 0"
        );
        let counted = RuleInput {
            missing_data: MissingData::Zero,
            aggregation: RuleAggregation::Count,
            ..score(Some("relevance"))
        };
        assert!(counted.checked().is_ok(), "a count of none is 0");
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
            assert_eq!(
                RuleMeasure::Steps.folds(aggregation),
                !matches!(aggregation, RuleAggregation::Count | RuleAggregation::Sum)
            );
            assert_eq!(
                RuleMeasure::Repeats.folds(aggregation),
                aggregation == RuleAggregation::Max
            );
            assert_eq!(
                RuleMeasure::Stalled.folds(aggregation),
                aggregation == RuleAggregation::Count
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

    /// A run's steps, repeats and stalls are a run's, read as its spans land,
    /// and a stall over a window of a fixed length.
    #[test]
    fn steps_repeats_and_stalls_are_read_run_by_run() {
        let of = |measure, aggregation| RuleInput {
            kind: SpanKind::Run,
            measure,
            aggregation,
            filters: RuleFilters {
                agent: vec!["support-agent".into()],
                release: vec!["v2".into()],
                ..RuleFilters::default()
            },
            alert_threshold: 12.0,
            warning_threshold: Some(8.0),
            ..input()
        };
        for kept in [
            of(RuleMeasure::Steps, RuleAggregation::P95),
            of(RuleMeasure::Steps, RuleAggregation::Max),
            of(RuleMeasure::Repeats, RuleAggregation::Max),
            of(RuleMeasure::Stalled, RuleAggregation::Count),
        ] {
            assert!(kept.clone().checked().is_ok(), "{kept:?}");
        }
        let monthly = RuleInput {
            window: RuleWindow::Month,
            ..of(RuleMeasure::Stalled, RuleAggregation::Count)
        };
        assert!(monthly.checked().is_err(), "a stall's window has a length");
        let weekly = RuleInput {
            window: RuleWindow::Week,
            ..of(RuleMeasure::Stalled, RuleAggregation::Count)
        };
        assert!(weekly.checked().is_ok());
        for measure in [
            RuleMeasure::Steps,
            RuleMeasure::Repeats,
            RuleMeasure::Stalled,
        ] {
            let aggregation = if measure == RuleMeasure::Stalled {
                RuleAggregation::Count
            } else {
                RuleAggregation::Max
            };
            for refused in [
                RuleFilters {
                    status: vec![SpanStatus::Error],
                    ..RuleFilters::default()
                },
                RuleFilters {
                    tool: vec!["search".into()],
                    ..RuleFilters::default()
                },
                RuleFilters {
                    model: vec!["gpt-4o".into()],
                    ..RuleFilters::default()
                },
                RuleFilters {
                    tag: vec!["team:support".into()],
                    ..RuleFilters::default()
                },
            ] {
                let filtered = RuleInput {
                    filters: refused,
                    ..of(measure, aggregation)
                };
                assert!(filtered.checked().is_err(), "{measure}");
            }
            let of_steps = RuleInput {
                kind: SpanKind::Step,
                ..of(measure, aggregation)
            };
            assert!(of_steps.checked().is_err(), "{measure} is a run's");
        }
    }

    /// A filter names a field the alert's spans carry, as the form offers them.
    #[test]
    fn a_filter_names_a_field_its_spans_carry() {
        let tools = || RuleFilters {
            tool: vec!["search_docs".into()],
            ..RuleFilters::default()
        };
        let calls = RuleInput {
            kind: SpanKind::ToolCall,
            filters: tools(),
            ..input()
        };
        assert!(calls.checked().is_ok());
        let sources = || RuleFilters {
            source: vec![" docs-index ".into()],
            ..RuleFilters::default()
        };
        let retrievals = RuleInput {
            kind: SpanKind::Retrieval,
            filters: sources(),
            ..input()
        };
        let checked = retrievals
            .checked()
            .expect("a retrieval alert takes its sources");
        assert_eq!(
            checked.definition.filters.names(),
            ["docs-index".to_owned()]
        );
        assert_eq!(tools().names(), ["search_docs".to_owned()]);
        for refused in [
            RuleInput {
                kind: SpanKind::ToolCall,
                filters: sources(),
                ..input()
            },
            RuleInput {
                kind: SpanKind::Run,
                filters: sources(),
                ..input()
            },
            RuleInput {
                kind: SpanKind::Retrieval,
                filters: tools(),
                ..input()
            },
            RuleInput {
                kind: SpanKind::Retrieval,
                filters: RuleFilters {
                    tool: tools().tool,
                    ..sources()
                },
                ..input()
            },
            RuleInput {
                kind: SpanKind::Step,
                ..input()
            },
            RuleInput {
                kind: SpanKind::Run,
                ..input()
            },
            RuleInput {
                kind: SpanKind::Step,
                filters: RuleFilters {
                    provider: vec!["anthropic".into()],
                    ..RuleFilters::default()
                },
                ..input()
            },
            RuleInput {
                filters: tools(),
                ..input()
            },
            RuleInput {
                kind: SpanKind::Run,
                filters: tools(),
                ..input()
            },
        ] {
            assert!(refused.checked().is_err());
        }
    }
}
