//! [`OrganizationRole`] — telmoni-side role of a user within an organization.

use serde::{Deserialize, Serialize};

/// Role a user holds across an entire organization.
///
/// Organization-wide roles govern the organization's management: creating
/// projects, managing organization-level settings, and invite authority.
/// Project-level permissions remain governed by [`crate::Role`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRole {
    /// Full access to the organization and all its projects.
    ///
    /// Can create and delete projects, manage organization settings, invite
    /// and remove organization-level members.
    Owner,
    /// Administrative access across all projects in the organization.
    ///
    /// Can create projects, manage organization settings, members and audit
    /// logs. Cannot delete the organization or its projects, hand over
    /// ownership, or hand a project to another organization.
    Admin,
    /// Default role for invited members.
    ///
    /// Has access only to projects they are explicitly added to, with
    /// permissions governed by their project-level role. Cannot create
    /// projects or manage organization settings.
    Member,
}

// Most privileged first.
crate::wire_enum!(OrganizationRole {
    Owner => "owner",
    Admin => "admin",
    Member => "member",
});

impl OrganizationRole {
    /// User-facing label, Title Cased.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Admin => "Admin",
            Self::Member => "Member",
        }
    }

    /// User-facing description of permissions.
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::Owner => {
                "Full organization access including project creation and deletion, organization settings, and rolled-up audit logs across all projects."
            }
            Self::Admin => {
                "Can create projects, manage organization settings, members and audit logs, and works as an admin in every project."
            }
            Self::Member => {
                "No organization-wide powers by default; access comes entirely from project-level roles."
            }
        }
    }

    /// Whether this role may do `action` to the organization.
    #[must_use]
    pub const fn may(self, action: OrganizationAction) -> bool {
        match action.minimum() {
            Self::Owner => matches!(self, Self::Owner),
            Self::Admin => matches!(self, Self::Owner | Self::Admin),
            Self::Member => true,
        }
    }
}

/// What a person may do to the organization itself, as distinct from a
/// project in it ([`crate::rbac::can`]): each names the least role that may,
/// and how a refusal says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizationAction {
    /// Read its roster and its invitations.
    ViewMembers,
    /// Invite, change a role, remove, and revoke an invitation.
    ManageMembers,
    /// Seat a member on a project from the roster.
    GiveSeats,
    /// Change its name or URL.
    ManageSettings,
    /// Create a project in it.
    CreateProjects,
    /// Read, and export, its audit log across every project.
    ReadWholeAudit,
    /// Take its export.
    Export,
    /// Delete it, and everything in it.
    Delete,
    /// Hand it over, or withdraw the offer.
    HandOver,
}

impl OrganizationAction {
    /// Every action, in the order above.
    #[must_use]
    pub const fn all() -> [Self; 9] {
        [
            Self::ViewMembers,
            Self::ManageMembers,
            Self::GiveSeats,
            Self::ManageSettings,
            Self::CreateProjects,
            Self::ReadWholeAudit,
            Self::Export,
            Self::Delete,
            Self::HandOver,
        ]
    }

    /// The least role that may.
    #[must_use]
    pub const fn minimum(self) -> OrganizationRole {
        match self {
            Self::Delete | Self::HandOver => OrganizationRole::Owner,
            Self::ViewMembers
            | Self::ManageMembers
            | Self::GiveSeats
            | Self::ManageSettings
            | Self::CreateProjects
            | Self::ReadWholeAudit
            | Self::Export => OrganizationRole::Admin,
        }
    }

    /// What it is, as a refusal names it after "can".
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            Self::ViewMembers => "view its members and invitations",
            Self::ManageMembers => "manage its members and invitations",
            Self::GiveSeats => "add a member from its roster",
            Self::ManageSettings => "change its name or URL",
            Self::CreateProjects => "create projects",
            Self::ReadWholeAudit => "read its whole audit log",
            Self::Export => "export it",
            Self::Delete => "delete it",
            Self::HandOver => "hand it over",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<OrganizationRole>();
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("editor".parse::<OrganizationRole>().is_err());
        assert!("viewer".parse::<OrganizationRole>().is_err());
        assert!("OWNER".parse::<OrganizationRole>().is_err());
    }

    /// The owner may everything, an admin all but what is the owner's,
    /// and a member nothing of the organization itself.
    #[test]
    fn each_action_names_the_least_role_that_may() {
        for action in OrganizationAction::all() {
            assert!(OrganizationRole::Owner.may(action), "{action:?}");
            assert_eq!(
                OrganizationRole::Admin.may(action),
                action.minimum() == OrganizationRole::Admin,
                "{action:?}"
            );
            assert!(!OrganizationRole::Member.may(action), "{action:?}");
        }
        assert_eq!(
            OrganizationAction::all()
                .iter()
                .filter(|action| action.minimum() == OrganizationRole::Owner)
                .count(),
            2,
            "deleting and handing over are the owner's alone"
        );
    }
}
