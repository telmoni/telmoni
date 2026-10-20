//! [`require_api_key`] — the gate in front of a lane a machine calls with an
//! API key rather than a person with a session.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::error::TelmoniError;
use crate::seam::Auth;
use crate::types::ApiKeyScope;

/// A lane's refusal on its way: a lane may read the request's body first.
pub type Refusal = Pin<Box<dyn Future<Output = Response> + Send>>;

/// What the gate is handed: who verifies a key, the scope its lane needs,
/// and how that lane refuses. The refusal is the lane's own because the
/// lanes answer in different shapes: ingest in OTLP's `Status`, which an
/// exporter reads, where every other lane answers a problem document.
#[derive(Clone)]
pub struct ApiKeyGate {
    /// Auth, in process: only it can verify a key.
    pub auth: Arc<dyn Auth>,
    /// The scope a key needs to pass.
    pub scope: ApiKeyScope,
    /// The lane's refusal, given the request: its headers name the encoding
    /// to answer in, and ingest reads its body out first.
    pub refuse: fn(TelmoniError, Request) -> Refusal,
}

impl std::fmt::Debug for ApiKeyGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyGate")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// Refuse a request whose bearer is not a live API key holding the gate's
/// scope; let one through with its [`ActingKey`](crate::acting::ActingKey)
/// on the request, which is all a lane behind the gate may act on. Before
/// the body is read, so a lane's work starts only for a key that may do it.
pub async fn require_api_key(
    State(gate): State<ApiKeyGate>,
    mut request: Request,
    next: Next,
) -> Response {
    let key = match gate.auth.resolve_api_key(request.headers()).await {
        Ok(key) => key,
        Err(e) => return (gate.refuse)(e, request).await,
    };
    if let Err(e) = key.require_scope(gate.scope) {
        return (gate.refuse)(e, request).await;
    }
    request.extensions_mut().insert(key);
    next.run(request).await
}
