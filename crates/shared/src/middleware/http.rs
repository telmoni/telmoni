//! The HTTP layers every service mounts: a request id on every request and a
//! span that carries it, so each JSON log line names its request and the BFF's
//! `x-request-id` follows a call across services.

use axum::body::Body;
use axum::http::Request;
use tower::ServiceBuilder;
use tower::layer::util::{Identity, Stack};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

/// The header the BFF mints and every service echoes.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Mount OUTSIDE [`trace_layer`] (add it to the router after), so the id
/// exists before the span reads it. An inbound id is kept, a missing one is
/// minted, and either way it is echoed on the response.
pub fn request_id_layers() -> ServiceBuilder<
    Stack<PropagateRequestIdLayer, Stack<SetRequestIdLayer<MakeRequestUuid>, Identity>>,
> {
    ServiceBuilder::new()
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
}

type MakeRequestSpan = fn(&Request<Body>) -> tracing::Span;
/// The trace layer with this crate's span, named so a service can hold one.
pub type HttpTraceLayer = TraceLayer<SharedClassifier<ServerErrorsAsFailures>, MakeRequestSpan>;

/// The request span: method, path and `request_id`, printed on every log line
/// inside it.
pub fn trace_layer() -> HttpTraceLayer {
    TraceLayer::new_for_http().make_span_with(make_span as MakeRequestSpan)
}

fn make_span(request: &Request<Body>) -> tracing::Span {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    tracing::info_span!(
        "request",
        method = %request.method(),
        path = %request.uri().path(),
        request_id = %request_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(trace_layer())
            .layer(request_id_layers())
    }

    #[tokio::test]
    async fn a_request_without_an_id_gets_one_on_the_response() {
        let res = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let id = res.headers().get(REQUEST_ID_HEADER).expect("minted");
        assert!(
            uuid::Uuid::parse_str(id.to_str().unwrap()).is_ok(),
            "a UUID is minted"
        );
    }

    #[tokio::test]
    async fn an_inbound_id_is_kept_and_echoed() {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(REQUEST_ID_HEADER, "req-from-the-bff")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.headers().get(REQUEST_ID_HEADER).unwrap(),
            "req-from-the-bff"
        );
    }
}
