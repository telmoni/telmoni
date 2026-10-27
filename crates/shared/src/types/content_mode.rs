//! [`ContentMode`] — what a project keeps of the content its spans carry.

use serde::{Deserialize, Serialize};

/// What a project keeps of prompts, completions and tool payloads: nothing
/// unless an organization owner says otherwise. Spans are kept in every mode;
/// content never reaches ClickHouse in any. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ContentMode {
    /// Every content field is dropped at the door, the span kept. A project
    /// with no settings of its own reads as this.
    #[default]
    Off,
    /// Content is kept as it arrives, in Postgres alone.
    On,
    /// Only content the SDK encrypted under the customer's key is kept,
    /// in Postgres alone; the key never reaches the server.
    Sealed,
}

crate::wire_enum!(ContentMode {
    Off => "off",
    On => "on",
    Sealed => "sealed",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<ContentMode>();
    }

    /// ⚠ Off until an owner changes it: the default is what a project with
    /// no settings row reads as.
    #[test]
    fn the_default_keeps_no_content() {
        assert_eq!(ContentMode::default(), ContentMode::Off);
    }

    #[test]
    fn from_str_rejects_unknown() {
        for mode in ["Off", "plain", "encrypted", "true", ""] {
            assert!(mode.parse::<ContentMode>().is_err(), "parsed: {mode:?}");
        }
    }
}
