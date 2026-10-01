//! `IntoResponse` impl for [`TelmoniError`] + an `AnyhowError` newtype.

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::error::{TelmoniError, TenantError};

impl IntoResponse for TelmoniError {
    fn into_response(self) -> Response {
        match &self {
            Self::Database(e) => tracing::error!(error = %e, "database error"),
            Self::Internal(m) => tracing::error!(error = %m, "internal error"),
            Self::InternalSource { context, source } => {
                tracing::error!(error = %source, cause = ?source, context = %context, "internal error");
            }
            Self::AgentModelUnavailable { context } => {
                tracing::warn!(context = %context, "agent model unavailable");
            }
            _ => {}
        }

        let pd = self.to_problem_details();
        let status = StatusCode::from_u16(pd.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let retry_after = if matches!(
            &self,
            Self::Tenant(TenantError::RateLimited { .. } | TenantError::FeatureOff { .. })
                | Self::AgentRateLimited { .. }
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
    use crate::error_schema::ProblemDetails;

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
}
