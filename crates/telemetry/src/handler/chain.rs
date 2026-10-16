//! The organization's chain lock, as a lane takes it before it files a row
//! under a project's organization — a monitor, a ping key, a saved view, a
//! project's content mode, a monitor its first ping makes, a key its
//! rotation retires — and the settings row the first of them makes; what a
//! change reads from across a seam, bounded, so that nobody waits under the
//! lock on an answer another deployment's database gives; and the
//! transaction a lane's write runs in, bound to the project and its
//! organization, and let go of, with whatever locks it holds, where it does
//! not commit.
//!
//! ⚠ A transfer's accept holds the lock from its audit rows to its commit and
//! moves the project's settings in between, as an ownership hand-over and a
//! deletion hold it over theirs: under it, the answer auth gives again is the
//! one that stands, and a write resolved before one of them committed is
//! refused rather than filed under an organization that no longer holds the
//! project.

use std::time::Duration;

use sqlx::PgPool;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::lock_chain;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::seam::{self, Auth, Entitlements};
use telmoni_shared::{AuthError, MonitorCeiling, OrganizationId, ProjectId, TelmoniError};

/// The longest a change waits for the organization's chain lock: the
/// role's own `lock_timeout` on a tier, set on the transaction too, so that a
/// deployment whose roles carry none — the Compose quickstart connects as the
/// superuser — never holds a connection on it for as long as another holds
/// the lock.
const CHAIN_LOCK_WAIT: Duration = Duration::from_secs(2);

/// How long a change waits for an answer from across a seam: auth's second
/// answer while it holds the organization's chain lock, well inside the two
/// seconds every other writer in the organization waits for that lock before
/// giving up, and the plan's ceiling before it takes the lock ([`ceiling`]).
const ANSWER_BUDGET: Duration = Duration::from_secs(1);

/// Postgres's `lock_not_available`: a `lock_timeout` ran out.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// A transaction of `pool`'s bound to the project and to the organization
/// `acting` acts in, as every write a lane files under both needs: each
/// row's policy files a write under the two, and [`hold`] checks the
/// organization this binds.
pub(crate) async fn scoped<'a>(
    pool: &'a PgPool,
    acting: &Acting,
    project_id: &ProjectId,
) -> Result<Scoped<'a, ProjectAndOrganization>, TelmoniError> {
    Ok(project_scope(pool, project_id)
        .await?
        .bind_organization(&acting.organization_id)
        .await?)
}

/// A transaction let go of without its commit — its write refused, its
/// project kept from it, or nothing left for it to do — answered by
/// `rollback`: nothing was written, and whichever locks it holds, the
/// organization's chain lock or the project's move lock, go with it before
/// the answer rather than whenever the pool next sees the connection, since
/// a move, a purge or the organization's other writers may be waiting on
/// them. `write` names the lane, for the log.
pub(crate) fn rolled_back(rollback: sqlx::Result<()>, project_id: &ProjectId, write: &'static str) {
    if let Err(e) = rollback {
        tracing::warn!(
            project_id = %project_id,
            write,
            error = %e,
            "telemetry: a transaction let go of could not be rolled back; its connection drops it"
        );
    }
}

/// The organization's chain lock, taken in `tx` and let go with it, then
/// who `acting` is now: auth's second answer, the one that stands until the
/// commit. Refused where the lock is not had in time, auth does not answer
/// again in time, or the project has moved to another organization. The
/// caller checks the role it needs on what this answers, not on `acting`.
pub(crate) async fn hold(
    auth: &dyn Auth,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<Acting, TelmoniError> {
    let organization_id = &acting.organization_id;
    lock(project_id, organization_id, tx).await?;
    let Ok(now) = tokio::time::timeout(ANSWER_BUDGET, auth.resolve_again(acting)).await else {
        tracing::warn!(
            project_id = %project_id,
            organization_id = %organization_id,
            "telemetry: auth did not answer a change to a project again in time"
        );
        return Err(AuthError::IdentityUnavailable.into());
    };
    let now = now?;
    if now.organization_id != *organization_id {
        return Err(AuthError::Conflict(
            "the project has moved to another organization; reload and try again".into(),
        )
        .into());
    }
    Ok(now)
}

/// The organization's chain lock alone, taken in `tx` and let go with it,
/// for a write nobody is acting as a person for: a monitor its first ping
/// makes. Refused where the lock is not had in time, so a ping waits on no
/// transfer for longer than any other writer would.
pub(crate) async fn lock(
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<(), TelmoniError> {
    if lock_in_time(organization_id, tx).await? {
        return Ok(());
    }
    tracing::warn!(
        project_id = %project_id,
        organization_id = %organization_id,
        "telemetry: a change to a project gave up on the organization's chain lock"
    );
    Err(AuthError::Conflict(
        "another change to this project's organization is under way; try again".into(),
    )
    .into())
}

/// The organization's chain lock, taken in `tx` and let go with it, where it
/// is had within [`CHAIN_LOCK_WAIT`]; `false` where it is not, `tx` then
/// aborted for the caller to roll back. For a write a later attempt does as
/// well as this one — a key its rotation retires, which the next tick finds
/// again — where [`lock`] refuses the caller who is waiting on an answer.
pub(crate) async fn lock_in_time(
    organization_id: &OrganizationId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> sqlx::Result<bool> {
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(format!("{}ms", CHAIN_LOCK_WAIT.as_millis()))
        .execute(tx.conn())
        .await?;
    match lock_chain(tx.conn(), organization_id).await {
        Ok(()) => Ok(true),
        Err(e)
            if e.as_database_error()
                .and_then(|db| db.code())
                .is_some_and(|code| code == LOCK_NOT_AVAILABLE) =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// The organization's monitor ceiling, for a change to read before it takes
/// the lock — only the count under the lock needs it — and within the same
/// second auth's second answer gets. A deployment answers it from a database
/// of its own, through a pool a burst can spend, and a read that waited on
/// that under the lock would hold every other writer in the organization to
/// the lock's two seconds and a 409 for as long as it waited; before the
/// lock, and bounded, it costs this change a second at most — and a move of
/// the project that second, where the change is a ping, which holds its
/// project in place meanwhile. An answer not had in time is no ceiling,
/// logged, as the seam answers one it cannot read.
pub(crate) async fn ceiling(
    entitlements: &dyn Entitlements,
    organization_id: &OrganizationId,
) -> Option<MonitorCeiling> {
    match tokio::time::timeout(
        ANSWER_BUDGET,
        seam::monitor_ceiling(entitlements, organization_id),
    )
    .await
    {
        Ok(ceiling) => ceiling,
        Err(_) => {
            tracing::warn!(
                organization_id = %organization_id,
                "telemetry: the monitor ceiling was not answered in time; none is held"
            );
            None
        }
    }
}
