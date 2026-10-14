//! [`MonitorState`] — closed vocabulary of what a monitor is in, the worst
//! first.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// What a monitor is in: the stable string its row carries, in the order
/// the Monitors page sorts by and the status board shows — the worst first,
/// so a list ordered by it puts what needs someone at the top. `[PUBLIC-API]`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, sqlx::Type, Serialize, Deserialize,
)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MonitorState {
    /// Its last run failed, or a run it expected never came within its grace.
    Failed,
    /// Its open run went quiet past its stall timeout.
    Stalled,
    /// Its open run crossed its step cap or its cost cap.
    OverBound,
    /// A run it expected has not started, and its grace is running out.
    Late,
    /// A run is going.
    Running,
    /// Made by its first ping or run, before its owner set it up; paused
    /// until they do.
    New,
    /// Paused by its owner: never late, its runs still kept.
    Paused,
    /// Its last run ended well, and nothing it expected is missing.
    Healthy,
}

impl MonitorState {
    /// Every state, in wire order: the worst first.
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::Failed,
            Self::Stalled,
            Self::OverBound,
            Self::Late,
            Self::Running,
            Self::New,
            Self::Paused,
            Self::Healthy,
        ]
    }

    /// The stable string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Stalled => "stalled",
            Self::OverBound => "over_bound",
            Self::Late => "late",
            Self::Running => "running",
            Self::New => "new",
            Self::Paused => "paused",
            Self::Healthy => "healthy",
        }
    }
}

impl fmt::Display for MonitorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MonitorState {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "failed" => Ok(Self::Failed),
            "stalled" => Ok(Self::Stalled),
            "over_bound" => Ok(Self::OverBound),
            "late" => Ok(Self::Late),
            "running" => Ok(Self::Running),
            "new" => Ok(Self::New),
            "paused" => Ok(Self::Paused),
            "healthy" => Ok(Self::Healthy),
            _ => Err(ParseEnumError::new(
                s,
                "failed, stalled, over_bound, late, running, new, paused, healthy",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [MonitorState; 8] = MonitorState::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for state in ALL {
            match state {
                MonitorState::Failed
                | MonitorState::Stalled
                | MonitorState::OverBound
                | MonitorState::Late
                | MonitorState::Running
                | MonitorState::New
                | MonitorState::Paused
                | MonitorState::Healthy => {}
            }
        }
        let mut seen: Vec<String> = ALL.iter().map(ToString::to_string).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len(), "all() repeats a variant");
    }

    /// The worst first: what a list sorted by state puts at the top.
    #[test]
    fn the_order_is_the_worst_first() {
        assert!(MonitorState::Failed < MonitorState::Healthy);
        assert!(MonitorState::Late < MonitorState::Running);
        let mut sorted = ALL;
        sorted.sort();
        assert_eq!(sorted, ALL, "all() is in the sorting order");
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in &ALL {
            assert_eq!(v.to_string().parse::<MonitorState>().unwrap(), *v);
        }
    }

    #[test]
    fn from_str_rejects_other_spellings() {
        for s in ["Failed", "over-bound", "overbound", "ok", "down", ""] {
            assert!(s.parse::<MonitorState>().is_err(), "parsed: {s:?}");
        }
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
