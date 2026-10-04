//! The session authority: every bearer this deployment relays is minted
//! here, whichever way the person proved who they are — a password checked
//! by [`crate::password`], an external OpenID Connect provider's answer
//! ([`crate::external`]), or a device the person approved from the console.
//!
//! One issuer for every sign-in is what lets the password form and an
//! external provider share one instance: the console's session, the
//! refresh, the sessions page, the sign-out and the CLI's device grant are
//! the same for both, and an external provider only answers "who is this".
//!
//! Every token it mints is an opaque secret stored as its hash: the bearer
//! (`auth.access_tokens`), which [`crate::person`] looks up on every person
//! request, the refresh token rotated on every use with reuse ending the
//! session, the one-time codes a sign-in hands the console, and the CLI's
//! device authorizations. Nothing is signed, so there is no key to hold,
//! rotate or leak; ending a session deletes its tokens.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use telmoni_shared::db::tenant_session::{Person, Scoped, maintenance_scope, person_scope};
use telmoni_shared::{AuthError, TelmoniError, UserId};

use crate::db::{
    AuthLane, access_tokens, authorization_codes, device_codes,
    identities::{self, IdentityRead},
    refresh_tokens,
};
use crate::oidc::with_query;
use crate::provider::{DeviceAuthorization, DevicePoll, Subject, TokenResponse};

/// How long a bearer is good for. Short, because a revoked session is
/// refused on every person request anyway and a bearer's lifetime only
/// decides how long a copied one keeps working with no refresh.
pub const BEARER_TTL_SECS: i64 = 15 * 60;

/// How long a refresh token is good for, unused: the console's session
/// cookie lives as long.
const REFRESH_TTL_DAYS: i64 = 30;

/// How long the code a sign-in hands the console is good for: one redirect.
const CODE_TTL_SECS: i64 = 60;

/// How long a device has to be approved (RFC 8628 § 3.2 `expires_in`).
const DEVICE_TTL_SECS: i64 = 10 * 60;

/// The fewest seconds between a device's polls.
const DEVICE_INTERVAL_SECS: i32 = 5;

/// The alphabet a device's user code is spelled in: consonants alone, so no
/// eight of them read as a word, and none of the pairs a screen confuses.
const USER_CODE_ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";

/// Characters in a user code, spelled `XXXX-XXXX` to the person.
const USER_CODE_LEN: usize = 8;

/// The issuer. One per process, shared by every sign-in path.
pub struct Issuer {
    db: PgPool,
    /// The console's origin: the base of every page this issuer points at.
    app_url: String,
    /// Whether an address must be confirmed from its inbox before the
    /// person is let in (`VERIFY_EMAIL`). Off, an address is taken at its
    /// word, as a deployment with no mail must.
    verify_email: bool,
}

/// Two v4 UUIDs — 244 random bits — base64url: a bearer, a refresh token, a
/// code, a link's token.
pub(crate) fn secret() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The stored form of any secret this service mints.
pub(crate) fn hash(secret: &str) -> String {
    telmoni_shared::digest::sha256_hex(secret.as_bytes())
}

/// A session id, the `sid` every token of one session is filed under.
fn mint_sid() -> String {
    format!("ses_{}", Uuid::new_v4().simple())
}

/// Eight letters of [`USER_CODE_ALPHABET`], drawn from random bytes.
fn mint_user_code() -> String {
    let mut code = String::with_capacity(USER_CODE_LEN);
    while code.len() < USER_CODE_LEN {
        for byte in Uuid::new_v4().as_bytes() {
            if code.len() == USER_CODE_LEN {
                break;
            }
            // Rejection sampling: 240 is the largest multiple of 20 in a byte.
            if *byte >= 240 {
                continue;
            }
            if let Some(&c) = USER_CODE_ALPHABET.get(usize::from(byte % 20)) {
                code.push(char::from(c));
            }
        }
    }
    code
}

/// `WDJBMJHT` as the person sees it: `WDJB-MJHT`.
fn display_user_code(code: &str) -> String {
    let (head, tail) = code.split_at(code.len() / 2);
    format!("{head}-{tail}")
}

/// What the person typed, as the row stores it: letters alone, upper case.
#[must_use]
pub fn normalize_user_code(typed: &str) -> String {
    typed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn no_such_device() -> TelmoniError {
    AuthError::NotFound(
        "no device is waiting for that code — check it, or start the sign-in again".into(),
    )
    .into()
}

impl Issuer {
    /// Build the issuer over the service's pool. `app_url` is the console's
    /// origin; `verify_email` is the deployment's `VERIFY_EMAIL`.
    #[must_use]
    pub fn new(db: PgPool, app_url: &str, verify_email: bool) -> Self {
        Self {
            db,
            app_url: app_url.trim_end_matches('/').to_owned(),
            verify_email,
        }
    }

    /// The console's origin.
    #[must_use]
    pub fn app_url(&self) -> &str {
        &self.app_url
    }

    /// Whether this deployment insists on a confirmed address.
    #[must_use]
    pub fn verify_email(&self) -> bool {
        self.verify_email
    }

    /// A page of the console's, with a query.
    pub(crate) fn page(&self, path: &str, params: &[(&str, &str)]) -> String {
        with_query(&format!("{}{path}", self.app_url), params)
    }

    /// What this deployment believes about the address: confirmed from its
    /// inbox, or taken at its word because nothing here asks for more.
    fn address_verified(&self, identity: &identities::Identity) -> bool {
        identity.email_verified || !self.verify_email
    }

    /// The tokens a grant answers with: a bearer for the session and a fresh
    /// refresh token, both written under `tx`. `auth_method` is how the
    /// sign-in was proved, and `id_token` the external provider's own token
    /// when the session began at one, for the sign-out that names it.
    pub(crate) async fn issue_tokens<B: IdentityRead>(
        &self,
        tx: &mut Scoped<'_, B>,
        user_id: &UserId,
        sid: &str,
        auth_method: Option<&str>,
        id_token: Option<String>,
    ) -> Result<TokenResponse, TelmoniError> {
        let Some(identity) = identities::identity(tx, user_id).await? else {
            tracing::error!(user_id = %user_id, "a grant for a person with no identity row");
            return Err(AuthError::InvalidToken.into());
        };
        let now = Utc::now();
        let access_token = secret();
        access_tokens::create(
            tx,
            user_id,
            sid,
            &hash(&access_token),
            now + Duration::seconds(BEARER_TTL_SECS),
        )
        .await?;
        let refresh_token = secret();
        refresh_tokens::create(
            tx,
            user_id,
            sid,
            &hash(&refresh_token),
            now + Duration::days(REFRESH_TTL_DAYS),
        )
        .await?;
        Ok(TokenResponse {
            subject: Subject {
                sub: user_id.to_string(),
                email: Some(identity.email.clone()),
                email_verified: self.address_verified(&identity),
                given_name: identity.first_name,
                family_name: identity.last_name,
            },
            access_token,
            session_id: sid.to_owned(),
            id_token,
            refresh_token: Some(refresh_token),
            expires_in: u64::try_from(BEARER_TTL_SECS).unwrap_or(0),
            auth_method: auth_method.map(str::to_owned),
        })
    }

    /// A sign-in proved out of band — a right password — earns the one-time
    /// code the console's callback spends at [`Issuer::exchange_code`],
    /// exactly as it spends an external provider's.
    pub(crate) async fn mint_code(
        &self,
        tx: &mut Scoped<'_, Person>,
        user_id: &UserId,
    ) -> Result<String, TelmoniError> {
        let code = secret();
        authorization_codes::create(
            tx,
            user_id,
            &mint_sid(),
            &hash(&code),
            Utc::now() + Duration::seconds(CODE_TTL_SECS),
        )
        .await?;
        Ok(code)
    }

    /// Spend a code a sign-in minted here, for the session's tokens.
    pub async fn exchange_code(&self, code: &str) -> Result<TokenResponse, TelmoniError> {
        let mut mtx = maintenance_scope(&self.db, AuthLane).await?;
        let spent = authorization_codes::spend(&mut mtx, &hash(code)).await?;
        mtx.commit().await?;
        let Some((user_id, sid)) = spent else {
            tracing::warn!("exchange of a code that is unknown, spent or expired");
            return Err(AuthError::InvalidToken.into());
        };
        let mut tx = person_scope(&self.db, &user_id).await?;
        let tokens = self
            .issue_tokens(&mut tx, &user_id, &sid, Some("password"), None)
            .await?;
        tx.commit().await?;
        Ok(tokens)
    }

    /// Open a session for a person an external provider has just named:
    /// what the exchange lane does once [`crate::external`] has resolved
    /// them. `id_token` is the provider's own, kept for the sign-out.
    pub async fn begin_session(
        &self,
        user_id: &UserId,
        auth_method: Option<&str>,
        id_token: Option<String>,
    ) -> Result<TokenResponse, TelmoniError> {
        let mut tx = person_scope(&self.db, user_id).await?;
        let tokens = self
            .issue_tokens(&mut tx, user_id, &mint_sid(), auth_method, id_token)
            .await?;
        tx.commit().await?;
        Ok(tokens)
    }

    /// Spend a refresh token for the next pair. A token presented again past
    /// the reuse grace is a copy somebody kept, and ends its whole session.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenResponse, TelmoniError> {
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        match refresh_tokens::spend(&mut tx, &hash(refresh_token), Utc::now()).await? {
            Some(refresh_tokens::Spent::Live { user_id, sid }) => {
                let tokens = self
                    .issue_tokens(&mut tx, &user_id, &sid, None, None)
                    .await?;
                tx.commit().await?;
                Ok(tokens)
            }
            Some(refresh_tokens::Spent::Reused) => {
                tx.commit().await?;
                tracing::warn!(
                    "a refresh token was presented twice; every token of its session is revoked"
                );
                Err(AuthError::InvalidToken.into())
            }
            Some(refresh_tokens::Spent::Expired) | None => {
                tx.commit().await?;
                tracing::debug!("refresh refused: the token is expired or unknown");
                Err(AuthError::InvalidToken.into())
            }
        }
    }

    /// End a session's tokens: its bearers, which stop resolving at once,
    /// and its refresh tokens, so nothing can mint another. The
    /// `auth.sessions` row, revoked by the caller, refuses the bearers too,
    /// should this fail after it. Idempotent.
    pub async fn revoke_sid(&self, sid: &str) -> Result<(), TelmoniError> {
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        access_tokens::delete_for_sid(&mut tx, sid).await?;
        refresh_tokens::revoke_sid(&mut tx, sid).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Begin a device authorization (RFC 8628 § 3.1) for the CLI: the codes
    /// it shows the person and polls with.
    pub async fn device_authorize(&self) -> Result<DeviceAuthorization, TelmoniError> {
        let device_code = secret();
        let user_code = mint_user_code();
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        device_codes::create(
            &mut tx,
            &hash(&device_code),
            &user_code,
            DEVICE_INTERVAL_SECS,
            Utc::now() + Duration::seconds(DEVICE_TTL_SECS),
        )
        .await?;
        tx.commit().await?;
        let shown = display_user_code(&user_code);
        Ok(DeviceAuthorization {
            device_code,
            verification_uri: self.page("/auth/device", &[]),
            verification_uri_complete: Some(self.page("/auth/device", &[("code", &shown)])),
            user_code: shown,
            expires_in: u64::try_from(DEVICE_TTL_SECS).unwrap_or(0),
            interval: u64::try_from(DEVICE_INTERVAL_SECS).unwrap_or(5),
        })
    }

    /// One poll of the device-code grant (RFC 8628 § 3.4).
    pub async fn device_token_grant(&self, device_code: &str) -> Result<DevicePoll, TelmoniError> {
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let Some(device) = device_codes::find_for_poll(&mut tx, &hash(device_code)).await? else {
            tx.commit().await?;
            return Err(AuthError::InvalidToken.into());
        };
        let now = Utc::now();
        if device.expires_at <= now {
            device_codes::delete(&mut tx, device.id).await?;
            tx.commit().await?;
            return Ok(DevicePoll::Expired);
        }
        let polled = match device.status {
            device_codes::Status::Pending => {
                let too_soon = device.last_polled_at.is_some_and(|last| {
                    last + Duration::seconds(i64::from(device.interval_secs)) > now
                });
                device_codes::touch(&mut tx, device.id).await?;
                if too_soon {
                    DevicePoll::SlowDown
                } else {
                    DevicePoll::Pending
                }
            }
            device_codes::Status::Denied => {
                device_codes::delete(&mut tx, device.id).await?;
                DevicePoll::Denied
            }
            device_codes::Status::Approved => {
                let Some((user_id, sid)) = device.approved else {
                    device_codes::delete(&mut tx, device.id).await?;
                    tx.commit().await?;
                    return Err(TelmoniError::Internal(
                        "an approved device authorization names nobody".into(),
                    ));
                };
                device_codes::delete(&mut tx, device.id).await?;
                // The device is a session of its own; how the person who
                // approved it signed in is nothing it can answer for.
                let tokens = self
                    .issue_tokens(&mut tx, &user_id, &sid, None, None)
                    .await?;
                DevicePoll::Granted(Box::new(tokens))
            }
        };
        tx.commit().await?;
        Ok(polled)
    }

    /// The signed-in person approves the device showing `user_code`.
    pub async fn approve_device(
        &self,
        user_id: &UserId,
        user_code: &str,
    ) -> Result<(), TelmoniError> {
        let code = normalize_user_code(user_code);
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let approved = device_codes::approve(&mut tx, &code, user_id, &mint_sid()).await?;
        tx.commit().await?;
        if !approved {
            return Err(no_such_device());
        }
        tracing::info!(user_id = %user_id, "device sign-in approved");
        Ok(())
    }

    /// The signed-in person refuses the device showing `user_code`.
    pub async fn deny_device(&self, user_id: &UserId, user_code: &str) -> Result<(), TelmoniError> {
        let code = normalize_user_code(user_code);
        let mut tx = maintenance_scope(&self.db, AuthLane).await?;
        let denied = device_codes::deny(&mut tx, &code).await?;
        tx.commit().await?;
        if !denied {
            return Err(no_such_device());
        }
        tracing::info!(user_id = %user_id, "device sign-in denied");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_code_is_eight_consonants_shown_in_two_halves() {
        let code = mint_user_code();
        assert_eq!(code.len(), USER_CODE_LEN);
        assert!(
            code.bytes().all(|b| USER_CODE_ALPHABET.contains(&b)),
            "{code}"
        );
        let shown = display_user_code(&code);
        assert_eq!(shown.len(), USER_CODE_LEN + 1);
        assert_eq!(normalize_user_code(&shown), code);
        assert_eq!(normalize_user_code(" wdjb-mjht "), "WDJBMJHT");
    }

    #[test]
    fn a_secret_is_256_bits_and_never_repeats() {
        let a = secret();
        let b = secret();
        assert_eq!(URL_SAFE_NO_PAD.decode(&a).unwrap().len(), 32);
        assert_ne!(a, b);
        assert!(mint_sid().starts_with("ses_"));
    }
}
