//! [`EvaluationTarget`] — closed vocabulary of what an evaluation rule judges.

use serde::{Deserialize, Serialize};

/// What a rule's evaluators judge. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EvaluationTarget {
    /// A run, once its own span has arrived.
    Run,
    /// A span of one kind, and of one name where the rule names one.
    Span,
    /// A session, once quiet for the project's idle time.
    Session,
}

crate::wire_enum!(EvaluationTarget {
    Run => "run",
    Span => "span",
    Session => "session",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<EvaluationTarget>();
    }
}
