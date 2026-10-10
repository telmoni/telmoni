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

/// What happened, as an audit event's details name it beside its action and
/// its resource. ⚠ **Each word is hashed into every row that records it, on
/// a chain that is never rewritten**: a word respelled would split one kind
/// in two across the chain's history, so once written it never changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    /// An organization `/me` provisioned for a person in none, and its owner.
    AutoProvision,
    /// An organization its founder asked for, and its owner.
    OnRequest,
    /// A project invitation sent.
    Invited,
    /// A project invitation withdrawn.
    InviteRevoked,
    /// A project invitation accepted: the seat it gave.
    InviteAccepted,
    /// A project invitation declined by the person it was sent to.
    InviteDeclined,
    /// The roster row a project invitation's accept adds to its organization.
    EnrolledWithProjectInvite,
    /// An organization invitation withdrawn.
    OrganizationInviteRevoked,
    /// An organization invitation accepted: the roster row it gave.
    OrganizationInviteAccepted,
    /// An organization invitation declined by the person it was sent to.
    OrganizationInviteDeclined,
    /// A person left a project, or an organization.
    Left,
    /// A person removed from an organization's roster.
    Removed,
    /// A person's seat on a project removed by its owner or an admin.
    RemovedByOwner,
    /// A seat that went with its holder's removal from the organization.
    OrganizationRemovalCascade,
    /// The owner offered the organization to an admin.
    OwnershipOffered,
    /// The owner withdrew that offer.
    OwnershipOfferCancelled,
    /// The admin turned it down.
    OwnershipOfferDeclined,
    /// The admin accepted it, and owns the organization.
    OwnershipTransferred,
    /// A seat its holder no longer needs, now owning what it was on.
    SeatFoldedIntoOwnership,
    /// The owner offered a project to one of its admins.
    ProjectOffered,
    /// The owner withdrew that offer.
    ProjectOfferCancelled,
    /// The admin turned it down.
    ProjectOfferDeclined,
    /// A project handed to another organization, on the chain it left.
    ProjectTransferred,
    /// The same handover, on the chain it joined.
    ProjectReceived,
    /// The admin's seat a project's previous owner keeps on it.
    PreviousOwnerSeated,
    /// The roster row a project's previous owner takes in its new organization.
    PreviousOwnerEnrolled,
    /// The roster row a seat holder takes in a project's new organization.
    SeatHolderEnrolled,
    /// A person's address changed.
    EmailChange,
    /// An organization's deletion asked for, or taken up by its owner's
    /// account deletion.
    OrganizationDeletion,
    /// An organization's row deleted, its wait over.
    DeletionFinalize,
    /// A seat or a roster row a person's erasure removed.
    AccountDeletion,
}

impl AuditKind {
    /// Every kind.
    #[must_use]
    pub const fn all() -> [Self; 31] {
        [
            Self::AutoProvision,
            Self::OnRequest,
            Self::Invited,
            Self::InviteRevoked,
            Self::InviteAccepted,
            Self::InviteDeclined,
            Self::EnrolledWithProjectInvite,
            Self::OrganizationInviteRevoked,
            Self::OrganizationInviteAccepted,
            Self::OrganizationInviteDeclined,
            Self::Left,
            Self::Removed,
            Self::RemovedByOwner,
            Self::OrganizationRemovalCascade,
            Self::OwnershipOffered,
            Self::OwnershipOfferCancelled,
            Self::OwnershipOfferDeclined,
            Self::OwnershipTransferred,
            Self::SeatFoldedIntoOwnership,
            Self::ProjectOffered,
            Self::ProjectOfferCancelled,
            Self::ProjectOfferDeclined,
            Self::ProjectTransferred,
            Self::ProjectReceived,
            Self::PreviousOwnerSeated,
            Self::PreviousOwnerEnrolled,
            Self::SeatHolderEnrolled,
            Self::EmailChange,
            Self::OrganizationDeletion,
            Self::DeletionFinalize,
            Self::AccountDeletion,
        ]
    }

    /// The word the chain records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AutoProvision => "auto_provision",
            Self::OnRequest => "on_request",
            Self::Invited => "invited",
            Self::InviteRevoked => "invite_revoked",
            Self::InviteAccepted => "invite_accepted",
            Self::InviteDeclined => "invite_declined",
            Self::EnrolledWithProjectInvite => "enrolled_with_project_invite",
            Self::OrganizationInviteRevoked => "organization_invite_revoked",
            Self::OrganizationInviteAccepted => "organization_invite_accepted",
            Self::OrganizationInviteDeclined => "organization_invite_declined",
            Self::Left => "left",
            Self::Removed => "removed",
            Self::RemovedByOwner => "removed_by_owner",
            Self::OrganizationRemovalCascade => "organization_removal_cascade",
            Self::OwnershipOffered => "ownership_offered",
            Self::OwnershipOfferCancelled => "ownership_offer_cancelled",
            Self::OwnershipOfferDeclined => "ownership_offer_declined",
            Self::OwnershipTransferred => "ownership_transferred",
            Self::SeatFoldedIntoOwnership => "seat_folded_into_ownership",
            Self::ProjectOffered => "project_offered",
            Self::ProjectOfferCancelled => "project_offer_cancelled",
            Self::ProjectOfferDeclined => "project_offer_declined",
            Self::ProjectTransferred => "project_transferred",
            Self::ProjectReceived => "project_received",
            Self::PreviousOwnerSeated => "previous_owner_seated",
            Self::PreviousOwnerEnrolled => "previous_owner_enrolled",
            Self::SeatHolderEnrolled => "seat_holder_enrolled",
            Self::EmailChange => "email_change",
            Self::OrganizationDeletion => "organization_deletion",
            Self::DeletionFinalize => "deletion_finalize",
            Self::AccountDeletion => "account_deletion",
        }
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

    /// A new kind fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_audit_kind() {
        for kind in AuditKind::all() {
            match kind {
                AuditKind::AutoProvision
                | AuditKind::OnRequest
                | AuditKind::Invited
                | AuditKind::InviteRevoked
                | AuditKind::InviteAccepted
                | AuditKind::InviteDeclined
                | AuditKind::EnrolledWithProjectInvite
                | AuditKind::OrganizationInviteRevoked
                | AuditKind::OrganizationInviteAccepted
                | AuditKind::OrganizationInviteDeclined
                | AuditKind::Left
                | AuditKind::Removed
                | AuditKind::RemovedByOwner
                | AuditKind::OrganizationRemovalCascade
                | AuditKind::OwnershipOffered
                | AuditKind::OwnershipOfferCancelled
                | AuditKind::OwnershipOfferDeclined
                | AuditKind::OwnershipTransferred
                | AuditKind::SeatFoldedIntoOwnership
                | AuditKind::ProjectOffered
                | AuditKind::ProjectOfferCancelled
                | AuditKind::ProjectOfferDeclined
                | AuditKind::ProjectTransferred
                | AuditKind::ProjectReceived
                | AuditKind::PreviousOwnerSeated
                | AuditKind::PreviousOwnerEnrolled
                | AuditKind::SeatHolderEnrolled
                | AuditKind::EmailChange
                | AuditKind::OrganizationDeletion
                | AuditKind::DeletionFinalize
                | AuditKind::AccountDeletion => {}
            }
        }
    }

    /// ⚠ The words the chains already hold, each under a row's hash: none may
    /// change, and no two kinds may share one.
    #[test]
    fn every_audit_kind_keeps_the_word_the_chain_records() {
        let words = AuditKind::all().map(AuditKind::as_str);
        assert_eq!(
            words,
            [
                "auto_provision",
                "on_request",
                "invited",
                "invite_revoked",
                "invite_accepted",
                "invite_declined",
                "enrolled_with_project_invite",
                "organization_invite_revoked",
                "organization_invite_accepted",
                "organization_invite_declined",
                "left",
                "removed",
                "removed_by_owner",
                "organization_removal_cascade",
                "ownership_offered",
                "ownership_offer_cancelled",
                "ownership_offer_declined",
                "ownership_transferred",
                "seat_folded_into_ownership",
                "project_offered",
                "project_offer_cancelled",
                "project_offer_declined",
                "project_transferred",
                "project_received",
                "previous_owner_seated",
                "previous_owner_enrolled",
                "seat_holder_enrolled",
                "email_change",
                "organization_deletion",
                "deletion_finalize",
                "account_deletion",
            ]
        );
        let mut unique = words.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), words.len(), "two kinds share a word");
    }

    /// The details are written by serde, so its word must be the one pinned.
    #[test]
    fn an_audit_kind_is_written_as_its_word() {
        for kind in AuditKind::all() {
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.as_str())
            );
        }
    }

    /// The roles a chain's details hold were written as `Display` spells them,
    /// and are written by serde now: the two must not part.
    #[test]
    fn a_role_is_written_into_the_details_as_it_displays() {
        for role in Role::all() {
            assert_eq!(
                serde_json::to_value(role).unwrap(),
                serde_json::json!(role.to_string())
            );
        }
        for role in telmoni_shared::OrganizationRole::all() {
            assert_eq!(
                serde_json::to_value(role).unwrap(),
                serde_json::json!(role.to_string())
            );
        }
    }
}
