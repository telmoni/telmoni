//! [`AuditAction`] — closed vocabulary of audit-loggable actions.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

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

impl AuditAction {
    /// Every action, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Created, Self::Updated, Self::Deleted, Self::Exported]
    }
}

impl fmt::Display for AuditAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Updated => write!(f, "updated"),
            Self::Deleted => write!(f, "deleted"),
            Self::Exported => write!(f, "exported"),
        }
    }
}

impl FromStr for AuditAction {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(Self::Created),
            "updated" => Ok(Self::Updated),
            "deleted" => Ok(Self::Deleted),
            "exported" => Ok(Self::Exported),
            _ => Err(ParseEnumError::new(
                s,
                "created, updated, deleted, exported",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, for the round-trip below.
    const ALL: &[AuditAction] = &AuditAction::all();

    /// `ALL` covers the enum, and says so by count.
    #[test]
    fn all_lists_every_variant() {
        fn named(action: AuditAction) -> &'static str {
            match action {
                AuditAction::Created => "created",
                AuditAction::Updated => "updated",
                AuditAction::Deleted => "deleted",
                AuditAction::Exported => "exported",
            }
        }
        assert_eq!(
            ALL.len(),
            4,
            "a variant reached the enum without reaching ALL"
        );
        for action in ALL {
            assert_eq!(named(*action), action.to_string());
        }
    }

    #[test]
    fn display_matches_snake_case() {
        assert_eq!(AuditAction::Created.to_string(), "created");
        assert_eq!(AuditAction::Deleted.to_string(), "deleted");
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in ALL {
            assert_eq!(v.to_string().parse::<AuditAction>().unwrap(), *v);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("CREATED".parse::<AuditAction>().is_err());
        assert!("".parse::<AuditAction>().is_err());
        for retired in ["read", "invoked", "recorded", "authorised", "denied"] {
            assert!(retired.parse::<AuditAction>().is_err(), "{retired}");
        }
    }

    #[test]
    fn serde_uses_snake_case_wire() {
        assert_eq!(
            serde_json::to_string(&AuditAction::Updated).unwrap(),
            "\"updated\""
        );
        let back: AuditAction = serde_json::from_str("\"deleted\"").unwrap();
        assert_eq!(back, AuditAction::Deleted);
    }
}
