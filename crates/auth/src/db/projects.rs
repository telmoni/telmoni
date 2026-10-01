//! `auth.projects` — the row every project-scoped foreign key points at.

use serde::Serialize;
use sqlx::prelude::FromRow;

use telmoni_shared::db::tenant_session::{
    Binding, Maintenance, Organization, PersonAndOrganization, ProjectAndOrganization,
    ProjectAndPerson, Scoped,
};
use telmoni_shared::{
    OrganizationId, OrganizationRole, OrganizationStatus, ProjectId, Role, UserId, derive_shard_key,
};

use crate::db::AuthLane;

/// Bindings a project is created under: the organization's own, and the
/// person's with it at first-sign-in provisioning.
pub trait ProjectCreate: Binding {}
impl ProjectCreate for Organization {}
impl ProjectCreate for PersonAndOrganization {}

/// Create an organization's project.
pub async fn create<B: ProjectCreate>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    owner: &OrganizationId,
    name: &str,
) -> sqlx::Result<bool> {
    let rows_affected = sqlx::query(
        "INSERT INTO auth.projects
             (id, external_id, organization_id, name, shard_key)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (external_id) DO NOTHING",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(project_id)
    .bind(owner)
    .bind(name)
    .bind(derive_shard_key(project_id))
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(rows_affected > 0)
}

/// One project as the console's switcher lists it.
#[derive(Debug, Serialize, FromRow, Clone)]
pub struct ProjectSummary {
    /// The project's `external_id` — the console URL segment and the value
    /// the services key on.
    pub id: ProjectId,
    pub name: String,
    /// The caller's role on it, as `telmoni_shared::rbac::project_role`
    /// decides it.
    pub role: Role,
}

/// `telmoni_shared::rbac::project_role` in SQL, for the two list reads below:
/// the organization's owner and admins hold that role on every project, a
/// seat cannot lower it, and below it the seat alone decides. Expects `tm`
/// (the seat) and `om` (the person's organization row) joined.
const ROLE_ON_PROJECT: &str = "CASE WHEN om.role = 'owner' THEN 'owner'
                                    WHEN om.role = 'admin' THEN 'admin'
                                    ELSE tm.role::text
                               END";

/// Every project this person can open inside one organization, by name, with
/// the role they hold on each — none in an organization being deleted, which
/// nothing acts in. The organization's projects and the person's rows in it
/// are read together.
pub async fn list_for_organization_user(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<Vec<ProjectSummary>> {
    sqlx::query_as::<_, ProjectSummary>(&format!(
        "SELECT t.external_id AS id, t.name, {ROLE_ON_PROJECT} AS role
           FROM auth.projects t
           JOIN auth.organizations o
             ON o.external_id = t.organization_id AND o.status = 'active'
           LEFT JOIN auth.project_members tm
                  ON tm.project_id = t.external_id AND tm.user_id = $2
           LEFT JOIN auth.organization_members om
                  ON om.organization_id = t.organization_id AND om.user_id = $2
          WHERE t.organization_id = $1
            AND (om.role IN ('owner', 'admin') OR tm.user_id IS NOT NULL)
          ORDER BY t.name"
    ))
    .bind(organization_id)
    .bind(user_id)
    .fetch_all(tx.conn())
    .await
}

/// The organization a project belongs to — the audit chain's root for anything
/// done inside it.
pub async fn organization_of<B: Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<OrganizationId>> {
    sqlx::query_scalar::<_, OrganizationId>(
        "SELECT organization_id FROM auth.projects WHERE external_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await
}

/// What `acting_project` decides a caller's role from: the project's
/// organization and its status, the caller's seat on the project, and their
/// row on the organization's roster.
#[derive(Debug, FromRow)]
pub struct ActingContext {
    pub organization_id: OrganizationId,
    pub status: OrganizationStatus,
    pub seat: Option<Role>,
    pub organization_role: Option<OrganizationRole>,
}

/// One read for [`ActingContext`]: each table through the policy that admits
/// exactly it — `project_self_read`, `project_read`, `tenant_isolation`,
/// `member_read` — so the roster beyond the caller's own row stays closed.
/// Runs behind every `resolve` a sibling module makes, which is why it is
/// one statement.
pub async fn acting_context(
    tx: &mut Scoped<'_, ProjectAndPerson>,
    project_id: &ProjectId,
    user_id: &UserId,
) -> sqlx::Result<Option<ActingContext>> {
    sqlx::query_as::<_, ActingContext>(
        "SELECT p.organization_id, o.status,
                (SELECT pm.role FROM auth.project_members pm
                  WHERE pm.project_id = p.external_id AND pm.user_id = $2) AS seat,
                (SELECT om.role FROM auth.organization_members om
                  WHERE om.organization_id = p.organization_id AND om.user_id = $2)
                    AS organization_role
           FROM auth.projects p
           JOIN auth.organizations o ON o.external_id = p.organization_id
          WHERE p.external_id = $1",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// A project's name and the organization holding it, read under any binding
/// that may see the row.
#[derive(Debug, FromRow)]
pub struct Standing {
    pub name: String,
    pub organization_id: OrganizationId,
}

/// What a project is called and whose it is; `None` when the binding sees no
/// such project.
pub async fn standing<B: Binding>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<Standing>> {
    sqlx::query_as::<_, Standing>(
        "SELECT name, organization_id FROM auth.projects WHERE external_id = $1",
    )
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await
}

/// How many projects an organization holds.
pub async fn count_in(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    organization_id: &OrganizationId,
) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM auth.projects WHERE organization_id = $1")
        .bind(organization_id)
        .fetch_one(tx.conn())
        .await
}

/// Whether a project in the organization is called `name`, compared as
/// `projects_organization_name_key` compares.
pub async fn holds_name(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    organization_id: &OrganizationId,
    name: &str,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM auth.projects
                         WHERE organization_id = $1 AND lower(name) = lower($2))",
    )
    .bind(organization_id)
    .bind(name)
    .fetch_one(tx.conn())
    .await
}

/// The first of `candidates` that no project in the organization is called.
/// Compared with Postgres's `lower()`, as `projects_organization_name_key`
/// compares: Rust's lowercasing differs on a few characters (`İ`, a final
/// `Σ`), and the index is the judge.
pub async fn first_free_name(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    organization_id: &OrganizationId,
    candidates: &[String],
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT t.name FROM unnest($2::text[]) WITH ORDINALITY AS t(name, position)
          WHERE NOT EXISTS (SELECT 1 FROM auth.projects p
                             WHERE p.organization_id = $1 AND lower(p.name) = lower(t.name))
          ORDER BY t.position
          LIMIT 1",
    )
    .bind(organization_id)
    .bind(candidates)
    .fetch_optional(tx.conn())
    .await
}

/// Hand a project from one organization to another, under `name`: the ONE
/// cross-tenant write in the schema, which is why it runs in the lane —
/// `tenant_isolation` on `auth.projects` would refuse the new row under either
/// organization's binding. Its API keys follow through
/// `api_tokens_project_fkey`'s `ON UPDATE CASCADE`; its seats and invitations
/// are keyed on the project and need no move. `false` when the project is not
/// `from`'s. A name already taken in `to` fails on
/// `projects_organization_name_key`, which the caller names.
pub async fn move_to_organization(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    project_id: &ProjectId,
    from: &OrganizationId,
    to: &OrganizationId,
    name: &str,
) -> sqlx::Result<bool> {
    let rows_affected = sqlx::query(
        "UPDATE auth.projects SET organization_id = $3, name = $4, updated_at = now()
          WHERE external_id = $1 AND organization_id = $2",
    )
    .bind(project_id)
    .bind(from)
    .bind(to)
    .bind(name)
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(rows_affected > 0)
}

/// Update a project's name. Keyed on the organization as well as the project:
/// the policy is the floor, the predicate says what the statement means.
pub async fn update_name(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    name: &str,
) -> sqlx::Result<bool> {
    let rows_affected = sqlx::query(
        "UPDATE auth.projects SET name = $1, updated_at = now()
          WHERE external_id = $2 AND organization_id = $3",
    )
    .bind(name)
    .bind(project_id)
    .bind(organization_id)
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(rows_affected > 0)
}

/// Destroy a project, answering its name for the audit row; `None` when
/// there was nothing to destroy.
pub async fn delete(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "DELETE FROM auth.projects
          WHERE external_id = $1 AND organization_id = $2
      RETURNING name",
    )
    .bind(project_id)
    .bind(organization_id)
    .fetch_optional(tx.conn())
    .await
}

/// One project as the switcher lists it ACROSS organizations, labelled so a
/// person can tell "Platform" here from "Platform" there.
#[derive(Debug, Serialize, FromRow, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEverywhere {
    pub id: ProjectId,
    pub name: String,
    pub role: Role,
    pub organization_id: OrganizationId,
    /// What the organization's owner called it; `None` means never named.
    pub organization_name: Option<String>,
    /// The organization's owner's address — what a page shows when it has no
    /// name.
    pub organization_owner_email: Option<String>,
}

/// Every project this person can open in ANY active organization. One query
/// rather than one per organization, because the rail renders on every page.
/// The owner who labels each organization is somebody else.
pub async fn list_everywhere_for_user(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_id: &UserId,
) -> sqlx::Result<Vec<ProjectEverywhere>> {
    // Starts from the person's two membership indexes, not from every project:
    // an OR across two LEFT JOINs cannot be driven by either and walked every
    // tenant's projects on every page.
    sqlx::query_as::<_, ProjectEverywhere>(&format!(
        "WITH reachable AS (
            SELECT t.external_id
              FROM auth.organization_members om
              JOIN auth.projects t ON t.organization_id = om.organization_id
             WHERE om.user_id = $1 AND om.role IN ('owner', 'admin')
             UNION
            SELECT tm.project_id FROM auth.project_members tm WHERE tm.user_id = $1
         )
         SELECT t.external_id AS id, t.name, {ROLE_ON_PROJECT} AS role,
                t.organization_id,
                a.name AS organization_name,
                owner_identity.email AS organization_owner_email
           FROM reachable r
           JOIN auth.projects t ON t.external_id = r.external_id
           JOIN auth.organizations a
             ON a.external_id = t.organization_id AND a.status = 'active'
           LEFT JOIN auth.project_members tm
                  ON tm.project_id = t.external_id AND tm.user_id = $1
           LEFT JOIN auth.organization_members om
                  ON om.organization_id = t.organization_id AND om.user_id = $1
           LEFT JOIN auth.organization_members owner_row
                  ON owner_row.organization_id = t.organization_id AND owner_row.role = 'owner'
           LEFT JOIN auth.identities owner_identity
                  ON owner_identity.user_id = owner_row.user_id
          ORDER BY COALESCE(a.name, owner_identity.email), t.organization_id, t.name"
    ))
    .bind(user_id)
    .fetch_all(tx.conn())
    .await
}
