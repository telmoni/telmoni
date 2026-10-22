//! `/cli/v1` — the CLI's session lanes, which the binary nests there for the
//! CLI to call straight, on the machine host. The device grant's start and
//! poll carry only the device code, worth nothing until a person approves it;
//! the refresh spends its token; `/me` and the session revoke act for the
//! person the bearer names. The shapes are the CLI's, camelCase throughout,
//! where the console's own refresh is snake_case.
//!
//! Nothing here is for a page: a browser sends `Sec-Fetch-Site` on every
//! request and a native client sends none, and a page that could reach the
//! refresh lane with a stored token would be a session it never held.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, header::USER_AGENT},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use telmoni_shared::extract::Json;
use telmoni_shared::middleware::problem;
use telmoni_shared::person_token::Principal;
use telmoni_shared::types::CONNECTION_BYTES;
use telmoni_shared::{AuthzError, TelmoniError};

use crate::AppState;
use crate::handler::{me, recording_organization_of, session};

/// The most a body on these lanes may be: a device code, a refresh token and
/// a row id are a few hundred bytes.
pub const BODY_CAP_BYTES: usize = 8 * 1024;

/// `POST /cli/v1/auth/refresh` request: the console's refresh, in the CLI's
/// spelling.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshRequest {
    /// The refresh token the CLI stored at sign-in or the last refresh.
    pub refresh_token: String,
    /// The `auth.sessions` row `/me` answered, which the revoke names.
    #[serde(default)]
    pub session_row_id: Option<Uuid>,
}

impl std::fmt::Debug for RefreshRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshRequest")
            .field("refresh_token", &"***")
            .field("session_row_id", &self.session_row_id)
            .finish()
    }
}

/// `POST /cli/v1/auth/refresh` — spend a refresh token for the next pair, as
/// the console's own refresh does.
pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RefreshRequest>,
) -> Result<impl IntoResponse, TelmoniError> {
    session::refresh_session(&state, &req.refresh_token, req.session_row_id)
        .await
        .map(Json)
}

/// `POST /cli/v1/me` — who the bearer is, as the console's `/me` answers it,
/// the device recorded from the request's own `User-Agent`. An organization
/// the CLI names has to be an id: the console's `/me` reads one it cannot
/// parse as none, right for a stale cookie and wrong for a client, which
/// would act in another organization without a word, given a slug by
/// mistake.
pub async fn me(
    State(state): State<Arc<AppState>>,
    principal: Principal,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    recording_organization_of(&headers)?;
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let answer = me::answer(&state, principal, &headers, user_agent).await?;
    // Counted against the organization it acted in, never refused: the day's
    // ceiling stops a script on the read API, not a person's terminal.
    if let (Some(organization_id), Some(telemetry)) = (
        answer.active_organization_id.as_ref(),
        state.siblings.telemetry.as_ref(),
    ) {
        let bytes = serde_json::to_vec(&answer).map_or(0, |body| body.len());
        telemetry
            .spend_data_out(
                organization_id,
                u64::try_from(bytes)
                    .unwrap_or(u64::MAX)
                    .saturating_add(CONNECTION_BYTES),
            )
            .await;
    }
    Ok(Json(answer))
}

/// A body is read up to [`BODY_CAP_BYTES`] and handed on whole; one past the
/// cap is refused as too large before any lane parses it. The one way a
/// bounded read of a body this small fails is the bound, so a read that
/// fails is answered as over it.
pub async fn cap_body(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    match axum::body::to_bytes(body, BODY_CAP_BYTES).await {
        Ok(bytes) => {
            next.run(Request::from_parts(parts, axum::body::Body::from(bytes)))
                .await
        }
        Err(_) => TelmoniError::PayloadTooLarge {
            max_bytes: u64::try_from(BODY_CAP_BYTES).unwrap_or(u64::MAX),
        }
        .into_response(),
    }
}

/// A request a browser made, by the `Sec-Fetch-Site` it alone sends, is
/// refused before any lane sees it.
pub async fn refuse_browsers(request: Request, next: Next) -> Response {
    let from_a_browser = request
        .headers()
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| site != "none");
    if from_a_browser {
        return TelmoniError::from(AuthzError::Forbidden("not a browser lane".into()))
            .into_response();
    }
    next.run(request).await
}

/// Every lane here takes `POST`.
pub async fn method_not_allowed() -> Response {
    problem::method_not_allowed("POST")
}

/// A path under `/cli/v1` no lane serves: a type of the door's own, since
/// auth's `/errors/auth/not-found` on a revoke says the session is already
/// gone, and a path no lane matches says nothing about any session.
pub async fn not_found() -> Response {
    problem::not_found("no such lane")
}
