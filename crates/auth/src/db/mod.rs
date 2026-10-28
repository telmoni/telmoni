pub mod access_tokens;
pub mod audit;
pub mod audit_exports;
pub mod authorization_codes;
pub mod confirmation_codes;
pub mod credentials;
pub mod device_codes;
pub mod external_identities;
pub mod flags;
pub mod identities;
pub mod invites;
pub mod locks;
pub mod members;
pub mod offers;
pub mod organization_members;
pub mod organizations;
pub mod projects;
pub mod refresh_tokens;
pub mod sessions;
pub mod tokens;

use telmoni_shared::TelmoniError;
use telmoni_shared::db::tenant_session::{
    Lane, Maintenance, MaintenanceLane, Scoped, maintenance_scope,
};

/// This service's cross-tenant lane: the one declaration, so a sibling's
/// lane is unnameable here. See `tenant_session::Lane`.
#[derive(Debug, Clone, Copy)]
pub struct AuthLane;
impl Lane for AuthLane {
    const SET_ROLE: &'static str = MaintenanceLane::Auth.set_role();
}

/// Open the maintenance lane, run `read` in it, and commit: what a lane
/// needing one statement of another person's rows, or no one's, writes.
pub(crate) async fn lane<T>(
    db: &sqlx::PgPool,
    read: impl AsyncFnOnce(&mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<T>,
) -> Result<T, TelmoniError> {
    let mut tx = maintenance_scope(db, AuthLane).await?;
    let value = read(&mut tx).await?;
    tx.commit().await?;
    Ok(value)
}
