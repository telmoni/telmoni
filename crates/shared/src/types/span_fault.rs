//! [`SpanFault`] — a failure class, never an error message.

use serde::{Deserialize, Serialize};

/// Why a span failed, or a model stopped its answer. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanFault {
    /// The provider refused more requests or tokens.
    RateLimited,
    /// The service could not accept the work.
    Overloaded,
    /// The request exceeded its time allowance.
    Timeout,
    /// The request exceeded the model's context window.
    ContextLength,
    /// Credentials or permissions were refused.
    Unauthorized,
    /// The answer failed its expected format or schema.
    InvalidOutput,
    /// A content filter or model refusal stopped the answer.
    Refused,
    /// The answer reached its output token limit.
    Truncated,
    /// A failure whose metadata names no recognized cause.
    Other,
}

crate::wire_enum!(SpanFault {
    RateLimited => "rate_limited",
    Overloaded => "overloaded",
    Timeout => "timeout",
    ContextLength => "context_length",
    Unauthorized => "unauthorized",
    InvalidOutput => "invalid_output",
    Refused => "refused",
    Truncated => "truncated",
    Other => "other",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<SpanFault>();
    }
}
