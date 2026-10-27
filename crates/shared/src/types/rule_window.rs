//! [`RuleWindow`] — how far back an alert reads.

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

crate::wire_enum!(RuleWindow {
    Hour => "hour",
    Day => "day",
    Week => "week",
});

impl RuleWindow {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<RuleWindow>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["1h", "month", ""] {
            assert!(given.parse::<RuleWindow>().is_err(), "parsed: {given:?}");
        }
    }
}
