//! End-to-end proof of the `SERVICE_SECRET_NEXT` rotation window.
#![expect(clippy::unwrap_used, reason = "test scaffolding")]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Router, middleware};
use tower::ServiceExt;

use telmoni_shared::middleware::service_auth::{
    SERVICE_SECRET_HEADER, ServiceSecrets, require_service_secret,
};

const PRIMARY: &str = "old-service-secret-0000000000000000000000000000";
const NEXT: &str = "new-service-secret-1111111111111111111111111111";

/// A minimal `/internal/*` router guarded by the real middleware, as every service wires it.
fn app(secrets: ServiceSecrets) -> Router {
    Router::new()
        .route("/internal/ping", get(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(
            secrets,
            require_service_secret,
        ))
}

/// Send one request carrying `header` (or none) and return the status.
async fn probe(secrets: ServiceSecrets, header: Option<&str>) -> StatusCode {
    let mut builder = Request::builder().uri("/internal/ping");
    if let Some(value) = header {
        builder = builder.header(SERVICE_SECRET_HEADER, value);
    }
    let request = builder.body(Body::empty()).unwrap();
    app(secrets).oneshot(request).await.unwrap().status()
}

/// Before rotation: only the single primary secret is valid.
#[tokio::test]
async fn single_secret_accepts_primary_rejects_others() {
    let secrets = ServiceSecrets::new(PRIMARY, None::<String>);
    assert_eq!(probe(secrets.clone(), Some(PRIMARY)).await, StatusCode::OK);
    assert_eq!(
        probe(secrets.clone(), Some(NEXT)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(probe(secrets, None).await, StatusCode::UNAUTHORIZED);
}

/// Mid-rotation both secrets are accepted, so the fleet can roll without downtime.
#[tokio::test]
async fn rotation_window_accepts_both_secrets() {
    let secrets = ServiceSecrets::new(PRIMARY, Some(NEXT.to_string()));
    assert_eq!(probe(secrets.clone(), Some(PRIMARY)).await, StatusCode::OK);
    assert_eq!(probe(secrets.clone(), Some(NEXT)).await, StatusCode::OK);
    assert_eq!(
        probe(secrets, Some("unrelated-secret")).await,
        StatusCode::UNAUTHORIZED,
    );
}

/// After promotion (`NEXT` is the only primary) the old secret is rejected.
#[tokio::test]
async fn after_promotion_old_secret_is_rejected() {
    let secrets = ServiceSecrets::new(NEXT, None::<String>);
    assert_eq!(probe(secrets.clone(), Some(NEXT)).await, StatusCode::OK);
    assert_eq!(
        probe(secrets, Some(PRIMARY)).await,
        StatusCode::UNAUTHORIZED
    );
}
