//! [`RuleAggregation`] — how an alert folds its measure over its window.

use serde::{Deserialize, Serialize};

/// How an alert folds its measure over the window: how many spans, their
/// average, their sum, the largest, or a percentile of them. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RuleAggregation {
    /// How many spans the window holds.
    Count,
    /// The measure's average over them.
    Avg,
    /// The measure summed over them.
    Sum,
    /// The largest of them: a cap on any one.
    Max,
    /// The 50th percentile.
    P50,
    /// The 90th percentile.
    P90,
    /// The 95th percentile.
    P95,
    /// The 99th percentile.
    P99,
}

crate::wire_enum!(RuleAggregation {
    Count => "count",
    Avg => "avg",
    Sum => "sum",
    Max => "max",
    P50 => "p50",
    P90 => "p90",
    P95 => "p95",
    P99 => "p99",
});

impl RuleAggregation {
    /// The level a percentile is read at, `None` for the rest.
    #[must_use]
    pub const fn level(self) -> Option<f64> {
        match self {
            Self::P50 => Some(0.5),
            Self::P90 => Some(0.9),
            Self::P95 => Some(0.95),
            Self::P99 => Some(0.99),
            Self::Count | Self::Avg | Self::Sum | Self::Max => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<RuleAggregation>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["average", "P95", ""] {
            assert!(
                given.parse::<RuleAggregation>().is_err(),
                "parsed: {given:?}"
            );
        }
    }
}
