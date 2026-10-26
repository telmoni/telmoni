//! [`RuleAggregation`] — how an alert folds its measure over its window.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// How an alert folds its measure over the window: how many spans, their
/// average, their sum, or a percentile of them. `[PUBLIC-API]`.
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
    /// The 50th percentile.
    P50,
    /// The 90th percentile.
    P90,
    /// The 95th percentile.
    P95,
    /// The 99th percentile.
    P99,
}

impl RuleAggregation {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Count,
            Self::Avg,
            Self::Sum,
            Self::P50,
            Self::P90,
            Self::P95,
            Self::P99,
        ]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Avg => "avg",
            Self::Sum => "sum",
            Self::P50 => "p50",
            Self::P90 => "p90",
            Self::P95 => "p95",
            Self::P99 => "p99",
        }
    }

    /// The level a percentile is read at, `None` for the rest.
    #[must_use]
    pub const fn level(self) -> Option<f64> {
        match self {
            Self::P50 => Some(0.5),
            Self::P90 => Some(0.9),
            Self::P95 => Some(0.95),
            Self::P99 => Some(0.99),
            Self::Count | Self::Avg | Self::Sum => None,
        }
    }
}

impl fmt::Display for RuleAggregation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RuleAggregation {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "count" => Ok(Self::Count),
            "avg" => Ok(Self::Avg),
            "sum" => Ok(Self::Sum),
            "p50" => Ok(Self::P50),
            "p90" => Ok(Self::P90),
            "p95" => Ok(Self::P95),
            "p99" => Ok(Self::P99),
            _ => Err(ParseEnumError::new(
                s,
                "count, avg, sum, p50, p90, p95, p99",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [RuleAggregation; 7] = RuleAggregation::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                RuleAggregation::Count
                | RuleAggregation::Avg
                | RuleAggregation::Sum
                | RuleAggregation::P50
                | RuleAggregation::P90
                | RuleAggregation::P95
                | RuleAggregation::P99 => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<RuleAggregation>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["average", "P95", ""] {
            assert!(
                given.parse::<RuleAggregation>().is_err(),
                "parsed: {given:?}"
            );
        }
    }
}
