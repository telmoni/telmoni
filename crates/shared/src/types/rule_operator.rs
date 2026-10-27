//! [`RuleOperator`] — how an alert compares its value with a threshold.

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

crate::wire_enum!(RuleOperator {
    Gt => "gt",
    Gte => "gte",
    Lt => "lt",
    Lte => "lte",
    Eq => "eq",
    Ne => "ne",
});

impl RuleOperator {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<RuleOperator>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
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
