//! [`OrganizationStatus`] — an organization's lifecycle state.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

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
    /// Fully deleted — retained only as a terminal marker.
    Deleted,
}

impl fmt::Display for OrganizationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::PendingDeletion => write!(f, "pending_deletion"),
            Self::Deleted => write!(f, "deleted"),
        }
    }
}

impl FromStr for OrganizationStatus {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "pending_deletion" => Ok(Self::PendingDeletion),
            "deleted" => Ok(Self::Deleted),
            _ => Err(ParseEnumError::new(s, "active, pending_deletion, deleted")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_spelling_is_snake_case() {
        assert_eq!(OrganizationStatus::Active.to_string(), "active");
        assert_eq!(
            OrganizationStatus::PendingDeletion.to_string(),
            "pending_deletion"
        );
        assert_eq!(
            serde_json::to_string(&OrganizationStatus::PendingDeletion).unwrap(),
            "\"pending_deletion\""
        );
        let back: OrganizationStatus = serde_json::from_str("\"active\"").unwrap();
        assert_eq!(back, OrganizationStatus::Active);
    }

    #[test]
    fn from_str_round_trips_and_rejects_unknown() {
        for v in [
            OrganizationStatus::Active,
            OrganizationStatus::PendingDeletion,
            OrganizationStatus::Deleted,
        ] {
            assert_eq!(v.to_string().parse::<OrganizationStatus>().unwrap(), v);
        }
        assert!("pending".parse::<OrganizationStatus>().is_err());
    }
}
