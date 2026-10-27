//! [`AuditAction`] — closed vocabulary of audit-loggable actions.

use serde::{Deserialize, Serialize};

/// Closed set of audit actions. Spelled `Authorised` (British) to match the
/// wire vocabulary, so nobody introduces a parallel `authorized` by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// Resource came into existence (POST / CREATE).
    Created,
    /// Resource state mutated (PATCH / PUT / UPDATE).
    Updated,
    /// Resource removed (DELETE).
    Deleted,
    /// A copy of the resource's data left the platform. ⚠ **The one READ this
    /// vocabulary spells**: reads are not audited as a rule, but an
    /// organization's whole record leaving the building is exactly what a leak
    /// investigation must find. The row records that it happened, never what
    /// was in it.
    Exported,
}

crate::wire_enum!(AuditAction {
    Created => "created",
    Updated => "updated",
    Deleted => "deleted",
    Exported => "exported",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<AuditAction>();
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("CREATED".parse::<AuditAction>().is_err());
        assert!("".parse::<AuditAction>().is_err());
        for retired in ["read", "invoked", "recorded", "authorised", "denied"] {
            assert!(retired.parse::<AuditAction>().is_err(), "{retired}");
        }
    }
}
