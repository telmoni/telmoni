//! `AuthProvider` trait — an external identity provider, beside the accounts
//! auth holds itself.
//!
//! Two halves. The sign-in (the authorize URL, the code exchange, the
//! browser half of sign-out) is what every provider has, and every
//! implementation supplies it; what comes back is who the person is, and
//! [`crate::issuer`] opens the session. The account-management calls
//! (deleting the person at the provider, minting a password reset, changing
//! the address) exist only where the provider offers a management API, so
//! they carry defaults: a provider reached over standard OpenID Connect
//! leaves them, and the lanes that need them refuse with a problem that
//! says whose job it is.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use telmoni_shared::{AuthzError, TelmoniError};

/// The identity a provider's code exchange names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    /// Stable provider-issued subject id (`sub` claim).
    pub sub: String,
    /// Email address, if the `email` scope was requested and granted.
    pub email: Option<String>,
    /// Whether the provider considers the email verified. `false` when it does
    /// not assert it — fail closed.
    pub email_verified: bool,
    /// Given name, if the `profile` scope was requested.
    pub given_name: Option<String>,
    /// Family name, if the `profile` scope was requested.
    pub family_name: Option<String>,
}

/// What a provider's code exchange answers: who the person is, and what the
/// sign-out will need to name the provider's own session.
#[derive(Clone)]
pub struct Authenticated {
    /// The person, as the provider asserts them.
    pub subject: Subject,
    /// The provider's id token, verified, when the exchange produced one.
    /// Sealed into the console's session for RP-initiated logout
    /// (`id_token_hint`); nothing here reads it again.
    pub id_token: Option<String>,
    /// How this sign-in was proved: `password`, `passkey`, `magic_link`, `sso`,
    /// or an OAuth connection's name. ⚠ `None` means the provider said nothing
    /// this answers for — never "no method" — so every reader needs a branch
    /// that claims nothing.
    pub auth_method: Option<String>,
}

impl std::fmt::Debug for Authenticated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Authenticated")
            .field("subject", &self.subject)
            .field("id_token", &self.id_token.as_ref().map(|_| "***"))
            .field("auth_method", &self.auth_method)
            .finish()
    }
}

/// A minted password-reset link, and when it stops working.
pub struct PasswordResetLink {
    /// The hosted page that spends the token. Email content only.
    pub url: String,
    /// When the token expires, as the provider stated it. For the LOG LINE,
    /// never the mail: an absolute UTC timestamp is nothing a reader can act on.
    pub expires_at: String,
}

impl std::fmt::Debug for PasswordResetLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordResetLink")
            .field("url", &"https://***")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// A pending email change the provider has mailed a one-time code for.
#[derive(Debug, Clone)]
pub struct EmailChangeChallenge {
    /// The address the code went to, as the provider echoed it back — compared
    /// and warned on, never stored. What is stored comes from the confirm.
    pub new_email: String,
    /// When the code stops working, as the provider stated it; for the log line.
    pub expires_at: String,
}

/// The address the provider holds after a confirmed change — the whole answer,
/// and the reason nothing about a pending change is stored here.
#[derive(Debug, Clone)]
pub struct ConfirmedEmail {
    /// The address the provider now holds. AUTHORITATIVE: the caller's own
    /// record converges onto this, never onto the value it asked for.
    pub email: String,
    /// The provider's own verdict. Spending the code IS the verification, so
    /// `false` is worth a `warn!` but never a reason to refuse to converge.
    pub email_verified: bool,
}

/// What [`crate::issuer::Issuer`] answers a grant with: the session's
/// tokens, and who they are for.
#[derive(Clone)]
pub struct TokenResponse {
    /// The bearer the console relays on every person request: an opaque
    /// secret this service stores as its hash and resolves to the person and
    /// the session ([`crate::person`]).
    pub access_token: String,
    /// The session the tokens belong to: what the sessions page lists and a
    /// sign-out names.
    pub session_id: String,
    /// The external provider's id token, when the session began at one:
    /// sealed by the console for the sign-out that names it, never relayed.
    pub id_token: Option<String>,
    /// The refresh token, rotated on every use.
    pub refresh_token: Option<String>,
    /// Seconds the access token remains valid from issue time.
    pub expires_in: u64,
    /// The person, with our own id as `sub`.
    pub subject: Subject,
    /// How this sign-in was proved, or `None`; see [`Authenticated`].
    pub auth_method: Option<String>,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"***")
            .field("session_id", &self.session_id)
            .field("id_token", &self.id_token.as_ref().map(|_| "***"))
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "***"))
            .field("expires_in", &self.expires_in)
            .field("subject", &self.subject)
            .field("auth_method", &self.auth_method)
            .finish()
    }
}

/// What the issuer hands a device that asked to sign in (RFC 8628 § 3.2).
#[derive(Clone)]
pub struct DeviceAuthorization {
    /// The secret the device polls with. Never shown to the person.
    pub device_code: String,
    /// The short code the person types on the console's device page.
    pub user_code: String,
    /// Where the person goes to type it.
    pub verification_uri: String,
    /// The same page with the code filled in.
    pub verification_uri_complete: Option<String>,
    /// Seconds until both codes expire.
    pub expires_in: u64,
    /// The fewest seconds the device may leave between polls.
    pub interval: u64,
}

impl std::fmt::Debug for DeviceAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceAuthorization")
            .field("device_code", &"***")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// What one poll of the device-code grant answered. The four refusals are
/// the RFC's own error codes; anything else is an error.
#[derive(Debug)]
pub enum DevicePoll {
    /// The person approved: tokens and who they are. Boxed so the four
    /// word-sized answers are not carried at the size of this one.
    Granted(Box<TokenResponse>),
    /// Not yet; poll again after the interval.
    Pending,
    /// Not yet, and the device is polling too fast: add five seconds.
    SlowDown,
    /// The person refused, and this device code is finished.
    Denied,
    /// The codes lapsed before anybody approved; start again.
    Expired,
}

/// An external identity provider. This repository's implementation is
/// [`crate::oidc::OidcProvider`]; a binary built on this library may supply
/// another. `[EXT]` `[SEC]`: every method is on the sign-in path, so a
/// signature change is breaking.
#[async_trait]
pub trait AuthProvider: Send + Sync + 'static {
    /// The stable key `auth.external_identities` files this provider's
    /// subjects under: the issuer URL for an OpenID Connect provider. Change
    /// it and every person linked through it is a stranger.
    fn id(&self) -> &str;

    /// What the sign-in page calls it: "Continue with {name}".
    fn name(&self) -> &str;

    /// Build the authorization endpoint URL for the web flow. Async because
    /// a provider's endpoints may be discovered on first use. `sign_up` asks
    /// for the provider's registration screen, `login_hint` pre-fills the
    /// address, where the provider offers either.
    async fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        sign_up: bool,
        login_hint: Option<&str>,
    ) -> Result<String, TelmoniError>;

    /// Exchange an authorization code for who the person is, checking the
    /// id token's signature, issuer, audience and expiry.
    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> Result<Authenticated, TelmoniError>;

    /// Build the provider's **browser** logout URL — the half of sign-out no
    /// server call can reach. While the provider's own cookie lives, the next
    /// sign-in silently re-authenticates. `id_token` is the one the exchange
    /// answered, for a page that wants `id_token_hint`; a provider with no
    /// such page answers `return_to` itself.
    async fn logout_url(&self, id_token: Option<&str>, return_to: &str) -> String;

    /// Delete the person at the identity provider, as the last step of an
    /// account deletion. The default deletes nothing and says so: a provider
    /// reached over standard OpenID Connect has no management API, and the
    /// record there is its administrator's to remove. An implementation that
    /// can delete MUST answer `Ok` only for a person who is now gone.
    async fn delete_user(&self, subject: &str) -> Result<(), TelmoniError> {
        tracing::info!(
            subject,
            "delete_user: this identity provider keeps its own record, and nothing here can remove it"
        );
        Ok(())
    }

    /// Mint a one-time password-reset link for `email`, which the provider
    /// mails itself. The default refuses: the provider's own page is where a
    /// password is reset.
    async fn create_password_reset(&self, _email: &str) -> Result<PasswordResetLink, TelmoniError> {
        Err(unsupported("password resets"))
    }

    /// Ask the provider to mail a one-time code to `new_email`, opening a
    /// pending change. Nothing here stores the pending address.
    ///
    /// ⚠ **Only half the proof, and the smaller half**: it proves the NEW
    /// address is reachable, never that the asker owns the account, so the
    /// caller pairs it with a factor on the CURRENT address.
    ///
    /// ⚠ `[EXT]` — the one method with no safe `Ok(())` fallback. A false
    /// success moves the address here while the provider keeps the old one,
    /// locking the person out. The default refuses.
    async fn send_email_change(
        &self,
        _subject: &str,
        _new_email: &str,
    ) -> Result<EmailChangeChallenge, TelmoniError> {
        Err(unsupported("email address changes"))
    }

    /// Spend the code the provider mailed, and answer with the address the
    /// provider now holds. AUTHORITATIVE: the provider normalises too, so a
    /// record written from the request is free to differ from the next sign-in.
    async fn confirm_email_change(
        &self,
        _subject: &str,
        _code: &str,
    ) -> Result<ConfirmedEmail, TelmoniError> {
        Err(unsupported("email address changes"))
    }
}

/// The refusal for an account-management call the provider has no API for.
/// A 403 rather than a 501: the request is well formed and the caller is who
/// they say, and the answer is that this deployment's identity provider owns
/// that setting.
fn unsupported(what: &str) -> TelmoniError {
    AuthzError::Forbidden(format!("your identity provider handles {what} itself")).into()
}

/// Compile-time snapshot of [`AuthProvider`]'s method set.
#[doc(hidden)]
async fn _auth_provider_signature_snapshot(p: std::sync::Arc<dyn AuthProvider>) {
    let _: &str = p.id();
    let _: &str = p.name();
    let _: Result<String, TelmoniError> = p.authorize_url("redirect", "state", false, None).await;
    let _: Result<Authenticated, TelmoniError> = p.exchange_code("code", "redirect").await;
    let _: String = p.logout_url(None, "https://app.example/").await;
    let _: Result<(), TelmoniError> = p.delete_user("subject").await;
    let _: Result<PasswordResetLink, TelmoniError> =
        p.create_password_reset("ada@example.com").await;
    let _: Result<EmailChangeChallenge, TelmoniError> =
        p.send_email_change("subject", "new@example.com").await;
    let _: Result<ConfirmedEmail, TelmoniError> = p.confirm_email_change("subject", "123456").await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_serialises_with_optional_fields_omitted_when_none() {
        let s = Subject {
            sub: "user-42".into(),
            email: None,
            email_verified: false,
            given_name: None,
            family_name: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"sub\":\"user-42\""));
        assert!(json.contains("\"email\":null"));
    }

    #[test]
    fn subject_round_trips() {
        let s = Subject {
            sub: "user-42".into(),
            email: Some("a@example.com".into()),
            email_verified: true,
            given_name: Some("Ada".into()),
            family_name: Some("Lovelace".into()),
        };
        let back: Subject = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn every_token_carrying_answer_debugs_without_its_tokens() {
        let subject = Subject {
            sub: "user-42".into(),
            email: None,
            email_verified: false,
            given_name: None,
            family_name: None,
        };
        let t = TokenResponse {
            access_token: "live_access_token_abc".into(),
            session_id: "ses_42".into(),
            id_token: Some("live_id_token_def".into()),
            refresh_token: Some("live_refresh_token_ghi".into()),
            expires_in: 3600,
            subject: subject.clone(),
            auth_method: Some("password".into()),
        };
        let a = Authenticated {
            subject,
            id_token: Some("live_id_token_def".into()),
            auth_method: None,
        };
        let rendered = format!("{t:?} {a:?}");
        for leaked in ["live_access_token", "live_id_token", "live_refresh_token"] {
            assert!(!rendered.contains(leaked), "leaked {leaked}: {rendered}");
        }
        assert!(rendered.contains("user-42"));
        assert!(rendered.contains("ses_42"), "the session id is no secret");
        assert!(rendered.contains("3600"));
        assert!(rendered.contains("password"));
    }
}
