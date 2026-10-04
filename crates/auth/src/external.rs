//! An external identity provider, beside the accounts auth holds itself.
//!
//! The provider answers one question — who is this — and the issuer opens
//! the session ([`crate::issuer`]). What is decided here is which person of
//! ours the provider's answer names: the subject it links to, else the
//! address when the deployment allows that, else a new person when the
//! provider may sign people up.

use std::sync::Arc;

use telmoni_shared::db::tenant_session::{maintenance_scope, person_scope};
use telmoni_shared::{AuthError, AuthzError, TelmoniError, UserId};

use crate::db::{AuthLane, external_identities, identities};
use crate::provider::{AuthProvider, Subject};

/// The external provider a deployment configured, and the policy it signs
/// people in under.
pub struct External {
    /// The provider itself: this crate's [`crate::oidc::OidcProvider`], or
    /// a binary's own.
    pub provider: Arc<dyn AuthProvider>,
    /// Whether a subject the provider names for the first time becomes a
    /// person here (`OIDC_ALLOW_SIGN_UP`). Off, only people already linked
    /// sign in through it.
    pub allow_sign_up: bool,
    /// Whether an unknown subject whose address a person here already holds
    /// is taken to be that person (`OIDC_ALLOW_INSECURE_EMAIL_LOOKUP`). Off,
    /// because it trusts the provider to have proved the address; on, it is
    /// how a deployment moves its password accounts onto a provider.
    pub link_by_email: bool,
}

impl External {
    /// Which person `subject` is, linking or creating them as the policy
    /// allows, and recording what the provider just asserted about them.
    pub async fn resolve_person(
        &self,
        db: &sqlx::PgPool,
        subject: &Subject,
    ) -> Result<UserId, TelmoniError> {
        let Some(email) = subject.email.as_deref().filter(|e| !e.trim().is_empty()) else {
            tracing::warn!("the identity provider named a person with no address; refused");
            return Err(AuthzError::Forbidden(
                "your identity provider did not share an email address".into(),
            )
            .into());
        };
        let email = crate::identity::validate_email(email)?;
        let identity = identities::Identity {
            email,
            // The provider's word, recorded as given: with `VERIFY_EMAIL` off
            // the session reports the address verified either way
            // (`Issuer::address_verified`), and with it on a provider that
            // vouches for none keeps the person out until it does. The column
            // itself gates only what needs the inbox proved — accepting an
            // invitation without its link.
            email_verified: subject.email_verified,
            first_name: crate::identity::sanitize_display_name(subject.given_name.as_deref()),
            last_name: crate::identity::sanitize_display_name(subject.family_name.as_deref()),
        };
        let provider = self.provider.id();

        let mut mtx = maintenance_scope(db, AuthLane).await?;
        let linked = external_identities::find(&mut mtx, provider, &subject.sub).await?;
        let by_address = match linked {
            Some(_) => None,
            None => identities::find_by_email(&mut mtx, &identity.email).await?,
        };
        mtx.commit().await?;

        if let Some(user_id) = linked {
            let mut tx = person_scope(db, &user_id).await?;
            identities::record(&mut tx, &user_id, &identity).await?;
            tx.commit().await?;
            return Ok(user_id);
        }

        if let Some(holder) = by_address {
            if !self.link_by_email {
                tracing::warn!(
                    "an external sign-in's address belongs to an account here that is not linked \
                     to it; OIDC_ALLOW_INSECURE_EMAIL_LOOKUP=true would link them"
                );
                return Err(AuthError::Conflict(
                    "an account here already holds that address; sign in with it".into(),
                )
                .into());
            }
            let mut tx = person_scope(db, &holder).await?;
            if external_identities::link(&mut tx, &holder, provider, &subject.sub).await? {
                identities::record(&mut tx, &holder, &identity).await?;
                tx.commit().await?;
                tracing::info!(user_id = %holder, "external subject linked to the account holding its address");
                return Ok(holder);
            }
            tx.rollback().await?;
            return self.linked_meanwhile(db, provider, &subject.sub).await;
        }

        if !self.allow_sign_up {
            tracing::info!(
                "an external sign-in for a subject nobody here is, with sign-ups closed"
            );
            // An invitation opens nothing here: this gate knows no addresses,
            // only identities the provider has brought before.
            return Err(AuthzError::Forbidden(format!(
                "sign-ups through {} are closed; ask the operator to let you in",
                self.provider.name()
            ))
            .into());
        }

        let user_id = UserId::new();
        let mut tx = person_scope(db, &user_id).await?;
        identities::record(&mut tx, &user_id, &identity).await?;
        if external_identities::link(&mut tx, &user_id, provider, &subject.sub).await? {
            tx.commit().await?;
            tracing::info!(user_id = %user_id, "account created from an external sign-in");
            return Ok(user_id);
        }
        tx.rollback().await?;
        self.linked_meanwhile(db, provider, &subject.sub).await
    }

    /// Two first sign-ins for one subject at once: the link's unique key
    /// picked the other, and this one is that person too.
    async fn linked_meanwhile(
        &self,
        db: &sqlx::PgPool,
        provider: &str,
        subject: &str,
    ) -> Result<UserId, TelmoniError> {
        let mut mtx = maintenance_scope(db, AuthLane).await?;
        let linked = external_identities::find(&mut mtx, provider, subject).await?;
        mtx.commit().await?;
        linked.ok_or_else(|| {
            TelmoniError::Internal("an external subject was linked and then was not".into())
        })
    }
}
