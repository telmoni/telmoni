//! [`SpanKind`] — closed vocabulary of the kinds of span telemetry keeps.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// What one finished span of a run is: the run itself, a step inside it, a
/// model or tool call, or anything else recorded under it. The stable string
/// a span's row carries. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// One execution of an agent: `invoke_agent`, or `invoke_workflow` for a
    /// run of several agents. What a monitor expects and holds to its bounds.
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

impl SpanKind {
    /// Every kind, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::Run,
            Self::Step,
            Self::ModelCall,
            Self::ToolCall,
            Self::Span,
        ]
    }
}

impl fmt::Display for SpanKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Run => write!(f, "run"),
            Self::Step => write!(f, "step"),
            Self::ModelCall => write!(f, "model_call"),
            Self::ToolCall => write!(f, "tool_call"),
            Self::Span => write!(f, "span"),
        }
    }
}

impl FromStr for SpanKind {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "run" => Ok(Self::Run),
            "step" => Ok(Self::Step),
            "model_call" => Ok(Self::ModelCall),
            "tool_call" => Ok(Self::ToolCall),
            "span" => Ok(Self::Span),
            _ => Err(ParseEnumError::new(
                s,
                "run, step, model_call, tool_call, span",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vocabulary, read from the enum rather than restated here.
    const ALL: [SpanKind; 5] = SpanKind::all();

    /// `all()` is a hand-written array, so nothing but a `match` forces it to
    /// stay complete: a new variant fails to compile here until it is listed.
    #[test]
    fn all_lists_every_variant() {
        for kind in ALL {
            match kind {
                SpanKind::Run
                | SpanKind::Step
                | SpanKind::ModelCall
                | SpanKind::ToolCall
                | SpanKind::Span => {}
            }
        }
        let mut seen: Vec<String> = ALL.iter().map(ToString::to_string).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len(), "all() repeats a variant");
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in &ALL {
            assert_eq!(v.to_string().parse::<SpanKind>().unwrap(), *v);
        }
    }

    /// The conventions' operation names are what ingest reads, never what a
    /// row carries.
    #[test]
    fn from_str_rejects_the_conventions_operation_names() {
        for name in ["invoke_agent", "chat", "execute_tool", "ModelCall", ""] {
            assert!(name.parse::<SpanKind>().is_err(), "parsed: {name:?}");
        }
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
