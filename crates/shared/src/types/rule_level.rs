//! [`RuleLevel`] — which of an alert's notices a connector it links hears.

use serde::{Deserialize, Serialize};

/// Which of an alert's notices a connector it links hears: its warnings and
/// alerts, or its alerts alone. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RuleLevel {
    /// Its warnings, its alerts, and an empty window told as no data.
    #[default]
    Warning,
    /// Its alerts alone.
    Alert,
}

crate::wire_enum!(RuleLevel {
    Warning => "warning",
    Alert => "alert",
});

impl RuleLevel {
    /// Whether a connector at this level hears a notice of an incident at
    /// `severity`.
    #[must_use]
    pub const fn hears(self, severity: crate::types::Severity) -> bool {
        use crate::types::Severity as S;
        match self {
            Self::Warning => matches!(severity, S::Warning | S::Alert | S::NoData),
            Self::Alert => matches!(severity, S::Alert),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<RuleLevel>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["warnings", "Alert", ""] {
            assert!(given.parse::<RuleLevel>().is_err(), "parsed: {given:?}");
        }
    }

    #[test]
    fn an_alerts_only_connector_hears_alerts_alone() {
        use crate::types::Severity;
        assert!(RuleLevel::Alert.hears(Severity::Alert));
        assert!(!RuleLevel::Alert.hears(Severity::Warning));
        assert!(!RuleLevel::Alert.hears(Severity::NoData));
        assert!(RuleLevel::Warning.hears(Severity::Warning));
        assert!(RuleLevel::Warning.hears(Severity::NoData));
        assert!(!RuleLevel::Warning.hears(Severity::Ok));
    }
}
