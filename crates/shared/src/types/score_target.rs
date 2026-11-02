//! [`ScoreTarget`] — closed vocabulary of what a score is of.

use serde::{Deserialize, Serialize};

/// What a score judges. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ScoreTarget {
    /// A run, its trace whole.
    Run,
    /// One span of a run.
    Span,
    /// A session, its runs together.
    Session,
}

crate::wire_enum!(ScoreTarget {
    Run => "run",
    Span => "span",
    Session => "session",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<ScoreTarget>();
    }
}
