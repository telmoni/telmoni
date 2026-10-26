//! [`RuleLevel`] — which of an alert's notices a connector it links hears.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

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

impl RuleLevel {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::Warning, Self::Alert]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Alert => "alert",
        }
    }

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

impl fmt::Display for RuleLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RuleLevel {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "warning" => Ok(Self::Warning),
            "alert" => Ok(Self::Alert),
            _ => Err(ParseEnumError::new(s, "warning, alert")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [RuleLevel; 2] = RuleLevel::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                RuleLevel::Warning | RuleLevel::Alert => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<RuleLevel>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
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
