//! Extractors for HTTP handlers — typed request parts that fail with the
//! crate's standard errors rather than axum's defaults, so a malformed payload
//! is a named 400 instead of a 500 or a leaked internal type name.

use axum::{
    extract::{
        FromRequest, FromRequestParts, Request,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::TelmoniError;

/// Like [`axum::Json`], but a deserialize rejection becomes a
/// [`TelmoniError::BadRequest`] naming the failure.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = TelmoniError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(reject(&rejection)),
        }
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Like [`axum::extract::Query`], but a deserialize rejection (unparseable or
/// an unknown parameter) becomes a [`TelmoniError::BadRequest`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = TelmoniError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(reject(&rejection)),
        }
    }
}

/// One conversion point so every extractor rejection renders the same 400 problem.
fn reject(rejection: &impl RejectionText) -> TelmoniError {
    TelmoniError::BadRequest(rejection.text())
}

/// The rejection's safe, client-facing body text, generic over both rejections.
trait RejectionText {
    fn text(&self) -> String;
}

impl RejectionText for JsonRejection {
    fn text(&self) -> String {
        self.body_text()
    }
}

impl RejectionText for QueryRejection {
    fn text(&self) -> String {
        self.body_text()
    }
}

/// Extract correlation ID (`traceparent` or fallback to `x-request-id`).
pub fn correlation_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("traceparent")
        .or_else(|| headers.get("x-request-id"))
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Dto {
        a: u32,
    }

    #[tokio::test]
    async fn json_unknown_field_is_a_400_problem() {
        let req = HttpRequest::builder()
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"a":1,"surprise":true}"#))
            .expect("build request");
        let err = Json::<Dto>::from_request(req, &())
            .await
            .err()
            .expect("an unknown field is rejected");
        assert_eq!(err.to_problem_details().status, 400);
    }

    #[tokio::test]
    async fn json_valid_body_parses() {
        let req = HttpRequest::builder()
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"a":7}"#))
            .expect("build request");
        let Json(dto) = Json::<Dto>::from_request(req, &())
            .await
            .expect("a well-formed body parses");
        assert_eq!(dto.a, 7);
    }
}
