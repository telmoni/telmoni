//! `x-service-secret` middleware — internal service-to-service auth.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::header::USER_AGENT,
    middleware::Next,
    response::Response,
};
use subtle::{Choice, ConstantTimeEq};

use crate::error::{AuthError, TelmoniError};

/// HTTP header carrying the shared service secret between Telmoni services.
pub const SERVICE_SECRET_HEADER: &str = "x-service-secret";

/// Primary service secret plus an optional secondary for rotation windows.
#[derive(Clone)]
pub struct ServiceSecrets {
    primary: Arc<str>,
    next: Option<Arc<str>>,
}

impl std::fmt::Debug for ServiceSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceSecrets")
            .field("primary", &"***")
            .field("next", &self.next.as_ref().map(|_| "***"))
            .finish()
    }
}

impl ServiceSecrets {
    /// Build a secrets bundle. `primary` is always required; `next` is
    /// `Some` only during a rotation window.
    pub fn new(primary: impl Into<Arc<str>>, next: Option<impl Into<Arc<str>>>) -> Self {
        Self {
            primary: primary.into(),
            next: next.map(Into::into),
        }
    }

    /// Constant-time match against either secret. Both comparisons always
    /// run, and the `Choice` OR avoids a short-circuit that would leak which
    /// secret matched. The length short-circuit is harmless: the length is fixed.
    #[must_use]
    pub fn matches(&self, presented: &[u8]) -> bool {
        let primary_match = self.primary.as_bytes().ct_eq(presented);
        let next_match = self
            .next
            .as_ref()
            .map_or(Choice::from(0), |n| n.as_bytes().ct_eq(presented));
        bool::from(primary_match | next_match)
    }
}

/// Axum middleware: require a valid `x-service-secret` header.
pub async fn require_service_secret(
    State(secrets): State<ServiceSecrets>,
    request: Request,
    next: Next,
) -> Result<Response, TelmoniError> {
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or("unmatched", axum::extract::MatchedPath::as_str)
        .to_owned();

    let Some(presented) = request.headers().get(SERVICE_SECRET_HEADER) else {
        tracing::warn!(
            method = %request.method(),
            route,
            "service-auth: missing x-service-secret header",
        );
        return Err(AuthError::ServiceCredentialRejected.into());
    };

    if !secrets.matches(presented.as_bytes()) {
        tracing::warn!(
            method = %request.method(),
            route,
            "service-auth: invalid x-service-secret",
        );
        return Err(AuthError::ServiceCredentialRejected.into());
    }

    let caller = request
        .headers()
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    tracing::debug!(
        caller = %caller,
        method = %request.method(),
        route,
        "service-auth: accepted",
    );

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single(v: &str) -> ServiceSecrets {
        ServiceSecrets::new(v, None::<String>)
    }

    fn dual(a: &str, b: &str) -> ServiceSecrets {
        ServiceSecrets::new(a, Some(b.to_string()))
    }

    #[test]
    fn matches_primary() {
        assert!(single("primary-secret").matches(b"primary-secret"));
    }

    #[test]
    fn rejects_wrong_secret() {
        assert!(!single("primary-secret").matches(b"different"));
    }

    #[test]
    fn rejects_empty() {
        assert!(!single("primary-secret").matches(b""));
    }

    #[test]
    fn rejects_truncated_prefix() {
        assert!(!single("primary-secret").matches(b"primary-secre"));
    }

    #[test]
    fn rejects_extra_suffix() {
        assert!(!single("primary-secret").matches(b"primary-secretX"));
    }

    #[test]
    fn dual_accepts_primary() {
        assert!(dual("primary", "next").matches(b"primary"));
    }

    #[test]
    fn dual_accepts_next() {
        assert!(dual("primary", "next").matches(b"next"));
    }

    #[test]
    fn dual_rejects_unrelated() {
        assert!(!dual("primary", "next").matches(b"third"));
    }
}
