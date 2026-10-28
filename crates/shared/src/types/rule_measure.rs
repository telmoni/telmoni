//! [`RuleMeasure`] — what an alert measures of each span it reads.

use serde::{Deserialize, Serialize};

use crate::{RuleAggregation, SpanKind};

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

impl RuleMeasure {
    /// Whether `aggregation` folds the measure: a count of spans; failures
    /// counted or as a share; durations averaged, summed or at a percentile;
    /// tokens averaged or summed, since the hourly totals keep their sum
    /// alone; and a run's cost averaged, summed or the largest, the cost cap.
    #[must_use]
    pub const fn folds(self, aggregation: RuleAggregation) -> bool {
        use RuleAggregation as A;
        match self {
            Self::Count => matches!(aggregation, A::Count),
            Self::Failures => matches!(aggregation, A::Count | A::Avg),
            Self::Duration => !matches!(aggregation, A::Count | A::Max),
            Self::Tokens => matches!(aggregation, A::Avg | A::Sum),
            Self::Cost => matches!(aggregation, A::Avg | A::Sum | A::Max),
        }
    }

    /// Whether the measure is counted on spans of `kind`: tokens on model
    /// calls alone, and cost on runs alone, from their calls.
    #[must_use]
    pub const fn measured_on(self, kind: SpanKind) -> bool {
        match self {
            Self::Tokens => matches!(kind, SpanKind::ModelCall),
            Self::Cost => matches!(kind, SpanKind::Run),
            Self::Count | Self::Failures | Self::Duration => true,
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
