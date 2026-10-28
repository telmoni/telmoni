//! An offer of the whole — the organization to one of its admins, a project to
//! one of its admin seat holders — as both rosters keep it: one live offer per
//! whole, on the admin's row (`transfer_offered_at`), dead after
//! [`OFFER_TTL_DAYS`], so removing them, their leaving or demoting them ends
//! it with no bookkeeping of its own. Offered, withdrawn and declined are the
//! same statement on either table; the accept is each roster's own
//! (`organization_members::promote_offer_holder`, `members::fold_offered_seat`).

use sqlx::Row;

use telmoni_shared::db::tenant_session::{
    Binding, Maintenance, Organization, PersonAndOrganization, PersonOrganizationProject, Project,
    ProjectAndOrganization, Scoped,
};
use telmoni_shared::{OrganizationId, ProjectId, UserId};

use crate::db::AuthLane;

/// How long an offer stands. After this it is dead: nothing lists it and
/// nothing accepts it, and the next offer clears it. One window for both
/// rosters, so a person holding one of each is not asked to remember two.
pub const OFFER_TTL_DAYS: i32 = 7;

/// The SQL spelling of "this row holds a live offer", the window bound as `$2`.
pub(crate) const LIVE: &str = "transfer_offered_at > now() - make_interval(days => $2)";

/// The SQL spelling of when the row `m`'s live offer lapses, `NULL` for none,
/// the window bound as `$2`.
pub(crate) const OFFER_EXPIRES_AT: &str = "CASE WHEN m.transfer_offered_at > now() - make_interval(days => $2)
                                                THEN m.transfer_offered_at + make_interval(days => $2)
                                            END";

/// Bindings an offer is written under: the organization's own and the lanes
/// that carry it beside a person or a project, under which a deletion ends
/// its offers; the project's own and its lane with the organization, which
/// the transfer lanes hold; and the maintenance lane, in which a move ends
/// the offer of the project moving.
pub trait OfferWrite: Binding {}
impl OfferWrite for Organization {}
impl OfferWrite for PersonAndOrganization {}
impl OfferWrite for ProjectAndOrganization {}
impl OfferWrite for PersonOrganizationProject {}
impl OfferWrite for Project {}
impl OfferWrite for Maintenance<AuthLane> {}

/// The roster an offer lives on, by the whole it is of.
#[derive(Debug, Clone, Copy)]
pub enum Roster<'a> {
    Organization(&'a OrganizationId),
    Project(&'a ProjectId),
}

impl<'a> Roster<'a> {
    /// The roster's table and the column naming the whole.
    fn parts(self) -> (&'static str, &'static str) {
        match self {
            Roster::Organization(_) => ("auth.organization_members", "organization_id"),
            Roster::Project(_) => ("auth.project_members", "project_id"),
        }
    }

    /// The whole's id: `$1` of every statement here.
    fn id(self) -> &'a str {
        match self {
            Roster::Organization(id) => id.as_str(),
            Roster::Project(id) => id.as_str(),
        }
    }
}

/// Every offer the whole has out, withdrawn. Offering to somebody new, and
/// every transfer, starts here: there is at most one. Answers how many.
pub async fn clear<B: OfferWrite>(tx: &mut Scoped<'_, B>, roster: Roster<'_>) -> sqlx::Result<u64> {
    let (table, key) = roster.parts();
    let result = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET transfer_offered_at = NULL
          WHERE {key} = $1 AND transfer_offered_at IS NOT NULL"
    )))
    .bind(roster.id())
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

/// The whole offered to `member`. `false` unless they are an admin on the roster.
pub async fn offer_to<B: OfferWrite>(
    tx: &mut Scoped<'_, B>,
    roster: Roster<'_>,
    member: &UserId,
) -> sqlx::Result<bool> {
    let (table, key) = roster.parts();
    let result = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET transfer_offered_at = now()
          WHERE {key} = $1 AND user_id = $2 AND role = 'admin'"
    )))
    .bind(roster.id())
    .bind(member)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Who holds the whole's live offer, if anybody.
pub async fn live_holder<B: OfferWrite>(
    tx: &mut Scoped<'_, B>,
    roster: Roster<'_>,
) -> sqlx::Result<Option<UserId>> {
    let (table, key) = roster.parts();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT user_id FROM {table} WHERE {key} = $1 AND {LIVE}"
    )))
    .bind(roster.id())
    .bind(OFFER_TTL_DAYS)
    .fetch_optional(tx.conn())
    .await?
    .map(|row| row.try_get(0))
    .transpose()
}

/// The holder turns the offer down. `false` when they held no LIVE offer —
/// the same window accept and withdraw use, so a lapsed offer is declined by
/// nobody and recorded as declined nowhere.
pub async fn decline<B: OfferWrite>(
    tx: &mut Scoped<'_, B>,
    roster: Roster<'_>,
    member: &UserId,
) -> sqlx::Result<bool> {
    let (table, key) = roster.parts();
    let result = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET transfer_offered_at = NULL
          WHERE {key} = $1 AND user_id = $3 AND {LIVE}"
    )))
    .bind(roster.id())
    .bind(OFFER_TTL_DAYS)
    .bind(member)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}
