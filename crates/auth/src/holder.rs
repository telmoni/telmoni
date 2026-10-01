//! Who holds a person's credential: the accounts auth keeps itself, or the
//! external provider. The lanes that change a credential — a password
//! reset, an address change, erasing the person — ask here and go to that
//! one; the other has nothing of the person's to change.

use std::sync::Arc;

use telmoni_shared::db::tenant_session::person_scope;
use telmoni_shared::{AuthzError, TelmoniError, UserId};

use crate::AppState;
use crate::db::{credentials, external_identities};
use crate::password::PasswordProvider;
use crate::provider::{AuthProvider, ConfirmedEmail, EmailChangeChallenge, PasswordResetLink};

/// Where a person's credential lives.
pub enum AccountHolder {
    /// A password here.
    Password(Arc<PasswordProvider>),
    /// An external provider, which names the person by `subject`.
    External {
        /// The provider holding the credential.
        provider: Arc<dyn AuthProvider>,
        /// The provider's own name for the person.
        subject: String,
    },
    /// Nobody: an identity a test door wrote, which no sign-in created.
    None,
}

impl AccountHolder {
    /// The refusal for a person with no credential anywhere.
    fn nobody() -> TelmoniError {
        AuthzError::Forbidden("no sign-in provider holds this account".into()).into()
    }

    /// Delete the person's credential, as the last step of an erasure.
    pub async fn delete_user(&self, user_id: &UserId) -> Result<(), TelmoniError> {
        match self {
            Self::Password(password) => password.delete_user(user_id).await,
            Self::External { provider, subject } => provider.delete_user(subject).await,
            Self::None => {
                tracing::info!(user_id = %user_id, "delete_user: no provider holds this account");
                Ok(())
            }
        }
    }

    /// Mint a one-time reset link for `email`, mailed by whoever holds it.
    pub async fn create_password_reset(
        &self,
        email: &str,
    ) -> Result<PasswordResetLink, TelmoniError> {
        match self {
            Self::Password(password) => password.create_password_reset(email).await,
            Self::External { provider, .. } => provider.create_password_reset(email).await,
            Self::None => Err(Self::nobody()),
        }
    }

    /// Open an address change: a code to the NEW address.
    pub async fn send_email_change(
        &self,
        user_id: &UserId,
        new_email: &str,
    ) -> Result<EmailChangeChallenge, TelmoniError> {
        match self {
            Self::Password(password) => password.send_email_change(user_id, new_email).await,
            Self::External { provider, subject } => {
                provider.send_email_change(subject, new_email).await
            }
            Self::None => Err(Self::nobody()),
        }
    }

    /// Spend the code the new address received.
    pub async fn confirm_email_change(
        &self,
        user_id: &UserId,
        code: &str,
    ) -> Result<ConfirmedEmail, TelmoniError> {
        match self {
            Self::Password(password) => password.confirm_email_change(user_id, code).await,
            Self::External { provider, subject } => {
                provider.confirm_email_change(subject, code).await
            }
            Self::None => Err(Self::nobody()),
        }
    }
}

impl AppState {
    /// Who holds `user_id`'s credential. A password row here says the
    /// accounts held here; a link says the external provider, under the
    /// subject it linked. Neither, with an external provider configured, is
    /// taken as that provider naming the person by our own id — the only
    /// way a person with no sign-in of their own exists is a test door,
    /// which a deployed tier does not mount.
    pub async fn account_holder(&self, user_id: &UserId) -> Result<AccountHolder, TelmoniError> {
        let mut tx = person_scope(&self.db, user_id).await?;
        let holds_password = credentials::get(&mut tx, user_id).await?.is_some();
        let linked = if holds_password {
            None
        } else {
            external_identities::for_person(&mut tx, user_id).await?
        };
        tx.commit().await?;

        if holds_password && let Some(password) = &self.password {
            return Ok(AccountHolder::Password(Arc::clone(password)));
        }
        let Some(external) = &self.external else {
            return Ok(AccountHolder::None);
        };
        let subject = match linked {
            Some((provider, subject)) if provider == external.provider.id() => subject,
            Some(_) => return Ok(AccountHolder::None),
            None => user_id.to_string(),
        };
        Ok(AccountHolder::External {
            provider: Arc::clone(&external.provider),
            subject,
        })
    }
}
