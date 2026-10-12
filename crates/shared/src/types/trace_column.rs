//! [`TraceColumn`] — closed vocabulary of the trace list's columns a person
//! shows or hides.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

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
    /// How the run ended.
    Status,
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

impl TraceColumn {
    /// Every column, in wire order: the list's own, left to right.
    #[must_use]
    pub const fn all() -> [Self; 19] {
        [
            Self::Started,
            Self::Ended,
            Self::Agent,
            Self::User,
            Self::Session,
            Self::Status,
            Self::Steps,
            Self::ModelCalls,
            Self::ToolCalls,
            Self::Errors,
            Self::Spans,
            Self::InputTokens,
            Self::OutputTokens,
            Self::CacheReadTokens,
            Self::CacheWriteTokens,
            Self::Tokens,
            Self::Duration,
            Self::Models,
            Self::Tools,
        ]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Ended => "ended",
            Self::Agent => "agent",
            Self::User => "user",
            Self::Session => "session",
            Self::Status => "status",
            Self::Steps => "steps",
            Self::ModelCalls => "model_calls",
            Self::ToolCalls => "tool_calls",
            Self::Errors => "errors",
            Self::Spans => "spans",
            Self::InputTokens => "input_tokens",
            Self::OutputTokens => "output_tokens",
            Self::CacheReadTokens => "cache_read_tokens",
            Self::CacheWriteTokens => "cache_write_tokens",
            Self::Tokens => "tokens",
            Self::Duration => "duration",
            Self::Models => "models",
            Self::Tools => "tools",
        }
    }
}

impl fmt::Display for TraceColumn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TraceColumn {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::all()
            .into_iter()
            .find(|column| column.as_str() == s)
            .ok_or_else(|| ParseEnumError::new(s, "a column of the trace list"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [TraceColumn; 19] = TraceColumn::all();

    /// `all()` is a hand-written array, so nothing but a `match` forces it to
    /// stay complete: a new variant fails to compile here until it is listed.
    #[test]
    fn all_lists_every_variant() {
        for column in ALL {
            match column {
                TraceColumn::Started
                | TraceColumn::Ended
                | TraceColumn::Agent
                | TraceColumn::User
                | TraceColumn::Session
                | TraceColumn::Status
                | TraceColumn::Steps
                | TraceColumn::ModelCalls
                | TraceColumn::ToolCalls
                | TraceColumn::Errors
                | TraceColumn::Spans
                | TraceColumn::InputTokens
                | TraceColumn::OutputTokens
                | TraceColumn::CacheReadTokens
                | TraceColumn::CacheWriteTokens
                | TraceColumn::Tokens
                | TraceColumn::Duration
                | TraceColumn::Models
                | TraceColumn::Tools => {}
            }
        }
        let mut seen: Vec<&str> = ALL.iter().map(|c| c.as_str()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len(), "all() repeats a variant");
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<TraceColumn>().unwrap(), v);
        }
        assert!("Started".parse::<TraceColumn>().is_err());
        assert!("run_id".parse::<TraceColumn>().is_err());
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
