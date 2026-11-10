//! [`RuleMeasure`] — what an alert measures of each span it reads.

use serde::{Deserialize, Serialize};

use crate::{RuleAggregation, SpanKind};

/// What an alert measures: of each span of the kind it watches, of each
/// trace, or of the project's scores of one name; or how fast an agent's
/// traces spend their objective's error budget. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RuleMeasure {
    /// One for each span.
    Count,
    /// One for each span that ended in an error, none for the rest.
    Failures,
    /// One for each span whose fault the alert names, zero for the rest.
    Faults,
    /// How long each took, in milliseconds.
    Duration,
    /// Each span's input and output tokens, summed.
    Tokens,
    /// Each trace's model calls' cost, in US dollars, as they land.
    Cost,
    /// Each score of the alert's name: a number's value, a yes or no's 1 or 0.
    Score,
    /// How fast one agent's traces spend their objective's error budget: the
    /// share that failed it, over the share it allows.
    Burn,
    /// Each trace's steps, counted as they land.
    Steps,
    /// The most times in a row each trace called one tool, its tool calls in
    /// the order they started: a loop.
    Repeats,
    /// One for each trace whose spans have landed and whose agent's own has
    /// not, quiet past the window: a stall.
    Stalled,
}

crate::wire_enum!(RuleMeasure {
    Count => "count",
    Failures => "failures",
    Faults => "faults",
    Duration => "duration",
    Tokens => "tokens",
    Cost => "cost",
    Score => "score",
    Burn => "burn",
    Steps => "steps",
    Repeats => "repeats",
    Stalled => "stalled",
});

impl RuleMeasure {
    /// Whether `aggregation` folds the measure as its read can answer it:
    /// the hourly totals keep tokens' sum alone, and a sum of scores, steps
    /// or repeats says nothing.
    #[must_use]
    pub const fn folds(self, aggregation: RuleAggregation) -> bool {
        use RuleAggregation as A;
        match self {
            Self::Count | Self::Stalled => matches!(aggregation, A::Count),
            Self::Failures | Self::Faults => matches!(aggregation, A::Count | A::Avg),
            Self::Duration => !matches!(aggregation, A::Count | A::Max),
            Self::Tokens => matches!(aggregation, A::Avg | A::Sum),
            Self::Cost => matches!(aggregation, A::Avg | A::Sum | A::Max),
            Self::Score => !matches!(aggregation, A::Sum),
            Self::Burn => matches!(aggregation, A::Avg),
            Self::Steps => !matches!(aggregation, A::Count | A::Sum),
            Self::Repeats => matches!(aggregation, A::Max),
        }
    }

    /// Whether the measure is counted on spans of `kind`: tokens on model
    /// calls alone, and what is read trace by trace, or from scores, on the
    /// agent's span.
    #[must_use]
    pub const fn measured_on(self, kind: SpanKind) -> bool {
        match self {
            Self::Tokens => matches!(kind, SpanKind::ModelCall),
            Self::Cost | Self::Score | Self::Burn | Self::Steps | Self::Repeats | Self::Stalled => {
                matches!(kind, SpanKind::Agent)
            }
            Self::Count | Self::Failures | Self::Faults | Self::Duration => true,
        }
    }
}

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
