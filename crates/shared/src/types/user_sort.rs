//! [`UserSort`] — closed vocabulary of the columns the users list sorts by.

use serde::{Deserialize, Serialize};

/// A column of the console's users list, named in the URL so a link holds
/// the sort: the end user last heard from first by default. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSort {
    /// When their last trace in the window ended: the default.
    LastSeen,
    /// When their first trace in the window started.
    FirstSeen,
    /// How many sessions their traces named in the window.
    Sessions,
    /// How many traces they hold in the window.
    Traces,
    /// How many of those traces ended in error.
    FailedTraces,
    /// Their traces' input and output tokens, summed.
    Tokens,
    /// What their traces' model calls cost, reported or priced.
    Spend,
    /// How long their traces ran, summed.
    Duration,
}

// The list's own columns, left to right.
crate::wire_enum!(UserSort {
    LastSeen => "last_seen",
    FirstSeen => "first_seen",
    Sessions => "sessions",
    Traces => "traces",
    FailedTraces => "failed_traces",
    Tokens => "tokens",
    Spend => "spend",
    Duration => "duration",
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
        assert!("LastSeen".parse::<UserSort>().is_err());
        assert!("".parse::<UserSort>().is_err());
    }
}
