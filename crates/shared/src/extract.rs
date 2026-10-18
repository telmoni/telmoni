//! Extractors for HTTP handlers — typed request parts that fail with the
//! crate's standard errors rather than axum's defaults, so a malformed payload
//! is a named 400 instead of a 500 or a leaked internal type name, and its
//! detail a fixed sentence rather than serde's words, which quote the value
//! the caller sent.

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
use crate::middleware::http::{REQUEST_ID_HEADER, is_request_id};

/// Like [`axum::Json`], but a deserialize rejection becomes a
/// [`TelmoniError::BadRequest`] naming the shape expected.
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
    TelmoniError::BadRequest(rejection.text().into())
}

/// What a rejection says to the caller: a sentence fixed per kind, as the
/// lanes' own refusals name a parameter and never its value.
trait RejectionText {
    fn text(&self) -> &'static str;
}

impl RejectionText for JsonRejection {
    fn text(&self) -> &'static str {
        match self {
            Self::JsonDataError(_) => {
                "the JSON body does not have the fields this lane takes, or a field holds a value of the wrong kind"
            }
            Self::JsonSyntaxError(_) => "the body is not valid JSON",
            Self::MissingJsonContentType(_) => "the body must be sent as application/json",
            Self::BytesRejection(_) => {
                "the body could not be read, or is larger than this lane takes"
            }
            _ => "the body was refused",
        }
    }
}

impl RejectionText for QueryRejection {
    fn text(&self) -> &'static str {
        match self {
            Self::FailedToDeserializeQueryString(_) => {
                "the query string does not have the parameters this lane takes, or one holds a value of the wrong kind"
            }
            _ => "the query string was refused",
        }
    }
}

/// The request's id, for an audit row: its `x-request-id`, only well formed
/// ([`is_request_id`]), since a caller's free text must never reach the
/// chain. Behind
/// [`fresh_request_id`](crate::middleware::http::fresh_request_id) it is the
/// id minted for the request, the one its span prints on every log line.
pub fn correlation_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| is_request_id(value))
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

    /// A refusal names the shape expected and never the caller's text: an
    /// unknown field's name, or a value that is no variant, stays out.
    #[tokio::test]
    async fn json_unknown_field_is_a_400_problem_that_echoes_nothing() {
        for body in [r#"{"a":1,"surprise":true}"#, r#"{"a":"surprise<script>"}"#] {
            let req = HttpRequest::builder()
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("build request");
            let err = Json::<Dto>::from_request(req, &())
                .await
                .err()
                .expect("the body is rejected");
            let problem = err.to_problem_details();
            assert_eq!(problem.status, 400);
            assert_eq!(
                problem.detail.as_deref(),
                Some(
                    "the JSON body does not have the fields this lane takes, or a field holds a value of the wrong kind"
                ),
                "{body}"
            );
        }
        let req = HttpRequest::builder()
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"a":"#))
            .expect("build request");
        let err = Json::<Dto>::from_request(req, &())
            .await
            .err()
            .expect("a body that is no JSON is rejected");
        assert_eq!(
            err.to_problem_details().detail.as_deref(),
            Some("the body is not valid JSON")
        );
    }

    #[tokio::test]
    async fn query_unknown_parameter_is_a_400_problem_that_echoes_nothing() {
        let (mut parts, _) = HttpRequest::builder()
            .uri("/?a=1&surprise=2")
            .body(Body::empty())
            .expect("build request")
            .into_parts();
        let err = Query::<Dto>::from_request_parts(&mut parts, &())
            .await
            .err()
            .expect("an unknown parameter is rejected");
        let problem = err.to_problem_details();
        assert_eq!(problem.status, 400);
        assert_eq!(
            problem.detail.as_deref(),
            Some(
                "the query string does not have the parameters this lane takes, or one holds a value of the wrong kind"
            )
        );
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

    fn headers(sent: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in sent {
            headers.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                value.parse().expect("a header value"),
            );
        }
        headers
    }

    /// An audit row keeps the request's id well formed or none: the
    /// `x-request-id` its span prints, a trace context never, and never a
    /// caller's free text.
    #[test]
    fn a_correlation_id_is_kept_only_well_formed() {
        let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        for (sent, kept) in [
            (vec![(REQUEST_ID_HEADER, "req-1")], Some("req-1")),
            (
                vec![("traceparent", traceparent), (REQUEST_ID_HEADER, "req-1")],
                Some("req-1"),
            ),
            (vec![("traceparent", traceparent)], None),
            (
                vec![(REQUEST_ID_HEADER, "it's free text, of any length")],
                None,
            ),
            (vec![(REQUEST_ID_HEADER, "ada@example.com")], None),
            (vec![(REQUEST_ID_HEADER, "")], None),
            (vec![], None),
        ] {
            assert_eq!(correlation_id(&headers(&sent)), kept, "{sent:?}");
        }
        let long = "a".repeat(129);
        assert_eq!(
            correlation_id(&headers(&[(REQUEST_ID_HEADER, &long)])),
            None
        );
    }
}
