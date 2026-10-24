//! The organization's chain lock, as a lane takes it before it files a row
//! under a project's organization; what a change reads from across a seam,
//! bounded, so that nobody waits under the lock on an answer another
//! deployment's database gives; and the transaction a lane's write runs in.
//!
//! ⚠ A transfer, a hand-over or a deletion holds the lock across its change,
//! so auth's answer under the lock is the one that stands, and a write
//! resolved before the change committed is refused.

use std::time::Duration;

use sqlx::PgPool;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::lock_chain;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::seam::Auth;
use telmoni_shared::{AuthError, OrganizationId, OrganizationStatus, ProjectId, TelmoniError};

/// The longest a change waits for the organization's chain lock: the
/// role's own `lock_timeout` on a tier, set on the transaction too, so that a
/// deployment whose roles carry none — the Compose quickstart connects as the
/// superuser — never holds a connection on it for as long as another holds
/// the lock.
const CHAIN_LOCK_WAIT: Duration = Duration::from_secs(2);

/// How long a change waits for an answer from across a seam: auth's second
/// answer while it holds the organization's chain lock, well inside the two
/// seconds every other writer in the organization waits for that lock before
/// giving up.
const ANSWER_BUDGET: Duration = Duration::from_secs(1);

/// Postgres's `lock_not_available`: a `lock_timeout` ran out.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// A transaction bound to the project and to the organization `acting` acts
/// in, as the row policies on a lane's writes need.
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

/// The rollback of a transaction that will not commit, logged if it failed.
/// Rolled back now, its chain or move lock goes before the answer rather
/// than whenever the pool next sees the connection. `write` names the lane.
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

/// The organization's chain lock alone, taken in `tx` and let go with it.
/// Refused where the lock is not had in time, so a change waits on no
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
/// aborted for the caller to roll back.
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

/// The organization holding `project_id` now, or `None` where the project
/// is gone. Without an answer within `ANSWER_BUDGET` the write is refused as
/// auth unavailable, which a machine retries.
pub(crate) async fn home(
    auth: &dyn Auth,
    project_id: &ProjectId,
) -> Result<Option<OrganizationId>, TelmoniError> {
    let homes = match tokio::time::timeout(
        ANSWER_BUDGET,
        auth.project_homes(std::slice::from_ref(project_id)),
    )
    .await
    {
        Ok(Ok(homes)) => homes,
        Ok(Err(e)) => {
            tracing::warn!(
                project_id = %project_id,
                error = %e,
                "telemetry: auth could not say which organization holds a project"
            );
            return Err(AuthError::IdentityUnavailable.into());
        }
        Err(_) => {
            tracing::warn!(
                project_id = %project_id,
                "telemetry: auth did not say in time which organization holds a project"
            );
            return Err(AuthError::IdentityUnavailable.into());
        }
    };
    Ok(homes
        .into_iter()
        .find(|home| home.project_id == *project_id)
        .map(|home| home.organization_id))
}

/// Whether `organization_id` takes writes now: not once its deletion is
/// marked, nor once it is gone. Without an answer within `ANSWER_BUDGET` it
/// counts as open, logged, since the organization's purge takes whatever
/// such a write left.
pub(crate) async fn open(auth: &dyn Auth, organization_id: &OrganizationId) -> bool {
    match tokio::time::timeout(ANSWER_BUDGET, auth.organization_standing(organization_id)).await {
        Ok(Ok(standing)) => standing == Some(OrganizationStatus::Active),
        Ok(Err(e)) => {
            tracing::warn!(
                organization_id = %organization_id,
                error = %e,
                "telemetry: auth could not say whether an organization is open; it is taken as open"
            );
            true
        }
        Err(_) => {
            tracing::warn!(
                organization_id = %organization_id,
                "telemetry: auth did not say in time whether an organization is open; it is taken \
                 as open"
            );
            true
        }
    }
}
