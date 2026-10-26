//! [`RuleWindow`] — how far back an alert reads.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// How far back each evaluation of an alert reads, from the minute it
/// runs: the last hour, day or week. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RuleWindow {
    /// The last hour.
    Hour,
    /// The last 24 hours.
    Day,
    /// The last seven days.
    Week,
}

impl RuleWindow {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Hour, Self::Day, Self::Week]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Week => "week",
        }
    }

    /// How long the window is.
    #[must_use]
    pub const fn duration(self) -> std::time::Duration {
        match self {
            Self::Hour => std::time::Duration::from_secs(3_600),
            Self::Day => std::time::Duration::from_secs(86_400),
            Self::Week => std::time::Duration::from_secs(7 * 86_400),
        }
    }
}

impl fmt::Display for RuleWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RuleWindow {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hour" => Ok(Self::Hour),
            "day" => Ok(Self::Day),
            "week" => Ok(Self::Week),
            _ => Err(ParseEnumError::new(s, "hour, day, week")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [RuleWindow; 3] = RuleWindow::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                RuleWindow::Hour | RuleWindow::Day | RuleWindow::Week => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<RuleWindow>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["1h", "month", ""] {
            assert!(given.parse::<RuleWindow>().is_err(), "parsed: {given:?}");
        }
    }
}
