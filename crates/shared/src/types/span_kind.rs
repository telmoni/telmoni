//! [`SpanKind`] — closed vocabulary of the kinds of span telemetry keeps.

use serde::{Deserialize, Serialize};

/// What one finished span of a trace is: the agent's own span, a step inside
/// it, a model or tool call, a retrieval, or anything else recorded under it.
/// The stable string a span's row carries. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// One execution of an agent, the trace's own span: `invoke_agent`, or
    /// `invoke_workflow` for one of several agents. One a trace.
    Agent,
    /// A trace's opening, `telmoni.trace.start`, sent as the agent starts so
    /// the trace is seen before it ends: of no length, never a step, costing
    /// no unit.
    TraceStart,
    /// A unit of work the SDKs, the CLI or a hook opened as one, under their
    /// instrumentation scope, that is no call, or an agent's `plan`. What the
    /// step cap counts.
    Step,
    /// A call to a model: `chat`, `generate_content`, `text_completion` or
    /// `embeddings`.
    ModelCall,
    /// A call to a tool: `execute_tool`.
    ToolCall,
    /// A search of a vector store or a memory: `retrieval` or `search_memory`.
    Retrieval,
    /// Any other span of a trace — an exporter's HTTP call or query among
    /// them — kept on the trace's timeline and counted by no bound.
    Span,
}

crate::wire_enum!(SpanKind {
    Agent => "agent",
    TraceStart => "trace_start",
    Step => "step",
    ModelCall => "model_call",
    ToolCall => "tool_call",
    Retrieval => "retrieval",
    Span => "span",
});

impl SpanKind {
    /// Whether an alert watches spans of this kind: the agent's span and its
    /// parts; a plain span is none of an agent's doings, and a trace's opening
    /// is the agent's, whose own span answers for it.
    #[must_use]
    pub const fn alertable(self) -> bool {
        !matches!(self, Self::Span | Self::TraceStart)
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
