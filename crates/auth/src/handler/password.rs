//! The login form's lanes, mounted only while it is on (`AppState::password`).
//! The console's sign-in, sign-up, verification, forgot and reset pages post
//! here through the service secret.
//!
//! Every body that carries a password, a code or a link's token holds it as
//! [`Redacted`], so a derived `Debug` prints none of them.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::extract::Json;
use telmoni_shared::{AuthError, Redacted, TelmoniError};

use crate::AppState;
use crate::password::{PasswordProvider, SignedUp};

/// The accounts held here, or a 404 for a deployment whose login form is
/// off. Unreachable through the router, which mounts nothing here then.
fn provider(state: &AppState) -> Result<&PasswordProvider, TelmoniError> {
    state
        .password
        .as_deref()
        .ok_or_else(|| AuthError::NotFound("this deployment's login form is off".into()).into())
}

/// `POST /internal/auth/password/sign-up`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignUpRequest {
    pub email: String,
    pub password: Redacted,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
}

/// `POST /internal/auth/password/sign-in`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignInRequest {
    pub email: String,
    pub password: Redacted,
}

/// `POST /internal/auth/password/sign-in` answer, and a sign-up's when the
/// account is usable at once: the one-time code the console's callback
/// spends at the exchange lane.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInResponse {
    pub code: String,
}

impl std::fmt::Debug for SignInResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInResponse")
            .field("code", &"***")
            .finish()
    }
}

/// `POST /internal/auth/password/verify`: the two halves of the link.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyEmailRequest {
    pub user_id: String,
    pub token: Redacted,
}

/// `POST /internal/auth/password/forgot`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

/// `POST /internal/auth/password/reset`: the link's two halves and the new
/// password.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetPasswordRequest {
    pub user_id: String,
    pub token: Redacted,
    pub password: Redacted,
}

/// `POST /internal/auth/password/sign-up` — create an account. 202 with the
/// address to be confirmed from its mail, or 200 with the code that opens
/// the session when the deployment asks for no confirmation.
pub async fn sign_up(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SignUpRequest>,
) -> Result<axum::response::Response, TelmoniError> {
    let signed_up = provider(&state)?
        .sign_up(
            &req.email,
            req.password.expose(),
            req.given_name.as_deref(),
            req.family_name.as_deref(),
        )
        .await?;
    Ok(match signed_up {
        SignedUp::LinkSent => (StatusCode::ACCEPTED, Json(json!({ "sent": true }))).into_response(),
        SignedUp::Code(code) => Json(SignInResponse { code }).into_response(),
    })
}

/// `POST /internal/auth/password/sign-in` — check the password and answer
/// the code. A wrong password and an unknown address are the same 401.
pub async fn sign_in(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SignInRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let code = provider(&state)?
        .sign_in(&req.email, req.password.expose())
        .await?;
    Ok(Json(SignInResponse { code }))
}

/// `POST /internal/auth/password/verify` — spend the verification link.
pub async fn verify_email(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyEmailRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = super::parse_user_id(&req.user_id)?;
    provider(&state)?
        .verify_email(&user_id, req.token.expose())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /internal/auth/password/forgot` — mail a reset link if the address
/// has an account, and say nothing either way.
pub async fn forgot_password(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ForgotPasswordRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    provider(&state)?.forgot_password(&req.email).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "sent": true }))))
}

/// `POST /internal/auth/password/reset` — spend the reset link and set the
/// password; every session ends.
pub async fn reset_password(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    let user_id = super::parse_user_id(&req.user_id)?;
    provider(&state)?
        .reset_password(&user_id, req.token.expose(), req.password.expose())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_credential_carrying_body_debugs_without_it() {
        let sign_up: SignUpRequest = serde_json::from_value(json!({
            "email": "ada@example.com", "password": "hunter22hunter22", "givenName": "Ada"
        }))
        .unwrap();
        let sign_in: SignInRequest = serde_json::from_value(json!({
            "email": "ada@example.com", "password": "hunter22hunter22"
        }))
        .unwrap();
        let reset: ResetPasswordRequest = serde_json::from_value(json!({
            "userId": "user_1", "token": "tok_live", "password": "hunter22hunter22"
        }))
        .unwrap();
        let verify: VerifyEmailRequest =
            serde_json::from_value(json!({ "userId": "user_1", "token": "tok_live" })).unwrap();
        let rendered = format!(
            "{sign_up:?} {sign_in:?} {reset:?} {verify:?} {:?}",
            SignInResponse {
                code: "code_live".into()
            }
        );
        for leaked in ["hunter22", "tok_live", "code_live"] {
            assert!(!rendered.contains(leaked), "leaked {leaked}: {rendered}");
        }
        assert!(rendered.contains("Ada"));
    }
}
