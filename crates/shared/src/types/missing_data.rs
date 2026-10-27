//! [`MissingData`] — what an alert does with a window that holds no span.

use serde::{Deserialize, Serialize};

/// What an alert does with a window that holds no span: reads it as 0,
/// keeps the severity it had, shows `no_data`, or shows it and tells the
/// alert's channels once it lasts. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MissingData {
    /// The value is 0, compared as any other.
    #[default]
    Zero,
    /// The severity stays as it was.
    Keep,
    /// The severity is `no_data`, and nobody is told.
    NoData,
    /// The severity is `no_data`, told once it has lasted a window.
    NotifyNoData,
}

crate::wire_enum!(MissingData {
    Zero => "zero",
    Keep => "keep",
    NoData => "no_data",
    NotifyNoData => "notify_no_data",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<MissingData>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["none", "NoData", ""] {
            assert!(given.parse::<MissingData>().is_err(), "parsed: {given:?}");
        }
    }
}
