//! An upstream's answer to `/internal/authorize`.
//!
//! Every sibling service that checks permissions with auth resolves the
//! incoming request to an [`Acting`], which names the person, the organization
//! they are acting on, and if the request was scoped to a project, the role
//! they hold within it.

use serde::{Deserialize, Serialize};

use crate::rbac::{Resource, Verb, can};
use crate::types::{OrganizationId, OrganizationRole, ProjectId, Role, UserId};
use crate::{AuthError, AuthzError, TelmoniError};

/// The authorization resolution auth returned for a request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acting {
    /// The authenticated person.
    pub user_id: UserId,
    /// The organization they act on, taken from the `x-organization-id` header
    /// or, if absent, the active organization on their session.
    pub organization_id: OrganizationId,
    /// The role the person holds on the organization, or `None` if they are
    /// not a member of it.
    pub organization_role: Option<OrganizationRole>,
    /// The project they act on and their role within it, when the request
    /// scoped to a project.
    pub project: Option<ActingProject>,
    /// The session the bearer belongs to. Optional on the wire so a module's
    /// own double can answer without inventing one; auth always names it.
    pub session_id: Option<String>,
    /// When the bearer stops being taken, unix seconds. Nothing built on
    /// this answer outlives it.
    pub expires_at: i64,
}

/// The project half of [`Acting`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActingProject {
    /// The project acted on.
    pub project_id: ProjectId,
    /// The person's role on it — the organization owner's, a seat, or the
    /// admin role an organization admin holds on every project.
    pub role: Role,
}

impl Acting {
    /// Owner-only lanes, such as deleting the organization or transferring it.
    /// The owner is whoever holds the organization's `owner` row, which a transfer moves.
    pub fn require_owner(&self) -> Result<&Self, TelmoniError> {
        if self.organization_role != Some(OrganizationRole::Owner) {
            return Err(AuthzError::Forbidden(
                "only the organization owner may act on the organization".into(),
            )
            .into());
        }
        Ok(self)
    }

    /// A project lane its organization's owner alone may use, such as a
    /// project's content mode, which changes what the organization is liable
    /// for. The matrix lets an admin update a project, so this reads the
    /// organization role; the refusal names the owner, the project role that
    /// would do, as the matrix's refusals name theirs.
    pub fn require_project_owner(&self) -> Result<&ActingProject, TelmoniError> {
        let project = self.project_or_bad_request()?;
        if self.organization_role != Some(OrganizationRole::Owner) {
            return Err(AuthzError::InsufficientRole {
                required: Role::Owner,
                actual: project.role,
            }
            .into());
        }
        Ok(project)
    }

    /// Organization admin or owner lanes, such as organization feeds.
    pub fn require_organization_admin(&self) -> Result<&Self, TelmoniError> {
        match self.organization_role {
            Some(OrganizationRole::Owner | OrganizationRole::Admin) => Ok(self),
            _ => Err(AuthzError::Forbidden(
                "only an organization owner or admin may act on the organization".into(),
            )
            .into()),
        }
    }

    /// A project lane: the project named, with a role the matrix admits.
    pub fn require_project(
        &self,
        verb: Verb,
        resource: Resource,
    ) -> Result<&ActingProject, TelmoniError> {
        let project = self.project_or_bad_request()?;
        if !can(project.role, verb, resource) {
            return Err(AuthzError::Forbidden(format!(
                "role {} may not {} {}",
                project.role,
                format!("{verb:?}").to_lowercase(),
                format!("{resource:?}").to_lowercase()
            ))
            .into());
        }
        Ok(project)
    }

    /// The project named, at any role auth admitted.
    pub fn project_or_bad_request(&self) -> Result<&ActingProject, TelmoniError> {
        self.project
            .as_ref()
            .ok_or_else(|| AuthError::BadRequest("missing x-project-id header".into()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(exp: i64) -> serde_json::Value {
        serde_json::json!({
            "user_id": "user_1",
            "organization_id": "org_1",
            "organization_role": "owner",
            "project": { "project_id": "proj_1", "role": "owner" },
            "session_id": "sid_1",
            "expires_at": exp,
        })
    }

    #[test]
    fn require_owner_and_require_project_read_the_tuple() {
        let acting: Acting = serde_json::from_value(answer(0)).unwrap();
        assert!(acting.require_owner().is_ok());
        assert!(acting.require_organization_admin().is_ok());
        assert!(
            acting
                .require_project(Verb::Delete, Resource::Connector)
                .is_ok()
        );
        let admin = Acting {
            organization_role: Some(OrganizationRole::Admin),
            ..acting.clone()
        };
        assert!(admin.require_owner().is_err());
        assert!(admin.require_organization_admin().is_ok());

        let member = Acting {
            organization_role: Some(OrganizationRole::Member),
            project: Some(ActingProject {
                project_id: ProjectId::try_new("proj_1").unwrap(),
                role: Role::Member,
            }),
            ..acting
        };
        assert!(member.require_owner().is_err());
        assert!(member.require_organization_admin().is_err());
        assert!(
            member
                .require_project(Verb::Delete, Resource::Connector)
                .is_err()
        );
        assert!(
            member
                .require_project(Verb::Read, Resource::Connector)
                .is_ok()
        );
    }

    /// ⚠ The organization's owner alone, though an admin may update a
    /// project: the refusal names the owner and the role held.
    #[test]
    fn require_project_owner_admits_the_organization_owner_alone() {
        let owner: Acting = serde_json::from_value(answer(0)).unwrap();
        assert!(owner.require_project_owner().is_ok());

        for (organization_role, role) in [
            (OrganizationRole::Admin, Role::Admin),
            (OrganizationRole::Member, Role::Admin),
            (OrganizationRole::Member, Role::Member),
        ] {
            let below = Acting {
                organization_role: Some(organization_role),
                project: Some(ActingProject {
                    project_id: ProjectId::try_new("proj_1").unwrap(),
                    role,
                }),
                ..owner.clone()
            };
            let refused = below.require_project_owner().unwrap_err();
            assert!(
                matches!(
                    &refused,
                    TelmoniError::Authz(AuthzError::InsufficientRole {
                        required: Role::Owner,
                        actual,
                    }) if *actual == role
                ),
                "{organization_role:?} seated as {role} was not refused as below the owner: {refused:?}"
            );
        }

        let no_project = Acting {
            project: None,
            ..owner
        };
        assert!(no_project.require_project_owner().is_err());
    }
}
