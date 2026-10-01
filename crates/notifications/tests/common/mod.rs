//! What both suites stand up around the module: auth as this module holds
//! it — a roster the suite seeds, answered in process — and the emit, the
//! purges and the redaction as auth calls them, mounted as the `/internal`
//! lanes, so the suites drive them with the
//! requests they always made.
#![allow(dead_code, reason = "each suite uses the part of the harness it needs")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use serde::Deserialize;
use serde_json::json;

use telmoni_notifications::{AppState, Config, handler};
use telmoni_shared::acting::{Acting, ActingProject};
use telmoni_shared::extract::Json;
use telmoni_shared::seam::{Auth, Notice};
use telmoni_shared::{
    AuthError, AuthzError, FlagSet, NotificationKind, OrganizationId, OrganizationRole,
    OrganizationStatus, ProjectId, Role, TelmoniError, UserId,
};

pub(crate) const SECRET: &str = "test-service-secret";

/// The flag set a suite scripts auth to answer.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Flags {
    /// `connectors` off: the queue pauses and no install starts.
    ConnectorsOff,
    /// The set cannot be read: an error, never all-on.
    Unreadable,
}

/// One seat: the person's organization role, and their project role when
/// they hold one.
type Seat = (OrganizationRole, Option<Role>);

/// Auth as this module holds it: the roster a suite seeds, and the flag set
/// it scripts. A bearer is `tok_<user>`, opaque to the module and turned
/// into a role here, as auth would.
pub(crate) struct AuthStub {
    /// Keyed on `(user, organization)`.
    roster: Mutex<HashMap<(String, String), Seat>>,
    flags: Mutex<Option<Flags>>,
    asked: AtomicUsize,
}

impl AuthStub {
    /// Nobody seated, every flag on.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            roster: Mutex::new(HashMap::new()),
            flags: Mutex::new(None),
            asked: AtomicUsize::new(0),
        })
    }

    /// Seat `user` on `organization` at `organization_role`, and on its
    /// project at `project_role` when they hold one.
    pub(crate) fn seat(
        &self,
        user: &str,
        organization: &str,
        organization_role: OrganizationRole,
        project_role: Option<Role>,
    ) -> &Self {
        self.roster
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                (user.to_owned(), organization.to_owned()),
                (organization_role, project_role),
            );
        self
    }

    /// Script the flag set.
    pub(crate) fn flags(&self, flags: Flags) -> &Self {
        *self
            .flags
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(flags);
        self
    }

    /// How many times a person lane asked who was calling.
    pub(crate) fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

#[async_trait]
impl Auth for AuthStub {
    async fn resolve(&self, headers: &HeaderMap) -> Result<Acting, TelmoniError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let user = header(headers, "authorization")
            .and_then(|bearer| bearer.strip_prefix("Bearer tok_"))
            .ok_or(AuthError::Unauthenticated)?;
        let organization = header(headers, "x-organization-id")
            .ok_or_else(|| AuthError::BadRequest("missing x-organization-id header".into()))?;
        let project = header(headers, "x-project-id");

        let seat = self
            .roster
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(user.to_owned(), organization.to_owned()))
            .copied();
        let Some((organization_role, project_role)) = seat else {
            return Err(
                AuthzError::Forbidden("you are not a member of this organization".into()).into(),
            );
        };
        let project = match project {
            None => None,
            Some(project) => {
                let Some(role) = project_role else {
                    return Err(AuthzError::Forbidden(
                        "you are not a member of this project".into(),
                    )
                    .into());
                };
                Some(ActingProject {
                    project_id: ProjectId::try_new(project)
                        .map_err(|e| AuthError::BadRequest(format!("invalid x-project-id: {e}")))?,
                    role,
                })
            }
        };
        Ok(Acting {
            user_id: UserId::try_new(user)
                .map_err(|e| AuthError::BadRequest(format!("invalid user id: {e}")))?,
            organization_id: OrganizationId::try_new(organization)
                .map_err(|e| AuthError::BadRequest(format!("invalid x-organization-id: {e}")))?,
            organization_role: Some(organization_role),
            project,
            session_id: None,
            expires_at: chrono::Utc::now().timestamp() + 3600,
        })
    }

    async fn global_flags(&self) -> Result<FlagSet, TelmoniError> {
        let scripted = *self
            .flags
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match scripted {
            None => Ok(FlagSet::all_on()),
            Some(Flags::ConnectorsOff) => {
                let mut set = FlagSet::all_on();
                set.set(telmoni_shared::Flag::Connectors, false);
                Ok(set)
            }
            Some(Flags::Unreadable) => Err(TelmoniError::Internal(
                "the flag set could not be read".into(),
            )),
        }
    }

    async fn organization_standing(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError> {
        Ok(Some(OrganizationStatus::Active))
    }
}

/// The module's configuration every suite starts from.
pub(crate) fn config() -> Config {
    Config {
        database_url: String::new(),
        service_secret: SECRET.into(),
        service_secret_next: None,
        app_url: "http://localhost:3000".into(),
        slack: None,
        slack_signing_secret: None,
        slack_api_base: "https://slack.com".into(),
        discord: None,
        discord_api_base: "https://discord.com".into(),
        connector_kek: None,
        kms_api_base: "https://cloudkms.googleapis.com".into(),
        metadata_api_base: "http://metadata.google.internal".into(),
        delivery_poll_secs: 5,
        delivery_batch: 200,
        delivery_concurrency: 64,
        delivery_drain_rounds: 20,
        delivery_max_attempts: 5,
        delivery_backoff_secs: 30,
    }
}

/// What auth calls in process, mounted as the lanes it called until the
/// modules were linked: the emit, the two purges and the redaction, at the
/// same paths with the same answers. Nothing gates them here.
pub(crate) fn sibling_lanes(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/internal/notifications/emit", post(emit_lane))
        .route(
            "/internal/notifications/organizations/{organization_id}/purge",
            post(purge_organization_lane),
        )
        .route(
            "/internal/notifications/projects/{project_id}/purge",
            post(purge_project_lane),
        )
        .route(
            "/internal/notifications/people/{user_id}/redact",
            post(redact_person_lane),
        )
        .with_state(state)
}

/// The body the emit lane took: what a producer sent over the wire.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmitBody {
    kind: NotificationKind,
    #[serde(default)]
    subject_user_id: Option<String>,
    title: String,
    body: String,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    dedup_key: Option<String>,
}

async fn emit_lane(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EmitBody>,
) -> Result<axum::response::Response, TelmoniError> {
    let organization_id = header(&headers, "x-organization-id")
        .ok_or_else(|| AuthError::BadRequest("missing x-organization-id header".into()))
        .and_then(|raw| {
            OrganizationId::try_new(raw)
                .map_err(|e| AuthError::BadRequest(format!("invalid x-organization-id: {e}")))
        })?;
    let project_id = match header(&headers, "x-project-id") {
        Some(raw) => Some(
            ProjectId::try_new(raw)
                .map_err(|e| AuthError::BadRequest(format!("invalid x-project-id: {e}")))?,
        ),
        None => None,
    };
    let subject_user_id = match body.subject_user_id.as_deref() {
        Some(raw) => Some(
            UserId::try_new(raw)
                .map_err(|e| AuthError::BadRequest(format!("invalid subject_user_id: {e}")))?,
        ),
        None => None,
    };
    let emitted = handler::emit_notice(
        &state,
        &organization_id,
        project_id.as_ref(),
        Notice {
            kind: body.kind,
            subject_user_id: subject_user_id.as_ref(),
            title: &body.title,
            body: &body.body,
            metadata: body.metadata.unwrap_or_else(|| json!({})),
            dedup_key: body.dedup_key.as_deref(),
        },
    )
    .await?;
    if emitted.deduplicated {
        return Ok((
            StatusCode::OK,
            axum::Json(json!({ "feed_id": emitted.feed_id, "deduplicated": true })),
        )
            .into_response());
    }
    Ok((
        StatusCode::ACCEPTED,
        axum::Json(json!({ "feed_id": emitted.feed_id })),
    )
        .into_response())
}

async fn purge_organization_lane(
    State(state): State<Arc<AppState>>,
    Path(organization_id): Path<String>,
) -> Result<axum::Json<serde_json::Value>, TelmoniError> {
    let organization_id = OrganizationId::try_new(organization_id)
        .map_err(|e| AuthError::BadRequest(format!("invalid organization id: {e}")))?;
    let purged = handler::purge_organization(&state, &organization_id).await?;
    Ok(axum::Json(json!({ "purged": purged })))
}

async fn purge_project_lane(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
) -> Result<axum::Json<serde_json::Value>, TelmoniError> {
    let project_id = ProjectId::try_new(project_id.as_str())
        .map_err(|e| AuthError::BadRequest(format!("invalid project id: {e}")))?;
    let purged = handler::purge_project(&state, &project_id).await?;
    Ok(axum::Json(json!({ "purged": purged })))
}

async fn redact_person_lane(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
) -> Result<axum::Json<serde_json::Value>, TelmoniError> {
    let user_id = UserId::try_new(user_id)
        .map_err(|e| AuthError::BadRequest(format!("invalid user id: {e}")))?;
    let redacted = handler::redact_person(&state, &user_id).await?;
    Ok(axum::Json(json!({ "redacted": redacted })))
}
