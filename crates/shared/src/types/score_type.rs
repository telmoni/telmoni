//! [`ScoreType`] — closed vocabulary of what a score's value is.

use serde::{Deserialize, Serialize};

/// What a score holds: a number on its range where it names one, yes or no,
/// or one or several categories of a set. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ScoreType {
    /// A number, held to its range where the score names one.
    Number,
    /// Yes or no, its value 1 or 0.
    Boolean,
    /// One or several categories, named.
    Category,
}

crate::wire_enum!(ScoreType {
    Number => "number",
    Boolean => "boolean",
    Category => "category",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<ScoreType>();
    }
}
