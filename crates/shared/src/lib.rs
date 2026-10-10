//! Shared types, errors, and helpers used across Telmoni services.

#![deny(missing_docs)]

/// The product's name, as a customer sees it — invite subjects and deletion
/// emails. The PRODUCT, never the legal company name: who runs a deployment
/// is configuration (`SUPPORT_EMAIL` on auth, the console's branding
/// variables), never a constant here.
pub const PRODUCT_NAME: &str = "Telmoni";

pub mod acting;
pub mod audit;
pub mod config;
pub mod db;
pub mod digest;
pub mod envelope;
pub mod error;
pub mod error_schema;
pub mod extract;
pub mod logging;
pub mod mail;
pub mod middleware;
pub mod net_guard;
pub mod oidc;
pub mod openapi;
pub mod person_token;
pub mod rbac;
pub mod seam;
pub mod sharding;
pub mod shutdown;
pub mod slug;
pub mod test_util;
pub mod text;
pub mod types;

pub use sharding::derive_shard_key;

pub use error::{AuthError, AuthzError, TelmoniError, TenantError};
pub use error_schema::ProblemDetails;
pub use extract::correlation_id;
pub use types::{
    AuditAction, ContentMode, Flag, FlagSet, NotificationKind, OrganizationId, OrganizationIdError,
    OrganizationRole, OrganizationStatus, ParseEnumError, ProjectId, ProjectIdError, Redacted,
    Role, SpanKind, SpanStatus, TelmoniResourceKind, UserId, UserIdError,
};
