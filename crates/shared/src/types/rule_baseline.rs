//! [`RuleBaseline`] — what an alert's value is read against.

use serde::{Deserialize, Serialize};

/// What an alert's value is read against: itself, or the window of the same
/// length before it, the value then its change from that window in percent,
/// so a drop is below 0. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RuleBaseline {
    /// The value as it is.
    #[default]
    None,
    /// Its change from the window before, in percent.
    Previous,
}

crate::wire_enum!(RuleBaseline {
    None => "none",
    Previous => "previous",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<RuleBaseline>();
    }
}
