//! [`ViewKind`] — what a project's saved view is a view of.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// What a saved view keeps: the trace list's filters, sort and columns, or
/// a layout of a session's page and a trace's. A name is held once per
/// project and kind, and each kind is counted on its own. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    /// A view of the trace list.
    #[default]
    List,
    /// A layout of a session's page and a trace's.
    Layout,
}

impl ViewKind {
    /// Both kinds, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::List, Self::Layout]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Layout => "layout",
        }
    }
}

impl fmt::Display for ViewKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ViewKind {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "list" => Ok(Self::List),
            "layout" => Ok(Self::Layout),
            _ => Err(ParseEnumError::new(s, "list, layout")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ViewKind; 2] = ViewKind::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for kind in ALL {
            match kind {
                ViewKind::List | ViewKind::Layout => {}
            }
        }
        assert_eq!(ViewKind::default(), ViewKind::List);
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<ViewKind>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["List", "views", ""] {
            assert!(given.parse::<ViewKind>().is_err(), "parsed: {given:?}");
        }
    }
}
