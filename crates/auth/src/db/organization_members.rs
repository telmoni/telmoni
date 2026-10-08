//! Queries against `auth.organization_members` and `auth.organization_invites`.
//!
//! **The owner is a row here**, and the statements below are what keep it the
//! only one: nothing inserts or updates to `owner` except provisioning and
//! [`promote_offer_holder`], and every UPDATE or DELETE a lane aims at a member
//! carries `role <> 'owner'`, so a stale read can never demote or remove the
//! owner it did not know was there.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{
    Binding, HasOrganization, Maintenance, Organization, Person, PersonAndOrganization,
    PersonOrganizationProject, ProjectAndOrganization, Scoped,
};
use telmoni_shared::{OrganizationId, OrganizationRole, UserId, derive_shard_key};

use crate::db::AuthLane;

/// Bindings a roster row is read under: the organization's own, the person's
/// (their own row, through `member_read`), the two lanes that carry the
/// organization beside a person or a project, and the maintenance lane.
pub trait RosterRead: Binding {}
impl RosterRead for Organization {}
impl RosterRead for Person {}
impl RosterRead for PersonAndOrganization {}
impl RosterRead for ProjectAndOrganization {}
impl RosterRead for Maintenance<AuthLane> {}

/// Bindings a member's row is written under: the organization's own, and the
/// lane the invite accepts run in, since the invitee holds no row yet.
pub trait RosterInsert: Binding {}
impl RosterInsert for Organization {}
impl RosterInsert for Maintenance<AuthLane> {}

/// How long an ownership offer stands. After this it is dead: nothing lists it
/// and nothing accepts it, and the next offer clears it.
pub const OFFER_TTL_DAYS: i32 = 7;

/// One row of an organization's roster, the owner included, with the person's
/// address and name joined in.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct OrganizationMember {
    /// The membership row's id.
    pub id: Uuid,
    /// The person's user id — what the role and removal lanes take.
    pub member_id: UserId,
    pub email: String,
    pub display_name: Option<String>,
    pub role: OrganizationRole,
    pub created_at: DateTime<Utc>,
    /// Whether this row is the owner's.
    pub is_owner: bool,
    /// When the live ownership offer to this admin lapses; `None` when there
    /// is no live offer.
    pub ownership_offer_expires_at: Option<DateTime<Utc>>,
}

/// One live organization invitation.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct OrganizationInvite {
    pub id: Uuid,
    pub email: String,
    pub role: OrganizationRole,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// An organization invitation resolved by its token hash for accept/look.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LiveOrganizationInvite {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub email: String,
    pub role: OrganizationRole,
    /// The person who sent it. `None` once they have been erased — the
    /// invitation outlives them, so this is a LEFT JOIN.
    pub inviter_email: Option<String>,
    pub inviter_display_name: Option<String>,
    /// What the organization is called.
    pub organization_name: String,
}

/// An organization the caller belongs to, as `/me` lists it.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationMembership {
    pub organization_id: OrganizationId,
    /// Where its paths begin in the console: `/{slug}`.
    pub slug: String,
    /// What it is called.
    pub name: String,
    /// The owner's address and name, for the console to name whom to ask.
    /// `None` only for an organization caught mid-transfer or mid-erasure.
    pub owner_email: Option<String>,
    pub owner_display_name: Option<String>,
    /// The caller's role there.
    pub role: OrganizationRole,
    /// When the owner's offer of this organization to the caller lapses;
    /// `None` when there is no live offer to them.
    pub ownership_offer_expires_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub created_at: DateTime<Utc>,
}

/// The SQL spelling of "this row holds a live offer", and when it lapses.
const OFFER_EXPIRES_AT: &str = "CASE WHEN m.transfer_offered_at > now() - make_interval(days => $2)
                                     THEN m.transfer_offered_at + make_interval(days => $2)
                                 END";

/// The organization's roster: the owner first, then everyone else newest
/// first. The identities join is what `identities.organization_member_read`
/// admits.
pub async fn list_for_organization(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Vec<OrganizationMember>> {
    sqlx::query_as::<_, OrganizationMember>(&format!(
        "SELECT m.id, m.user_id AS member_id, i.email, i.display_name, m.role,
                m.created_at, m.role = 'owner' AS is_owner,
                {OFFER_EXPIRES_AT} AS ownership_offer_expires_at
           FROM auth.organization_members m
           JOIN auth.identities i ON i.user_id = m.user_id
          WHERE m.organization_id = $1
          ORDER BY (m.role = 'owner') DESC, m.created_at DESC, m.id DESC"
    ))
    .bind(organization_id)
    .bind(OFFER_TTL_DAYS)
    .fetch_all(tx.conn())
    .await
}

/// A person's role on an organization, or `None` when they hold no row there.
pub async fn role_on_organization<B: RosterRead>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<Option<OrganizationRole>> {
    sqlx::query_scalar::<_, OrganizationRole>(
        "SELECT role FROM auth.organization_members WHERE organization_id = $1 AND user_id = $2",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}

/// A member's display name, resolved through the organization's view of `auth.identities`.
pub async fn member_display_name<B: RosterRead>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<Option<String>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT i.email, i.display_name
           FROM auth.organization_members m
           JOIN auth.identities i ON i.user_id = m.user_id
          WHERE m.organization_id = $1 AND m.user_id = $2",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(row
        .map(|(email, display_name)| crate::identity::display_for(display_name.as_deref(), &email)))
}

/// Bindings the owner's row is read under: each with the organization bound,
/// and the lane the project transfer's accept runs in, which seats the owner
/// of the organization the project leaves.
pub trait OwnerRead: Binding {}
impl OwnerRead for Organization {}
impl OwnerRead for PersonAndOrganization {}
impl OwnerRead for ProjectAndOrganization {}
impl OwnerRead for PersonOrganizationProject {}
impl OwnerRead for Maintenance<AuthLane> {}

/// Who owns the organization. `None` only between the two statements of a
/// transfer, which nobody else can observe.
pub async fn owner_of<B: OwnerRead>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Option<UserId>> {
    sqlx::query_scalar::<_, UserId>(
        "SELECT user_id FROM auth.organization_members
          WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(organization_id)
    .fetch_optional(tx.conn())
    .await
}

/// Write the owner's row of an organization being founded — provisioned at a
/// first sign-in, or created on request (`found_organization`) — and nowhere
/// else: a transfer promotes an admin's row rather than writing one.
pub async fn insert_owner(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO auth.organization_members
             (id, organization_id, user_id, role, added_by, shard_key)
         VALUES ($1, $2, $3, 'owner', $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .bind(user_id)
    .bind(derive_shard_key(organization_id))
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Change a member's role. `false` when there is no such member — or when the
/// row is the owner's, which no role change may touch.
///
/// ⚠ **Clears an ownership offer the new role cannot hold.** Only an admin may
/// hold one (the table's CHECK); demoting an admin with a live offer to
/// member withdraws it in the same statement rather than failing on the CHECK.
pub async fn update_role(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
    member: &UserId,
    role: OrganizationRole,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.organization_members
            SET role = $3,
                transfer_offered_at = CASE WHEN $3 = 'admin' THEN transfer_offered_at END
          WHERE organization_id = $1 AND user_id = $2 AND role <> 'owner'",
    )
    .bind(organization_id)
    .bind(member)
    .bind(role.to_string())
    .execute(tx.conn())
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Remove a member from an organization — never the owner. `false` when
/// nothing was removed.
pub async fn remove(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    organization_id: &OrganizationId,
    member: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM auth.organization_members
          WHERE organization_id = $1 AND user_id = $2 AND role <> 'owner'",
    )
    .bind(organization_id)
    .bind(member)
    .execute(tx.conn())
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Whether anyone other than `user_id` is on the organization's roster.
/// `EXISTS`, not a count: the one caller asks a yes-or-no, and a count reads
/// the whole roster to answer it.
pub async fn has_others(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    user_id: &UserId,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM auth.organization_members
                         WHERE organization_id = $1 AND user_id <> $2)",
    )
    .bind(organization_id)
    .bind(user_id)
    .fetch_one(tx.conn())
    .await
}

/// The organizations this person owns, in id order — the order their locks
/// are taken in.
pub async fn owned_by(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<Vec<OrganizationId>> {
    sqlx::query_scalar(
        "SELECT organization_id FROM auth.organization_members
          WHERE user_id = $1 AND role = 'owner'
          ORDER BY organization_id",
    )
    .bind(user_id)
    .fetch_all(tx.conn())
    .await
}

/// The ACTIVE organizations this person owns, in id order — where a project
/// offered to them may land. The lane the project transfer's accept runs in.
pub async fn active_organizations_owned_by(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_id: &UserId,
) -> sqlx::Result<Vec<OrganizationId>> {
    sqlx::query_scalar(
        "SELECT m.organization_id FROM auth.organization_members m
           JOIN auth.organizations o
             ON o.external_id = m.organization_id AND o.status = 'active'
          WHERE m.user_id = $1 AND m.role = 'owner'
          ORDER BY m.organization_id",
    )
    .bind(user_id)
    .fetch_all(tx.conn())
    .await
}

/// What labels an organization: the name it was given.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LabelParts {
    pub name: String,
}

/// An organization's label parts, under any binding that sees its row: its
/// own, a project's with it, or the lane. `None` when the binding sees no
/// such organization.
pub async fn label_parts<B: RosterRead>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Option<LabelParts>> {
    sqlx::query_as::<_, LabelParts>("SELECT name FROM auth.organizations WHERE external_id = $1")
        .bind(organization_id)
        .fetch_optional(tx.conn())
        .await
}

/// Every organization this person holds a row in, with the role — the
/// erasure's read.
pub async fn memberships_of_person(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_id: &UserId,
) -> sqlx::Result<Vec<(OrganizationId, OrganizationRole)>> {
    sqlx::query_as(
        "SELECT organization_id, role FROM auth.organization_members
          WHERE user_id = $1
          ORDER BY organization_id",
    )
    .bind(user_id)
    .fetch_all(tx.conn())
    .await
}

/// Withdraw every ownership offer the organization has out. Offering to
/// somebody new, and every transfer, starts here: there is at most one.
pub async fn clear_offers<B: HasOrganization>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "UPDATE auth.organization_members SET transfer_offered_at = NULL
          WHERE organization_id = $1 AND transfer_offered_at IS NOT NULL",
    )
    .bind(organization_id)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

/// Offer the organization to this member. `false` unless they are an admin.
pub async fn offer_to(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
    member: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.organization_members SET transfer_offered_at = now()
          WHERE organization_id = $1 AND user_id = $2 AND role = 'admin'",
    )
    .bind(organization_id)
    .bind(member)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Who holds the organization's live offer, if anybody.
pub async fn live_offer_holder(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Option<UserId>> {
    sqlx::query_scalar(
        "SELECT user_id FROM auth.organization_members
          WHERE organization_id = $1
            AND transfer_offered_at > now() - make_interval(days => $2)",
    )
    .bind(organization_id)
    .bind(OFFER_TTL_DAYS)
    .fetch_optional(tx.conn())
    .await
}

/// The first half of a transfer: whoever owns the organization now becomes an
/// admin. Returns who that was, or `None` when the only owner is `keep` (or
/// there is none). Never touches `keep`'s row.
pub async fn demote_owner(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    keep: &UserId,
) -> sqlx::Result<Option<UserId>> {
    sqlx::query_scalar(
        "UPDATE auth.organization_members SET role = 'admin'
          WHERE organization_id = $1 AND role = 'owner' AND user_id <> $2
      RETURNING user_id",
    )
    .bind(organization_id)
    .bind(keep)
    .fetch_optional(tx.conn())
    .await
}

/// The second half: the admin holding a live offer becomes the owner, and the
/// offer is spent — in ONE statement, so the offer CHECK (an owner holds no
/// offer) and the one-owner index both hold at every step. `false` when the
/// caller no longer holds a live offer, is no longer an admin, or the
/// organization is no longer active; the caller then rolls back the demotion.
pub async fn promote_offer_holder(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    member: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.organization_members m
            SET role = 'owner', transfer_offered_at = NULL
          WHERE m.organization_id = $1 AND m.user_id = $2 AND m.role = 'admin'
            AND m.transfer_offered_at > now() - make_interval(days => $3)
            AND EXISTS (SELECT 1 FROM auth.organizations o
                         WHERE o.external_id = m.organization_id AND o.status = 'active')",
    )
    .bind(organization_id)
    .bind(member)
    .bind(OFFER_TTL_DAYS)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The offer holder turns it down. `false` when they held no LIVE offer — the
/// same window accept and withdraw use, so a lapsed offer is declined by
/// nobody and recorded as declined nowhere.
pub async fn decline_offer(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
    member: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.organization_members SET transfer_offered_at = NULL
          WHERE organization_id = $1 AND user_id = $2
            AND transfer_offered_at > now() - make_interval(days => $3)",
    )
    .bind(organization_id)
    .bind(member)
    .bind(OFFER_TTL_DAYS)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// List live invitations for an organization.
pub async fn list_invites(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<Vec<OrganizationInvite>> {
    sqlx::query_as::<_, OrganizationInvite>(
        "SELECT id, email, role, expires_at, created_at
           FROM auth.organization_invites
          WHERE organization_id = $1
            AND accepted_at IS NULL
            AND revoked_at IS NULL
            AND expires_at > now()
          ORDER BY created_at DESC, id DESC",
    )
    .bind(organization_id)
    .fetch_all(tx.conn())
    .await
}

/// Whether somebody on the organization's roster — the owner included —
/// holds this address.
pub async fn address_on_roster(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
    email: &str,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM auth.organization_members m
               JOIN auth.identities i ON i.user_id = m.user_id
              WHERE m.organization_id = $1 AND i.email = $2)",
    )
    .bind(organization_id)
    .bind(email)
    .fetch_one(tx.conn())
    .await
}

/// Write an organization invitation.
pub async fn create_invite(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
    email: &str,
    role: OrganizationRole,
    token_hash: &str,
    invited_by: &UserId,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO auth.organization_invites
             (id, organization_id, email, role, token_hash, invited_by, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .bind(email)
    .bind(role.to_string())
    .bind(token_hash)
    .bind(invited_by)
    .bind(expires_at)
    .bind(derive_shard_key(organization_id))
    .fetch_one(tx.conn())
    .await
}

/// Revoke whatever live organization invitation this organization already sent to this email.
pub async fn revoke_live_for_email(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
    email: &str,
) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "UPDATE auth.organization_invites SET revoked_at = now()
          WHERE organization_id = $1 AND email = $2
            AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(organization_id)
    .bind(email)
    .execute(tx.conn())
    .await?;

    Ok(result.rows_affected())
}

/// Revoke a specific organization invitation by ID. Answers the address it
/// was sent to, or `None` when no live invitation carries that id.
pub async fn revoke_invite(
    tx: &mut Scoped<'_, Organization>,
    organization_id: &OrganizationId,
    invite_id: Uuid,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        "UPDATE auth.organization_invites SET revoked_at = now()
          WHERE organization_id = $1 AND id = $2
            AND accepted_at IS NULL AND revoked_at IS NULL
      RETURNING email",
    )
    .bind(organization_id)
    .bind(invite_id)
    .fetch_optional(tx.conn())
    .await
}

/// Find a live organization invitation by its SHA-256 token hash. The caller
/// is named by a link, not a GUC.
pub async fn find_live_organization_invite(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    token_hash: &str,
) -> sqlx::Result<Option<LiveOrganizationInvite>> {
    sqlx::query_as::<_, LiveOrganizationInvite>(
        "SELECT i.id, i.organization_id, i.email, i.role,
                inviter.email AS inviter_email, inviter.display_name AS inviter_display_name,
                o.name AS organization_name
           FROM auth.organization_invites i
           JOIN auth.organizations o
             ON o.external_id = i.organization_id AND o.status = 'active'
           LEFT JOIN auth.identities inviter ON inviter.user_id = i.invited_by
          WHERE i.token_hash = $1
            AND i.accepted_at IS NULL
            AND i.revoked_at IS NULL
            AND i.expires_at > now()",
    )
    .bind(token_hash)
    .fetch_optional(tx.conn())
    .await
}

/// Mark an organization invitation accepted.
pub async fn mark_accepted(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    invite_id: Uuid,
    accepted_by: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.organization_invites
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

/// Add a member to an organization. `false` when they already hold a row
/// there, whatever its role.
///
/// ⚠ **DO NOTHING on conflict, never an update.** This was an upsert that
/// rewrote the role, and once the owner became a row an invitation accepted
/// by the owner — or by an admin holding an ownership offer — would have
/// demoted them. A role changes through `update_role` and nowhere else.
pub async fn add_member<B: RosterInsert>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    member: &UserId,
    role: OrganizationRole,
    added_by: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO auth.organization_members
             (id, organization_id, user_id, role, added_by, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (organization_id, user_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .bind(member)
    .bind(role.to_string())
    .bind(added_by)
    .bind(derive_shard_key(organization_id))
    .execute(tx.conn())
    .await?;

    Ok(result.rows_affected() == 1)
}

/// Every ACTIVE organization this person belongs to, oldest membership
/// first, each with its owner to contact and any live offer to the person.
///
/// The owner's row and identity belong to other people, which nothing in the
/// person's own scope may see.
pub async fn organizations_of(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_id: &UserId,
) -> sqlx::Result<Vec<OrganizationMembership>> {
    sqlx::query_as::<_, OrganizationMembership>(&format!(
        "SELECT m.organization_id, o.slug, o.name,
                owner_identity.email AS owner_email,
                owner_identity.display_name AS owner_display_name,
                m.role,
                {OFFER_EXPIRES_AT} AS ownership_offer_expires_at,
                m.created_at
           FROM auth.organization_members m
           JOIN auth.organizations o
             ON o.external_id = m.organization_id AND o.status = 'active'
           LEFT JOIN auth.organization_members owner_row
                  ON owner_row.organization_id = m.organization_id AND owner_row.role = 'owner'
           LEFT JOIN auth.identities owner_identity
                  ON owner_identity.user_id = owner_row.user_id
          WHERE m.user_id = $1
          ORDER BY m.created_at ASC, m.id ASC"
    ))
    .bind(user_id)
    .bind(OFFER_TTL_DAYS)
    .fetch_all(tx.conn())
    .await
}

/// Whether this person holds a row in any active organization — `/me`'s
/// cheap question before it considers provisioning, through `member_read`.
pub async fn belongs_anywhere(tx: &mut Scoped<'_, Person>, user_id: &UserId) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM auth.organization_members m
               JOIN auth.organizations o
                 ON o.external_id = m.organization_id AND o.status = 'active'
              WHERE m.user_id = $1)",
    )
    .bind(user_id)
    .fetch_one(tx.conn())
    .await
}

/// Where the default of a person who owns no active organization falls while
/// they choose none: the oldest active one they belong to, in
/// [`organizations_of`]'s order, as `/me` reckons it. `None` when they own one
/// — the oldest of those is their default then, whatever else they found — or
/// belong nowhere. Through `member_read`, their own seats.
pub async fn default_while_owning_none(
    tx: &mut Scoped<'_, Person>,
    user_id: &UserId,
) -> sqlx::Result<Option<OrganizationId>> {
    sqlx::query_scalar(
        "SELECT m.organization_id
           FROM auth.organization_members m
           JOIN auth.organizations o
             ON o.external_id = m.organization_id AND o.status = 'active'
          WHERE m.user_id = $1
            AND NOT EXISTS (
                SELECT 1 FROM auth.organization_members owned
                  JOIN auth.organizations owned_organization
                    ON owned_organization.external_id = owned.organization_id
                   AND owned_organization.status = 'active'
                 WHERE owned.user_id = $1 AND owned.role = 'owner')
          ORDER BY m.created_at ASC, m.id ASC
          LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(tx.conn())
    .await
}
