//! [`SessionColumn`] — closed vocabulary of the sessions list's columns a
//! person shows or hides.

use serde::{Deserialize, Serialize};

/// A column of the console's sessions list that a person may hide, named in
/// a saved view so the view opens with the columns it was saved with. The
/// session's own id is always shown, and is none of these. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionColumn {
    /// The end user the session was with, as stored.
    User,
    /// How many runs it holds in the window.
    Traces,
    /// How many of those runs had a span that failed.
    Errors,
    /// How many steps its runs took, summed.
    Steps,
    /// Its runs' input tokens, summed.
    InputTokens,
    /// Its runs' output tokens, summed.
    OutputTokens,
    /// Input and output tokens, summed.
    Tokens,
    /// How long its runs took, summed.
    Duration,
    /// When its first run in the window started.
    FirstSeen,
    /// When its last run in the window ended.
    LastSeen,
}

// The list's own columns, left to right.
crate::wire_enum!(SessionColumn {
    User => "user",
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
        crate::test_util::assert_wire_words::<SessionColumn>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        assert!("User".parse::<SessionColumn>().is_err());
        assert!("session".parse::<SessionColumn>().is_err());
    }
}
