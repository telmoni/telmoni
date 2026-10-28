//! [`OrganizationStatus`] — an organization's lifecycle state.

use serde::{Deserialize, Serialize};

/// An organization's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OrganizationStatus {
    /// Normal, usable organization. The default at creation.
    Active,
    /// Deletion requested; locked and inaccessible while the tail runs. This
    /// mark is the deletion saga's durable queue.
    PendingDeletion,
}

crate::wire_enum!(OrganizationStatus {
    Active => "active",
    PendingDeletion => "pending_deletion",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<OrganizationStatus>();
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("pending".parse::<OrganizationStatus>().is_err());
    }
}
