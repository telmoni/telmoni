//! `IntoResponse` impl for [`TelmoniError`] + an `AnyhowError` newtype, and
//! the two problem documents a door answers about its own lanes: a path none
//! of them serves, and a method none takes.

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::error::{TelmoniError, TenantError};
use crate::error_schema::ProblemDetails;

/// A path no lane serves, on a door that answers problem documents for
/// everything: `/errors/not-found`, apart from `/errors/auth/not-found`,
/// which a lane answers about a resource it looked for, so a client can
/// tell a missing lane from a missing row.
#[must_use]
pub fn not_found(detail: &str) -> Response {
    problem(not_found_details(detail), &[])
}

/// A method no lane at this path takes: `405`, with `Allow` naming the
/// methods it does.
#[must_use]
pub fn method_not_allowed(allow: &'static str) -> Response {
    problem(
        method_not_allowed_details(allow),
        &[(header::ALLOW, HeaderValue::from_static(allow))],
    )
}

/// [`not_found`]'s problem, for a lane that answers in a shape of its own.
#[must_use]
pub fn not_found_details(detail: &str) -> ProblemDetails {
    details(
        StatusCode::NOT_FOUND,
        "/errors/not-found",
        "not found",
        detail,
    )
}

/// [`method_not_allowed`]'s problem, for a lane that answers in a shape of
/// its own; `Allow` is the lane's to send.
#[must_use]
pub fn method_not_allowed_details(allow: &str) -> ProblemDetails {
    details(
        StatusCode::METHOD_NOT_ALLOWED,
        "/errors/method-not-allowed",
        "method not allowed",
        &format!("this lane takes {allow}"),
    )
}

fn details(status: StatusCode, type_uri: &str, title: &str, detail: &str) -> ProblemDetails {
    ProblemDetails {
        type_uri: type_uri.into(),
        title: title.into(),
        status: status.as_u16(),
        detail: Some(detail.into()),
        instance: None,
        extensions: serde_json::Map::new(),
    }
}

fn problem(body: ProblemDetails, headers: &[(header::HeaderName, HeaderValue)]) -> Response {
    let status = StatusCode::from_u16(body.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    for (name, value) in headers {
        response.headers_mut().insert(name.clone(), value.clone());
    }
    response
}

impl IntoResponse for TelmoniError {
    fn into_response(self) -> Response {
        match &self {
            Self::Database(e) => tracing::error!(error = %e, "database error"),
            Self::Internal(m) => tracing::error!(error = %m, "internal error"),
            Self::InternalSource { context, source } => {
                tracing::error!(error = %source, cause = ?source, context = %context, "internal error");
            }
            Self::AgentModelUnavailable { context, failure } => {
                tracing::warn!(
                    context = %context,
                    failure = failure.as_str(),
                    "agent model unavailable"
                );
            }
            _ => {}
        }

        let pd = self.to_problem_details();
        let status = StatusCode::from_u16(pd.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let retry_after = if matches!(
            &self,
            Self::Tenant(
                TenantError::RateLimited { .. }
                    | TenantError::PastCeiling { .. }
                    | TenantError::FeatureOff { .. }
            ) | Self::AgentRateLimited { .. }
        ) {
            pd.extensions
                .get("retry_after_secs")
                .and_then(serde_json::Value::as_u64)
        } else {
            None
        };

        let mut resp = Json(pd).into_response();
        *resp.status_mut() = status;
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(secs) = retry_after
            && let Ok(value) = HeaderValue::from_str(&secs.to_string())
        {
            resp.headers_mut().insert(header::RETRY_AFTER, value);
        }
        resp
    }
}

/// Newtype around `anyhow::Error` with an `IntoResponse` impl.
pub struct AnyhowError(pub anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AnyhowError {
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

impl IntoResponse for AnyhowError {
    fn into_response(self) -> Response {
        match self.0.downcast::<TelmoniError>() {
            Ok(typed) => typed.into_response(),
            Err(other) => TelmoniError::InternalSource {
                context: "unhandled error".into(),
                source: other.into(),
            }
            .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    use super::*;
    use crate::error::{AuthError, TenantError};

    async fn body_problem(resp: Response) -> ProblemDetails {
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("response body");
        serde_json::from_slice(&bytes).expect("body must parse as ProblemDetails")
    }

    #[tokio::test]
    async fn sets_status_and_problem_json_content_type() {
        let resp = TelmoniError::from(AuthError::TokenExpired).into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/problem+json")
        );

        let pd = body_problem(resp).await;
        assert_eq!(pd.type_uri, "/errors/auth/token-expired");
        assert_eq!(pd.status, 401);
    }

    #[tokio::test]
    async fn rate_limited_lifts_retry_after_header() {
        let resp = TelmoniError::from(TenantError::RateLimited {
            retry_after_secs: 42,
        })
        .into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("42")
        );
        let pd = body_problem(resp).await;
        assert_eq!(
            pd.extensions
                .get("retry_after_secs")
                .and_then(serde_json::Value::as_u64),
            Some(42)
        );
    }

    /// A plan's ceiling says when it lifts in the header too, as a rate limit does.
    #[tokio::test]
    async fn past_a_ceiling_lifts_retry_after_header() {
        let resp = TelmoniError::from(TenantError::PastCeiling {
            detail: "the day's data out is spent".into(),
            retry_after_secs: 3_600,
        })
        .into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("3600")
        );
    }

    #[tokio::test]
    async fn non_rate_limit_errors_carry_no_retry_after() {
        let resp = TelmoniError::from(AuthError::Unauthenticated).into_response();
        assert!(resp.headers().get(header::RETRY_AFTER).is_none());
    }

    #[tokio::test]
    async fn anyhow_error_downcasts_to_the_typed_response() {
        let err = anyhow::Error::new(TelmoniError::from(AuthError::Unauthenticated));
        let resp = AnyhowError(err).into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let pd = body_problem(resp).await;
        assert_eq!(pd.type_uri, "/errors/auth/unauthenticated");
    }

    #[tokio::test]
    async fn anyhow_error_without_telmoni_error_is_a_redacted_500() {
        let resp = AnyhowError(anyhow::anyhow!("boom: secret detail")).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let pd = body_problem(resp).await;
        assert_eq!(pd.type_uri, "/errors/internal/unknown");
        assert!(pd.detail.is_none());
    }

    #[tokio::test]
    async fn a_doors_own_answers_are_problem_documents_with_their_headers() {
        let resp = method_not_allowed("GET, HEAD, OPTIONS");
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            resp.headers()
                .get(header::ALLOW)
                .and_then(|v| v.to_str().ok()),
            Some("GET, HEAD, OPTIONS")
        );
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/problem+json")
        );
        let pd = body_problem(resp).await;
        assert_eq!(pd.type_uri, "/errors/method-not-allowed");
        assert_eq!(pd.status, 405);
        assert_eq!(
            pd.detail.as_deref(),
            Some("this lane takes GET, HEAD, OPTIONS")
        );

        let resp = not_found("no such lane");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let pd = body_problem(resp).await;
        assert_eq!(pd.type_uri, "/errors/not-found");
        assert_eq!(pd.detail.as_deref(), Some("no such lane"));
    }
}
