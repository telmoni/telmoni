//! [`SessionSort`] — closed vocabulary of the columns the sessions list
//! sorts by.

use serde::{Deserialize, Serialize};

/// A column of the console's sessions list, named in the URL so a link holds
/// the sort: the session last heard from first by default, so those still
/// going stand at the top. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSort {
    /// When its last trace in the window ended: the default.
    LastSeen,
    /// When its first trace in the window started.
    FirstSeen,
    /// How many traces it holds in the window.
    Traces,
    /// How many of those traces ended in error.
    FailedTraces,
    /// Its traces' input and output tokens, summed.
    Tokens,
    /// What its traces' model calls cost, reported or priced.
    Spend,
    /// How long its traces ran, summed.
    Duration,
}

// The list's own columns, left to right.
crate::wire_enum!(SessionSort {
    LastSeen => "last_seen",
    FirstSeen => "first_seen",
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
        crate::test_util::assert_wire_words::<SessionSort>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        assert!("LastSeen".parse::<SessionSort>().is_err());
        assert!("".parse::<SessionSort>().is_err());
    }
}
