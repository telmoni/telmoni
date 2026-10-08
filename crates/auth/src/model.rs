use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::{OrganizationId, OrganizationStatus, ProjectId, Role, UserId};

/// Which level an invitation is to — a project inside an organization, or the
/// organization itself. The two live in separate tables with separate role
/// vocabularies; this says which half a row came from, and so how to read the
/// `target_id` beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum InviteScope {
    /// An invitation to one project: `target_id` is a `ProjectId`, the role a [`Role`].
    Project,
    /// An invitation to the organization: `target_id` is an `OrganizationId`.
    Organization,
}

impl InviteScope {
    /// The wire spelling — what the SQL selects and the console reads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Organization => "organization",
        }
    }
}

impl std::fmt::Display for InviteScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who asked for an organization's deletion, as its audit chain records it.
/// Mirrors `auth.organizations.deletion_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DeletionKind {
    /// The owner, from organization settings.
    Owner,
    /// The owner's account deletion took it.
    Account,
    /// Telmoni terminated it.
    Operator,
}

impl DeletionKind {
    /// The wire spelling — what the SQL stores and the audit row records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Account => "account",
            Self::Operator => "operator",
        }
    }
}

impl std::fmt::Display for DeletionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Organization — the tenant, the payer and the container of projects. It is
/// nobody: the owner is a row in its roster.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Organization {
    pub id: Uuid,
    #[serde(rename = "organization_id")]
    pub external_id: OrganizationId,
    /// The organization's segment in console paths (`telmoni_shared::slug`).
    pub slug: String,
    /// What the organization is called: the name it was created under, or its
    /// owner's when it was provisioned ("Ada's organization"), whatever an
    /// owner or admin renames it to after.
    pub name: String,
    /// Lifecycle state. See the schema comment on `auth.organizations.status`.
    pub status: OrganizationStatus,
    /// When deletion was requested; NULL outside `pending_deletion`.
    pub deletion_requested_at: Option<DateTime<Utc>>,
    /// When the sweep may hard-delete the row; NULL outside `pending_deletion`.
    pub erase_after: Option<DateTime<Utc>>,
    /// Who asked; NULL outside `pending_deletion`.
    pub deletion_kind: Option<DeletionKind>,
    /// When the purge hook answered for the organization; NULL until that
    /// lands, and outside `pending_deletion`.
    pub hook_purged_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Project — encapsulates api keys and runs.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Project {
    pub id: Uuid,
    #[serde(rename = "project_id")]
    pub external_id: ProjectId,
    pub organization_id: OrganizationId,
    pub name: String,
    pub status: OrganizationStatus,
    pub deletion_requested_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// ProjectMember — a person holding a role in a project.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProjectMember {
    pub id: Uuid,
    pub project_id: ProjectId,
    /// The person's own id.
    pub user_id: UserId,
    pub role: Role,
    pub added_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ApiToken {
    pub id: Uuid,
    /// The project it was minted in. Every key names one.
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_by: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// "Valid until": NULL or future is active, past is invalid.
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTokenRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Provider sub of the minter, supplied by the BFF, which holds the
    /// request's authenticated identity.
    pub created_by: String,
    /// TTL in seconds. **Omitted** ⇒ the safe default 90-day expiry. Explicit
    /// `null` ⇒ no expiry. Zero is a 400. The double `Option` is load-bearing:
    /// it distinguishes "absent" from `null`, and `u32` makes a negative TTL
    /// unrepresentable.
    #[serde(default, deserialize_with = "double_option")]
    pub expires_in_seconds: Option<Option<u32>>,
}

/// Deserialize that preserves the absent-vs-`null` distinction for
/// [`CreateTokenRequest::expires_in_seconds`].
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}

#[derive(Serialize)]
pub struct CreateTokenResponse {
    pub id: Uuid,
    pub name: String,
    /// Raw token — returned only once, never stored.
    pub token: String,
}

impl std::fmt::Debug for CreateTokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateTokenResponse")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("token", &"telmoni_***")
            .finish()
    }
}

/// Request body for `POST /internal/tokens/{token_id}/rotate`.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RotateTokenRequest {
    #[serde(default)]
    pub grace_seconds: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateTokenRequest {
    pub token: String,
}

impl std::fmt::Debug for ValidateTokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidateTokenRequest")
            .field("token", &"telmoni_***")
            .finish()
    }
}

/// What `/internal/tokens/validate` answers. A token acts AS its organization,
/// which holds no seats anywhere, so there is no membership list to carry.
#[derive(Debug, Serialize)]
pub struct ValidateTokenResponse {
    /// The organization the token belongs to — the tenancy boundary a `/v1`
    pub organization_id: OrganizationId,
    /// What to PRINT for that organization — its name, else a generic label.
    /// A name, never a key: nothing may match on it.
    pub name: String,
    /// The feature flags resolved for `organization_id`, so one round trip
    /// says who is asking and what is switched on for them.
    pub flags: telmoni_shared::FlagSet,
    /// The validated token's own id, so an action is attributable to the
    /// workload, not just the organization.
    pub token_id: Uuid,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The redaction is mechanical: no raw token survives a Debug render, and a
    /// refactor that re-derives Debug fails here.
    #[test]
    fn create_token_response_debug_masks_the_raw_token() {
        let resp = CreateTokenResponse {
            id: Uuid::new_v4(),
            name: "ci-pipeline".into(),
            token: "telmoni_super_secret_value_1234567890".into(),
        };
        let rendered = format!("{resp:?}");
        assert!(
            !rendered.contains("super_secret_value"),
            "leaked: {rendered}"
        );
        assert!(rendered.contains("telmoni_***"));
        assert!(rendered.contains("ci-pipeline"));
    }

    #[test]
    fn validate_token_request_debug_masks_the_raw_token() {
        let req = ValidateTokenRequest {
            token: "telmoni_inbound_secret_9876".into(),
        };
        let rendered = format!("{req:?}");
        assert!(!rendered.contains("inbound_secret"), "leaked: {rendered}");
        assert!(rendered.contains("telmoni_***"));
    }

    /// The two words are read by the console (the notifications bell prints
    /// them), so a variant renamed without `rename_all` would silently change
    /// what a customer's invitation says it is an invitation to.
    #[test]
    fn the_scope_a_customer_reads_is_still_spelled_the_way_the_console_expects() {
        assert_eq!(
            serde_json::to_string(&InviteScope::Project).unwrap(),
            "\"project\""
        );
        assert_eq!(
            serde_json::to_string(&InviteScope::Organization).unwrap(),
            "\"organization\""
        );
        for scope in [InviteScope::Project, InviteScope::Organization] {
            assert_eq!(
                serde_json::to_string(&scope).unwrap(),
                format!("\"{}\"", scope.as_str())
            );
            assert_eq!(scope.to_string(), scope.as_str());
        }
    }
}
