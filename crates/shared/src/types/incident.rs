//! [`IncidentDetail`] and [`IncidentResolution`] — closed vocabularies of
//! what opened a monitor's incident, finer than the state it put the monitor
//! in, and how it closed.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// What opened an incident: the stable string its row carries and its
/// notice names. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentDetail {
    /// Its run ended in error.
    RunFailed,
    /// A run it expected never came within its grace.
    RunNeverCame,
    /// Its run never said it ended, and was closed at its ceiling.
    RunNeverEnded,
    /// A run it expected has not started, and its grace is running out.
    RunLate,
    /// Its run went quiet past its stall timeout.
    RunStalled,
    /// Its run crossed its step cap or its cost cap.
    RunOverBound,
}

impl IncidentDetail {
    /// Every detail, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [
            Self::RunFailed,
            Self::RunNeverCame,
            Self::RunNeverEnded,
            Self::RunLate,
            Self::RunStalled,
            Self::RunOverBound,
        ]
    }

    /// The stable string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunFailed => "run_failed",
            Self::RunNeverCame => "run_never_came",
            Self::RunNeverEnded => "run_never_ended",
            Self::RunLate => "run_late",
            Self::RunStalled => "run_stalled",
            Self::RunOverBound => "run_over_bound",
        }
    }
}

impl fmt::Display for IncidentDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IncidentDetail {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::all()
            .into_iter()
            .find(|detail| detail.as_str() == s)
            .ok_or_else(|| {
                ParseEnumError::new(
                    s,
                    "run_failed, run_never_came, run_never_ended, run_late, run_stalled, \
                     run_over_bound",
                )
            })
    }
}

/// How an incident closed: the monitor healthy again, or paused by its
/// owner. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentResolution {
    /// The monitor came back healthy.
    Recovered,
    /// The monitor's owner paused it.
    Paused,
}

impl IncidentResolution {
    /// Every resolution, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::Recovered, Self::Paused]
    }

    /// The stable string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recovered => "recovered",
            Self::Paused => "paused",
        }
    }
}

impl fmt::Display for IncidentResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IncidentResolution {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::all()
            .into_iter()
            .find(|resolution| resolution.as_str() == s)
            .ok_or_else(|| ParseEnumError::new(s, "recovered, paused"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for detail in IncidentDetail::all() {
            match detail {
                IncidentDetail::RunFailed
                | IncidentDetail::RunNeverCame
                | IncidentDetail::RunNeverEnded
                | IncidentDetail::RunLate
                | IncidentDetail::RunStalled
                | IncidentDetail::RunOverBound => {}
            }
        }
        for resolution in IncidentResolution::all() {
            match resolution {
                IncidentResolution::Recovered | IncidentResolution::Paused => {}
            }
        }
    }

    #[test]
    fn every_word_round_trips_through_serde_and_from_str() {
        for detail in IncidentDetail::all() {
            assert_eq!(
                serde_json::to_value(detail).unwrap(),
                serde_json::json!(detail.as_str())
            );
            assert_eq!(detail.as_str().parse::<IncidentDetail>().unwrap(), detail);
        }
        for resolution in IncidentResolution::all() {
            assert_eq!(
                serde_json::to_value(resolution).unwrap(),
                serde_json::json!(resolution.as_str())
            );
            assert_eq!(
                resolution.as_str().parse::<IncidentResolution>().unwrap(),
                resolution
            );
        }
        assert!("RunFailed".parse::<IncidentDetail>().is_err());
        assert!("healthy".parse::<IncidentResolution>().is_err());
    }
}
