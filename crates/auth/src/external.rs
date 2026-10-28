//! An external identity provider, beside the accounts auth holds itself.
//!
//! The provider answers one question — who is this — and the issuer opens
//! the session ([`crate::issuer`]). What is decided here is which person of
//! ours the provider's answer names: the subject it links to, else the
//! address when the deployment allows that and both the provider and the
//! account here proved it, else a new person when the provider may sign
//! people up, or when an invitation waits for the address it verified.

use std::sync::Arc;

use telmoni_shared::Email;
use telmoni_shared::db::tenant_session::{maintenance_scope, person_scope};
use telmoni_shared::{AuthError, AuthzError, TelmoniError, UserId};

use crate::db::{AuthLane, external_identities, identities, invites};
use crate::default_organization::{self, DefaultOrganization};
use crate::provider::{AuthProvider, Subject};

/// The external provider a deployment configured, and the policy it signs
/// people in under.
pub struct External {
    /// The provider itself: this crate's [`crate::oidc::OidcProvider`], or
    /// a binary's own.
    pub provider: Arc<dyn AuthProvider>,
    /// Whether a subject the provider names for the first time becomes a
    /// person here (`AUTH_DISABLE_SIGNUP` unset or `false`). Off, only people
    /// already linked sign in through it, and those whose verified address
    /// an invitation waits for.
    pub allow_sign_up: bool,
    /// Whether an unknown subject whose address a person here already holds
    /// is taken to be that person (`AUTH_CUSTOM_ALLOW_ACCOUNT_LINKING`), and
    /// then only where the provider verified the address and the account
    /// here proved it too: else an address typed at a lax provider, or an
    /// account made under someone's address before they came, takes them
    /// over. On, it is how a deployment moves its accounts onto a provider.
    pub link_by_email: bool,
}

impl External {
    /// Which person `subject` is, linking or creating them as the policy
    /// allows — one created joining `default_organization` — and recording
    /// what the provider just asserted about them.
    pub async fn resolve_person(
        &self,
        db: &sqlx::PgPool,
        subject: &Subject,
        default_organization: Option<&DefaultOrganization>,
    ) -> Result<UserId, TelmoniError> {
        let Some(email) = subject.email.as_deref().filter(|e| !e.trim().is_empty()) else {
            tracing::warn!("the identity provider named a person with no address; refused");
            return Err(AuthzError::Forbidden(
                "your identity provider did not share an email address".into(),
            )
            .into());
        };
        let email = Email::parse(email)?;
        let mut identity = identities::Identity {
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
            // An address the provider did not verify never takes one another
            // person holds, which it would shadow at every lookup by address.
            let shadows =
                !identity.email_verified && by_other(db, &identity.email, &user_id).await?;
            let mut tx = person_scope(db, &user_id).await?;
            if shadows && let Some(recorded) = identities::identity(&mut tx, &user_id).await? {
                tracing::warn!(user_id = %user_id,
                    "an external sign-in named an unverified address another person holds; the \
                     person's own address was kept");
                identity.email = recorded.email;
            }
            identities::record(&mut tx, &user_id, &identity).await?;
            tx.commit().await?;
            return Ok(user_id);
        }

        if let Some(holder) = by_address {
            let mut tx = person_scope(db, &holder).await?;
            let holder_proved = identities::identity(&mut tx, &holder)
                .await?
                .is_some_and(|held| held.email_verified);
            if !self.link_by_email || !subject.email_verified || !holder_proved {
                tx.rollback().await?;
                tracing::warn!(
                    linking = self.link_by_email,
                    verified = subject.email_verified,
                    holder_proved,
                    "an external sign-in's address belongs to an account here that is not linked \
                     to it; linking takes the deployment's switch and an address both the \
                     provider and the account proved"
                );
                return Err(AuthError::Conflict(
                    "an account here already holds that address; sign in with it".into(),
                )
                .into());
            }
            if external_identities::link(&mut tx, &holder, provider, &subject.sub).await? {
                identities::record(&mut tx, &holder, &identity).await?;
                tx.commit().await?;
                tracing::info!(user_id = %holder, "external subject linked to the account holding its address");
                return Ok(holder);
            }
            tx.rollback().await?;
            return self.linked_meanwhile(db, provider, &subject.sub).await;
        }

        // An invitation is the operator's own yes, as at the login form, for
        // an address the provider vouches for.
        if !self.allow_sign_up && !(subject.email_verified && invited(db, &identity.email).await?) {
            tracing::info!(
                "an external sign-in for a subject nobody here is, with sign-ups closed and no \
                 invitation for a verified address"
            );
            return Err(AuthError::SignUpClosed(format!(
                "sign-ups through {} are closed; ask somebody to invite you",
                self.provider.name()
            ))
            .into());
        }

        let user_id = UserId::new();
        let mut tx = person_scope(db, &user_id).await?;
        identities::record(&mut tx, &user_id, &identity).await?;
        if external_identities::link(&mut tx, &user_id, provider, &subject.sub).await? {
            default_organization::join(tx, &user_id, default_organization)
                .await?
                .commit()
                .await?;
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

/// Whether someone other than `user_id` holds `email`.
async fn by_other(
    db: &sqlx::PgPool,
    email: &Email,
    user_id: &UserId,
) -> Result<bool, TelmoniError> {
    let mut tx = maintenance_scope(db, AuthLane).await?;
    let holder = identities::find_by_email(&mut tx, email).await?;
    tx.commit().await?;
    Ok(holder.is_some_and(|holder| holder != *user_id))
}

/// Whether a live invitation names `email`, as the login form asks.
async fn invited(db: &sqlx::PgPool, email: &Email) -> Result<bool, TelmoniError> {
    let mut tx = maintenance_scope(db, AuthLane).await?;
    let invited = invites::any_live_for_email(&mut tx, email).await?;
    tx.commit().await?;
    Ok(invited)
}
