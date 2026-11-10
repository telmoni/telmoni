//! A monitor's schedule: a period, or a five-field cron line read as Vixie
//! cron reads it — never seconds, the `@daily` nicknames, and day-of-month
//! *or* day-of-week where both are set — in an IANA zone, so a trace due at
//! nine stays due at nine across a clock change. A fixed time a spring
//! change skips is due at the first minute after it, and one an autumn change
//! repeats is due once, the first time (croner's rules).

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use croner::Cron;
use croner::errors::CronError;
use croner::parser::{CronParser, Seconds, Year};

/// The shortest period and grace, in seconds: a minute, a crontab's grain.
pub const SECONDS_MIN: i32 = 60;
/// The longest period and grace, in seconds: 31 days.
pub const SECONDS_MAX: i32 = 31 * 24 * 60 * 60;
/// The longest cron line, in characters.
pub const CRON_MAX: usize = 100;

/// What a schedule could not be read as, named by its field and never by its
/// value: the sentence a 400 carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// `cron` is no five-field cron line.
    Cron,
    /// `cron` names a time that never comes.
    NeverDue,
    /// `every_seconds` is under a minute or over 31 days.
    Every,
    /// `grace_seconds` is under a minute or over 31 days.
    Grace,
    /// `time_zone` is no zone the zone database names.
    TimeZone,
}

impl Fault {
    /// The field, and the rule it broke.
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Cron => {
                "a monitor's cron is a five-field cron line, as a crontab's is, up to 100 characters"
            }
            Self::NeverDue => "a monitor's cron names a time that never comes",
            Self::Every => {
                "a monitor's every_seconds is whole seconds, from a minute (60) to 31 days (2678400)"
            }
            Self::Grace => {
                "a monitor's grace_seconds is whole seconds, from a minute (60) to 31 days (2678400)"
            }
            Self::TimeZone => {
                "a monitor's time_zone is a zone the zone database names, such as Europe/London"
            }
        }
    }
}

/// When an agent's traces are due.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// Every so many seconds after the last trace started.
    Every(i32),
    /// At each time a cron line names, read in its zone; boxed, since a parsed
    /// line is many times the size of a period.
    Cron { line: Box<Cron>, zone: Tz },
}

impl Schedule {
    /// A period, checked.
    pub fn every(seconds: i32) -> Result<Self, Fault> {
        if !(SECONDS_MIN..=SECONDS_MAX).contains(&seconds) {
            return Err(Fault::Every);
        }
        Ok(Self::Every(seconds))
    }

    /// A cron line in a zone, each checked, and refused where it never comes.
    pub fn cron(line: &str, zone: &str) -> Result<Self, Fault> {
        let zone: Tz = zone.parse().map_err(|_| Fault::TimeZone)?;
        let schedule = Self::Cron {
            line: Box::new(parse_cron(line)?),
            zone,
        };
        schedule.next_after(Utc::now())?;
        Ok(schedule)
    }

    /// The first time due after `after`, strictly: a trace at its due minute
    /// counts for it, and the next is the one after.
    pub fn next_after(&self, after: DateTime<Utc>) -> Result<DateTime<Utc>, Fault> {
        match self {
            Self::Every(seconds) => Ok(after + TimeDelta::seconds(i64::from(*seconds))),
            Self::Cron { line, zone } => line
                .find_next_occurrence(&after.with_timezone(zone), false)
                .map(|next| next.with_timezone(&Utc))
                .map_err(|error| match error {
                    CronError::TimeSearchLimitExceeded => Fault::NeverDue,
                    _ => Fault::Cron,
                }),
        }
    }
}

/// A grace, checked.
pub fn grace(seconds: i32) -> Result<Duration, Fault> {
    if !(SECONDS_MIN..=SECONDS_MAX).contains(&seconds) {
        return Err(Fault::Grace);
    }
    Ok(Duration::from_secs(u64::from(seconds.unsigned_abs())))
}

/// A five-field cron line, as Vixie cron reads it: no seconds and no year.
fn parse_cron(line: &str) -> Result<Cron, Fault> {
    if line.trim().is_empty() || line.chars().count() > CRON_MAX {
        return Err(Fault::Cron);
    }
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(line)
        .map_err(|_| Fault::Cron)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("a time")
            .with_timezone(&Utc)
    }

    /// What a crontab takes, this takes; seconds, a year or prose are refused.
    #[test]
    fn a_crontab_line_is_read_and_anything_else_refused() {
        for line in [
            "0 2 * * *",
            "*/15 * * * *",
            "0 9-17 * * MON-FRI",
            "30 4 1,15 * *",
            "@daily",
            "0 0 * * 7",
        ] {
            assert!(Schedule::cron(line, "UTC").is_ok(), "{line}");
        }
        for line in [
            "",
            "0 0 2 * * *",
            "60 * * * *",
            "* * * *",
            "every day at nine",
            &"*".repeat(CRON_MAX + 1),
        ] {
            assert_eq!(
                Schedule::cron(line, "UTC").err(),
                Some(Fault::Cron),
                "{line:?}"
            );
        }
        assert_eq!(
            Schedule::cron("0 0 30 2 *", "UTC").err(),
            Some(Fault::NeverDue)
        );
    }

    /// A zone is read by its exact name in the database.
    #[test]
    fn a_zone_is_read_by_its_name() {
        for name in ["Europe/London", "UTC", "America/Argentina/Buenos_Aires"] {
            assert!(Schedule::cron("0 2 * * *", name).is_ok(), "{name}");
        }
        for name in ["", "europe/london", "GMT+1", "London"] {
            assert_eq!(
                Schedule::cron("0 2 * * *", name).err(),
                Some(Fault::TimeZone),
                "{name:?}"
            );
        }
    }

    /// Periods and graces run from a minute to 31 days.
    #[test]
    fn periods_and_graces_are_bounded() {
        assert!(Schedule::every(60).is_ok());
        assert!(Schedule::every(SECONDS_MAX).is_ok());
        for seconds in [0, 59, -60, SECONDS_MAX + 1] {
            assert_eq!(Schedule::every(seconds).err(), Some(Fault::Every));
            assert_eq!(grace(seconds).err(), Some(Fault::Grace));
        }
        assert_eq!(grace(90), Ok(Duration::from_secs(90)));
        let every = Schedule::every(900).expect("a period");
        assert_eq!(
            every.next_after(at("2026-10-14T10:00:00Z")),
            Ok(at("2026-10-14T10:15:00Z"))
        );
    }

    /// Nine in London is eight UTC in summer and nine in winter, and a trace at
    /// its due minute makes the next one due.
    #[test]
    fn the_next_due_time_is_found_in_the_zone() {
        let nine = Schedule::cron("0 9 * * *", "Europe/London").expect("a schedule");
        assert_eq!(
            nine.next_after(at("2026-07-01T00:00:00Z")),
            Ok(at("2026-07-01T08:00:00Z"))
        );
        assert_eq!(
            nine.next_after(at("2026-12-01T09:00:00Z")),
            Ok(at("2026-12-02T09:00:00Z"))
        );
    }

    /// Across London's clock changes: 01:30 skipped in spring is due at the
    /// first minute after the gap, and repeated in autumn is due once.
    #[test]
    fn a_clock_change_moves_a_fixed_time_once() {
        let half_one = Schedule::cron("30 1 * * *", "Europe/London").expect("a schedule");
        assert_eq!(
            half_one.next_after(at("2027-03-27T12:00:00Z")),
            Ok(at("2027-03-28T01:00:00Z")),
            "spring: 02:00 BST, the first minute after the gap"
        );
        assert_eq!(
            half_one.next_after(at("2026-10-24T12:00:00Z")),
            Ok(at("2026-10-25T00:30:00Z")),
            "autumn: 01:30 BST, the first of the two"
        );
        assert_eq!(
            half_one.next_after(at("2026-10-25T00:30:05Z")),
            Ok(at("2026-10-26T01:30:00Z")),
            "the repeated 01:30 GMT is not due again"
        );
    }
}
