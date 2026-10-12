//! The organization's chain lock, as a lane takes it before it files a row
//! under a project's organization: a project's content mode, and the
//! settings row the first view saved on a project makes.
//!
//! ⚠ A transfer's accept holds the lock from its audit rows to its commit and
//! moves the project's settings in between, as an ownership hand-over and a
//! deletion hold it over theirs: under it, the answer auth gives again is the
//! one that stands, and a write resolved before one of them committed is
//! refused rather than filed under an organization that no longer holds the
//! project.

use std::time::Duration;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::lock_chain;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped};
use telmoni_shared::seam::Auth;
use telmoni_shared::{AuthError, ProjectId, TelmoniError};

/// The longest a change waits for the organization's chain lock: the
/// role's own `lock_timeout` on a tier, set on the transaction too, so that a
/// deployment whose roles carry none — the Compose quickstart connects as the
/// superuser — never holds a connection on it for as long as another holds
/// the lock.
const CHAIN_LOCK_WAIT: Duration = Duration::from_secs(2);

/// How long a change waits for auth's second answer while it holds the
/// organization's chain lock: well inside the two seconds every other writer
/// in the organization waits for that lock before giving up.
const READ_AGAIN_BUDGET: Duration = Duration::from_secs(1);

/// Postgres's `lock_not_available`: a `lock_timeout` ran out.
const LOCK_NOT_AVAILABLE: &str = "55P03";

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
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(format!("{}ms", CHAIN_LOCK_WAIT.as_millis()))
        .execute(tx.conn())
        .await?;
    if let Err(e) = lock_chain(tx.conn(), organization_id).await {
        let timed_out = e
            .as_database_error()
            .and_then(|db| db.code())
            .is_some_and(|code| code == LOCK_NOT_AVAILABLE);
        if !timed_out {
            return Err(e.into());
        }
        tracing::warn!(
            project_id = %project_id,
            organization_id = %organization_id,
            "telemetry: a change to a project gave up on the organization's chain lock"
        );
        return Err(AuthError::Conflict(
            "another change to this project's organization is under way; try again".into(),
        )
        .into());
    }
    let Ok(now) = tokio::time::timeout(READ_AGAIN_BUDGET, auth.resolve_again(acting)).await else {
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
