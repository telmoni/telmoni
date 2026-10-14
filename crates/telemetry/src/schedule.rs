//! A monitor's timing: when its runs are expected — a five-field cron line in
//! the monitor's own zone, or an interval, or neither — and when the next one
//! is due. The line is read as Vixie cron reads it, so what a crontab or
//! crontab.guru means, this means: five fields and never seconds, the
//! `@daily` nicknames, and day-of-month *or* day-of-week where both are set.
//! The zone is any the zone database names, `Europe/London`, and the next due
//! time is found in it, so a monitor due at nine stays due at nine across a
//! clock change.

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use croner::Cron;
use croner::errors::CronError;
use croner::parser::{CronParser, Seconds, Year};

/// The shortest interval, in seconds: a minute, a crontab's own grain.
pub const INTERVAL_MIN_S: i32 = 60;
/// The longest an interval, a grace, an expected duration or a stall timeout
/// is, in seconds: a year and a day, past which a number is a mistake.
pub const PERIOD_MAX_S: i32 = 366 * 24 * 60 * 60;
/// The longest a cron line is, in characters: room for every field's list,
/// and a bound on what the parser is handed.
pub const CRON_LINE_MAX: usize = 100;
/// How many due times a schedule is shown as, as it is typed.
pub const PREVIEW: usize = 3;

/// What a timing could not be read as, named by its field and never by its
/// value: the sentence a 400 carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// `schedule` and `interval_s` were both sent.
    BothTimings,
    /// `schedule` is no five-field cron line.
    Schedule,
    /// `schedule` names a time that never comes.
    NeverDue,
    /// `interval_s` is under a minute or over a year.
    Interval,
    /// `timezone` is no zone the zone database names.
    Timezone,
}

impl Fault {
    /// The field, and the rule it broke.
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::BothTimings => {
                "a monitor's schedule and interval_s are one or the other, never both"
            }
            Self::Schedule => {
                "a monitor's schedule is a five-field cron line, as a crontab's is, up to 100 \
                 characters"
            }
            Self::NeverDue => "a monitor's schedule names a time that never comes",
            Self::Interval => {
                "a monitor's interval_s is whole seconds, from a minute (60) to a year and a day \
                 (31622400)"
            }
            Self::Timezone => {
                "a monitor's timezone is a zone the zone database names, such as Europe/London"
            }
        }
    }
}

/// When a monitor's runs are expected.
#[derive(Debug, Clone)]
pub enum Timing {
    /// Never: the monitor holds its runs to their bounds and expects none.
    Never,
    /// Every so many seconds.
    Every(i32),
    /// A five-field cron line, read in the schedule's zone; boxed, since a
    /// parsed line is many times the size of an interval.
    Cron(Box<Cron>),
}

/// A monitor's timing in its zone.
#[derive(Debug, Clone)]
pub struct Schedule {
    pub timing: Timing,
    pub zone: Tz,
}

impl Schedule {
    /// A schedule as a monitor is sent one: a cron line or an interval, or
    /// neither, and a zone name, each checked.
    pub fn read(
        schedule: Option<&str>,
        interval_s: Option<i32>,
        timezone: &str,
    ) -> Result<Self, Fault> {
        let zone = parse_zone(timezone)?;
        let timing = match (schedule, interval_s) {
            (Some(_), Some(_)) => return Err(Fault::BothTimings),
            (Some(line), None) => Timing::Cron(Box::new(parse_cron(line)?)),
            (None, Some(every)) => {
                if !(INTERVAL_MIN_S..=PERIOD_MAX_S).contains(&every) {
                    return Err(Fault::Interval);
                }
                Timing::Every(every)
            }
            (None, None) => Timing::Never,
        };
        Ok(Self { timing, zone })
    }

    /// Whether the monitor expects its runs at all.
    #[must_use]
    pub const fn is_scheduled(&self) -> bool {
        !matches!(self.timing, Timing::Never)
    }

    /// When the next run is due, after `after`: `None` for a monitor that
    /// expects none. A cron line that never matches is refused here rather
    /// than at parse, since only the search finds it out.
    pub fn next_due(&self, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, Fault> {
        match &self.timing {
            Timing::Never => Ok(None),
            Timing::Every(every) => Ok(Some(after + Duration::seconds(i64::from(*every)))),
            Timing::Cron(cron) => {
                let zoned = after.with_timezone(&self.zone);
                let next =
                    cron.find_next_occurrence(&zoned, false)
                        .map_err(|error| match error {
                            CronError::TimeSearchLimitExceeded => Fault::NeverDue,
                            _ => Fault::Schedule,
                        })?;
                Ok(Some(next.with_timezone(&Utc)))
            }
        }
    }

    /// The next `count` due times after `after`: none for a monitor that
    /// expects no run.
    pub fn next_due_times(
        &self,
        after: DateTime<Utc>,
        count: usize,
    ) -> Result<Vec<DateTime<Utc>>, Fault> {
        let mut times = Vec::with_capacity(count);
        let mut from = after;
        for _ in 0..count {
            let Some(next) = self.next_due(from)? else {
                break;
            };
            times.push(next);
            from = next;
        }
        Ok(times)
    }
}

/// A zone by its name in the zone database, case and all.
pub fn parse_zone(name: &str) -> Result<Tz, Fault> {
    name.parse().map_err(|_| Fault::Timezone)
}

/// A five-field cron line, as Vixie cron reads it: no seconds and no year,
/// day-of-month or day-of-week where both are set, the `@daily` nicknames,
/// and the ranges a crontab takes.
pub fn parse_cron(line: &str) -> Result<Cron, Fault> {
    if line.is_empty() || line.chars().count() > CRON_LINE_MAX {
        return Err(Fault::Schedule);
    }
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(line)
        .map_err(|_| Fault::Schedule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("a time")
            .with_timezone(&Utc)
    }

    /// What a crontab takes, this takes: five fields, lists, steps, names and
    /// nicknames; what it does not — seconds, a sixth field — is refused.
    #[test]
    fn a_crontab_line_is_read_and_a_six_field_one_is_refused() {
        for line in [
            "0 2 * * *",
            "*/15 * * * *",
            "0 9-17 * * MON-FRI",
            "30 4 1,15 * *",
            "0 0 1 JAN *",
            "@daily",
            "@hourly",
            "0 0 * * 7",
        ] {
            assert!(parse_cron(line).is_ok(), "{line}");
        }
        for line in [
            "",
            "0 0 2 * * *",
            "60 * * * *",
            "* * * *",
            "every day at nine",
            "0 0 * * * 2027",
            &"*".repeat(CRON_LINE_MAX + 1),
        ] {
            assert_eq!(parse_cron(line), Err(Fault::Schedule), "{line:?}");
        }
    }

    /// A zone is read by its name in the database, as spelled there.
    #[test]
    fn a_zone_is_read_by_its_name() {
        assert!(parse_zone("Europe/London").is_ok());
        assert!(parse_zone("UTC").is_ok());
        assert!(parse_zone("America/Argentina/Buenos_Aires").is_ok());
        for name in ["", "europe/london", "GMT+1", "London", "Europe/Atlantis"] {
            assert_eq!(parse_zone(name), Err(Fault::Timezone), "{name:?}");
        }
    }

    /// The next due time is found in the schedule's zone: nine in London is
    /// eight in UTC in summer and nine in winter.
    #[test]
    fn the_next_due_time_is_found_in_the_zone() {
        let schedule =
            Schedule::read(Some("0 9 * * *"), None, "Europe/London").expect("a schedule");
        assert_eq!(
            schedule.next_due(at("2026-07-01T00:00:00Z")).expect("due"),
            Some(at("2026-07-01T08:00:00Z"))
        );
        assert_eq!(
            schedule.next_due(at("2026-12-01T00:00:00Z")).expect("due"),
            Some(at("2026-12-01T09:00:00Z"))
        );
        // Due at this very minute means the next one: the minute is not
        // counted twice.
        assert_eq!(
            schedule.next_due(at("2026-12-01T09:00:00Z")).expect("due"),
            Some(at("2026-12-02T09:00:00Z"))
        );
        assert_eq!(
            schedule
                .next_due_times(at("2026-12-01T00:00:00Z"), 3)
                .expect("due"),
            [
                at("2026-12-01T09:00:00Z"),
                at("2026-12-02T09:00:00Z"),
                at("2026-12-03T09:00:00Z"),
            ]
        );
    }

    /// An interval is due so many seconds on; a monitor with neither expects
    /// no run and has no due time.
    #[test]
    fn an_interval_and_no_timing_at_all() {
        let every = Schedule::read(None, Some(900), "UTC").expect("a schedule");
        assert!(every.is_scheduled());
        assert_eq!(
            every.next_due(at("2026-10-14T10:00:00Z")).expect("due"),
            Some(at("2026-10-14T10:15:00Z"))
        );
        assert_eq!(
            every
                .next_due_times(at("2026-10-14T10:00:00Z"), 2)
                .expect("due"),
            [at("2026-10-14T10:15:00Z"), at("2026-10-14T10:30:00Z")]
        );
        let never = Schedule::read(None, None, "UTC").expect("a schedule");
        assert!(!never.is_scheduled());
        assert_eq!(
            never.next_due(at("2026-10-14T10:00:00Z")).expect("due"),
            None
        );
        assert!(
            never
                .next_due_times(at("2026-10-14T10:00:00Z"), 3)
                .expect("due")
                .is_empty()
        );
    }

    /// Each refusal names its field: both timings, an interval out of range,
    /// a zone that is none, and a line that never comes.
    #[test]
    fn each_refusal_names_its_field() {
        assert_eq!(
            Schedule::read(Some("0 2 * * *"), Some(60), "UTC").err(),
            Some(Fault::BothTimings)
        );
        for every in [0, 59, -60, PERIOD_MAX_S + 1] {
            assert_eq!(
                Schedule::read(None, Some(every), "UTC").err(),
                Some(Fault::Interval)
            );
        }
        assert_eq!(
            Schedule::read(None, Some(60), "Mars/Olympus").err(),
            Some(Fault::Timezone)
        );
        let never = Schedule::read(Some("0 0 30 2 *"), None, "UTC").expect("a line that parses");
        assert_eq!(
            never.next_due(at("2026-10-14T10:00:00Z")),
            Err(Fault::NeverDue)
        );
        for fault in [
            Fault::BothTimings,
            Fault::Schedule,
            Fault::NeverDue,
            Fault::Interval,
            Fault::Timezone,
        ] {
            assert!(fault.detail().starts_with("a monitor's "), "{fault:?}");
        }
    }
}
