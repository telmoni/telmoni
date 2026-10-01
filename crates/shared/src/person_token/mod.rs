//! The person behind a request: who auth resolved the session's bearer to.
//!
//! The bearer is an opaque secret auth's issuer minted at sign-in, which the
//! console relays as `Authorization: Bearer`. Nothing is read out of it:
//! auth looks its hash up in `auth.access_tokens`, where it names a person
//! and a session, and refuses it once the session is ended or the person has
//! asked to be deleted.
//!
//! Opaque because a signature would buy nothing here. Every person request
//! reads the session to refuse a revoked one anyway, so the lookup that
//! answers who is asking costs nothing a signed token would have saved, and
//! there is no signing key to provision, rotate or leak. The one token this
//! system checks by signature is an external provider's id token, at the
//! code exchange ([`crate::oidc::id_token`]).
//!
//! Auth is the one resolver. The modules beside it hand the request's
//! headers back to auth ([`crate::seam::Auth`]) and enforce on what auth
//! answers.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::error::{AuthError, TelmoniError};
use crate::types::UserId;

/// Who a bearer names. Inserted into the request by the lane's middleware;
/// handlers take it as an extractor and never read a header.
#[derive(Clone, Debug)]
pub struct Principal {
    /// The person — their id everywhere in this system.
    pub user_id: UserId,
    /// The session the bearer belongs to (`sid`): what `auth.sessions`
    /// records, and what a sign-out ends.
    pub session_id: String,
    /// When the bearer stops being taken, unix seconds. A cache built on
    /// this answer must not outlive it.
    pub expires_at: i64,
}

impl<S: Send + Sync> FromRequestParts<S> for Principal {
    type Rejection = TelmoniError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Self>().cloned().ok_or_else(|| {
            // A handler on a lane the middleware does not wrap. Loud, because
            // the fix is in the router and nothing about the request explains it.
            tracing::error!(
                route = parts
                    .extensions
                    .get::<axum::extract::MatchedPath>()
                    .map_or("unmatched", axum::extract::MatchedPath::as_str),
                "person lane reached without a resolved bearer; the route is mounted outside require_person_token"
            );
            AuthError::Unauthenticated.into()
        })
    }
}
