//! [`Role`] — telmoni-side RBAC role for a user within a project.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// A user's role within a project. The same three names as
/// [`crate::OrganizationRole`], on purpose: a person reads one ladder, and
/// which level it is on comes from where they are standing. The two enums
/// stay separate because the levels answer different questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full control: the organization owner's role on every project it holds.
    /// Never granted on a project — the organization's one `owner` row is the
    /// only source, and only an ownership transfer moves it.
    Owner,
    /// Runs the project: everything the owner may do on it except delete it.
    /// See [`crate::rbac::can`]. An organization admin holds it on every
    /// project, whatever seat they also hold.
    Admin,
    /// The read-only role: reads members, tokens and connectors, not the
    /// audit log.
    Member,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owner => write!(f, "owner"),
            Self::Admin => write!(f, "admin"),
            Self::Member => write!(f, "member"),
        }
    }
}

impl FromStr for Role {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            _ => Err(ParseEnumError::new(s, "owner, admin, member")),
        }
    }
}

impl Role {
    /// Every variant, most privileged first, so a caller that must enumerate
    /// roles never hand-writes a list that can fall behind.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Owner, Self::Admin, Self::Member]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_snake_case() {
        assert_eq!(Role::Owner.to_string(), "owner");
        assert_eq!(Role::Admin.to_string(), "admin");
        assert_eq!(Role::Member.to_string(), "member");
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in Role::all() {
            assert_eq!(v.to_string().parse::<Role>().unwrap(), v);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("readonly".parse::<Role>().is_err());
        assert!("OWNER".parse::<Role>().is_err());
        assert!("".parse::<Role>().is_err());
    }

    /// Every spelling that is not a project role must fail loudly here.
    #[test]
    fn the_retired_spellings_are_unparseable() {
        for retired in [
            "editor",
            "viewer",
            "read_only",
            "developer",
            "security",
            "contributor",
        ] {
            assert!(
                retired.parse::<Role>().is_err(),
                "`{retired}` is not a role and must not parse as one"
            );
        }
    }

    #[test]
    fn serde_uses_snake_case_wire() {
        assert_eq!(serde_json::to_string(&Role::Member).unwrap(), "\"member\"");
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), "\"admin\"");
        let back: Role = serde_json::from_str("\"admin\"").unwrap();
        assert_eq!(back, Role::Admin);
    }

    /// `serde` and `Display` must agree, or a role written by one path and read
    /// by the other silently changes meaning.
    #[test]
    fn serde_and_display_agree_on_every_variant() {
        for v in Role::all() {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, format!("\"{v}\""), "serde/Display disagree on {v:?}");
        }
    }
}
