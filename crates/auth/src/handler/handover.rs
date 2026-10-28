//! What the two handovers share once their own checks pass
//! (`handler::ownership`, `handler::project_transfer`): the request naming
//! the admin, the answer naming who holds the offer now, and the two mails
//! an offer sends.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use telmoni_shared::{OrganizationId, ProjectId, UserId};

use crate::{AppState, db::offers::OFFER_TTL_DAYS, mailer::MailError};

/// Body of `POST /internal/v1/organization/owner-transfer` and of
/// `POST /internal/v1/projects/{project_id}/transfer`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfferRequest {
    /// The admin to offer it to — their user id, as the roster lists it.
    pub member_id: String,
}

/// An offer made or withdrawn: who holds it, at the address the roster has,
/// whose live offer it replaced, when it lapses, and the organization whose
/// pages show it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferAnswer {
    pub offered_to: UserId,
    pub offered_to_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdrawn_from_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub owner_organization_id: OrganizationId,
}

/// When an offer made now lapses.
pub(crate) fn offer_expires_at() -> DateTime<Utc> {
    Utc::now() + Duration::days(i64::from(OFFER_TTL_DAYS))
}

/// What is on offer, for the mails and the log: the organization, or a
/// project in the organization named.
pub(crate) enum Offered<'a> {
    Organization {
        id: &'a OrganizationId,
        name: &'a str,
    },
    Project {
        id: &'a ProjectId,
        name: &'a str,
        organization: &'a str,
    },
}

impl Offered<'_> {
    /// The offer mailed to `to`, and the withdrawal to `withdrawn_from`, whose
    /// live offer this one replaced, side by side: each can wait out the mail
    /// client's timeout, and the console abandons the whole request at ten
    /// seconds. Either stands without its mail: the recipient's console lists
    /// the offer, and the replaced holder's no longer does.
    pub(crate) async fn mail(
        &self,
        state: &AppState,
        owner: &str,
        to: Option<&str>,
        withdrawn_from: Option<&str>,
    ) {
        let link = format!(
            "{}/account/notifications",
            state.config.app_url.trim_end_matches('/')
        );
        let (offered, withdrawn) = tokio::join!(
            async {
                match to {
                    Some(to) => self.send(state, to, owner, &link).await,
                    None => Ok(()),
                }
            },
            async {
                match withdrawn_from {
                    Some(to) => self.send_withdrawn(state, to, owner).await,
                    None => Ok(()),
                }
            },
        );
        if let Err(e) = offered {
            self.warn(&e, "offer mail failed");
        }
        if let Err(e) = withdrawn {
            self.warn(&e, "offer withdrawal mail failed");
        }
    }

    async fn send(
        &self,
        state: &AppState,
        to: &str,
        owner: &str,
        link: &str,
    ) -> Result<(), MailError> {
        match self {
            Offered::Organization { name, .. } => {
                state
                    .mailer
                    .send_ownership_offer(to, owner, name, link)
                    .await
            }
            Offered::Project {
                name, organization, ..
            } => {
                state
                    .mailer
                    .send_project_offer(to, owner, name, organization, link)
                    .await
            }
        }
    }

    async fn send_withdrawn(
        &self,
        state: &AppState,
        to: &str,
        owner: &str,
    ) -> Result<(), MailError> {
        match self {
            Offered::Organization { name, .. } => {
                state
                    .mailer
                    .send_ownership_offer_withdrawn(to, owner, name)
                    .await
            }
            Offered::Project { name, .. } => {
                state
                    .mailer
                    .send_project_offer_withdrawn(to, owner, name)
                    .await
            }
        }
    }

    fn warn(&self, e: &MailError, what: &str) {
        match self {
            Offered::Organization { id, .. } => {
                tracing::warn!(organization_id = %id, error = %e, "{what}");
            }
            Offered::Project { id, .. } => {
                tracing::warn!(project_id = %id, error = %e, "{what}");
            }
        }
    }
}
