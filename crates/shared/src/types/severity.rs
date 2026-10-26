//! [`Severity`] — where an alert stands at its last evaluation.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// Where an alert stands: `unknown` until its first evaluation, then `ok`,
/// `warning` or `alert` as its value crosses its thresholds, `no_data` where
/// its window held no span and it asks to be told so, and `paused` while
/// it is not evaluated. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Not evaluated yet.
    #[default]
    Unknown,
    /// Inside both thresholds.
    Ok,
    /// Past the warning threshold, short of the alert's.
    Warning,
    /// Past the alert threshold.
    Alert,
    /// The window held no span, read as no data.
    NoData,
    /// Not evaluated: paused by a person or by its plan.
    Paused,
}

impl Severity {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [
            Self::Unknown,
            Self::Ok,
            Self::Warning,
            Self::Alert,
            Self::NoData,
            Self::Paused,
        ]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Alert => "alert",
            Self::NoData => "no_data",
            Self::Paused => "paused",
        }
    }

    /// Whether the alert stands past a threshold: what an incident opens on.
    #[must_use]
    pub const fn is_breach(self) -> bool {
        matches!(self, Self::Warning | Self::Alert)
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Severity {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unknown" => Ok(Self::Unknown),
            "ok" => Ok(Self::Ok),
            "warning" => Ok(Self::Warning),
            "alert" => Ok(Self::Alert),
            "no_data" => Ok(Self::NoData),
            "paused" => Ok(Self::Paused),
            _ => Err(ParseEnumError::new(
                s,
                "unknown, ok, warning, alert, no_data, paused",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Severity; 6] = Severity::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                Severity::Unknown
                | Severity::Ok
                | Severity::Warning
                | Severity::Alert
                | Severity::NoData
                | Severity::Paused => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<Severity>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["OK", "breached", ""] {
            assert!(given.parse::<Severity>().is_err(), "parsed: {given:?}");
        }
    }
}
