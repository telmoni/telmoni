//! [`EvaluationStatus`] — closed vocabulary of where an evaluation stands.

use serde::{Deserialize, Serialize};

/// Where one evaluator's judgement of one target stands. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EvaluationStatus {
    /// Found, and waiting for the loop.
    Pending,
    /// Waiting out a provider's rate limit, or the evaluator's weekly limit.
    Delayed,
    /// Judged: its score written.
    Done,
    /// Given up, with its reason.
    Failed,
}

crate::wire_enum!(EvaluationStatus {
    Pending => "pending",
    Delayed => "delayed",
    Done => "done",
    Failed => "failed",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<EvaluationStatus>();
    }
}
