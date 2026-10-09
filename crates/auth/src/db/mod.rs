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
pub mod organization_members;
pub mod organizations;
pub mod projects;
pub mod refresh_tokens;
pub mod sessions;
pub mod tokens;

use telmoni_shared::db::tenant_session::{Lane, MaintenanceLane};

/// This service's cross-tenant lane: the one declaration, so a sibling's
/// lane is unnameable here. See `tenant_session::Lane`.
#[derive(Debug, Clone, Copy)]
pub struct AuthLane;
impl Lane for AuthLane {
    const SET_ROLE: &'static str = MaintenanceLane::Auth.set_role();
}
