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

use crate::types::{API_TOKEN_PREFIX, PING_KEY_PREFIX};

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
        path = %loggable_path(request.uri().path()),
        request_id = %request_id,
    )
}

/// The path as the span prints it: a ping's key rides in its URL,
/// `/ping/{ping_key}/{slug}`, so a credential is cut out of every log line
/// two ways, and a log holds none. By place: the first segment after the
/// lane's name is one, whatever was pasted there — an API key as likely as
/// a ping key — in whatever case a shell's join left the name and however
/// many slashes it doubled, since the likeliest leak is a path the router
/// never matches and the span prints all the same. By shape: a segment
/// carrying a ping key's prefix or an API key's ([`PING_KEY_PREFIX`],
/// [`API_TOKEN_PREFIX`]) wherever it sits, the lane's name misspelt or the
/// key in a stray quote. Every other path is printed as it came.
fn loggable_path(path: &str) -> std::borrow::Cow<'_, str> {
    let keys_place = keys_place(path);
    if keys_place.is_none() && !path.split('/').any(shaped_as_a_credential) {
        return path.into();
    }
    path.split('/')
        .enumerate()
        .map(|(at, segment)| {
            if Some(at) == keys_place || shaped_as_a_credential(segment) {
                "…"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
        .into()
}

/// Where the key sits in a path to the lane, counted in segments: the first
/// non-empty one after the lane's name; `None` on any other path, and on
/// the lane's own root.
fn keys_place(path: &str) -> Option<usize> {
    let mut named = path
        .split('/')
        .enumerate()
        .filter(|(_, segment)| !segment.is_empty());
    let (_, lane) = named.next()?;
    if !lane.eq_ignore_ascii_case("ping") {
        return None;
    }
    named.next().map(|(at, _)| at)
}

/// Whether a path segment carries a key's prefix anywhere in it: what a
/// shell wraps a key in rides with it.
fn shaped_as_a_credential(segment: &str) -> bool {
    segment.contains(PING_KEY_PREFIX) || segment.contains(API_TOKEN_PREFIX)
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

    /// ⚠ A credential never reaches a log line: the span prints the path with
    /// the first segment after the lane's name cut out, whatever it holds and
    /// however a shell's join spelled the name or doubled the slash, every
    /// segment shaped as a key wherever it sits, and every other path as it
    /// came.
    #[test]
    fn a_pings_key_is_cut_out_of_the_logged_path() {
        for (path, logged) in [
            (
                "/ping/telmoniping_k3yk3yk3yk3yk3yk3yk3yk3yk3yk3yk3y/nightly/start",
                "/ping/…/nightly/start",
            ),
            ("/ping/telmoniping_k3y/nightly", "/ping/…/nightly"),
            ("/ping/telmoniping_k3y/", "/ping/…/"),
            ("/ping/telmoniping_k3y", "/ping/…"),
            ("/ping/notakey/nightly", "/ping/…/nightly"),
            ("/ping/telmoni_4p1k3y/nightly", "/ping/…/nightly"),
            ("/ping//telmoniping_k3y/nightly", "/ping//…/nightly"),
            ("/ping//notakey/nightly", "/ping//…/nightly"),
            ("//ping/notakey/nightly", "//ping/…/nightly"),
            ("/PING/notakey/nightly", "/PING/…/nightly"),
            ("/pings/telmoniping_k3y/nightly", "/pings/…/nightly"),
            ("/pings/notakey/nightly", "/pings/notakey/nightly"),
            ("/ping/'telmoniping_k3y'/nightly", "/ping/…/nightly"),
            ("/nightly/telmoniping_k3y", "/nightly/…"),
            ("/v1/telmoni_4p1k3y", "/v1/…"),
            ("/ping/telmoniping_k3y/telmoniping_k3y", "/ping/…/…"),
            ("/ping/", "/ping/"),
            ("/ping", "/ping"),
            ("/v1/organization", "/v1/organization"),
        ] {
            assert_eq!(loggable_path(path), logged, "{path}");
        }
    }
}
