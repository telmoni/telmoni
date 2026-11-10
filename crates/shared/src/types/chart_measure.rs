//! [`ChartMeasure`] — what one of a dashboard's charts draws.

use serde::{Deserialize, Serialize};

/// What a dashboard's chart draws over its window. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartMeasure {
    /// What the model calls cost; the one measure a chart may split by a group.
    Spend,
    /// Input and output tokens.
    Tokens,
    /// Traces, those that failed apart.
    Traces,
    /// The traces' durations at the 50th, 95th and 99th percentiles.
    Latency,
    /// Time to first token, from the spans.
    FirstToken,
}

crate::wire_enum!(ChartMeasure {
    Spend => "spend",
    Tokens => "tokens",
    Traces => "traces",
    Latency => "latency",
    FirstToken => "first_token",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<ChartMeasure>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["Spend", "first-token", ""] {
            assert!(given.parse::<ChartMeasure>().is_err(), "parsed: {given:?}");
        }
    }
}
