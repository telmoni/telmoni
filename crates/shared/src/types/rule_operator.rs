//! [`RuleOperator`] — how an alert compares its value with a threshold.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// How an alert's value is compared with its thresholds: a threshold is
/// crossed when the value stands to it as the operator says. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RuleOperator {
    /// Above it: `>`.
    Gt,
    /// At it or above: `≥`.
    Gte,
    /// Below it: `<`.
    Lt,
    /// At it or below: `≤`.
    Lte,
    /// At it: `=`.
    Eq,
    /// Anywhere but at it: `≠`.
    Ne,
}

impl RuleOperator {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [Self::Gt, Self::Gte, Self::Lt, Self::Lte, Self::Eq, Self::Ne]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gt => "gt",
            Self::Gte => "gte",
            Self::Lt => "lt",
            Self::Lte => "lte",
            Self::Eq => "eq",
            Self::Ne => "ne",
        }
    }

    /// Whether `value` crosses `threshold` under this operator.
    #[must_use]
    pub fn crosses(self, value: f64, threshold: f64) -> bool {
        match self {
            Self::Gt => value > threshold,
            Self::Gte => value >= threshold,
            Self::Lt => value < threshold,
            Self::Lte => value <= threshold,
            Self::Eq => (value - threshold).abs() <= f64::EPSILON * threshold.abs().max(1.0),
            Self::Ne => (value - threshold).abs() > f64::EPSILON * threshold.abs().max(1.0),
        }
    }

    /// The sign a notice writes it with.
    #[must_use]
    pub const fn sign(self) -> &'static str {
        match self {
            Self::Gt => ">",
            Self::Gte => "≥",
            Self::Lt => "<",
            Self::Lte => "≤",
            Self::Eq => "=",
            Self::Ne => "≠",
        }
    }
}

impl fmt::Display for RuleOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RuleOperator {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gt" => Ok(Self::Gt),
            "gte" => Ok(Self::Gte),
            "lt" => Ok(Self::Lt),
            "lte" => Ok(Self::Lte),
            "eq" => Ok(Self::Eq),
            "ne" => Ok(Self::Ne),
            _ => Err(ParseEnumError::new(s, "gt, gte, lt, lte, eq, ne")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [RuleOperator; 6] = RuleOperator::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                RuleOperator::Gt
                | RuleOperator::Gte
                | RuleOperator::Lt
                | RuleOperator::Lte
                | RuleOperator::Eq
                | RuleOperator::Ne => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<RuleOperator>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in [">", "GT", ""] {
            assert!(given.parse::<RuleOperator>().is_err(), "parsed: {given:?}");
        }
    }

    #[test]
    fn each_operator_crosses_as_its_sign_says() {
        assert!(RuleOperator::Gt.crosses(2.0, 1.0) && !RuleOperator::Gt.crosses(1.0, 1.0));
        assert!(RuleOperator::Gte.crosses(1.0, 1.0) && !RuleOperator::Gte.crosses(0.5, 1.0));
        assert!(RuleOperator::Lt.crosses(0.5, 1.0) && !RuleOperator::Lt.crosses(1.0, 1.0));
        assert!(RuleOperator::Lte.crosses(1.0, 1.0) && !RuleOperator::Lte.crosses(1.5, 1.0));
        assert!(RuleOperator::Eq.crosses(0.1 + 0.2, 0.3) && !RuleOperator::Eq.crosses(0.31, 0.3));
        assert!(RuleOperator::Ne.crosses(0.31, 0.3) && !RuleOperator::Ne.crosses(0.1 + 0.2, 0.3));
    }
}
