//! [`SortDirection`] — closed vocabulary of the two ways a list is sorted.

use serde::{Deserialize, Serialize};

/// Which way a console list is sorted, named in the URL beside the column so
/// a link holds both. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    /// Smallest, earliest or first in the alphabet first.
    Asc,
    /// Largest, latest or last in the alphabet first.
    Desc,
}

crate::wire_enum!(SortDirection {
    Asc => "asc",
    Desc => "desc",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<SortDirection>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        assert!("ascending".parse::<SortDirection>().is_err());
        assert!("DESC".parse::<SortDirection>().is_err());
    }
}
