//! [`Severity`] — where an alert stands at its last evaluation.

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

crate::wire_enum!(Severity {
    Unknown => "unknown",
    Ok => "ok",
    Warning => "warning",
    Alert => "alert",
    NoData => "no_data",
    Paused => "paused",
});

impl Severity {
    /// Whether the alert stands past a threshold: what an incident opens on.
    #[must_use]
    pub const fn is_breach(self) -> bool {
        matches!(self, Self::Warning | Self::Alert)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<Severity>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["OK", "breached", ""] {
            assert!(given.parse::<Severity>().is_err(), "parsed: {given:?}");
        }
    }
}
