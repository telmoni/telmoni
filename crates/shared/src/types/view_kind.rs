//! [`ViewKind`] — what a project's saved view is a view of.

use serde::{Deserialize, Serialize};

/// What a saved view keeps: a list's filters, sort and columns — the
/// traces', the sessions' or the users' — or a layout of a session's page and
/// a trace's. A name is held once per project and kind, and each kind is
/// counted on its own. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    /// A view of the trace list.
    #[default]
    Traces,
    /// A view of the sessions list.
    Sessions,
    /// A view of the users list.
    Users,
    /// A layout of a session's page and a trace's.
    Layout,
}

crate::wire_enum!(ViewKind {
    Traces => "traces",
    Sessions => "sessions",
    Users => "users",
    Layout => "layout",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<ViewKind>();
        assert_eq!(ViewKind::default(), ViewKind::Traces);
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["Traces", "list", ""] {
            assert!(given.parse::<ViewKind>().is_err(), "parsed: {given:?}");
        }
    }
}
