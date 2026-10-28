//! Queries against `auth.member_invites` — the offer, before there is a member.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{Maintenance, Project, ProjectAndOrganization, Scoped};
use telmoni_shared::{OrganizationId, OrganizationRole, ProjectId, Role, UserId, derive_shard_key};

use crate::db::AuthLane;
use crate::db::organization_members::{self, LiveOrganizationInvite};
use crate::model::InviteScope;

/// One live invitation, as the roster shows it to owners and admins.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct Invite {
    pub id: Uuid,
    pub email: String,
    pub role: Role,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// An invitation resolved from the secret in a link, with who sent it joined
/// in so the accept page can say who is asking.
#[derive(Debug, sqlx::FromRow)]
pub struct LiveInvite {
    pub id: Uuid,
    /// The project being shared — the one a seat will name.
    pub project_id: ProjectId,
    /// The organization that holds it.
    pub organization_id: OrganizationId,
    /// The address this invitation was sent to; the acceptor must hold it.
    pub email: String,
    pub role: Role,
    /// The person who sent it, for the accept page's "X invited you". `None`
    /// once they have been erased: the invitation outlives them.
    pub inviter_email: Option<String>,
    pub inviter_display_name: Option<String>,
    /// What the organization is called.
    pub organization_name: String,
}

/// Write the offer. **Every add lands here**, for an address with an
/// account and one without alike — nothing here can tell the difference,
/// which keeps the create lane from answering "is this person a customer".
pub async fn create(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    email: &str,
    role: Role,
    token_hash: &str,
    invited_by: &UserId,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO auth.member_invites
             (id, project_id, email, role, token_hash, invited_by, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(email)
    .bind(role)
    .bind(token_hash)
    .bind(invited_by)
    .bind(expires_at)
    .bind(derive_shard_key(project_id))
    .fetch_one(tx.conn())
    .await
}

/// Revoke whatever live invitation this project already sent to this address.
pub async fn revoke_live_for_email(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    email: &str,
) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "UPDATE auth.member_invites SET revoked_at = now()
          WHERE project_id = $1 AND email = $2
            AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(project_id)
    .bind(email)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

/// The owner's pending list, newest first. Expired rows are excluded: an
/// invitation nobody can accept is not pending.
pub async fn list_pending(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<Invite>> {
    sqlx::query_as::<_, Invite>(
        "SELECT id, email, role, expires_at, created_at
           FROM auth.member_invites
          WHERE project_id = $1
            AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()
          ORDER BY created_at DESC, id DESC",
    )
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// An owner or admin takes the offer back. Answers the address it was sent to, so
/// that person's open sessions can be told; `None` when nothing live matched.
pub async fn revoke(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    invite_id: Uuid,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        "UPDATE auth.member_invites SET revoked_at = now()
          WHERE project_id = $1 AND id = $2
            AND accepted_at IS NULL AND revoked_at IS NULL
      RETURNING email",
    )
    .bind(project_id)
    .bind(invite_id)
    .fetch_optional(tx.conn())
    .await
}

/// Resolve the secret in a link to the offer it opens. The caller is named by
/// the link, not a GUC.
pub async fn find_live(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    token_hash: &str,
) -> sqlx::Result<Option<LiveInvite>> {
    sqlx::query_as::<_, LiveInvite>(
        "SELECT i.id, i.project_id, p.organization_id, i.email, i.role,
                inviter.email        AS inviter_email,
                inviter.display_name AS inviter_display_name,
                o.name               AS organization_name
           FROM auth.member_invites i
           JOIN auth.projects p ON p.external_id = i.project_id
           JOIN auth.organizations o
             ON o.external_id = p.organization_id AND o.status = 'active'
           LEFT JOIN auth.identities inviter ON inviter.user_id = i.invited_by
          WHERE i.token_hash = $1
            AND i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > now()",
    )
    .bind(token_hash)
    .fetch_optional(tx.conn())
    .await
}

/// Close the offer, naming who took it.
pub async fn mark_accepted(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    invite_id: Uuid,
    accepted_by: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.member_invites
            SET accepted_at = now(), accepted_by = $2
          WHERE id = $1
            AND accepted_at IS NULL
            AND revoked_at IS NULL
            AND expires_at > now()",
    )
    .bind(invite_id)
    .bind(accepted_by)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// How many invitations this project has sent since `since`, revoked ones
/// included — the relay cap counts sends, not survivors.
pub async fn count_since(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    since: DateTime<Utc>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM auth.member_invites WHERE project_id = $1 AND created_at >= $2",
    )
    .bind(project_id)
    .bind(since)
    .fetch_one(tx.conn())
    .await
}

/// Whether this project already seated somebody holding this address.
///
/// ⚠ The address lives in `auth.identities`, which a project-only scope
/// cannot see, and the answer would be a silent "no".
pub async fn already_a_member_by_email(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    email: &str,
) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1 FROM auth.project_members m
               JOIN auth.identities i ON i.user_id = m.user_id
              WHERE m.project_id = $1 AND i.email = $2
         )",
    )
    .bind(project_id)
    .bind(email)
    .fetch_one(tx.conn())
    .await
}

/// Whether a live invitation, to a project or to an organization, names
/// `email`: what lets the invited create an account while sign-ups are
/// closed. Read in the maintenance lane, since there is no person yet.
pub async fn any_live_for_email(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    email: &str,
) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1 FROM auth.member_invites
              WHERE email = $1 AND accepted_at IS NULL AND revoked_at IS NULL
                AND expires_at > now())
            OR EXISTS (
             SELECT 1 FROM auth.organization_invites
              WHERE email = $1 AND accepted_at IS NULL AND revoked_at IS NULL
                AND expires_at > now())",
    )
    .bind(email)
    .fetch_one(tx.conn())
    .await
}

/// An incoming invitation addressed to the current person's email, across
/// projects and organizations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct IncomingInvite {
    pub id: Uuid,
    /// Which half of the union this row came from, and so how to read `target_id`.
    pub scope: crate::model::InviteScope,
    /// A `ProjectId` or an `OrganizationId` depending on `scope`. A plain
    /// string because no one newtype is honest for both halves of the union.
    pub target_id: String,
    /// The project's name, or the organization's. An organization invites
    /// nobody until it is named, so the latter is never blank in practice.
    pub target_name: String,
    pub role: String,
    /// Who sent it. `None` once they have been erased.
    pub inviter_email: Option<String>,
    pub inviter_display_name: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// What an invitation seats somebody on, at which role there. The two
/// ladders spell their roles the same, so the role alone cannot say what an
/// admin may do; the target can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteTarget {
    /// A seat on one project.
    Project { project_id: ProjectId, role: Role },
    /// A row on the organization's roster.
    Organization { role: OrganizationRole },
}

impl InviteTarget {
    /// Which roster, as the wire says it.
    #[must_use]
    pub const fn scope(&self) -> InviteScope {
        match self {
            Self::Project { .. } => InviteScope::Project,
            Self::Organization { .. } => InviteScope::Organization,
        }
    }

    /// The role, as either ladder spells it.
    #[must_use]
    pub fn role_word(&self) -> &'static str {
        match self {
            Self::Project { role, .. } => role.as_str(),
            Self::Organization { role } => role.as_str(),
        }
    }
}

/// A live invitation the link's token names, from either table: what the
/// accept page says, and what an accept seats.
#[derive(Debug, Clone)]
pub struct Invitation {
    pub id: Uuid,
    /// The organization it is recorded on: the project's owner, or the one
    /// invited to.
    pub organization_id: OrganizationId,
    pub organization_name: String,
    /// The address it was sent to; the acceptor must hold it.
    pub email: String,
    /// Who sent it; `None` once they have been erased.
    pub inviter_email: Option<String>,
    pub inviter_display_name: Option<String>,
    pub target: InviteTarget,
}

impl Invitation {
    fn of_project(live: LiveInvite) -> Self {
        Self {
            id: live.id,
            organization_id: live.organization_id,
            organization_name: live.organization_name,
            email: live.email,
            inviter_email: live.inviter_email,
            inviter_display_name: live.inviter_display_name,
            target: InviteTarget::Project {
                project_id: live.project_id,
                role: live.role,
            },
        }
    }

    fn of_organization(live: LiveOrganizationInvite) -> Self {
        Self {
            id: live.id,
            organization_id: live.organization_id,
            organization_name: live.organization_name,
            email: live.email,
            inviter_email: live.inviter_email,
            inviter_display_name: live.inviter_display_name,
            target: InviteTarget::Organization { role: live.role },
        }
    }
}

/// The live invitation a link's token names, in whichever table holds it.
pub async fn find_live_by_token(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    token_hash: &str,
) -> sqlx::Result<Option<Invitation>> {
    if let Some(live) = find_live(tx, token_hash).await? {
        return Ok(Some(Invitation::of_project(live)));
    }
    Ok(
        organization_members::find_live_organization_invite(tx, token_hash)
            .await?
            .map(Invitation::of_organization),
    )
}

/// A live invitation addressed to the person, by its id, from either table:
/// what the console's accept and decline act on.
#[derive(Debug, Clone)]
pub struct Pending {
    pub id: Uuid,
    pub email: String,
    pub inviter_email: Option<String>,
    /// The organization it is recorded on.
    pub owner_organization_id: OrganizationId,
    pub target: InviteTarget,
}

/// The live invitation `invite_id` names, addressed to `email`, in
/// whichever table holds it.
pub async fn find_incoming(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    invite_id: Uuid,
    email: &str,
) -> sqlx::Result<Option<Pending>> {
    if let Some(invite) = find_incoming_member_invite(tx, invite_id, email).await? {
        return Ok(Some(Pending {
            id: invite.id,
            email: invite.email,
            inviter_email: invite.inviter_email,
            owner_organization_id: invite.owner_organization_id,
            target: InviteTarget::Project {
                project_id: invite.project_id,
                role: invite.role,
            },
        }));
    }
    Ok(find_incoming_organization_invite(tx, invite_id, email)
        .await?
        .map(|invite| Pending {
            id: invite.id,
            email: invite.email,
            inviter_email: invite.inviter_email,
            owner_organization_id: invite.owner_organization_id,
            target: InviteTarget::Organization { role: invite.role },
        }))
}

/// Decline a pending invitation, in the table that holds it.
pub async fn decline_incoming(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    pending: &Pending,
) -> sqlx::Result<bool> {
    match pending.target {
        InviteTarget::Project { .. } => {
            decline_incoming_member_invite(tx, pending.id, &pending.email).await
        }
        InviteTarget::Organization { .. } => {
            decline_incoming_organization_invite(tx, pending.id, &pending.email).await
        }
    }
}

/// A pending PROJECT invite found by id and recipient email.
#[derive(Debug, sqlx::FromRow)]
pub struct PendingMemberInvite {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub email: String,
    pub role: Role,
    pub inviter_email: Option<String>,
    pub owner_organization_id: OrganizationId,
}

/// A pending ORGANIZATION invite found by id and recipient email.
#[derive(Debug, sqlx::FromRow)]
pub struct PendingOrganizationInvite {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub email: String,
    pub role: telmoni_shared::OrganizationRole,
    pub inviter_email: Option<String>,
    pub owner_organization_id: OrganizationId,
}

/// Pending unexpired invitations sent to `email` that the caller could still
/// accept: not to a project they already hold a seat on or whose organization
/// they own, and not to an organization they are already in. Every row is
/// another organization's.
pub async fn list_incoming(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    email: &str,
    caller: &UserId,
) -> sqlx::Result<Vec<IncomingInvite>> {
    sqlx::query_as::<_, IncomingInvite>(
        "SELECT i.id,
                'project'::text AS scope,
                i.project_id AS target_id,
                p.name AS target_name,
                i.role,
                inviter.email AS inviter_email,
                inviter.display_name AS inviter_display_name,
                i.expires_at,
                i.created_at
           FROM auth.member_invites i
           JOIN auth.projects p ON p.external_id = i.project_id
           JOIN auth.organizations po
             ON po.external_id = p.organization_id AND po.status = 'active'
           LEFT JOIN auth.identities inviter ON inviter.user_id = i.invited_by
          WHERE i.email = $1
            AND i.accepted_at IS NULL
            AND i.revoked_at IS NULL
            AND i.expires_at > now()
            AND NOT EXISTS (
                SELECT 1 FROM auth.organization_members om
                 WHERE om.organization_id = p.organization_id
                   AND om.user_id = $2 AND om.role = 'owner'
            )
            AND NOT EXISTS (
                SELECT 1 FROM auth.project_members pm
                 WHERE pm.project_id = i.project_id AND pm.user_id = $2
            )
          UNION ALL
         SELECT i.id,
                'organization'::text AS scope,
                i.organization_id AS target_id,
                o.name AS target_name,
                i.role,
                inviter.email AS inviter_email,
                inviter.display_name AS inviter_display_name,
                i.expires_at,
                i.created_at
           FROM auth.organization_invites i
           JOIN auth.organizations o
             ON o.external_id = i.organization_id AND o.status = 'active'
           LEFT JOIN auth.identities inviter ON inviter.user_id = i.invited_by
          WHERE i.email = $1
            AND i.accepted_at IS NULL
            AND i.revoked_at IS NULL
            AND i.expires_at > now()
            AND NOT EXISTS (
                SELECT 1 FROM auth.organization_members am
                 WHERE am.organization_id = i.organization_id AND am.user_id = $2
            )
          ORDER BY created_at DESC, id DESC",
    )
    .bind(email)
    .bind(caller)
    .fetch_all(tx.conn())
    .await
}

/// Find an unaccepted, unexpired project invitation by ID and email.
pub async fn find_incoming_member_invite(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    invite_id: Uuid,
    email: &str,
) -> sqlx::Result<Option<PendingMemberInvite>> {
    sqlx::query_as::<_, PendingMemberInvite>(
        "SELECT i.id, i.project_id, i.email, i.role,
                inviter.email     AS inviter_email,
                p.organization_id AS owner_organization_id
           FROM auth.member_invites i
           JOIN auth.projects p ON p.external_id = i.project_id
           JOIN auth.organizations o
             ON o.external_id = p.organization_id AND o.status = 'active'
           LEFT JOIN auth.identities inviter ON inviter.user_id = i.invited_by
          WHERE i.id = $1 AND i.email = $2
            AND i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > now()",
    )
    .bind(invite_id)
    .bind(email)
    .fetch_optional(tx.conn())
    .await
}

/// Find an unaccepted, unexpired organization invitation by ID and email.
pub async fn find_incoming_organization_invite(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    invite_id: Uuid,
    email: &str,
) -> sqlx::Result<Option<PendingOrganizationInvite>> {
    sqlx::query_as::<_, PendingOrganizationInvite>(
        "SELECT i.id, i.organization_id, i.email, i.role,
                inviter.email     AS inviter_email,
                i.organization_id AS owner_organization_id
           FROM auth.organization_invites i
           JOIN auth.organizations o
             ON o.external_id = i.organization_id AND o.status = 'active'
           LEFT JOIN auth.identities inviter ON inviter.user_id = i.invited_by
          WHERE i.id = $1 AND i.email = $2
            AND i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > now()",
    )
    .bind(invite_id)
    .bind(email)
    .fetch_optional(tx.conn())
    .await
}

/// Decline an incoming invitation by revoking it.
pub async fn decline_incoming_member_invite(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    invite_id: Uuid,
    email: &str,
) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE auth.member_invites
            SET revoked_at = now()
          WHERE id = $1 AND email = $2
            AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(invite_id)
    .bind(email)
    .execute(tx.conn())
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Decline an incoming organization invitation by revoking it.
pub async fn decline_incoming_organization_invite(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    invite_id: Uuid,
    email: &str,
) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE auth.organization_invites
            SET revoked_at = now()
          WHERE id = $1 AND email = $2
            AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(invite_id)
    .bind(email)
    .execute(tx.conn())
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Delete every invitation, to a project or to an organization, that this
/// person accepted — the erasure's step, since they may have left the
/// organizations that sent them long before. A spent invitation's only
/// remaining content is the address it was sent to; the audit chain keeps
/// the history. Keyed on `accepted_by`, never the address, which somebody
/// else may hold by now. Returns how many rows went.
pub async fn forget_accepted_by(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_id: &UserId,
) -> sqlx::Result<u64> {
    let seats = sqlx::query("DELETE FROM auth.member_invites WHERE accepted_by = $1")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
    let organizations = sqlx::query("DELETE FROM auth.organization_invites WHERE accepted_by = $1")
        .bind(user_id)
        .execute(tx.conn())
        .await?;
    Ok(seats.rows_affected() + organizations.rows_affected())
}

/// Delete every invitation, to a project or to an organization, that nobody
/// accepted and that expired or was withdrawn more than thirty days ago —
/// the nightly retention sweep, which spans every organization. An
/// unaccepted invitation's only content is the address it went to, and the
/// audit chain keeps that it was sent. Accepted ones are kept until their
/// acceptor's erasure ([`forget_accepted_by`]). Returns how many rows went.
pub async fn delete_stale(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    const WHERE: &str = "WHERE accepted_at IS NULL
                           AND (revoked_at < now() - interval '30 days'
                                OR expires_at < now() - interval '30 days')";
    let seats = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM auth.member_invites {WHERE}"
    )))
    .execute(tx.conn())
    .await?;
    let organizations = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM auth.organization_invites {WHERE}"
    )))
    .execute(tx.conn())
    .await?;
    Ok(seats.rows_affected() + organizations.rows_affected())
}
