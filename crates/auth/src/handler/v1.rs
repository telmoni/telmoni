//! `/v1` — the public read API, authenticated by a `telmoni_` API token.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::extract::Json;
use telmoni_shared::openapi::{Answer, Method, Route};

use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::types::API_TOKEN_PREFIX;
use telmoni_shared::{AuthError, OrganizationId, ProjectId, TelmoniError};

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
                          The slug is what the console's URLs name it by: set \
                          from the name it was created under, when that reads \
                          as one, and moved only when the URL is changed in \
                          Settings; the id never moves. A \
                          token IS an organization, so this answers for the \
                          caller and for nobody else.",
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
            summary: "Who is on the project",
            description: "Everyone on the project the token was minted on, the \
                          owner first, with the role each holds. The same body \
                          the console's project roster shows, so a token reads \
                          no more than the project admin who minted it could; \
                          the organization's roster is the console's alone.",
            params: &[],
            answers: &[Answer {
                status: 200,
                description: "The members: the owner, then the project's seats.",
            }],
        },
    },
];

/// The organization a validated `telmoni_` token resolved to. A newtype, so an
/// extension lookup cannot confuse it with an id from a path or a body.
#[derive(Debug, Clone)]
pub struct TokenOrganization {
    pub organization_id: OrganizationId,
    /// The project the token was minted on, which bounds what it reads: a
    /// project admin may mint one and sees no further than their project.
    pub project_id: ProjectId,
    /// The row the bearer hashed to, so a lane can act on the credential
    /// without ever seeing it.
    pub token_id: Uuid,
    /// What the validation's join already carries about the organization —
    /// its slug, its name and its owner — so `/v1/organization` reads nothing
    /// more.
    pub slug: String,
    pub name: String,
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

    let mut tx = match maintenance_scope(&state.db, AuthLane).await {
        Ok(tx) => tx,
        Err(e) => return unavailable(&e, "v1: token check could not open a transaction"),
    };
    let validated = tokens::validate(&mut tx, &token_hash).await;

    match validated {
        Ok(Some(v)) => {
            let flags =
                crate::db::flags::resolve_for_organization(&mut tx, &v.organization_id).await;
            let flags = match flags {
                Ok(f) => f,
                Err(e) => return unavailable(&e, "v1: flag resolution failed"),
            };
            if let Err(e) = tx.commit().await {
                return unavailable(&e, "v1: token validation commit failed");
            }
            if !flags.is_on(telmoni_shared::Flag::PublicApi) {
                return TelmoniError::from(telmoni_shared::TenantError::FeatureOff {
                    flag: telmoni_shared::Flag::PublicApi,
                })
                .into_response();
            }
            request.extensions_mut().insert(TokenOrganization {
                organization_id: v.organization_id,
                project_id: v.project_id,
                token_id: v.id,
                slug: v.slug,
                name: v.name,
                owner_email: v.owner_email,
                owner_display_name: v.owner_display_name,
            });
            next.run(request).await
        }
        Ok(None) => unauthorized(),
        Err(e) => unavailable(&e, "v1: token validation failed"),
    }
}

/// The token check could not reach the database: a `503` problem document,
/// the same `identity-unavailable` a sign-in gets when the identity provider
/// cannot be read, and a log line, since this is an outage and not a caller's
/// doing.
fn unavailable(error: &dyn std::fmt::Display, what: &str) -> Response {
    tracing::error!(error = %error, "{what}");
    TelmoniError::from(AuthError::IdentityUnavailable).into_response()
}

/// A path under `/v1` no lane serves: a `404` problem document, like every
/// other answer here, in place of axum's bare one.
pub async fn not_found() -> TelmoniError {
    AuthError::NotFound("no such lane under /v1".into()).into()
}

/// `401` with a `WWW-Authenticate` challenge: a problem document, as every
/// answer under `/v1` is, built by hand because an `AuthError` can carry
/// neither the challenge header nor this detail. The type is the one a bad
/// credential gets everywhere else.
fn unauthorized() -> Response {
    let mut problem = AuthError::InvalidToken.to_problem_details();
    problem.detail = Some("a live telmoni_ API key is required".into());
    let mut resp = (StatusCode::UNAUTHORIZED, Json(problem)).into_response();
    resp.headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    resp
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

/// `GET /v1/members` — the roster of the PROJECT the token was minted on,
/// with each role. ⚠ Never the organization's: a project admin may mint a
/// key, and the console refuses that seat the organization's roster, so a key
/// that listed it would read past its minter. Read under the owning
/// organization as the console's roster page is — the owner's row and every
/// address are the organization's to show, and a project-only scope would
/// join them against nothing.
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    token_organization: TokenOrganization,
) -> Result<impl IntoResponse, TelmoniError> {
    let TokenOrganization {
        organization_id,
        project_id,
        ..
    } = token_organization;
    let tx = project_scope(&state.db, &project_id).await?;
    let mut tx = tx.bind_organization(&organization_id).await?;
    let rows = crate::db::members::list_for_project(&mut tx, &project_id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "members": rows })))
}
