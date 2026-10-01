//! The accounts auth holds itself: email and password. The login form, in
//! Grafana's words — the way in every deployment has unless it turns the
//! form off (`DISABLE_LOGIN_FORM`) in favour of an external provider.
//!
//! The console's sign-in, sign-up, verification, forgot and reset pages post
//! to the lanes in [`crate::handler::password`], which call here. A right
//! password earns a one-time code from [`crate::issuer`], which the
//! console's callback spends at the exchange lane exactly as it spends an
//! external provider's; the session that opens is the issuer's, the same
//! as for any other sign-in.
//!
//! What it holds (`crates/auth/migrations`): the password as an Argon2id
//! hash, the guard against guessing it, and the one-time links it mails —
//! the address verification, when the deployment asks for one
//! (`VERIFY_EMAIL`), the password reset, and the code to a new address.
//!
//! Who may sign up is the deployment's call (`ALLOW_SIGN_UP`, off unless
//! set): closed, an invitation is the way in, and completes an account for
//! the address it was sent to. `ADMIN_EMAIL` and `ADMIN_PASSWORD` seed the
//! first account at boot, so a fresh deployment with sign-ups closed has
//! somebody to send the invitations.

pub mod hashing;

use std::sync::Arc;

use chrono::{Duration, Utc};
use sqlx::PgPool;

use telmoni_shared::db::tenant_session::{maintenance_scope, person_scope};
use telmoni_shared::{AuthError, AuthzError, TelmoniError, TenantError, UserId};

use crate::db::{
    AuthLane, access_tokens,
    confirmation_codes::{self, Act, Purpose},
    credentials, identities, invites, locks, refresh_tokens, sessions,
};
use crate::issuer::{Issuer, secret};
use crate::mailer::Mailer;
use crate::oidc::with_query;
use crate::provider::{ConfirmedEmail, EmailChangeChallenge, PasswordResetLink};

/// How long the address-verification link works.
pub const VERIFICATION_TTL_HOURS: i64 = 24;

/// How long the password-reset link works.
pub const RESET_TTL_MINUTES: i64 = 60;

/// How many links of one kind an address is mailed in a day.
const LINKS_PER_DAY: i64 = 5;

/// What a sign-up earned.
#[derive(Clone)]
pub enum SignedUp {
    /// The address must be confirmed first: a link went to it.
    LinkSent,
    /// The account is usable now, and this code opens its first session.
    Code(String),
}

impl std::fmt::Debug for SignedUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LinkSent => f.write_str("LinkSent"),
            Self::Code(_) => f.write_str("Code(***)"),
        }
    }
}

/// The accounts held here. One per process, when the login form is on.
pub struct PasswordProvider {
    db: PgPool,
    issuer: Arc<Issuer>,
    mailer: Arc<dyn Mailer>,
    /// Whether anybody may create an account (`ALLOW_SIGN_UP`). Off, an
    /// invitation is the only way in.
    allow_sign_up: bool,
}

impl PasswordProvider {
    /// Build the provider over the service's pool. `issuer` opens the
    /// sessions; `mailer` is where its mails go.
    #[must_use]
    pub fn new(
        db: PgPool,
        issuer: Arc<Issuer>,
        mailer: Arc<dyn Mailer>,
        allow_sign_up: bool,
    ) -> Self {
        Self {
            db,
            issuer,
            mailer,
            allow_sign_up,
        }
    }

    /// Whether anybody may create an account, or only the invited.
    #[must_use]
    pub fn allow_sign_up(&self) -> bool {
        self.allow_sign_up
    }

    /// Who holds `email`, if anybody. The one read keyed on an address.
    async fn holder_of(&self, email: &str) -> Result<Option<UserId>, TelmoniError> {
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let holder = identities::find_by_email(&mut tx, email).await?;
        tx.commit().await?;
        Ok(holder)
    }

    /// Whether a live invitation names `email`: what lets the invited in
    /// while sign-ups are closed.
    async fn invited(&self, email: &str) -> Result<bool, TelmoniError> {
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let invited = invites::any_live_for_email(&mut tx, email).await?;
        tx.commit().await?;
        Ok(invited)
    }

    /// Create an account. With `VERIFY_EMAIL` on, the address must be
    /// confirmed from the link this mails before the account is usable;
    /// off, the account is usable now and the answer opens its session.
    pub async fn sign_up(
        &self,
        email: &str,
        password: &str,
        given_name: Option<&str>,
        family_name: Option<&str>,
    ) -> Result<SignedUp, TelmoniError> {
        let email = crate::identity::validate_email(email)?;
        hashing::validate(password)?;

        if self.holder_of(&email).await?.is_some() {
            return Err(AuthError::Conflict(
                "that address already has an account; sign in instead".into(),
            )
            .into());
        }
        if !self.allow_sign_up && !self.invited(&email).await? {
            tracing::info!(
                "sign-up refused: sign-ups are closed and the address holds no invitation"
            );
            return Err(AuthzError::Forbidden(
                "sign-ups are closed; ask somebody to invite you".into(),
            )
            .into());
        }

        let verify = self.issuer.verify_email();
        let password_hash = hashing::hash(password.to_owned()).await?;
        let identity = identities::Identity {
            email: email.clone(),
            email_verified: !verify,
            first_name: crate::identity::sanitize_display_name(given_name),
            last_name: crate::identity::sanitize_display_name(family_name),
        };

        let user_id = UserId::new();
        let mut tx = person_scope(&self.db, &user_id).await?;
        identities::record(&mut tx, &user_id, &identity).await?;
        if !credentials::create(&mut tx, &user_id, &email, &password_hash).await? {
            // Two sign-ups for one address at once: the unique index picked
            // the other one.
            tx.rollback().await?;
            return Err(AuthError::Conflict(
                "that address already has an account; sign in instead".into(),
            )
            .into());
        }
        if !verify {
            let code = self.issuer.mint_code(&mut tx, &user_id).await?;
            tx.commit().await?;
            tracing::info!(user_id = %user_id, "account created and signed in");
            return Ok(SignedUp::Code(code));
        }
        let token = secret();
        confirmation_codes::create(
            &mut tx,
            &user_id,
            Act::EmailVerification,
            &confirmation_codes::hash_code(&token),
            Utc::now() + Duration::hours(VERIFICATION_TTL_HOURS),
        )
        .await?;
        tx.commit().await?;
        tracing::info!(user_id = %user_id, "account created; verification link issued");

        self.mail_verification(&user_id, &email, &token).await?;
        Ok(SignedUp::LinkSent)
    }

    /// The first account, from the deployment's configuration
    /// (`ADMIN_EMAIL`, `ADMIN_PASSWORD`): created at boot when the address
    /// has no account yet, and left alone from then on. `true` when this
    /// boot created it.
    pub async fn seed_admin(&self, email: &str, password: &str) -> Result<bool, TelmoniError> {
        let email = crate::identity::validate_email(email)?;
        hashing::validate(password)?;
        if self.holder_of(&email).await?.is_some() {
            return Ok(false);
        }
        let password_hash = hashing::hash(password.to_owned()).await?;
        let user_id = UserId::new();
        let mut tx = person_scope(&self.db, &user_id).await?;
        identities::record(
            &mut tx,
            &user_id,
            &identities::Identity {
                email: email.clone(),
                // The operator vouches for their own address.
                email_verified: true,
                first_name: None,
                last_name: None,
            },
        )
        .await?;
        if !credentials::create(&mut tx, &user_id, &email, &password_hash).await? {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        tracing::info!(user_id = %user_id, "admin account created from ADMIN_EMAIL");
        Ok(true)
    }

    async fn mail_verification(
        &self,
        user_id: &UserId,
        email: &str,
        token: &str,
    ) -> Result<(), TelmoniError> {
        let link = self.issuer.page(
            "/auth/verify",
            &[("uid", user_id.as_str()), ("token", token)],
        );
        self.mailer
            .send_email_verification(email, &link)
            .await
            .map_err(|e| {
                tracing::error!(user_id = %user_id, error = %e,
                    "verification mail failed — check MAIL_FROM and SMTP_URL");
                TelmoniError::MailDelivery {
                    context: "email verification link".into(),
                }
            })
    }

    /// Mail a fresh verification link to an account that never confirmed its
    /// address, within the daily cap.
    async fn resend_verification(&self, user_id: &UserId, email: &str) -> Result<(), TelmoniError> {
        let mut tx = person_scope(&self.db, user_id).await?;
        let since = Utc::now() - Duration::days(1);
        let issued =
            confirmation_codes::count_since(&mut tx, user_id, Purpose::EmailVerification, since)
                .await?;
        if issued >= LINKS_PER_DAY {
            tx.commit().await?;
            return Err(TenantError::RateLimited {
                retry_after_secs: 24 * 60 * 60,
            }
            .into());
        }
        let token = secret();
        confirmation_codes::create(
            &mut tx,
            user_id,
            Act::EmailVerification,
            &confirmation_codes::hash_code(&token),
            Utc::now() + Duration::hours(VERIFICATION_TTL_HOURS),
        )
        .await?;
        tx.commit().await?;
        self.mail_verification(user_id, email, &token).await
    }

    /// Spend a verification link: the address is the person's from now on.
    pub async fn verify_email(&self, user_id: &UserId, token: &str) -> Result<(), TelmoniError> {
        let mut tx = person_scope(&self.db, user_id).await?;
        let live = confirmation_codes::find_live(
            &mut tx,
            user_id,
            Act::EmailVerification,
            &confirmation_codes::hash_code(token.trim()),
        )
        .await?;
        let Some(code_id) = live else {
            confirmation_codes::register_failure(&mut tx, user_id, Purpose::EmailVerification)
                .await?;
            tx.commit().await?;
            return Err(stale_link());
        };
        confirmation_codes::consume(&mut tx, code_id).await?;
        identities::set_verified(&mut tx, user_id).await?;
        tx.commit().await?;
        tracing::info!(user_id = %user_id, "email address verified");
        Ok(())
    }

    /// Check a password, and earn the one-time code the console's callback
    /// spends for the session. Every refusal that could tell an address with
    /// an account from one without is the same 401, taking the same time.
    pub async fn sign_in(&self, email: &str, password: &str) -> Result<String, TelmoniError> {
        let email = crate::identity::validate_email(email)?;
        if password.is_empty() {
            return Err(AuthError::BadRequest("enter your password".into()).into());
        }

        let Some(user_id) = self.holder_of(&email).await? else {
            hashing::burn(password.to_owned()).await;
            return Err(AuthError::Unauthenticated.into());
        };

        let mut tx = person_scope(&self.db, &user_id).await?;
        let credential = credentials::get(&mut tx, &user_id).await?;
        let person = identities::get(&mut tx, &user_id).await?;
        tx.commit().await?;
        let (Some(credential), Some(person)) = (credential, person) else {
            hashing::burn(password.to_owned()).await;
            return Err(AuthError::Unauthenticated.into());
        };

        if let Some(until) = credential.locked_until.filter(|u| *u > Utc::now()) {
            let wait = (until - Utc::now()).num_seconds();
            return Err(TenantError::RateLimited {
                retry_after_secs: u64::try_from(wait).unwrap_or(1).max(1),
            }
            .into());
        }

        if !hashing::verify(password.to_owned(), credential.password_hash).await? {
            let mut tx = person_scope(&self.db, &user_id).await?;
            let locked = credentials::register_failure(&mut tx, &user_id).await?;
            tx.commit().await?;
            if let Some(until) = locked {
                tracing::warn!(user_id = %user_id, until = %until,
                    "account locked after repeated wrong passwords");
            }
            return Err(AuthError::Unauthenticated.into());
        }

        let mut tx = person_scope(&self.db, &user_id).await?;
        credentials::clear_failures(&mut tx, &user_id).await?;
        if person.deletion_requested_at.is_some() {
            tx.commit().await?;
            return Err(AuthzError::Forbidden("account deletion in progress".into()).into());
        }
        if self.issuer.verify_email() && !person.email_verified {
            tx.commit().await?;
            self.resend_verification(&user_id, &email).await?;
            return Err(AuthzError::Forbidden(
                "verify your email address first — we sent a new link to it".into(),
            )
            .into());
        }
        let code = self.issuer.mint_code(&mut tx, &user_id).await?;
        tx.commit().await?;
        tracing::info!(user_id = %user_id, "signed in with a password");
        Ok(code)
    }

    /// Mail a reset link to `email` if an account holds it, and say nothing
    /// either way. The send is not waited for: an answer that took longer
    /// for a real address would say which addresses are real.
    pub async fn forgot_password(&self, email: &str) -> Result<(), TelmoniError> {
        let email = crate::identity::validate_email(email)?;
        if let Some(user_id) = self.holder_of(&email).await? {
            let mut tx = person_scope(&self.db, &user_id).await?;
            let holds_password = credentials::get(&mut tx, &user_id).await?.is_some();
            tx.commit().await?;
            if holds_password {
                let this = Arc::new(self.clone_for_task());
                tokio::spawn(async move {
                    if let Err(e) = this.send_reset_link(&user_id, &email).await {
                        tracing::warn!(user_id = %user_id, error = %e, "reset link not sent");
                    }
                });
            }
        }
        Ok(())
    }

    /// A handle for a task that outlives the request: the pool and the
    /// mailer are shared.
    fn clone_for_task(&self) -> ResetSender {
        ResetSender {
            db: self.db.clone(),
            app_url: self.issuer.app_url().to_owned(),
            mailer: Arc::clone(&self.mailer),
        }
    }

    /// Mint and mail a reset link, within the daily cap.
    async fn send_reset_link(
        &self,
        user_id: &UserId,
        email: &str,
    ) -> Result<PasswordResetLink, TelmoniError> {
        self.clone_for_task().send_reset_link(user_id, email).await
    }

    /// Spend a reset link and set the password. Every session ends: the link
    /// came from an inbox, and whoever held the old password holds it still.
    pub async fn reset_password(
        &self,
        user_id: &UserId,
        token: &str,
        password: &str,
    ) -> Result<(), TelmoniError> {
        hashing::validate(password)?;
        let password_hash = hashing::hash(password.to_owned()).await?;

        let mut tx = person_scope(&self.db, user_id).await?;
        let live = confirmation_codes::find_live(
            &mut tx,
            user_id,
            Act::PasswordReset,
            &confirmation_codes::hash_code(token.trim()),
        )
        .await?;
        let Some(code_id) = live else {
            confirmation_codes::register_failure(&mut tx, user_id, Purpose::PasswordReset).await?;
            tx.commit().await?;
            return Err(stale_link());
        };
        confirmation_codes::consume(&mut tx, code_id).await?;
        if !credentials::set_password(&mut tx, user_id, &password_hash).await? {
            tx.commit().await?;
            return Err(stale_link());
        }
        // The link came from the address, which is as much proof as the
        // verification link gives.
        identities::set_verified(&mut tx, user_id).await?;
        confirmation_codes::delete_all_for_person(&mut tx, user_id).await?;
        let revoked = sessions::revoke_all_for_person(&mut tx, user_id).await?;
        access_tokens::delete_for_person(&mut tx, user_id).await?;
        refresh_tokens::delete_for_person(&mut tx, user_id).await?;
        tx.commit().await?;
        tracing::info!(user_id = %user_id, sessions_revoked = revoked.len(),
            "password reset; every session ended");
        Ok(())
    }

    /// Erase the person's password and tokens, as the last step of an
    /// account deletion.
    pub async fn delete_user(&self, user_id: &UserId) -> Result<(), TelmoniError> {
        let mut tx = person_scope(&self.db, user_id).await?;
        credentials::delete(&mut tx, user_id).await?;
        access_tokens::delete_for_person(&mut tx, user_id).await?;
        refresh_tokens::delete_for_person(&mut tx, user_id).await?;
        tx.commit().await?;
        tracing::info!(user_id = %user_id, "credentials erased");
        Ok(())
    }

    /// Mint and mail a reset link for the account holding `email`: the
    /// settings page's "email me a reset link".
    pub async fn create_password_reset(
        &self,
        email: &str,
    ) -> Result<PasswordResetLink, TelmoniError> {
        let email = crate::identity::validate_email(email)?;
        let Some(user_id) = self.holder_of(&email).await? else {
            return Err(AuthError::NotFound("no account holds that address".into()).into());
        };
        self.send_reset_link(&user_id, &email).await
    }

    /// Open an address change: a code to the NEW address, proving it is
    /// reachable. The lane pairs it with a code to the current one.
    pub async fn send_email_change(
        &self,
        user_id: &UserId,
        new_email: &str,
    ) -> Result<EmailChangeChallenge, TelmoniError> {
        let new_email = crate::identity::validate_email(new_email)?;
        if self
            .holder_of(&new_email)
            .await?
            .is_some_and(|holder| holder != *user_id)
        {
            return Err(AuthError::Conflict("that address already has an account".into()).into());
        }
        let code = confirmation_codes::generate_code();
        let expires_at = Utc::now() + Duration::minutes(confirmation_codes::CODE_TTL_MINUTES);
        let mut tx = person_scope(&self.db, user_id).await?;
        let opened = credentials::set_pending_email(
            &mut tx,
            user_id,
            &new_email,
            &confirmation_codes::hash_code(&code),
            expires_at,
        )
        .await?;
        tx.commit().await?;
        if !opened {
            return Err(AuthzError::Forbidden(
                "this account has no password here; its identity provider holds its address".into(),
            )
            .into());
        }
        self.mailer
            .send_new_address_code(&new_email, &code)
            .await
            .map_err(|e| {
                tracing::error!(user_id = %user_id, error = %e,
                    "new-address code mail failed — check MAIL_FROM and SMTP_URL");
                TelmoniError::MailDelivery {
                    context: "email-change code to the new address".into(),
                }
            })?;
        Ok(EmailChangeChallenge {
            new_email,
            expires_at: expires_at.to_rfc3339(),
        })
    }

    /// Spend the code the new address received, and take the address.
    pub async fn confirm_email_change(
        &self,
        user_id: &UserId,
        code: &str,
    ) -> Result<ConfirmedEmail, TelmoniError> {
        let mut tx = person_scope(&self.db, user_id).await?;
        let Some(pending) = credentials::pending_email(&mut tx, user_id).await? else {
            tx.commit().await?;
            return Err(AuthError::BadRequest("no email change is pending".into()).into());
        };
        if pending.expires_at <= Utc::now() {
            credentials::clear_pending_email(&mut tx, user_id).await?;
            tx.commit().await?;
            return Err(crate::handler::account::wrong_code(false));
        }
        if confirmation_codes::hash_code(code.trim()) != pending.code_hash {
            let burned = credentials::register_pending_email_failure(&mut tx, user_id).await?;
            tx.commit().await?;
            return Err(crate::handler::account::wrong_code(burned));
        }
        tx.commit().await?;

        // Somebody may have signed up with the address since the code went
        // out; the lock and the check are what refuse the second holder, and
        // the unique index is what refuses a third that slips between them.
        if self
            .holder_of(&pending.email)
            .await?
            .is_some_and(|holder| holder != *user_id)
        {
            return Err(AuthError::Conflict("that address already has an account".into()).into());
        }
        let mut tx = person_scope(&self.db, user_id).await?;
        locks::lock_email(&mut tx, &pending.email).await?;
        if !credentials::take_pending_email(&mut tx, user_id).await? {
            // The refused UPDATE left the transaction aborted; nothing in it
            // is worth keeping.
            tx.rollback().await?;
            return Err(AuthError::Conflict("that address already has an account".into()).into());
        }
        tx.commit().await?;
        Ok(ConfirmedEmail {
            email: pending.email,
            email_verified: true,
        })
    }
}

/// The part of the provider a reset mail needs after the request that asked
/// for it has been answered.
struct ResetSender {
    db: PgPool,
    app_url: String,
    mailer: Arc<dyn Mailer>,
}

impl ResetSender {
    async fn send_reset_link(
        &self,
        user_id: &UserId,
        email: &str,
    ) -> Result<PasswordResetLink, TelmoniError> {
        let mut tx = person_scope(&self.db, user_id).await?;
        let since = Utc::now() - Duration::days(1);
        let issued =
            confirmation_codes::count_since(&mut tx, user_id, Purpose::PasswordReset, since)
                .await?;
        if issued >= LINKS_PER_DAY {
            tx.commit().await?;
            return Err(TenantError::RateLimited {
                retry_after_secs: 24 * 60 * 60,
            }
            .into());
        }
        let token = secret();
        let expires_at = Utc::now() + Duration::minutes(RESET_TTL_MINUTES);
        confirmation_codes::create(
            &mut tx,
            user_id,
            Act::PasswordReset,
            &confirmation_codes::hash_code(&token),
            expires_at,
        )
        .await?;
        tx.commit().await?;

        let url = with_query(
            &format!("{}/auth/reset", self.app_url),
            &[("uid", user_id.as_str()), ("token", &token)],
        );
        self.mailer
            .send_password_reset(email, &url)
            .await
            .map_err(|e| {
                tracing::error!(user_id = %user_id, error = %e,
                    "password-reset mail failed — check MAIL_FROM and SMTP_URL");
                TelmoniError::MailDelivery {
                    context: "password reset link".into(),
                }
            })?;
        tracing::info!(user_id = %user_id, "password-reset link issued");
        Ok(PasswordResetLink {
            url,
            expires_at: expires_at.to_rfc3339(),
        })
    }
}

/// A link that is not live: unknown, spent, expired, or guessed too often.
fn stale_link() -> TelmoniError {
    AuthError::BadRequest("that link is invalid or has expired — ask for a new one".into()).into()
}
