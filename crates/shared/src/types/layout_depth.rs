//! [`LayoutDepth`] — how far a trace opens in a session's or a trace's layout.

use serde::{Deserialize, Serialize};

/// How far a trace opens: its line alone, its steps, or its steps and the
/// calls made inside them, nested as the spans recorded them. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutDepth {
    /// The trace's line alone.
    #[default]
    Trace,
    /// Its steps.
    Steps,
    /// Its steps and the calls inside them.
    Calls,
}

// The shallowest first.
crate::wire_enum!(LayoutDepth {
    Trace => "trace",
    Steps => "steps",
    Calls => "calls",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<LayoutDepth>();
        assert_eq!(LayoutDepth::default(), LayoutDepth::Trace);
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["Trace", "spans", ""] {
            assert!(given.parse::<LayoutDepth>().is_err(), "parsed: {given:?}");
        }
    }
}
