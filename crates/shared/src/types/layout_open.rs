//! [`LayoutOpen`] — which traces, and which branches of them, start open.

use serde::{Deserialize, Serialize};

/// Which of a session's traces, and which branches inside a trace, start
/// open: none, those that failed, or all of them. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutOpen {
    /// Every one shut until opened.
    #[default]
    None,
    /// Those holding a failed span open.
    Failed,
    /// Every one open.
    All,
}

// The least open first.
crate::wire_enum!(LayoutOpen {
    None => "none",
    Failed => "failed",
    All => "all",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<LayoutOpen>();
        assert_eq!(LayoutOpen::default(), LayoutOpen::None);
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["None", "errors", ""] {
            assert!(given.parse::<LayoutOpen>().is_err(), "parsed: {given:?}");
        }
    }
}
