//! [`MetricsGroup`] — the column a project's totals over time are split by.

use serde::{Deserialize, Serialize};

/// What the totals are split by: a column the hourly totals keep, or a trace's
/// tag, its end user or its session, which the spans alone keep. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricsGroup {
    /// The agent a span's trace is of.
    Agent,
    /// A model call's model.
    Model,
    /// A model call's provider.
    Provider,
    /// A tool call's name.
    Tool,
    /// How a span ended.
    Status,
    /// The deployment's environment.
    Environment,
    /// The deployment's release.
    Release,
    /// One tag key's values.
    Tag,
    /// The trace's end user.
    User,
    /// The trace's session.
    Session,
}

crate::wire_enum!(MetricsGroup {
    Agent => "agent",
    Model => "model",
    Provider => "provider",
    Tool => "tool",
    Status => "status",
    Environment => "environment",
    Release => "release",
    Tag => "tag",
    User => "user",
    Session => "session",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<MetricsGroup>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["Agent", "name", ""] {
            assert!(given.parse::<MetricsGroup>().is_err(), "parsed: {given:?}");
        }
    }
}
