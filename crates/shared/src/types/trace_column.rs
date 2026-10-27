//! [`TraceColumn`] — closed vocabulary of the trace list's columns a person
//! shows or hides.

use serde::{Deserialize, Serialize};

/// A column of the console's trace list that a person may hide, named in a
/// saved view so the view opens with the columns it was saved with. The
/// trace's own id is always shown, and is none of these. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceColumn {
    /// When the run started.
    Started,
    /// When its last span ended.
    Ended,
    /// The run's agent.
    Agent,
    /// The end user, as stored.
    User,
    /// The session the run belongs to.
    Session,
    /// The deployments its spans were sent from.
    Environment,
    /// The releases its spans were sent from.
    Release,
    /// How the run ended.
    Status,
    /// The class of the error it ended on, never its message.
    ErrorType,
    /// How many steps it took.
    Steps,
    /// How many model calls it made.
    ModelCalls,
    /// How many tool calls it made.
    ToolCalls,
    /// How many of its spans failed.
    Errors,
    /// How many spans it sent.
    Spans,
    /// Its model calls' input tokens.
    InputTokens,
    /// Their output tokens.
    OutputTokens,
    /// The input tokens read from a cache.
    CacheReadTokens,
    /// The input tokens written to one.
    CacheWriteTokens,
    /// Input and output tokens, summed.
    Tokens,
    /// How long it ran.
    Duration,
    /// The models its calls answered with.
    Models,
    /// The tools its calls ran.
    Tools,
}

// The list's own columns, left to right.
crate::wire_enum!(TraceColumn {
    Started => "started",
    Ended => "ended",
    Agent => "agent",
    User => "user",
    Session => "session",
    Environment => "environment",
    Release => "release",
    Status => "status",
    ErrorType => "error_type",
    Steps => "steps",
    ModelCalls => "model_calls",
    ToolCalls => "tool_calls",
    Errors => "errors",
    Spans => "spans",
    InputTokens => "input_tokens",
    OutputTokens => "output_tokens",
    CacheReadTokens => "cache_read_tokens",
    CacheWriteTokens => "cache_write_tokens",
    Tokens => "tokens",
    Duration => "duration",
    Models => "models",
    Tools => "tools",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<TraceColumn>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        assert!("Started".parse::<TraceColumn>().is_err());
        assert!("run_id".parse::<TraceColumn>().is_err());
    }
}
