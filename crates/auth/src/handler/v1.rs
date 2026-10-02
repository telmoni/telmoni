//! `/v1` — the public read API, authenticated by a `telmoni_` API token.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::extract::Json;
use telmoni_shared::openapi::{Answer, Method, Route};

use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope};
use telmoni_shared::types::API_TOKEN_PREFIX;
use telmoni_shared::{AuthError, OrganizationId, TelmoniError};

use crate::AppState;
use crate::db::{AuthLane, tokens};

/// One `/v1` lane this service serves: what it is, and which handler answers.
pub struct V1Lane {
    /// Which handler answers.
    pub op: V1Op,
    /// What a customer is told about it.
    pub route: Route,
}

/// Every `/v1` handler this service has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V1Op {
    /// [`get_organization`]
    Organization,
    /// [`list_members`]
    Members,
}

/// Every lane this service serves under `/v1`.
pub const V1_LANES: &[V1Lane] = &[
    V1Lane {
        op: V1Op::Organization,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/organization",
            summary: "Who this token is",
            description: "The organization the token belongs to — its id, \
                          its slug, its name and its owner's address and name. \
                          The slug is what the console's URLs name it by, and \
                          it follows the name: a rename moves it, the id \
                          never moves. A token IS an organization, so this \
                          answers for the caller and for nobody else.",
            params: &[],
            answers: &[Answer {
                status: 200,
                description: "The organization.",
            }],
        },
    },
    V1Lane {
        op: V1Op::Members,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/members",
            summary: "Who is in the organization",
            description: "Everyone in the organization, the owner first, with \
                          the role each holds. The same body the console reads, \
                          so a token and a browser cannot be told two different \
                          member lists.",
            params: &[],
            answers: &[Answer {
                status: 200,
                description: "The members: the owner, then everyone else newest first.",
            }],
        },
    },
];

/// The organization a validated `telmoni_` token resolved to. A newtype, so an
/// extension lookup cannot confuse it with an id from a path or a body.
#[derive(Debug, Clone)]
pub struct TokenOrganization {
    pub organization_id: OrganizationId,
    /// The row the bearer hashed to, so a lane can act on the credential
    /// without ever seeing it.
    pub token_id: Uuid,
    /// What the validation's join already carries about the organization —
    /// its slug, its name and its owner — so `/v1/organization` reads nothing
    /// more.
    pub slug: String,
    pub name: Option<String>,
    pub owner_email: Option<String>,
    pub owner_display_name: Option<String>,
}

/// Reject anything without a live `telmoni_` bearer; on success, attach the
/// organization it names. No path or body under `/v1` names an organization,
/// which makes cross-tenant access impossible to express, not just checked.
pub async fn require_token(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let (scheme, token) = v.split_at_checked(7)?;
            scheme.eq_ignore_ascii_case("bearer ").then_some(token)
        })
        .map(str::trim)
        .filter(|t| !t.is_empty());

    let Some(token) = presented else {
        return unauthorized();
    };
    if !token.starts_with(API_TOKEN_PREFIX) {
        return unauthorized();
    }

    let token_hash = crate::handler::tokens::hash_token(token);

    let Ok(mut tx) = maintenance_scope(&state.db, AuthLane).await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "database unavailable" })),
        )
            .into_response();
    };
    let validated = tokens::validate(&mut tx, &token_hash).await;

    match validated {
        Ok(Some(v)) => {
            let flags =
                crate::db::flags::resolve_for_organization(&mut tx, &v.organization_id).await;
            let flags = match flags {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!(error = %e, "v1: flag resolution failed");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({ "error": "database unavailable" })),
                    )
                        .into_response();
                }
            };
            if let Err(e) = tx.commit().await {
                tracing::error!(error = %e, "v1: token validation commit failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "database unavailable" })),
                )
                    .into_response();
            }
            for flag in [
                telmoni_shared::Flag::BetaAccess,
                telmoni_shared::Flag::PublicApi,
            ] {
                if !flags.is_on(flag) {
                    return TelmoniError::from(telmoni_shared::TenantError::FeatureOff { flag })
                        .into_response();
                }
            }
            request.extensions_mut().insert(TokenOrganization {
                organization_id: v.organization_id,
                token_id: v.id,
                slug: v.slug,
                name: v.name,
                owner_email: v.owner_email,
                owner_display_name: v.owner_display_name,
            });
            next.run(request).await
        }
        Ok(None) => unauthorized(),
        Err(e) => {
            tracing::error!(error = %e, "v1: token validation failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "database unavailable" })),
            )
                .into_response()
        }
    }
}

/// `401` with a `WWW-Authenticate` challenge, outside the RFC 9457 path on
/// purpose: routing it through `AuthError` would `error!`-log every scanner.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        Json(json!({ "error": "a live telmoni_ API token is required" })),
    )
        .into_response()
}

/// Pull the organization the middleware resolved.
impl<S: Send + Sync> FromRequestParts<S> for TokenOrganization {
    type Rejection = TelmoniError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or_else(|| AuthError::Unauthenticated.into())
    }
}

/// `GET /v1/organization` — who the calling token is. Answered from what the
/// token's validation already joined: the row, its name and its owner. No
/// second transaction.
pub async fn get_organization(
    token_organization: TokenOrganization,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization_id = token_organization.organization_id.clone();
    let TokenOrganization {
        slug,
        name,
        owner_email,
        owner_display_name,
        ..
    } = token_organization;
    Ok(Json(json!({
        "organization_id": organization_id,
        "slug": slug,
        "name": name,
        "owner": owner_email.map(|email| json!({ "email": email, "display_name": owner_display_name })),
    })))
}

/// `GET /v1/members` — everyone in the calling token's ORGANIZATION, with
/// their roles.
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    token_organization: TokenOrganization,
) -> Result<impl IntoResponse, TelmoniError> {
    let organization_id = token_organization.organization_id.clone();
    let mut tx = organization_scope(&state.db, &organization_id).await?;
    let rows =
        crate::db::organization_members::list_for_organization(&mut tx, &organization_id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "members": rows })))
}
