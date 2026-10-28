//! The organization's chain lock, as a lane takes it before it files a row
//! under a project's organization; what a change reads from across a seam,
//! bounded, so that nobody waits under the lock on an answer another
//! deployment's database gives; and the transaction a lane's write runs in.
//!
//! ⚠ A transfer, a hand-over or a deletion holds the lock across its change,
//! so auth's answer under the lock is the one that stands, and a write
//! resolved before the change committed is refused.

use axum::http::HeaderMap;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain_in_time};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::correlation_id;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::{ANSWER_BUDGET, Auth, resolve_again_in_time};
use telmoni_shared::{
    AuditAction, AuthError, OrganizationId, OrganizationStatus, ProjectId, TelmoniError,
    TelmoniResourceKind,
};

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

/// `write` in a transaction bound to the project and its organization:
/// committed where it answers, rolled back at once where it is refused, so
/// its chain lock goes before the answer. `lane` names it in the log.
pub(crate) async fn write<T>(
    pool: &PgPool,
    acting: &Acting,
    project_id: &ProjectId,
    lane: &'static str,
    write: impl AsyncFnOnce(&mut Scoped<'_, ProjectAndOrganization>) -> Result<T, TelmoniError>,
) -> Result<T, TelmoniError> {
    let mut tx = scoped(pool, acting, project_id).await?;
    match write(&mut tx).await {
        Ok(done) => {
            tx.commit().await?;
            Ok(done)
        }
        Err(refused) => {
            rolled_back(tx.rollback().await, project_id, lane);
            Err(refused)
        }
    }
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
    tx: &mut Scoped<'_, ProjectAndOrganization>,
) -> Result<Acting, TelmoniError> {
    lock_chain_in_time(tx.conn(), &acting.organization_id).await?;
    resolve_again_in_time(auth, acting).await
}

/// [`hold`], and `verb` on `resource` checked on the answer it gives.
pub(crate) async fn held(
    auth: &dyn Auth,
    acting: &Acting,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    verb: Verb,
    resource: Resource,
) -> Result<(), TelmoniError> {
    hold(auth, acting, tx)
        .await?
        .require_project(verb, resource)?;
    Ok(())
}

/// A member's change to the project's alerting, as its audit row names it.
pub(crate) struct Change {
    pub action: AuditAction,
    pub kind: TelmoniResourceKind,
    pub id: Uuid,
    pub metadata: Value,
}

/// `change` filed in the organization's audit chain, in `tx`, whose lock
/// [`hold`] took: by the member `acting` names, inside the project.
pub(crate) async fn file(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    acting: &Acting,
    project_id: &ProjectId,
    headers: &HeaderMap,
    change: Change,
) -> Result<(), TelmoniError> {
    let id = change.id.to_string();
    emit_audit(
        tx,
        AuditEvent {
            organization_id: &acting.organization_id,
            in_project: Some(project_id),
            actor: Actor::User(acting.user_id.as_str()),
            action: change.action,
            resource_kind: change.kind,
            resource_id: Some(&id),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(change.metadata),
        },
    )
    .await?;
    Ok(())
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
