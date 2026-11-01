//! Telemetry's `/v1` lanes, behind a project API key holding the `ingest`
//! scope: the files an SDK lifts out of a span's messages before it exports
//! the span, each kept once by the hash of its bytes. The binary merges this
//! module's [`router`] into the one it nests at `/v1`, whose fallback answers
//! every path no lane here serves.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware;
use axum::response::Response;
use axum::routing::put;

use telmoni_shared::TelmoniError;
use telmoni_shared::acting::ActingKey;
use telmoni_shared::digest::sha256_hex;
use telmoni_shared::middleware::api_key::{ApiKeyGate, refuse_as_problem, require_api_key};
use telmoni_shared::middleware::problem;
use telmoni_shared::openapi::{Answer, Method, Param, Route};
use telmoni_shared::types::ApiKeyScope;

use crate::AppState;
use crate::content::{self, MEDIA_MAX};

/// One `/v1` lane this module serves: what it is, and which handler answers.
pub struct V1Lane {
    /// Which handler answers.
    pub op: V1Op,
    /// What a customer is told about it.
    pub route: Route,
}

/// Every `/v1` handler this module has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V1Op {
    /// [`put_media`]
    Media,
}

/// Every lane this module serves under `/v1`.
pub const V1_LANES: &[V1Lane] = &[V1Lane {
    op: V1Op::Media,
    route: Route {
        method: Method::Put,
        bearer: true,
        path: "/v1/media/{sha256}",
        summary: "Keep a file a message carried",
        description: "An image, audio or a file a span's message carried, sent \
                      once by the SHA-256 of its bytes, which its message then \
                      names as `telmoni:` and the hash. Kept, sealed, for a \
                      project that keeps content, and nothing for one that \
                      does not; sent again, it changes nothing.",
        params: &[Param {
            name: "sha256",
            in_query: false,
            required: true,
            description: "The SHA-256 of the body, in lowercase hex.",
        }],
        answers: &[
            Answer {
                status: 204,
                description: "Kept, kept already, or nothing kept for a project that keeps no content.",
            },
            Answer {
                status: 400,
                description: "A hash that is not the body's.",
            },
            Answer {
                status: 413,
                description: "A body past 5 MiB.",
            },
        ],
    },
}];

/// This module's `/v1` lanes, the `/v1` taken off, behind the ingest key's
/// gate. A method none takes is a problem document outside the gate, as
/// auth's lanes answer one; a path none serves is the merged router's.
pub fn router(state: Arc<AppState>) -> Router {
    let gate = ApiKeyGate {
        auth: state.auth.clone(),
        scope: ApiKeyScope::Ingest,
        refuse: refuse_as_problem,
    };
    V1_LANES
        .iter()
        .fold(Router::new(), |router, lane| {
            let path = lane
                .route
                .path
                .strip_prefix("/v1")
                .unwrap_or(lane.route.path);
            router.route(
                path,
                match lane.op {
                    V1Op::Media => put(put_media),
                },
            )
        })
        .layer(DefaultBodyLimit::max(MEDIA_MAX))
        .route_layer(middleware::from_fn_with_state(gate, require_api_key))
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
}

/// A method the upload does not take: `405` with `Allow`.
async fn method_not_allowed() -> Response {
    problem::method_not_allowed("PUT")
}

/// `PUT /v1/media/{sha256}` — keep a file for the key's project, by the hash
/// of its bytes, which must be the path's.
pub async fn put_media(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
    Path(sha256): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, TelmoniError> {
    // axum refuses a body past the limit in plain text; every `/v1` refusal is a problem.
    let body = body.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => TelmoniError::PayloadTooLarge {
            max_bytes: u64::try_from(MEDIA_MAX).unwrap_or(u64::MAX),
        },
        _ => TelmoniError::BadRequest("the body could not be read".into()),
    })?;
    if sha256_hex(&body) != sha256 {
        return Err(TelmoniError::BadRequest(
            "the path names a file by the SHA-256 of its body, in lowercase hex".into(),
        ));
    }
    let mime_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|v| is_mime_type(v))
        .unwrap_or("application/octet-stream");
    content::keep_media(&state, &key.project_id, (&sha256, mime_type), &body).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Whether `value` is a media type as a file is drawn by: `type/subtype`, a
/// short run of the characters a token holds, parameters left off.
fn is_mime_type(value: &str) -> bool {
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 63
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b))
    };
    value
        .split_once('/')
        .is_some_and(|(kind, subtype)| token(kind) && token(subtype))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lane's words name what the code keeps: the prefix a message names
    /// a file by, and the cap a 413 refuses past.
    #[test]
    fn the_lanes_words_name_its_prefix_and_its_cap() {
        let route = &V1_LANES[0].route;
        let prefix = format!("`{}`", content::FILE_ID_PREFIX);
        assert!(route.description.contains(&prefix), "{}", route.description);
        let past = route.answers.iter().find(|answer| answer.status == 413);
        let cap = format!("{} MiB", MEDIA_MAX / (1024 * 1024));
        assert!(past.is_some_and(|answer| answer.description.contains(&cap)));
    }

    #[test]
    fn a_media_type_is_type_and_subtype_alone() {
        for good in ["image/png", "audio/mpeg", "application/vnd.ms-excel"] {
            assert!(is_mime_type(good), "{good}");
        }
        for bad in [
            "image",
            "image/",
            "/png",
            "text/html; charset=utf-8",
            "image/p ng",
        ] {
            assert!(!is_mime_type(bad), "{bad}");
        }
    }
}
