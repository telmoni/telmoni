//! [`SpanKind`] — closed vocabulary of the kinds of span telemetry keeps.

use serde::{Deserialize, Serialize};

/// What one finished span of a run is: the run itself, a step inside it, a
/// model or tool call, or anything else recorded under it. The stable string
/// a span's row carries. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// One execution of an agent: `invoke_agent`, or `invoke_workflow` for a
    /// run of several agents.
    Run,
    /// A unit of work the SDKs, the CLI or a hook opened as one, under their
    /// instrumentation scope, that is no model or tool call. What the step
    /// cap counts.
    Step,
    /// A call to a model: `chat`, `generate_content`, `text_completion` or
    /// `embeddings`.
    ModelCall,
    /// A call to a tool: `execute_tool`.
    ToolCall,
    /// Any other span under a run — an exporter's HTTP call or query among
    /// them — kept on the run's timeline and counted by no bound.
    Span,
}

crate::wire_enum!(SpanKind {
    Run => "run",
    Step => "step",
    ModelCall => "model_call",
    ToolCall => "tool_call",
    Span => "span",
});

impl SpanKind {
    /// Whether an alert watches spans of this kind: the run and its parts;
    /// a plain span is none of an agent's doings.
    #[must_use]
    pub const fn alertable(self) -> bool {
        !matches!(self, Self::Span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<SpanKind>();
    }

    /// The conventions' operation names are what ingest reads, never what a
    /// row carries.
    #[test]
    fn from_str_rejects_the_conventions_operation_names() {
        for name in ["invoke_agent", "chat", "execute_tool", "ModelCall", ""] {
            assert!(name.parse::<SpanKind>().is_err(), "parsed: {name:?}");
        }
    }
}
