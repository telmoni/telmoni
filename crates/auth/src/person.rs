//! The person lane's gate: resolve the bearer to the person and the session
//! it belongs to, and hand the handler a
//! [`telmoni_shared::person_token::Principal`].
//!
//! Every route that acts for a person is wrapped in this and reads who is
//! asking from the `Principal` extension, never from a header. The service
//! secret still gates the hop (it proves the request began inside the
//! platform); this proves the person. A module beside auth gets the same
//! answer through [`principal_of`], which its [`telmoni_shared::seam::Auth`]
//! implementation calls.
//!
//! The bearer is opaque. Its hash is looked up in `auth.access_tokens`
//! before anybody is known, so on the maintenance lane, and the same
//! statement reads whether its session was ended here or its person has
//! asked to be deleted.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use telmoni_shared::db::tenant_session::maintenance_scope;
use telmoni_shared::person_token::Principal;
use telmoni_shared::{AuthError, TelmoniError};

use crate::AppState;
use crate::db::{AuthLane, access_tokens};

/// How far past its expiry a bearer is still taken: slack for a request
/// already in flight when it ran out, never a second lifetime. The console
/// refreshes with a minute left.
const EXP_LEEWAY_SECS: i64 = 30;

/// The prefix of a `telmoni_` API token, which rides the same header on the
/// `/v1` lane and must never be mistaken for a person.
const API_TOKEN_PREFIX: &str = "telmoni_";

/// The token in an `Authorization: Bearer` header, or the refusal it earns.
fn bearer_of(authorization: Option<&str>) -> Result<&str, TelmoniError> {
    let Some(raw) = authorization else {
        return Err(refuse("no bearer", AuthError::Unauthenticated));
    };
    let Some(token) = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return Err(refuse(
            "authorization is not a bearer",
            AuthError::Unauthenticated,
        ));
    };
    if token.starts_with(API_TOKEN_PREFIX) {
        return Err(refuse(
            "a telmoni_ API token on a person lane",
            AuthError::InvalidToken,
        ));
    }
    Ok(token)
}

/// Resolve `Authorization: Bearer` to the person and session it was minted
/// for, and refuse it once that session was ended here or its person has
/// confirmed deleting their account.
pub async fn principal_of(
    state: &AppState,
    authorization: Option<&str>,
) -> Result<Principal, TelmoniError> {
    let token = bearer_of(authorization)?;
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let found = access_tokens::resolve(&mut tx, &crate::issuer::hash(token)).await?;
    tx.commit().await?;
    let Some(found) = found else {
        return Err(refuse("unknown bearer", AuthError::InvalidToken));
    };
    let expires_at = found.expires_at.timestamp();
    if expires_at.saturating_add(EXP_LEEWAY_SECS) <= chrono::Utc::now().timestamp() {
        return Err(refuse("expired", AuthError::TokenExpired));
    }
    if found.refused {
        tracing::warn!(user_id = %found.user_id,
            "person lane: session revoked or account deletion in progress");
        return Err(AuthError::Unauthenticated.into());
    }
    Ok(Principal {
        user_id: found.user_id,
        session_id: found.sid,
        expires_at,
    })
}

/// Every refusal is one of three wire answers; the distinction lives in the log.
fn refuse(why: &'static str, error: AuthError) -> TelmoniError {
    tracing::warn!(why, "person lane: bearer refused");
    error.into()
}

/// The middleware form of [`principal_of`]: the resolved principal rides the
/// request as an extension for the handler's extractor.
pub async fn require_person_token(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, TelmoniError> {
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    let principal = principal_of(&state, authorization).await?;
    request.extensions_mut().insert(principal);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(result: Result<&str, TelmoniError>) -> u16 {
        result.unwrap_err().to_problem_details().status
    }

    #[test]
    fn the_header_takes_a_bearer_and_nothing_else() {
        assert_eq!(bearer_of(Some("Bearer abc")).unwrap(), "abc");
        assert_eq!(bearer_of(Some("bearer  abc ")).unwrap(), "abc");
        assert_eq!(status(bearer_of(Some("Basic abc"))), 401);
        assert_eq!(status(bearer_of(Some("Bearer "))), 401);
        assert_eq!(status(bearer_of(None)), 401);
    }

    /// A `telmoni_` API token rides the same header on `/v1`, and it is not a
    /// person: on this lane it is refused before anything is looked up.
    #[test]
    fn an_api_token_is_refused_before_any_lookup() {
        let err = bearer_of(Some("Bearer telmoni_abc")).unwrap_err();
        assert_eq!(
            err.to_problem_details().type_uri,
            "/errors/auth/invalid-token"
        );
    }
}
