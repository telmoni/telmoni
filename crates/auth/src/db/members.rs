//! Queries against `auth.project_members` — who else may see an organization's
//! runs, one seat per person per project.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{
    Binding, Maintenance, Person, PersonAndOrganization, PersonOrganizationProject, Project,
    ProjectAndOrganization, Scoped,
};
use telmoni_shared::{OrganizationId, ProjectId, Role, UserId, derive_shard_key};

use crate::db::AuthLane;

/// Bindings a person's own seats are read under: theirs (`member_read`), and
/// the erasure's lane.
pub trait OwnSeatsRead: Binding {}
impl OwnSeatsRead for Person {}
impl OwnSeatsRead for Maintenance<AuthLane> {}

/// Bindings a person's seats inside one organization are read under: theirs
/// with that organization, for the ownership accept folding them, and the
/// lane the removal and erasure lanes run in.
pub trait OrganizationSeatsRead: Binding {}
impl OrganizationSeatsRead for PersonAndOrganization {}
impl OrganizationSeatsRead for Maintenance<AuthLane> {}

/// Bindings a seat is written under: the project's own, and the lane the
/// invite accept runs in, since the invitee holds nothing in the project yet.
pub trait SeatInsert: Binding {}
impl SeatInsert for Project {}
impl SeatInsert for Maintenance<AuthLane> {}

/// Bindings a seat is removed under: the project's own, the ownership accept
/// walking the new owner's seats, and the lane a member's own leaving and an
/// erasure run in, since the leaver may only read their row.
pub trait SeatRemove: Binding {}
impl SeatRemove for Project {}
impl SeatRemove for PersonOrganizationProject {}
impl SeatRemove for Maintenance<AuthLane> {}

/// Bindings one seat is read under: the project's own, the project's with its
/// organization (the transfer lanes re-reading roles under the lock), and the
/// lane the transfer's accept runs in, which asks whether the caller holds a
/// seat before it queues on any lock.
pub trait SeatRead: Binding {}
impl SeatRead for Project {}
impl SeatRead for ProjectAndOrganization {}
impl SeatRead for Maintenance<AuthLane> {}

/// Bindings an offer on a seat is written under: the project's own, and the
/// project's with its organization, which the transfer lanes hold once they
/// have the organization's lock.
pub trait OfferWrite: Binding {}
impl OfferWrite for Project {}
impl OfferWrite for ProjectAndOrganization {}

/// How long an offer of a project stands — the organization's window, so a
/// person holding one of each is not asked to remember two.
pub const OFFER_TTL_DAYS: i32 = crate::db::organization_members::OFFER_TTL_DAYS;

/// The SQL spelling of "this seat holds a live offer", and when it lapses.
const OFFER_EXPIRES_AT: &str = "CASE WHEN m.transfer_offered_at > now() - make_interval(days => $2)
                                     THEN m.transfer_offered_at + make_interval(days => $2)
                                 END";

/// One row of a project's roster, with the person's address and name joined in.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct Member {
    pub id: Uuid,
    /// The person's user id — what the role and removal lanes take.
    pub member_id: UserId,
    pub email: String,
    pub display_name: Option<String>,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub is_owner: bool,
    /// When the owner's live offer of the project to this admin lapses;
    /// `None` when there is no live offer, and always for the owner's row.
    pub transfer_offer_expires_at: Option<DateTime<Utc>>,
}

/// A project offered to the caller, as `/me` lists it: enough to say what is
/// on offer and who is offering, labelled the way the console labels the
/// organization it would leave.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOffer {
    pub project_id: ProjectId,
    /// The project's name, which it keeps.
    pub name: String,
    /// The organization it would leave.
    pub organization_id: OrganizationId,
    /// What that organization is called; `None` means never named.
    pub organization_name: Option<String>,
    /// Its owner — who is offering — by address and name.
    pub owner_email: Option<String>,
    pub owner_display_name: Option<String>,
    /// When the offer lapses.
    pub expires_at: DateTime<Utc>,
}

/// One seat the CALLER holds, as `/me` returns it: the mirror of [`Member`],
/// read from the other end. The organization's label comes from `/me`'s list
/// of organizations — every seat holder is on the owning organization's
/// roster — so nothing about another person is read here.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    /// The project the caller may act on.
    pub project_id: ProjectId,
    /// The organization that holds it.
    pub organization_id: OrganizationId,
    /// The caller's role there.
    pub role: Role,
}

/// The project's roster: its organization's owner first, then the seats,
/// newest first.
///
/// ⚠ The owner's row lives in the organization's roster and every person's
/// address in `auth.identities`, which a project-only scope sees none of —
/// the join would come back empty. A seat the owner still holds (an admin
/// who became the owner) is left out: the owner row already says everything
/// it could.
pub async fn list_for_project(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<Member>> {
    sqlx::query_as::<_, Member>(&format!(
        "SELECT o.id, o.user_id AS member_id, i.email, i.display_name,
                'owner'::text AS role, p.created_at, true AS is_owner,
                NULL::timestamptz AS transfer_offer_expires_at
           FROM auth.projects p
           JOIN auth.organization_members o
             ON o.organization_id = p.organization_id AND o.role = 'owner'
           JOIN auth.identities i ON i.user_id = o.user_id
          WHERE p.external_id = $1
          UNION ALL
         SELECT m.id, m.user_id, i.email, i.display_name,
                m.role, m.created_at, false AS is_owner,
                {OFFER_EXPIRES_AT} AS transfer_offer_expires_at
           FROM auth.project_members m
           JOIN auth.projects p ON p.external_id = m.project_id
           JOIN auth.identities i ON i.user_id = m.user_id
          WHERE m.project_id = $1
            AND NOT EXISTS (
                SELECT 1 FROM auth.organization_members o
                 WHERE o.organization_id = p.organization_id
                   AND o.user_id = m.user_id AND o.role = 'owner')
          ORDER BY is_owner DESC, created_at DESC, id DESC"
    ))
    .bind(project_id)
    .bind(OFFER_TTL_DAYS)
    .fetch_all(tx.conn())
    .await
}

/// Every project offered to this person and still open: their admin seats
/// carrying a live offer, in an active organization. The maintenance lane,
/// because each names the owner who offers it — another person's row and
/// identity.
pub async fn project_offers_to(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_id: &UserId,
) -> sqlx::Result<Vec<ProjectOffer>> {
    sqlx::query_as::<_, ProjectOffer>(
        "SELECT m.project_id, p.name, p.organization_id,
                o.name AS organization_name,
                owner_identity.email AS owner_email,
                owner_identity.display_name AS owner_display_name,
                m.transfer_offered_at + make_interval(days => $2) AS expires_at
           FROM auth.project_members m
           JOIN auth.projects p ON p.external_id = m.project_id
           JOIN auth.organizations o
             ON o.external_id = p.organization_id AND o.status = 'active'
           LEFT JOIN auth.organization_members owner_row
                  ON owner_row.organization_id = p.organization_id AND owner_row.role = 'owner'
           LEFT JOIN auth.identities owner_identity
                  ON owner_identity.user_id = owner_row.user_id
          WHERE m.user_id = $1 AND m.role = 'admin'
            AND m.transfer_offered_at > now() - make_interval(days => $2)
          ORDER BY m.transfer_offered_at DESC, m.id DESC",
    )
    .bind(user_id)
    .bind(OFFER_TTL_DAYS)
    .fetch_all(tx.conn())
    .await
}

/// Every seat a person holds — the `memberships` half of `/me`. Reads through
/// `member_read`, which is why that policy exists: otherwise a question about
/// oneself would need the cross-tenant lane.
pub async fn memberships_of<B: OwnSeatsRead>(
    tx: &mut Scoped<'_, B>,
    user_id: &UserId,
) -> sqlx::Result<Vec<Membership>> {
    sqlx::query_as::<_, Membership>(
        "SELECT m.project_id, p.organization_id, m.role
           FROM auth.project_members m
           JOIN auth.projects p ON p.external_id = m.project_id
          WHERE m.user_id = $1
          ORDER BY m.created_at DESC, m.id DESC",
    )
    .bind(user_id)
    .fetch_all(tx.conn())
    .await
}

/// One project membership, returned before cascade removal so each revoked
/// role can be audited.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProjectMembershipRole {
    pub project_id: ProjectId,
    pub role: Role,
}

/// The seats a person holds across all projects belonging to an organization.
pub async fn memberships_in_organization<B: OrganizationSeatsRead>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
    member: &UserId,
) -> sqlx::Result<Vec<ProjectMembershipRole>> {
    sqlx::query_as::<_, ProjectMembershipRole>(
        "SELECT pm.project_id, pm.role
           FROM auth.project_members pm
           JOIN auth.projects p ON p.external_id = pm.project_id
          WHERE p.organization_id = $1 AND pm.user_id = $2",
    )
    .bind(organization_id)
    .bind(member)
    .fetch_all(tx.conn())
    .await
}

/// One seat on a project, as the transfer's accept walks them to enrol each
/// holder on the destination's roster.
#[derive(Debug, sqlx::FromRow)]
pub struct Seat {
    pub user_id: UserId,
    pub role: Role,
}

/// Every seat on the project, oldest first. The lane: the accept holds no
/// project binding, and reads them across the two organizations it moves the
/// project between.
pub async fn seats_on(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<Seat>> {
    sqlx::query_as::<_, Seat>(
        "SELECT user_id, role FROM auth.project_members
          WHERE project_id = $1
          ORDER BY created_at, id",
    )
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// The person's seat on one project, or `None` when they hold none there.
pub async fn role_on<B: SeatRead>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    member: &UserId,
) -> sqlx::Result<Option<Role>> {
    sqlx::query_scalar::<_, Role>(
        "SELECT role FROM auth.project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project_id)
    .bind(member)
    .fetch_optional(tx.conn())
    .await
}

/// Seat a member. Called by the invite ACCEPT and by nothing else: an owner
/// cannot write this row directly, because the person it names has to agree.
pub async fn insert<B: SeatInsert>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    member: &UserId,
    role: Role,
    added_by: &UserId,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO auth.project_members
             (id, project_id, user_id, role, added_by, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(member)
    .bind(role.to_string())
    .bind(added_by)
    .bind(derive_shard_key(project_id))
    .fetch_one(tx.conn())
    .await
}

/// Seat somebody unless they already hold a seat, answering whether one was
/// written. The transfer's accept seats the previous owner with it: they
/// never hold one, since no owner does, but a statement that could abort the
/// transaction on a row that turns out to be there is not one to run at the
/// end of a handover.
pub async fn insert_if_absent<B: SeatInsert>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    member: &UserId,
    role: Role,
    added_by: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO auth.project_members
             (id, project_id, user_id, role, added_by, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (project_id, user_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(member)
    .bind(role.to_string())
    .bind(added_by)
    .bind(derive_shard_key(project_id))
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// True when the insert failed because the row already exists.
#[must_use]
pub fn already_a_member(e: &sqlx::Error) -> bool {
    matches!(
        e,
        sqlx::Error::Database(db)
            if db.constraint() == Some("project_members_project_member_key")
    )
}

/// Change a member's role. Returns `false` when there is no such member.
///
/// ⚠ **Clears an offer the new role cannot hold.** Only an admin may hold one
/// (the table's CHECK); demoting an admin with a live offer to member
/// withdraws it in the same statement rather than failing on the CHECK.
pub async fn update_role(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    member: &UserId,
    role: Role,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.project_members
            SET role = $3,
                transfer_offered_at = CASE WHEN $3 = 'admin' THEN transfer_offered_at END
          WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project_id)
    .bind(member)
    .bind(role.to_string())
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Withdraw every offer the project has out. Offering to somebody new starts
/// here: there is at most one.
pub async fn clear_project_offers<B: OfferWrite>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "UPDATE auth.project_members SET transfer_offered_at = NULL
          WHERE project_id = $1 AND transfer_offered_at IS NOT NULL",
    )
    .bind(project_id)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

/// Offer the project to this seat holder. `false` unless they are an admin.
pub async fn offer_project_to<B: OfferWrite>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    member: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.project_members SET transfer_offered_at = now()
          WHERE project_id = $1 AND user_id = $2 AND role = 'admin'",
    )
    .bind(project_id)
    .bind(member)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Who holds the project's live offer, if anybody.
pub async fn live_project_offer_holder<B: OfferWrite>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
) -> sqlx::Result<Option<UserId>> {
    sqlx::query_scalar(
        "SELECT user_id FROM auth.project_members
          WHERE project_id = $1
            AND transfer_offered_at > now() - make_interval(days => $2)",
    )
    .bind(project_id)
    .bind(OFFER_TTL_DAYS)
    .fetch_optional(tx.conn())
    .await
}

/// The offer holder turns it down. `false` when they held no LIVE offer — the
/// same window accept and withdraw use, so a lapsed offer is declined by
/// nobody and recorded as declined nowhere.
pub async fn decline_project_offer<B: OfferWrite>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    member: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.project_members SET transfer_offered_at = NULL
          WHERE project_id = $1 AND user_id = $2
            AND transfer_offered_at > now() - make_interval(days => $3)",
    )
    .bind(project_id)
    .bind(member)
    .bind(OFFER_TTL_DAYS)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Spend the offer and fold the seat into the ownership it becomes, in ONE
/// statement whose `WHERE` re-checks everything: the caller is still an admin
/// and the offer still live. The project's new owner holds no seat, as no
/// owner does. `false` when nothing matched, and the caller then rolls back
/// whatever it did before.
pub async fn fold_offered_seat(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    project_id: &ProjectId,
    member: &UserId,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM auth.project_members
          WHERE project_id = $1 AND user_id = $2 AND role = 'admin'
            AND transfer_offered_at > now() - make_interval(days => $3)",
    )
    .bind(project_id)
    .bind(member)
    .bind(OFFER_TTL_DAYS)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Remove a seat — an owner or admin removing a member, the member leaving, or an
/// erasure. Returns `false` when there is no such seat.
pub async fn remove<B: SeatRemove>(
    tx: &mut Scoped<'_, B>,
    project_id: &ProjectId,
    member: &UserId,
) -> sqlx::Result<bool> {
    let result =
        sqlx::query("DELETE FROM auth.project_members WHERE project_id = $1 AND user_id = $2")
            .bind(project_id)
            .bind(member)
            .execute(tx.conn())
            .await?;
    Ok(result.rows_affected() > 0)
}
