//! [`RuleMeasure`] — what an alert measures of each span it reads.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// What an alert measures of each span of the kind it watches: the spans
/// themselves, those that failed, how long each took, the tokens each model
/// call spent, input and output, or what each run spent. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RuleMeasure {
    /// One for each span.
    Count,
    /// One for each span that ended in an error, none for the rest.
    Failures,
    /// How long each took, in milliseconds.
    Duration,
    /// Each span's input and output tokens, summed.
    Tokens,
    /// Each run's model calls' cost, in US dollars, as they land.
    Cost,
}

impl RuleMeasure {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::Count,
            Self::Failures,
            Self::Duration,
            Self::Tokens,
            Self::Cost,
        ]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Failures => "failures",
            Self::Duration => "duration",
            Self::Tokens => "tokens",
            Self::Cost => "cost",
        }
    }
}

impl fmt::Display for RuleMeasure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RuleMeasure {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "count" => Ok(Self::Count),
            "failures" => Ok(Self::Failures),
            "duration" => Ok(Self::Duration),
            "tokens" => Ok(Self::Tokens),
            "cost" => Ok(Self::Cost),
            _ => Err(ParseEnumError::new(
                s,
                "count, failures, duration, tokens, cost",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [RuleMeasure; 5] = RuleMeasure::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                RuleMeasure::Count
                | RuleMeasure::Failures
                | RuleMeasure::Duration
                | RuleMeasure::Tokens
                | RuleMeasure::Cost => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<RuleMeasure>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["Count", "spend", ""] {
            assert!(given.parse::<RuleMeasure>().is_err(), "parsed: {given:?}");
        }
    }
}
