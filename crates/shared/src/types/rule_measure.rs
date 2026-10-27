//! [`RuleMeasure`] — what an alert measures of each span it reads.

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

crate::wire_enum!(RuleMeasure {
    Count => "count",
    Failures => "failures",
    Duration => "duration",
    Tokens => "tokens",
    Cost => "cost",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<RuleMeasure>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["Count", "spend", ""] {
            assert!(given.parse::<RuleMeasure>().is_err(), "parsed: {given:?}");
        }
    }
}
