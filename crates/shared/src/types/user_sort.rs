//! [`UserSort`] — closed vocabulary of the columns the users page sorts by.

use serde::{Deserialize, Serialize};

/// A column of the console's users page, named in the URL so a link holds
/// the sort: by tokens first, until a call is priced, so the users an agent
/// spends the most on stand at the top. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSort {
    /// The end user's id, as stored.
    User,
    /// How many runs were theirs in the window.
    Traces,
    /// How many of those runs had a span that failed.
    Errors,
    /// How many steps their runs took, summed.
    Steps,
    /// Their runs' input tokens, summed.
    InputTokens,
    /// Their runs' output tokens, summed.
    OutputTokens,
    /// Their runs' input and output tokens, summed: the default.
    Tokens,
    /// How long their runs took, summed.
    Duration,
    /// When their first run in the window started.
    FirstSeen,
    /// When their last run in the window ended.
    LastSeen,
}

crate::wire_enum!(UserSort {
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
        crate::test_util::assert_wire_words::<UserSort>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        assert!("Tokens".parse::<UserSort>().is_err());
        assert!("firstSeen".parse::<UserSort>().is_err());
        assert!("".parse::<UserSort>().is_err());
    }
}
