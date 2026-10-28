//! `/v1` — the public read API, opened by a `tm_` API key holding the
//! `read` scope. The binary nests this module's [`router`] under its one
//! `/v1`, on the machine host, behind the door's limits; the key gate every
//! machine lane shares (`require_api_key`) puts the key's [`ActingKey`] on
//! the request, and the handlers act on that and on nothing else. No path or
//! body under `/v1` names an organization, which makes cross-tenant access
//! impossible to express, not just checked.

use std::sync::Arc;

use axum::{
    Router,
    body::HttpBody,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method as HttpMethod, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_shared::acting::ActingKey;
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::extract::Json;
use telmoni_shared::middleware::api_key::{ApiKeyGate, Refusal, require_api_key};
use telmoni_shared::middleware::problem;
use telmoni_shared::openapi::{Answer, Method, Route};
use telmoni_shared::slug::Slug;
use telmoni_shared::types::{API_TOKEN_PREFIX, ApiKeyScope, CONNECTION_BYTES};
use telmoni_shared::{AuthError, Flag, OrganizationId, Role, TelmoniError, TenantError, UserId};

use crate::AppState;
use crate::db::{AuthLane, members, organizations};

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
            summary: "Who this key is",
            description: "The organization the key belongs to — its id, \
                          its slug, its name and its owner's address and name. \
                          The slug is what the console's URLs name it by: set \
                          from the name it was created under, when that reads \
                          as one, and moved only when the URL is changed in \
                          Settings; the id never moves. A key is minted on \
                          one project of one organization, so this answers \
                          for the caller's organization and for no other.",
            params: &[],
            answers: &[
                Answer {
                    status: 200,
                    description: "The organization.",
                },
                Answer {
                    status: 403,
                    description: "A live key without the `read` scope.",
                },
            ],
        },
    },
    V1Lane {
        op: V1Op::Members,
        route: Route {
            method: Method::Get,
            bearer: true,
            path: "/v1/members",
            summary: "Who is on the project",
            description: "Everyone on the project the key was minted on, the \
                          owner first, with the role each holds. The same list \
                          the console's project roster shows, so a key reads \
                          no more than the project admin who minted it could; \
                          the organization's roster is the console's alone.",
            params: &[],
            answers: &[
                Answer {
                    status: 200,
                    description: "The members: the owner, then the project's seats.",
                },
                Answer {
                    status: 403,
                    description: "A live key without the `read` scope.",
                },
            ],
        },
    },
];

/// What a lane answers to a method it does not take, and what `OPTIONS` is
/// told: every lane here is a read.
const ALLOW: &str = "GET, HEAD, OPTIONS";

/// This module's `/v1` subtree: every lane of [`V1_LANES`] under its path
/// with the `/v1` taken off, since the binary nests it there. Each lane a
/// key opens sits behind two gates: the key gate, which refuses as every
/// `/v1` answer refuses — a problem document, with the bearer challenge on a
/// 401 — and the organization's own switch. Built by folding the table the
/// OpenAPI document is generated from, split on `bearer`, so a future lane
/// without one does not silently inherit the gates. A path no lane serves,
/// and a method no lane takes, are problem documents too, outside the gates,
/// so they name no organization and cost no database round trip.
pub fn router(state: Arc<AppState>) -> Router {
    let mount = |router: Router<Arc<AppState>>, lane: &V1Lane| {
        let path = lane
            .route
            .path
            .strip_prefix("/v1")
            .unwrap_or(lane.route.path);
        router.route(
            path,
            match lane.op {
                V1Op::Organization => get(get_organization),
                V1Op::Members => get(list_members),
            },
        )
    };
    let gate = ApiKeyGate {
        auth: state.clone(),
        scope: ApiKeyScope::Read,
        refuse,
    };
    // Layers apply outermost-last: the key is checked first, the switch is
    // read for a key that passed, and the day's data out last.
    let keyed = V1_LANES
        .iter()
        .filter(|lane| lane.route.bearer)
        .fold(Router::new(), mount)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            meter_data_out,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_public_api,
        ))
        .route_layer(middleware::from_fn_with_state(gate, require_api_key));
    let open = V1_LANES
        .iter()
        .filter(|lane| !lane.route.bearer)
        .fold(Router::new(), mount);
    keyed
        .merge(open)
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(not_found)
        .with_state(state)
}

/// How every `/v1` lane refuses a key: a problem document, as every answer
/// here is, and on a `401` the `WWW-Authenticate` challenge and the detail a
/// customer can act on. The type is the one a bad credential gets everywhere
/// else; the request is read no further, since a `/v1` answer has one shape
/// and no `/v1` lane takes a body.
fn refuse(error: TelmoniError, _request: Request) -> Refusal {
    let mut problem = error.to_problem_details();
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status == StatusCode::UNAUTHORIZED {
        problem.detail = Some(format!("a live {API_TOKEN_PREFIX} API key is required"));
    }
    let mut response = (status, Json(problem)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    if status == StatusCode::UNAUTHORIZED {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    Box::pin(std::future::ready(response))
}

/// The organization's own switch, `Flag::PublicApi`: off, every lane is a
/// `503` that says when to come back, as a switched-off feature is answered
/// everywhere else — the route exists and the switch is Telmoni's, so a
/// program waits rather than concluding the route is gone.
async fn require_public_api(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(key) = request.extensions().get::<ActingKey>().cloned() else {
        return TelmoniError::Internal("v1: the request carries no key".into()).into_response();
    };
    let flags = async {
        let mut tx = maintenance_scope(&state.db, AuthLane).await?;
        let flags =
            crate::db::flags::resolve_for_organization(&mut tx, &key.organization_id).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(flags)
    }
    .await;
    match flags {
        Ok(flags) if flags.is_on(Flag::PublicApi) => next.run(request).await,
        Ok(_) => TelmoniError::from(TenantError::FeatureOff {
            flag: Flag::PublicApi,
        })
        .into_response(),
        Err(e) => unavailable(&e, "v1: flag resolution failed"),
    }
}

/// The organization's data out, held to the day its plan sets: refused, a
/// 429 until midnight UTC, once the day is spent, and each answer counted
/// against it with what its connection costs.
async fn meter_data_out(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(key) = request.extensions().get::<ActingKey>().cloned() else {
        return TelmoniError::Internal("v1: the request carries no key".into()).into_response();
    };
    let Some(telemetry) = state.siblings.telemetry.clone() else {
        return next.run(request).await;
    };
    if let Err(refused) = telemetry.admit_data_out(&key.organization_id).await {
        return refused.into_response();
    }
    // A HEAD's body is dropped past this layer, so only its connection costs.
    let head = request.method() == HttpMethod::HEAD;
    let response = next.run(request).await;
    // Every answer here is a whole JSON document, its length known.
    let sent = HttpBody::size_hint(response.body());
    let bytes = if head {
        0
    } else {
        sent.exact().unwrap_or_else(|| sent.lower())
    };
    telemetry
        .spend_data_out(&key.organization_id, bytes.saturating_add(CONNECTION_BYTES))
        .await;
    response
}

/// The `tm_` key a request's `authorization` presents as its bearer,
/// or `None` for no bearer, an empty one, or one that is not an API key.
pub(crate) fn presented_key(headers: &HeaderMap) -> Option<&str> {
    telmoni_shared::extract::header(headers, header::AUTHORIZATION.as_str())
        .and_then(telmoni_shared::extract::bearer)
        .filter(|token| token.starts_with(API_TOKEN_PREFIX))
}

/// A read could not reach the database: a `503` problem document, the same
/// `identity-unavailable` a sign-in gets when the identity provider cannot be
/// read, and a log line, since this is an outage and not a caller's doing.
fn unavailable(error: &dyn std::fmt::Display, what: &str) -> Response {
    tracing::error!(error = %error, "{what}");
    TelmoniError::from(AuthError::IdentityUnavailable).into_response()
}

/// A path under `/v1` no lane serves: a `404` problem document, like every
/// other answer here, in place of axum's bare one.
async fn not_found() -> Response {
    problem::not_found("no such lane under /v1")
}

/// A method no lane takes: `405` with `Allow`, as a problem document, but for
/// `OPTIONS`, which is told the methods and nothing else.
async fn method_not_allowed(request: Request) -> Response {
    if request.method() == HttpMethod::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static(ALLOW));
        return response;
    }
    problem::method_not_allowed(ALLOW)
}

/// `GET /v1/organization` — who the calling key is: the organization the key
/// gate resolved, read with its owner, whose row is another person's and so
/// the maintenance lane's. An organization gone between the key check and
/// this read is a key that opens nothing.
pub async fn get_organization(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
) -> Result<impl IntoResponse, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, AuthLane).await?;
    let summary = organizations::summary_with_owner(&mut tx, &key.organization_id).await?;
    tx.commit().await?;
    let Some(summary) = summary else {
        return Err(AuthError::InvalidToken.into());
    };
    Ok(Json(V1Organization {
        organization_id: key.organization_id,
        slug: summary.slug,
        name: summary.name,
        owner: summary.owner_email.map(|email| V1Owner {
            email,
            display_name: summary.owner_display_name,
        }),
    }))
}

/// `/v1/organization`'s answer, as `api/reference.mdx` publishes it.
#[derive(Debug, Serialize)]
pub struct V1Organization {
    pub organization_id: OrganizationId,
    pub slug: Slug,
    pub name: String,
    /// None once the owner's row is gone.
    pub owner: Option<V1Owner>,
}

/// The organization's owner, by address and name.
#[derive(Debug, Serialize)]
pub struct V1Owner {
    pub email: String,
    pub display_name: Option<String>,
}

/// `/v1/members`' answer.
#[derive(Debug, Serialize)]
pub struct V1Members {
    pub members: Vec<V1Member>,
}

/// One row of `/v1/members`, as `api/reference.mdx` publishes it: a copy of
/// the console's roster row, so a field the console gains is not published.
#[derive(Debug, Serialize)]
pub struct V1Member {
    pub id: Uuid,
    pub member_id: UserId,
    pub email: String,
    pub display_name: Option<String>,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub offer_expires_at: Option<DateTime<Utc>>,
}

impl From<members::Member> for V1Member {
    fn from(row: members::Member) -> Self {
        Self {
            id: row.id,
            member_id: row.member_id,
            email: row.email,
            display_name: row.display_name,
            role: row.role,
            created_at: row.created_at,
            offer_expires_at: row.offer_expires_at,
        }
    }
}

/// `GET /v1/members` — the roster of the PROJECT the key was minted on,
/// with each role. ⚠ Never the organization's: a project admin may mint a
/// key, and the console refuses that seat the organization's roster, so a key
/// that listed it would read past its minter. Read under the owning
/// organization as the console's roster page is — the owner's row and every
/// address are the organization's to show, and a project-only scope would
/// join them against nothing.
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    key: ActingKey,
) -> Result<impl IntoResponse, TelmoniError> {
    let tx = project_scope(&state.db, &key.project_id).await?;
    let mut tx = tx.bind_organization(&key.organization_id).await?;
    let rows = members::list_for_project(&mut tx, &key.project_id).await?;
    tx.commit().await?;
    Ok(Json(V1Members {
        members: rows.into_iter().map(V1Member::from).collect(),
    }))
}
