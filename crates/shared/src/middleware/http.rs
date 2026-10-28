//! The HTTP layers the binary mounts: an id minted for every request and a
//! span that carries it, so each JSON log line names its request, and an
//! audit row written in it records the same id. No caller names a request:
//! on a machine lane the header is the caller's free text, and the console
//! keeps none of the ids it sends.

use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use tower::ServiceBuilder;
use tower::layer::util::{Identity, Stack};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::types::API_TOKEN_PREFIX;

/// The header that names a request: minted for each, and echoed on its
/// response.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// The longest request id a log line or an audit row keeps: the one minted
/// here, a UUID, is 36 characters.
const REQUEST_ID_MAX: usize = 128;

/// Whether `value` is a request id a log line and an audit row may keep:
/// 1 to 128 letters, digits, dots, underscores and hyphens.
#[must_use]
pub fn is_request_id(value: &str) -> bool {
    (1..=REQUEST_ID_MAX).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Mount OUTSIDE [`request_id_layers`] (add it to the router after them):
/// an `x-request-id` a request arrives with is taken off, whoever sent it,
/// for [`request_id_layers`] to mint the request's own. On a machine lane it
/// is the caller's word, free text of any length, which the span would print
/// on every log line and [`correlation_id`](crate::extract::correlation_id)
/// hand the append-only audit chain; and nothing here asks who the caller
/// is, so the answer gives nothing away about anything it presented.
pub async fn fresh_request_id(mut request: Request<Body>, next: Next) -> Response {
    request.headers_mut().remove(REQUEST_ID_HEADER);
    next.run(request).await
}

tokio::task_local! {
    static REQUEST_ID: String;
}

/// Mount INSIDE [`request_id_layers`], beside [`trace_layer`]: the request's
/// id held for everything its handler runs, so an audit row written while it
/// is answered records it ([`request_id`]).
pub async fn request_scope(request: Request<Body>, next: Next) -> Response {
    let id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|id| is_request_id(id))
        .map(str::to_owned);
    match id {
        Some(id) => REQUEST_ID.scope(id, next.run(request)).await,
        None => next.run(request).await,
    }
}

/// The id of the request this task is answering; none outside one.
#[must_use]
pub fn request_id() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}

/// Mount OUTSIDE [`trace_layer`] (add it to the router after), so the id
/// exists before the span reads it, and [`fresh_request_id`] outside both.
/// An id a request still carries is kept, a missing one is minted, and
/// either way it is echoed on the response.
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
        .filter(|id| is_request_id(id))
        .unwrap_or("-");
    tracing::info_span!(
        "request",
        method = %request.method(),
        path = %loggable_path(request.uri().path()),
        request_id = %request_id,
    )
}

/// The path as the span prints it, an API key pasted into a URL cut out of
/// it: any segment carrying the key's prefix ([`API_TOKEN_PREFIX`]), since
/// the likeliest leak is a path the router never matches.
fn loggable_path(path: &str) -> std::borrow::Cow<'_, str> {
    if !path.split('/').any(shaped_as_a_credential) {
        return path.into();
    }
    path.split('/')
        .map(|segment| {
            if shaped_as_a_credential(segment) {
                "…"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
        .into()
}

/// Whether a path segment carries a key's prefix anywhere in it: what a
/// shell wraps a key in rides with it.
fn shaped_as_a_credential(segment: &str) -> bool {
    segment.contains(API_TOKEN_PREFIX)
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

    /// The id layers alone keep an id they are handed, which is why the
    /// binary mounts [`fresh_request_id`] outside them.
    #[tokio::test]
    async fn the_id_layers_alone_keep_an_id_they_are_handed() {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(REQUEST_ID_HEADER, "req-handed-in")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.headers().get(REQUEST_ID_HEADER).unwrap(),
            "req-handed-in"
        );
    }

    /// The layers as the binary mounts them, over a handler that answers the
    /// id an audit row would record.
    fn mounted() -> Router {
        Router::new()
            .route(
                "/",
                get(|headers: axum::http::HeaderMap| async move {
                    crate::extract::correlation_id(&headers)
                        .unwrap_or("-")
                        .to_owned()
                }),
            )
            .layer(trace_layer())
            .layer(request_id_layers())
            .layer(axum::middleware::from_fn(fresh_request_id))
    }

    /// The id the response echoes, and the one an audit row would record.
    async fn ids_of(sent: &[(&str, &str)]) -> (String, String) {
        let mut request = Request::builder().uri("/");
        for (name, value) in sent {
            request = request.header(*name, *value);
        }
        let response = mounted()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let echoed = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .expect("an id echoed")
            .to_str()
            .unwrap()
            .to_owned();
        let recorded = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        (echoed, String::from_utf8(recorded.to_vec()).unwrap())
    }

    fn minted(id: &str) -> bool {
        uuid::Uuid::parse_str(id).is_ok()
    }

    /// ⚠ No caller names its request: an id, well formed or not, sent alone,
    /// beside a trace context, or beside a service secret — which nothing
    /// here compares, so no answer tells a guessed secret from a wrong one —
    /// gives way to an id minted here, the one the response echoes and an
    /// audit row records.
    #[tokio::test]
    async fn no_caller_names_its_request() {
        let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        for sent in [
            vec![(REQUEST_ID_HEADER, "req-1")],
            vec![(REQUEST_ID_HEADER, "it's free text, of any length")],
            vec![("traceparent", traceparent)],
            vec![("traceparent", traceparent), (REQUEST_ID_HEADER, "req-1")],
            vec![
                ("x-service-secret", "a-guess"),
                (REQUEST_ID_HEADER, "req-1"),
            ],
        ] {
            let (echoed, recorded) = ids_of(&sent).await;
            assert!(minted(&echoed), "{sent:?}: {echoed}");
            assert_eq!(recorded, echoed, "{sent:?}");
        }
    }

    #[test]
    fn an_id_is_well_formed_only_bounded_and_plain() {
        let longest = "a".repeat(128);
        for kept in [
            "0b6f3a3e-6a2c-4d0e-9f53-0f6b2a1b9c11",
            "req_1.a-B",
            &longest,
        ] {
            assert!(is_request_id(kept), "{kept:?}");
        }
        let too_long = "a".repeat(129);
        for refused in [
            "",
            &too_long,
            "free text",
            "tab\there",
            "line\nbreak",
            "é",
            "a/b",
            "a;b",
            "<b>",
            "ada@example.com",
            "a=b",
            "a:b",
            "a+b",
        ] {
            assert!(!is_request_id(refused), "{refused:?}");
        }
    }

    /// ⚠ The span prints a request id only well formed, behind
    /// [`fresh_request_id`] or not: a router that mounts the span and the id
    /// layers alone keeps a caller's id, and still no log line prints its
    /// free text.
    #[tokio::test]
    async fn the_span_prints_no_free_text_for_a_request_id() {
        let (_logging, logs) = crate::test_util::captured_logs();
        let answered = Router::new()
            .route(
                "/",
                get(|| async {
                    tracing::info!("handled");
                    "ok"
                }),
            )
            .layer(trace_layer())
            .layer(request_id_layers())
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(REQUEST_ID_HEADER, "it's free text")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(answered.status(), axum::http::StatusCode::OK);
        let lines = logs.text();
        assert!(lines.contains(r#""request_id":"-""#), "{lines}");
        assert!(!lines.contains("free text"), "{lines}");
    }

    // What an audit row written while the request is answered records is the
    // id the response echoes; outside a request there is none.
    #[tokio::test]
    async fn a_handler_holds_the_id_the_response_echoes() {
        let response = Router::new()
            .route(
                "/",
                get(|| async { request_id().unwrap_or_else(|| "-".into()) }),
            )
            .layer(axum::middleware::from_fn(request_scope))
            .layer(trace_layer())
            .layer(request_id_layers())
            .layer(axum::middleware::from_fn(fresh_request_id))
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let echoed = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), echoed);
        assert_eq!(request_id(), None);
    }

    /// ⚠ The span cuts any segment shaped as an API key, wherever it sits.
    #[test]
    fn an_api_key_is_cut_out_of_the_logged_path() {
        for (path, logged) in [
            ("/v1/tm_4p1k3y", "/v1/…"),
            ("/v1/'tm_4p1k3y'/members", "/v1/…/members"),
            ("//v1/tm_4p1k3y/tm_4p1k3y", "//v1/…/…"),
            ("/v1/organization", "/v1/organization"),
            ("/v1/", "/v1/"),
        ] {
            assert_eq!(loggable_path(path), logged, "{path}");
        }
    }
}
