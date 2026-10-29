//! [`UserColumn`] — closed vocabulary of the users list's columns a person
//! shows or hides.

use serde::{Deserialize, Serialize};

/// A column of the console's users list that a person may hide, named in a
/// saved view so the view opens with the columns it was saved with. The
/// user's own id is always shown, and is none of these. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserColumn {
    /// How many runs were for them in the window.
    Traces,
    /// How many of those runs had a span that failed.
    Errors,
    /// How many steps their runs took, summed.
    Steps,
    /// Their runs' input tokens, summed.
    InputTokens,
    /// Their runs' output tokens, summed.
    OutputTokens,
    /// Input and output tokens, summed.
    Tokens,
    /// How long their runs took, summed.
    Duration,
    /// When their first run in the window started.
    FirstSeen,
    /// When their last run in the window ended.
    LastSeen,
}

// The list's own columns, left to right.
crate::wire_enum!(UserColumn {
    Traces => "traces",
    Errors => "errors",
    Steps => "steps",
    InputTokens => "input_tokens",
    OutputTokens => "output_tokens",
    Tokens => "tokens",
    Duration => "duration",
    FirstSeen => "first_seen",
    LastSeen => "last_seen",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<UserColumn>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        assert!("Traces".parse::<UserColumn>().is_err());
        assert!("user".parse::<UserColumn>().is_err());
    }
}
