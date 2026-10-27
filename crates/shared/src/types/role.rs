//! [`Role`] — telmoni-side RBAC role for a user within a project.

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

// Most privileged first, which `rbac::minimum_role` searches from the end of.
crate::wire_enum!(Role {
    Owner => "owner",
    Admin => "admin",
    Member => "member",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<Role>();
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
}
