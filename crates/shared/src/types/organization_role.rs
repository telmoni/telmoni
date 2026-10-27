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

    /// Whether this role can create new projects.
    #[must_use]
    pub const fn can_create_projects(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Whether this role can delete projects.
    #[must_use]
    pub const fn can_delete_projects(&self) -> bool {
        matches!(self, Self::Owner)
    }

    /// Whether this role can manage organization-wide settings.
    #[must_use]
    pub const fn can_manage_org_settings(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Whether this role can view rolled-up audit logs across all projects.
    #[must_use]
    pub const fn can_view_rolled_up_audit(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Whether this role can manage organization-level members and invites.
    #[must_use]
    pub const fn can_manage_org_members(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Whether this role can view organization-level members.
    #[must_use]
    pub const fn can_view_org_members(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Whether this role can take the organization's export: its roster,
    /// invitations, key metadata and audit chain, which both already read.
    #[must_use]
    pub const fn can_export_organization(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
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

    #[test]
    fn permissions_per_role() {
        assert!(OrganizationRole::Owner.can_create_projects());
        assert!(OrganizationRole::Owner.can_delete_projects());
        assert!(OrganizationRole::Owner.can_manage_org_settings());
        assert!(OrganizationRole::Owner.can_view_rolled_up_audit());
        assert!(OrganizationRole::Owner.can_manage_org_members());
        assert!(OrganizationRole::Owner.can_view_org_members());
        assert!(OrganizationRole::Owner.can_export_organization());

        assert!(OrganizationRole::Admin.can_create_projects());
        assert!(!OrganizationRole::Admin.can_delete_projects());
        assert!(OrganizationRole::Admin.can_manage_org_settings());
        assert!(OrganizationRole::Admin.can_view_rolled_up_audit());
        assert!(OrganizationRole::Admin.can_manage_org_members());
        assert!(OrganizationRole::Admin.can_view_org_members());
        assert!(OrganizationRole::Admin.can_export_organization());

        assert!(!OrganizationRole::Member.can_create_projects());
        assert!(!OrganizationRole::Member.can_delete_projects());
        assert!(!OrganizationRole::Member.can_manage_org_settings());
        assert!(!OrganizationRole::Member.can_view_rolled_up_audit());
        assert!(!OrganizationRole::Member.can_manage_org_members());
        assert!(!OrganizationRole::Member.can_view_org_members());
        assert!(!OrganizationRole::Member.can_export_organization());
    }
}
