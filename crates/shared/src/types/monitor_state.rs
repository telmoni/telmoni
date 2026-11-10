//! [`MonitorState`] and [`MonitorCause`] — where a monitor stands, and why.

use serde::{Deserialize, Serialize};

use crate::Severity;

/// Where a monitor stands: `new` until its agent's first run since it was
/// made or resumed, then `up`, `late` once a trace due has not started, and
/// `down` past its grace or after a trace that failed; `paused` while it is
/// not judged. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MonitorState {
    /// No run expected yet.
    #[default]
    New,
    /// Its runs come when due, and the last ended well.
    Up,
    /// A run due has not started.
    Late,
    /// A run due has not started within its grace, or the last one failed.
    Down,
    /// Not judged, by a member's hand.
    Paused,
}

crate::wire_enum!(MonitorState {
    New => "new",
    Up => "up",
    Late => "late",
    Down => "down",
    Paused => "paused",
});

impl MonitorState {
    /// The alert severity it reads as, which its incident takes: none where
    /// nothing is expected of it.
    #[must_use]
    pub const fn severity(self) -> Option<Severity> {
        match self {
            Self::Up => Some(Severity::Ok),
            Self::Late => Some(Severity::Warning),
            Self::Down => Some(Severity::Alert),
            Self::New | Self::Paused => None,
        }
    }
}

/// Why a monitor is `late` or `down`. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MonitorCause {
    /// A run due has not started.
    Missed,
    /// Its last run ended in error.
    Failed,
}

crate::wire_enum!(MonitorCause {
    Missed => "missed",
    Failed => "failed",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<MonitorState>();
        crate::test_util::assert_wire_words::<MonitorCause>();
    }

    #[test]
    fn a_state_reads_as_an_alerts_severity() {
        assert_eq!(MonitorState::Up.severity(), Some(Severity::Ok));
        assert_eq!(MonitorState::Late.severity(), Some(Severity::Warning));
        assert_eq!(MonitorState::Down.severity(), Some(Severity::Alert));
        assert_eq!(MonitorState::New.severity(), None);
        assert_eq!(MonitorState::Paused.severity(), None);
    }
}
