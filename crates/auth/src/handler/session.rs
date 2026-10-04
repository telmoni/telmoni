//! BFF-facing session endpoints — the thin-client interface.
//!
//! Every session is the issuer's ([`crate::issuer`]), however the person
//! proved who they are. The start lane decides where the browser goes to
//! prove it: the console's own sign-in page while the login form is on, or
//! the external provider's when it is off or when the page's "Continue
//! with…" asked for it. The exchange lane spends the code either earned and
//! opens the session; refresh, sign-out and the device grant are the same
//! for both.

use std::sync::Arc;

use axum::{extract::State, response::IntoResponse};
use serde::{Deserialize, Serialize};

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::extract::Json;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{AuthError, TelmoniError};

use crate::AppState;
use crate::db::AuthLane;
use crate::provider::{DevicePoll, TokenResponse};

/// The one provider a caller may name beside the accounts held here.
const EXTERNAL: &str = "external";

/// `POST /internal/auth/start` request. `state` is the CSRF token the BFF
/// sealed into its PKCE cookie, echoed so the callback can be cross-checked.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    /// Opaque CSRF/state value, generated + stored by the BFF.
    pub state: String,
    /// Open the create-account screen rather than sign-in, for
    /// `/auth/signup` and for an invite link.
    #[serde(default)]
    pub sign_up: bool,
    /// The email to pre-fill on that screen (`login_hint`), when the caller
    /// collected one first.
    #[serde(default)]
    pub login_hint: Option<String>,
    /// `external` to go to the external provider's page rather than the
    /// console's own: the sign-in page's "Continue with…". Absent, the
    /// console's page while the login form is on, the provider's otherwise.
    #[serde(default)]
    pub provider: Option<String>,
}

/// `POST /internal/auth/start` response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResponse {
    /// Fully-built authorization URL to redirect the browser to.
    pub authorize_url: String,
}

/// `POST /internal/auth/exchange` request — the `?code=...` the sign-in
/// redirected back to the BFF's callback with.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeRequest {
    /// Authorization code from the redirect.
    pub code: String,
    /// `external` when the code is the external provider's; absent, it is
    /// one a sign-in here minted.
    #[serde(default)]
    pub provider: Option<String>,
}

impl std::fmt::Debug for ExchangeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExchangeRequest")
            .field("code", &"***")
            .field("provider", &self.provider)
            .finish()
    }
}

/// `POST /internal/auth/device/start` response — what the CLI shows the
/// person and what it polls with (RFC 8628 § 3.2).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStartResponse {
    /// The secret the CLI polls with. Shown to nobody.
    pub device_code: String,
    /// The short code the person types on the console's device page.
    pub user_code: String,
    /// Where they type it.
    pub verification_uri: String,
    /// The same page with the code filled in, for a CLI that can open a browser.
    pub verification_uri_complete: Option<String>,
    /// Seconds until both codes expire.
    pub expires_in: u64,
    /// The fewest seconds to leave between polls.
    pub interval: u64,
}

impl std::fmt::Debug for DeviceStartResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceStartResponse")
            .field("device_code", &"***")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// `POST /internal/auth/device/poll` request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DevicePollRequest {
    /// The device code the start answered.
    pub device_code: String,
}

impl std::fmt::Debug for DevicePollRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevicePollRequest")
            .field("device_code", &"***")
            .finish()
    }
}

/// `POST /internal/auth/device/{approve,deny}`: the code the device shows.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceDecisionRequest {
    pub user_code: String,
}

/// `POST /internal/auth/device/poll` answer while nothing has been granted:
/// a 202 carrying the RFC's own word for why.
#[derive(Debug, Serialize)]
struct DevicePending {
    status: &'static str,
}

/// `POST /internal/auth/refresh` request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    /// The refresh token the BFF stored in the prior session.
    pub refresh_token: String,
    /// The `auth.sessions` row the cookie names. A revoked row refuses the
    /// refresh BEFORE the single-use token is spent, and a successful grant
    /// touches it.
    #[serde(default)]
    pub session_row_id: Option<uuid::Uuid>,
}

impl std::fmt::Debug for RefreshRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshRequest")
            .field("refresh_token", &"***")
            .finish()
    }
}

/// `POST /internal/auth/logout` request — the session to end.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogoutRequest {
    /// The session id the exchange answered with.
    pub session_id: String,
}

/// `POST /internal/auth/logout-url` request — mint the external provider's
/// **browser** logout URL, for a session that began there.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogoutUrlRequest {
    /// The session id the exchange answered with.
    pub session_id: String,
    /// The external provider's id token the exchange answered and the
    /// console sealed, for a logout page that names the session by it.
    /// Absent for a session that began here: nothing to end elsewhere.
    #[serde(default)]
    pub id_token: Option<String>,
    /// Where the browser lands after the sign-out; must be allowlisted at
    /// an external provider.
    pub return_to: String,
}

impl std::fmt::Debug for LogoutUrlRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogoutUrlRequest")
            .field("session_id", &self.session_id)
            .field("id_token", &self.id_token.as_ref().map(|_| "***"))
            .field("return_to", &self.return_to)
            .finish()
    }
}

/// `POST /internal/auth/logout-url` response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogoutUrlResponse {
    /// Fully-built logout URL to redirect the browser to.
    pub logout_url: String,
}

/// `GET /internal/auth/config` — what the console's sign-in pages show.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInConfig {
    /// Whether the login form is on: the email and password fields.
    pub password_sign_in: bool,
    /// Whether anybody may create an account there, or only the invited.
    pub allow_sign_up: bool,
    /// Whether a new account confirms its address from a mail first.
    pub verify_email: bool,
    /// The external provider, when one is configured.
    pub external: Option<ExternalSummary>,
}

/// The external provider as the sign-in page names it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSummary {
    /// "Continue with {name}".
    pub name: String,
}

/// Authenticated identity + tokens the BFF seals into `telmoni_session`, for
/// both `exchange` and `refresh`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthnResult {
    /// The person's id — the BFF's `userId`, and their key in
    /// `auth.identities`. Never an organization's id.
    pub user_id: String,
    /// Email, as recorded.
    pub email: Option<String>,
    /// Whether the address counts as verified here; the callback refuses
    /// to seal a session when it is `false`.
    pub email_verified: bool,
    /// Given name, when present.
    pub first_name: Option<String>,
    /// Family name, when present.
    pub last_name: Option<String>,
    /// The bearer, stored in the session and relayed on every request. An
    /// opaque secret: nothing is read out of it.
    pub access_token: String,
    /// The session the tokens belong to, which the sign-out names.
    pub session_id: String,
    /// The refresh token, rotated on every use.
    pub refresh_token: Option<String>,
    /// Seconds until the access token expires (drives the BFF's `expiresAt`).
    pub expires_in: u64,
    /// The external provider's id token, when the session began at one and
    /// this is the exchange; a refresh carries none, and the BFF keeps the
    /// one it sealed.
    pub id_token: Option<String>,
    /// How this sign-in was proved, or `None`. ⚠ Sealed into the session and
    /// never stored here: a column would outlive the sign-in it describes and
    /// answer for a person who has since signed in another way. A refresh
    /// carries none, and the BFF keeps what it sealed.
    pub auth_method: Option<String>,
}

impl From<TokenResponse> for AuthnResult {
    fn from(t: TokenResponse) -> Self {
        Self {
            user_id: t.subject.sub,
            email: t.subject.email,
            email_verified: t.subject.email_verified,
            first_name: t.subject.given_name,
            last_name: t.subject.family_name,
            access_token: t.access_token,
            session_id: t.session_id,
            refresh_token: t.refresh_token,
            expires_in: t.expires_in,
            id_token: t.id_token,
            auth_method: t.auth_method,
        }
    }
}

impl std::fmt::Debug for AuthnResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthnResult")
            .field("user_id", &self.user_id)
            .field("email", &self.email)
            .field("email_verified", &self.email_verified)
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("access_token", &"***")
            .field("session_id", &self.session_id)
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "***"))
            .field("expires_in", &self.expires_in)
            .field("id_token", &self.id_token.as_ref().map(|_| "***"))
            .field("auth_method", &self.auth_method)
            .finish()
    }
}

/// Whether a request named the external provider, refusing any other name.
fn wants_external(provider: Option<&str>) -> Result<bool, TelmoniError> {
    match provider {
        None => Ok(false),
        Some(EXTERNAL) => Ok(true),
        Some(_) => Err(AuthError::BadRequest(format!("provider must be \"{EXTERNAL}\"")).into()),
    }
}

/// The refusal for a lane the external provider serves when none is
/// configured.
fn no_external() -> TelmoniError {
    AuthError::NotFound("this deployment has no external identity provider".into()).into()
}

/// `GET /internal/auth/config` — what the sign-in pages offer.
pub async fn config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(SignInConfig {
        password_sign_in: state.password.is_some(),
        allow_sign_up: state.password.as_ref().is_some_and(|p| p.allow_sign_up()),
        verify_email: state.issuer.verify_email(),
        external: state.external.as_ref().map(|e| ExternalSummary {
            name: e.provider.name().to_owned(),
        }),
    })
}

/// `POST /internal/auth/start` — where the browser goes to sign in: the
/// console's own page while the login form is on, the external provider's
/// page when it is off or when asked for by name.
pub async fn start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let external = wants_external(req.provider.as_deref())?;
    let url = match (&state.password, &state.external) {
        (Some(_), _) if !external => {
            let page = if req.sign_up {
                "/auth/sign-up"
            } else {
                "/auth/sign-in"
            };
            let mut params = vec![("state", req.state.as_str())];
            if let Some(hint) = req.login_hint.as_deref() {
                params.push(("email", hint));
            }
            state.issuer.page(page, &params)
        }
        (_, Some(external)) => {
            external
                .provider
                .authorize_url(
                    &state.config.redirect_uri,
                    &req.state,
                    req.sign_up,
                    req.login_hint.as_deref(),
                )
                .await?
        }
        (_, None) => return Err(no_external()),
    };
    Ok(Json(StartResponse { authorize_url: url }))
}

/// `POST /internal/auth/device/start` — begin a device authorization for a
/// client with no browser of its own. The person approves it on the
/// console's device page from any browser on any machine, which is what
/// lets the CLI sign in over SSH or in a container.
pub async fn device_start(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, TelmoniError> {
    let started = state.issuer.device_authorize().await?;
    Ok(Json(DeviceStartResponse {
        device_code: started.device_code,
        user_code: started.user_code,
        verification_uri: started.verification_uri,
        verification_uri_complete: started.verification_uri_complete,
        expires_in: started.expires_in,
        interval: started.interval,
    }))
}

/// `POST /internal/auth/device/poll` — one poll of the device-code grant,
/// as a public client. Granted is a 200 with the same `AuthnResult` the
/// console's exchange answers. Not yet is a 202 saying
/// `authorization_pending` or `slow_down`; refused is a 403; lapsed is a 400.
pub async fn device_poll(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DevicePollRequest>,
) -> Result<axum::response::Response, TelmoniError> {
    if req.device_code.trim().is_empty() {
        return Err(AuthError::BadRequest("deviceCode is required".into()).into());
    }
    match state.issuer.device_token_grant(&req.device_code).await? {
        DevicePoll::Granted(tokens) => Ok(Json(AuthnResult::from(*tokens)).into_response()),
        DevicePoll::Pending => Ok((
            axum::http::StatusCode::ACCEPTED,
            Json(DevicePending {
                status: "authorization_pending",
            }),
        )
            .into_response()),
        DevicePoll::SlowDown => Ok((
            axum::http::StatusCode::ACCEPTED,
            Json(DevicePending {
                status: "slow_down",
            }),
        )
            .into_response()),
        DevicePoll::Denied => Err(telmoni_shared::AuthzError::Forbidden(
            "the sign-in was denied from the console".into(),
        )
        .into()),
        DevicePoll::Expired => Err(AuthError::BadRequest(
            "the device code expired before it was approved; start again".into(),
        )
        .into()),
    }
}

/// `POST /internal/auth/device/approve` — the signed-in person approves the
/// device showing the code; its next poll is granted a session as them.
pub async fn approve_device(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Json(req): Json<DeviceDecisionRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    state
        .issuer
        .approve_device(&principal.user_id, &req.user_code)
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /internal/auth/device/deny` — the signed-in person refuses it.
pub async fn deny_device(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    Json(req): Json<DeviceDecisionRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    state
        .issuer
        .deny_device(&principal.user_id, &req.user_code)
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /internal/auth/exchange` — trade the authorization code for an
/// authenticated identity and the session's tokens.
pub async fn exchange(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExchangeRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let code = req.code.clone();
    // As `start` decides: with the login form off the browser went to the
    // external provider unasked, and the console's cookie names no provider.
    // Only the form mints codes here, so without it every code is the
    // provider's.
    if wants_external(req.provider.as_deref())? || state.password.is_none() {
        let Some(external) = state.external.as_ref() else {
            return Err(no_external());
        };
        let grant = async {
            let authenticated = external
                .provider
                .exchange_code(&req.code, &state.config.redirect_uri)
                .await?;
            let user_id = external
                .resolve_person(&state.db, &authenticated.subject)
                .await?;
            state
                .issuer
                .begin_session(
                    &user_id,
                    authenticated.auth_method.as_deref(),
                    authenticated.id_token,
                )
                .await
        };
        return exchange_once(&state, code, grant).await.map(Json);
    }
    let grant = state.issuer.exchange_code(&req.code);
    exchange_once(&state, code, grant).await.map(Json)
}

/// Spend `code` at most once: the first caller runs `grant`, a concurrent
/// caller waits for that outcome, and a retry within the cache's TTL gets
/// the same answer without a second grant.
async fn exchange_once(
    state: &AppState,
    code: String,
    grant: impl Future<Output = Result<TokenResponse, TelmoniError>>,
) -> Result<AuthnResult, TelmoniError> {
    match state.exchange_cache.start_or_join(&code) {
        crate::CacheDecision::Hit(cached) => Ok(*cached),
        crate::CacheDecision::Wait(mut rx) => loop {
            if let Some(res) = rx.borrow().clone() {
                return match res {
                    Ok(authn) => Ok(authn),
                    Err(msg) => Err(AuthError::BadRequest(msg).into()),
                };
            }
            if rx.changed().await.is_err() {
                return Err(
                    AuthError::BadRequest("concurrent code exchange canceled".into()).into(),
                );
            }
        },
        crate::CacheDecision::Leader(tx) => match grant.await {
            Ok(tokens) => {
                let result = AuthnResult::from(tokens);
                state.exchange_cache.insert(code, result.clone());
                let _ = tx.send(Some(Ok(result.clone())));
                Ok(result)
            }
            Err(err) => {
                state.exchange_cache.remove(&code);
                let _ = tx.send(Some(Err(err.to_string())));
                Err(err)
            }
        },
    }
}

/// `POST /internal/auth/refresh` — spend a refresh token for the next pair.
pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RefreshRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    // Maintenance scope: a row id is the only handle this pre-session lane
    // carries, and the person is not known until the grant.
    if let Some(row) = req.session_row_id {
        let mut tx = maintenance_scope(&state.db, AuthLane).await?;
        let revoked = crate::db::sessions::is_revoked(&mut tx, row).await?;
        tx.commit().await?;
        if revoked == Some(true) {
            tracing::info!(session = %row, "refresh refused: the session was revoked here");
            return Err(AuthError::Unauthenticated.into());
        }
    }

    let tokens = state.issuer.refresh(&req.refresh_token).await?;

    if let Some(row) = req.session_row_id {
        let mut tx = maintenance_scope(&state.db, AuthLane).await?;
        let live = crate::db::sessions::touch_by_id(&mut tx, row).await?;
        tx.commit().await?;
        if !live {
            // Revoked between the pre-check and the grant; the token is spent,
            // and the device signs out now rather than at its next refresh.
            return Err(AuthError::Unauthenticated.into());
        }
    }
    Ok(Json(AuthnResult::from(tokens)))
}

/// `POST /internal/auth/logout` — end the session: its row, which refuses
/// any bearer of it still out, and its tokens, so none resolves and nothing
/// can mint another.
pub async fn logout(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LogoutRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let row = crate::db::sessions::revoke_by_sid(&mut tx, &req.session_id).await?;
    tx.commit().await?;
    if row.is_none() {
        tracing::debug!("logout for a session with no row here");
    }
    state.issuer.revoke_sid(&req.session_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /internal/auth/logout-url` — where the browser goes to finish the
/// sign-out: the external provider's logout page for a session that began
/// there, else straight back. Ends nothing itself; `logout` does.
pub async fn logout_url(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LogoutUrlRequest>,
) -> impl IntoResponse {
    let logout_url = match (&state.external, req.id_token.as_deref()) {
        (Some(external), Some(id_token)) => {
            external
                .provider
                .logout_url(Some(id_token), &req.return_to)
                .await
        }
        _ => req.return_to,
    };
    Json(LogoutUrlResponse { logout_url })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authn_result_debug_masks_every_token() {
        let r = AuthnResult {
            user_id: "user-42".into(),
            email: Some("a@example.com".into()),
            email_verified: true,
            first_name: None,
            last_name: None,
            access_token: "live_access_token_abc".into(),
            session_id: "ses_42".into(),
            refresh_token: Some("live_refresh_token_def".into()),
            expires_in: 3600,
            id_token: Some("live_id_token_ghi".into()),
            auth_method: Some("google".into()),
        };
        let rendered = format!("{r:?}");
        for leaked in ["live_access_token", "live_refresh_token", "live_id_token"] {
            assert!(!rendered.contains(leaked), "leaked {leaked}: {rendered}");
        }
        assert!(rendered.contains("user-42"));
        assert!(rendered.contains("ses_42"), "the session id is no secret");
        assert!(rendered.contains("google"));
    }

    #[test]
    fn credential_request_debugs_mask_their_payloads() {
        let rendered = format!(
            "{:?} {:?} {:?}",
            ExchangeRequest {
                code: "live_auth_code".into(),
                provider: None,
            },
            RefreshRequest {
                refresh_token: "live_refresh".into(),
                session_row_id: None,
            },
            LogoutUrlRequest {
                session_id: "sess_1".into(),
                id_token: Some("live_id_token".into()),
                return_to: "https://app/".into(),
            },
        );
        for leaked in ["live_auth_code", "live_refresh", "live_id_token"] {
            assert!(!rendered.contains(leaked), "leaked {leaked}: {rendered}");
        }
        assert!(rendered.contains("sess_1"));
    }

    #[test]
    fn a_device_poll_request_debugs_without_its_code() {
        let rendered = format!(
            "{:?}",
            DevicePollRequest {
                device_code: "live_device_code".into(),
            }
        );
        assert!(!rendered.contains("live_device_code"), "leaked: {rendered}");
    }

    #[test]
    fn only_the_external_provider_may_be_named() {
        assert!(!wants_external(None).unwrap());
        assert!(wants_external(Some("external")).unwrap());
        assert!(wants_external(Some("google")).is_err());
    }
}
