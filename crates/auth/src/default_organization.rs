//! The organization, and the project in it, that every person created here
//! joins:
//! `TELMONI_DEFAULT_ORG_ID` with `TELMONI_DEFAULT_ORG_ROLE`, and
//! `TELMONI_DEFAULT_PROJECT_ID` with `TELMONI_DEFAULT_PROJECT_ROLE`. A person
//! joins once, in the transaction that creates them, so one removed later is
//! not put back at their next sign-in.

use anyhow::bail;
use serde_json::json;
use sqlx::PgPool;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain};
use telmoni_shared::config::optional;
use telmoni_shared::db::tenant_session::{Person, Scoped, maintenance_scope};
use telmoni_shared::{
    AuditAction, OrganizationId, OrganizationRole, OrganizationStatus, ProjectId, Role,
    TelmoniError, TelmoniResourceKind, UserId,
};

use crate::db::{AuthLane, members, organization_members, organizations, projects};
use crate::model::AuditKind;

/// Who the joining is recorded as: the deployment's setting, never a person.
const ACTOR: &str = "default-organization";

/// Where every person created here starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultOrganization {
    /// The organization they join.
    pub organization_id: OrganizationId,
    /// Their role on its roster: `member` or `admin`, never its one owner.
    pub role: OrganizationRole,
    /// The project of the organization they are seated on, with the seat's
    /// role, when one is named.
    pub project: Option<(ProjectId, Role)>,
}

impl DefaultOrganization {
    /// The four variables as the environment sets them; `None` when no
    /// organization is named.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        Self::from_values(
            optional("TELMONI_DEFAULT_ORG_ID"),
            optional("TELMONI_DEFAULT_ORG_ROLE"),
            optional("TELMONI_DEFAULT_PROJECT_ID"),
            optional("TELMONI_DEFAULT_PROJECT_ROLE"),
        )
    }

    fn from_values(
        organization: Option<String>,
        organization_role: Option<String>,
        project: Option<String>,
        project_role: Option<String>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(organization) = organization else {
            // A role or a project without the organization is a mistake,
            // said before anybody signs up.
            if let Some(name) = [
                ("TELMONI_DEFAULT_ORG_ROLE", &organization_role),
                ("TELMONI_DEFAULT_PROJECT_ID", &project),
                ("TELMONI_DEFAULT_PROJECT_ROLE", &project_role),
            ]
            .into_iter()
            .find_map(|(name, value)| value.is_some().then_some(name))
            {
                bail!("{name} is set without TELMONI_DEFAULT_ORG_ID");
            }
            return Ok(None);
        };
        let Ok(organization_id) = OrganizationId::try_new(organization.trim()) else {
            bail!(
                "TELMONI_DEFAULT_ORG_ID is no organization's id: copy it from the organization's \
                 Settings"
            );
        };
        let role = match organization_role.as_deref() {
            None => OrganizationRole::Member,
            Some(role) => roster_role(role)?,
        };
        let project = match (project, project_role) {
            (None, Some(_)) => {
                bail!("TELMONI_DEFAULT_PROJECT_ROLE is set without TELMONI_DEFAULT_PROJECT_ID")
            }
            (None, None) => None,
            (Some(project), role) => {
                let Ok(project_id) = ProjectId::try_new(project.trim()) else {
                    bail!(
                        "TELMONI_DEFAULT_PROJECT_ID is no project's id: copy it from the \
                         project's Settings"
                    );
                };
                let role = match role.as_deref() {
                    None => Role::Member,
                    Some(role) => seat_role(role)?,
                };
                Some((project_id, role))
            }
        };
        Ok(Some(Self {
            organization_id,
            role,
            project,
        }))
    }

    /// Refuses an organization that is not active, or a project that is not
    /// an active one of it: read at boot, so a mistyped id is said before
    /// anybody signs up.
    pub async fn check(&self, pool: &PgPool) -> anyhow::Result<()> {
        let mut tx = maintenance_scope(pool, AuthLane).await?;
        let status = organizations::status(&mut tx, &self.organization_id).await?;
        let home = match &self.project {
            Some((project_id, _)) => projects::home_of_active(&mut tx, project_id).await?,
            None => None,
        };
        tx.commit().await?;
        if status != Some(OrganizationStatus::Active) {
            bail!("TELMONI_DEFAULT_ORG_ID names no active organization");
        }
        if self.project.is_some() && home.as_ref() != Some(&self.organization_id) {
            bail!(
                "TELMONI_DEFAULT_PROJECT_ID names no active project of TELMONI_DEFAULT_ORG_ID's \
                 organization"
            );
        }
        Ok(())
    }
}

/// A roster role as the variable spells it, in any case.
fn roster_role(role: &str) -> anyhow::Result<OrganizationRole> {
    match role.trim().to_ascii_lowercase().as_str() {
        "member" => Ok(OrganizationRole::Member),
        "admin" => Ok(OrganizationRole::Admin),
        "owner" => bail!(
            "TELMONI_DEFAULT_ORG_ROLE is owner: an organization has one owner, who is never made \
             by a sign-up; set member or admin"
        ),
        "viewer" => bail!(
            "TELMONI_DEFAULT_ORG_ROLE is viewer, which has no role here: member is the read-only \
             one"
        ),
        "none" => bail!(
            "TELMONI_DEFAULT_ORG_ROLE is none: leave TELMONI_DEFAULT_ORG_ID unset to join nobody"
        ),
        _ => bail!("TELMONI_DEFAULT_ORG_ROLE is member or admin"),
    }
}

/// A seat's role as the variable spells it.
fn seat_role(role: &str) -> anyhow::Result<Role> {
    match role.trim().to_ascii_lowercase().as_str() {
        "member" => Ok(Role::Member),
        "admin" => Ok(Role::Admin),
        "owner" => bail!(
            "TELMONI_DEFAULT_PROJECT_ROLE is owner: a project's owner is its organization's; set \
             member or admin"
        ),
        "viewer" => bail!(
            "TELMONI_DEFAULT_PROJECT_ROLE is viewer, which has no role here: member is the \
             read-only one"
        ),
        "none" => bail!(
            "TELMONI_DEFAULT_PROJECT_ROLE is none: leave TELMONI_DEFAULT_PROJECT_ID unset to seat \
             nobody"
        ),
        _ => bail!("TELMONI_DEFAULT_PROJECT_ROLE is member or admin"),
    }
}

/// Joins `user_id`, being created in `tx`, to `to`'s organization and its
/// project, each row audited as the deployment's act. An organization or a
/// project no longer there is logged and passed over: the person is created
/// all the same, never refused for it.
pub(crate) async fn join<'c>(
    tx: Scoped<'c, Person>,
    user_id: &UserId,
    to: Option<&DefaultOrganization>,
) -> Result<Scoped<'c, Person>, TelmoniError> {
    let Some(to) = to else {
        return Ok(tx);
    };
    let organization_id = &to.organization_id;
    let mut tx = tx.bind_organization(organization_id).await?;
    // Before the status is read, so a deletion cannot land between the read
    // and the row.
    lock_chain(tx.conn(), organization_id).await?;
    if organizations::status(&mut tx, organization_id).await? != Some(OrganizationStatus::Active) {
        tracing::error!(organization_id = %organization_id, user_id = %user_id,
            "TELMONI_DEFAULT_ORG_ID names no active organization any more; a new person joined none");
        return Ok(tx.clear_organization().await?);
    }
    if organization_members::add_member(&mut tx, organization_id, user_id, to.role, user_id).await?
    {
        emit_audit(
            &mut tx,
            AuditEvent::new(
                organization_id,
                Actor::Service(ACTOR),
                AuditAction::Created,
                TelmoniResourceKind::Member,
            )
            .on(user_id.as_str())
            .details(json!({ "kind": AuditKind::DefaultOrganization, "role": to.role })),
        )
        .await?;
    }
    let Some((project_id, role)) = &to.project else {
        return Ok(tx.clear_organization().await?);
    };
    let mut tx = tx.bind_project(project_id).await?;
    if projects::home_of_active(&mut tx, project_id)
        .await?
        .as_ref()
        != Some(organization_id)
    {
        tracing::error!(organization_id = %organization_id, project_id = %project_id,
            user_id = %user_id,
            "TELMONI_DEFAULT_PROJECT_ID names no active project of the default organization any \
             more; a new person was seated on none");
    } else if members::insert_if_absent(&mut tx, project_id, user_id, *role, user_id).await? {
        emit_audit(
            &mut tx,
            AuditEvent::new(
                organization_id,
                Actor::Service(ACTOR),
                AuditAction::Created,
                TelmoniResourceKind::Member,
            )
            .in_project(project_id)
            .on(user_id.as_str())
            .details(json!({ "kind": AuditKind::DefaultOrganization, "role": role })),
        )
        .await?;
    }
    Ok(tx.clear_project().await?.clear_organization().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(values: [Option<&str>; 4]) -> anyhow::Result<Option<DefaultOrganization>> {
        let [organization, organization_role, project, project_role] =
            values.map(|value| value.map(str::to_owned));
        DefaultOrganization::from_values(organization, organization_role, project, project_role)
    }

    /// Unset, nobody joins anything; set, the roles are `member` unless named.
    #[test]
    fn the_organization_is_joined_as_a_member_unless_named() {
        assert_eq!(read([None; 4]).expect("unset"), None);
        let joined = read([Some("org_abc"), None, Some("project_xyz"), None])
            .expect("set")
            .expect("an organization");
        assert_eq!(joined.organization_id.as_str(), "org_abc");
        assert_eq!(joined.role, OrganizationRole::Member);
        let (project_id, role) = joined.project.expect("a project");
        assert_eq!(project_id.as_str(), "project_xyz");
        assert_eq!(role, Role::Member);

        let joined = read([
            Some("org_abc"),
            Some("ADMIN"),
            Some("project_xyz"),
            Some(" Admin "),
        ])
        .expect("set")
        .expect("an organization");
        assert_eq!(joined.role, OrganizationRole::Admin);
        assert_eq!(joined.project.map(|(_, role)| role), Some(Role::Admin));
    }

    /// ⚠ Owner, viewer and none, an unknown role, a malformed id
    /// and a variable without its id each refuse boot, naming the variable.
    #[test]
    fn a_setting_that_cannot_hold_refuses_boot() {
        for (values, variable) in [
            (
                [Some("org_abc"), Some("owner"), None, None],
                "TELMONI_DEFAULT_ORG_ROLE",
            ),
            (
                [Some("org_abc"), Some("VIEWER"), None, None],
                "TELMONI_DEFAULT_ORG_ROLE",
            ),
            (
                [Some("org_abc"), Some("none"), None, None],
                "TELMONI_DEFAULT_ORG_ROLE",
            ),
            (
                [Some("org_abc"), Some("root"), None, None],
                "TELMONI_DEFAULT_ORG_ROLE",
            ),
            (
                [Some("org_abc"), None, Some("project_xyz"), Some("owner")],
                "TELMONI_DEFAULT_PROJECT_ROLE",
            ),
            (
                [Some("org_abc"), None, Some("project_xyz"), Some("viewer")],
                "TELMONI_DEFAULT_PROJECT_ROLE",
            ),
            ([Some("acme"), None, None, None], "TELMONI_DEFAULT_ORG_ID"),
            (
                [Some("org_abc"), None, Some("two words"), None],
                "TELMONI_DEFAULT_PROJECT_ID",
            ),
            (
                [None, Some("admin"), None, None],
                "TELMONI_DEFAULT_ORG_ROLE",
            ),
            (
                [None, None, Some("project_xyz"), None],
                "TELMONI_DEFAULT_PROJECT_ID",
            ),
            (
                [Some("org_abc"), None, None, Some("admin")],
                "TELMONI_DEFAULT_PROJECT_ROLE",
            ),
        ] {
            let refused = read(values).expect_err(&format!("{values:?}")).to_string();
            assert!(refused.starts_with(variable), "{values:?}: {refused}");
        }
    }
}
