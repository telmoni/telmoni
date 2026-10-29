//! [`SessionSort`] — closed vocabulary of the columns the sessions page sorts
//! by.

use serde::{Deserialize, Serialize};

/// A column of the console's sessions page, named in the URL so a link holds
/// the sort: last seen first by default, so the sessions still going stand at
/// the top. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSort {
    /// The session's id, as the application set it.
    Session,
    /// The end user the session was with, as stored.
    User,
    /// How many runs the session holds in the window.
    Traces,
    /// How many of those runs had a span that failed.
    Errors,
    /// How many steps its runs took, summed.
    Steps,
    /// Its runs' input tokens, summed.
    InputTokens,
    /// Its runs' output tokens, summed.
    OutputTokens,
    /// Its runs' input and output tokens, summed.
    Tokens,
    /// How long its runs took, summed.
    Duration,
    /// When its first run in the window started.
    FirstSeen,
    /// When its last run in the window ended: the default.
    LastSeen,
}

crate::wire_enum!(SessionSort {
    Session => "session",
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
        crate::test_util::assert_wire_words::<SessionSort>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        assert!("LastSeen".parse::<SessionSort>().is_err());
        assert!("".parse::<SessionSort>().is_err());
    }
}
