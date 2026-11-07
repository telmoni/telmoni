//! The answers more than one lane gives, each spelled once.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::Email;
use telmoni_shared::OrganizationId;

use super::invite::RosterCeiling;

/// A mail sent, a code, a link or a confirmation, and nothing more to say.
#[derive(Debug, Serialize)]
pub struct Sent {
    pub sent: bool,
}

impl Sent {
    pub const MAILED: Self = Self { sent: true };
}

/// An invitation made, to a project or to the organization: the link it
/// opens with, when that lapses, and whose console to tell.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteCreated {
    pub id: Uuid,
    pub link: String,
    pub expires_at: DateTime<Utc>,
    pub owner_organization_id: OrganizationId,
}

/// An invitation withdrawn: whose it was, and whose console to tell.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteRevoked {
    pub invite_id: Uuid,
    pub email: Email,
    pub owner_organization_id: OrganizationId,
}

/// A handover declined: whose console to tell.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Declined {
    pub owner_organization_id: OrganizationId,
}

/// A roster: its rows, and the plan's ceiling on it where one is held.
#[derive(Debug, Serialize)]
pub struct Roster<T> {
    pub members: Vec<T>,
    pub ceiling: Option<RosterCeiling>,
}

/// Invitations, of a roster or of a person.
#[derive(Debug, Serialize)]
pub struct Invites<T> {
    pub invites: Vec<T>,
}

/// Projects, of an organization or of a person.
#[derive(Debug, Serialize)]
pub struct Projects<T> {
    pub projects: Vec<T>,
}
