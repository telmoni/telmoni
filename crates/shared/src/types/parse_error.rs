//! [`ParseEnumError`] — the shared `FromStr::Err` for the wire-enum vocabulary.

use std::fmt;

/// Returned by a wire enum's [`FromStr`](std::str::FromStr) when the input
/// matches no variant. Names the rejected value and the accepted set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEnumError {
    /// The rejected input.
    pub got: String,
    /// The accepted values, comma-joined (e.g. `"owner, admin, member"`).
    pub expected: &'static str,
}

impl ParseEnumError {
    /// Build from the rejected slice and the enum's accepted set.
    #[must_use]
    pub fn new(got: &str, expected: &'static str) -> Self {
        Self {
            got: got.to_owned(),
            expected,
        }
    }
}

impl fmt::Display for ParseEnumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not one of: {}", self.got, self.expected)
    }
}

impl std::error::Error for ParseEnumError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_names_the_value_and_the_set() {
        let e = ParseEnumError::new("superadmin", "owner, admin, member");
        assert_eq!(
            e.to_string(),
            "\"superadmin\" is not one of: owner, admin, member"
        );
    }

    #[test]
    fn empty_input_is_visible_in_the_message() {
        assert!(ParseEnumError::new("", "a, b").to_string().contains("\"\""));
    }
}
