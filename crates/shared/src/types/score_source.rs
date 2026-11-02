//! [`ScoreSource`] — closed vocabulary of where a score came from.

use serde::{Deserialize, Serialize};

/// Who gave a score. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ScoreSource {
    /// The application's own code, through an SDK or any exporter.
    Sdk,
    /// A member, on a trace's page.
    Member,
    /// An evaluator Telmoni ran.
    Evaluator,
}

crate::wire_enum!(ScoreSource {
    Sdk => "sdk",
    Member => "member",
    Evaluator => "evaluator",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<ScoreSource>();
    }
}
