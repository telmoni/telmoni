//! [`TraceSort`] — closed vocabulary of the columns the trace list sorts by.

use serde::{Deserialize, Serialize};

/// A column of the console's trace list, named in the URL so a link holds
/// the sort: by user and by session above all, so one user's traces, and one
/// session's, stand together. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceSort {
    /// When the run started: newest first by default.
    Started,
    /// The run's agent.
    Agent,
    /// The end user, as stored.
    User,
    /// The session the run belongs to.
    Session,
    /// How the run ended.
    Status,
    /// How many steps the run took.
    Steps,
    /// How many model calls it made.
    ModelCalls,
    /// How many tool calls it made.
    ToolCalls,
    /// How many of its spans failed.
    Errors,
    /// Its model calls' input tokens.
    InputTokens,
    /// Their output tokens.
    OutputTokens,
    /// Its input and output tokens, summed.
    Tokens,
    /// How long it ran.
    Duration,
    /// How soon its first model call's answer began.
    TimeToFirstToken,
}

// The list's own columns, left to right.
crate::wire_enum!(TraceSort {
    Started => "started",
    Agent => "agent",
    User => "user",
    Session => "session",
    Status => "status",
    Steps => "steps",
    ModelCalls => "model_calls",
    ToolCalls => "tool_calls",
    Errors => "errors",
    InputTokens => "input_tokens",
    OutputTokens => "output_tokens",
    Tokens => "tokens",
    Duration => "duration",
    TimeToFirstToken => "time_to_first_token",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<TraceSort>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        assert!("Started".parse::<TraceSort>().is_err());
        assert!("".parse::<TraceSort>().is_err());
    }
}
