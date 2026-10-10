//! A project's content mode: `GET /internal/telemetry/content-mode`, which
//! every role on the project reads, and `PUT`, which its organization's owner
//! alone may send, since what a project keeps changes what its organization
//! is liable for. The project is the one `x-project-id` names.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::json;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::extract::{Json, correlation_id};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::Auth;
use telmoni_shared::{
    AuditAction, AuthError, ContentMode, OrganizationId, ProjectId, TelmoniError,
    TelmoniResourceKind,
};

use crate::{AppState, db};

/// The modes an owner may switch a project to: `off` alone, until a table
/// keeps content as it arrives (`on`) and our SDK seals it (`sealed`). A mode
/// with nowhere to keep content would tell the SDK, in every answer, to send
/// what ingest must then drop.
pub const OFFERED: &[ContentMode] = &[ContentMode::Off];

/// The longest a change waits for the organization's chain lock: the
/// role's own `lock_timeout` on a tier, set on the transaction too, so that a
/// deployment whose roles carry none — the Compose quickstart connects as the
/// superuser — never holds a connection on it for as long as another holds
/// the lock.
const CHAIN_LOCK_WAIT: Duration = Duration::from_secs(2);

/// How long a change waits for auth's second answer while it holds the
/// organization's chain lock: well inside the two seconds every other writer
/// in the organization waits for that lock before giving up.
const READ_AGAIN_BUDGET: Duration = Duration::from_secs(1);

/// Postgres's `lock_not_available`: a `lock_timeout` ran out.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// What both lanes answer: the project's mode, and the modes it may be
/// switched to.
#[derive(Debug, Serialize)]
pub struct ContentModeSetting {
    /// The project's mode: its own row's, or `off` where it has none.
    pub content_mode: ContentMode,
    /// [`OFFERED`].
    pub offered: &'static [ContentMode],
}

impl ContentModeSetting {
    const fn of(content_mode: ContentMode) -> Self {
        Self {
            content_mode,
            offered: OFFERED,
        }
    }
}

/// The body of a change.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModeChange {
    /// The mode to switch the project to.
    pub content_mode: ContentMode,
}

/// What a change came to, logged once it has committed.
struct Switched {
    /// The mode it replaced, when it changed the mode.
    from: Option<ContentMode>,
    /// The organization a stray row was filed under, when it re-filed one.
    refiled_from: Option<OrganizationId>,
}

/// `GET /internal/telemetry/content-mode` — the project's mode.
pub async fn get_content_mode(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ContentModeSetting>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Project)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let content_mode = db::content_mode(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(ContentModeSetting::of(content_mode)))
}

/// `PUT /internal/telemetry/content-mode` — switch the project to one of the
/// [`OFFERED`] modes. Written under the project and its organization both,
/// which the row's policy requires of a write, by an upsert that files the
/// row under that organization, which heals a row a transfer's failed
/// settling left under another. A change of mode is recorded, from and to, in
/// the same transaction; a save that changes nothing records nothing.
pub async fn put_content_mode(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(change): Json<ContentModeChange>,
) -> Result<Json<ContentModeSetting>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    // Refused before any lock is taken, so nobody below the owner waits on
    // the organization's chain to be told no.
    let project_id = &acting.require_project_owner()?.project_id;
    let mode = change.content_mode;
    if !OFFERED.contains(&mode) {
        return Err(AuthError::Conflict(format!(
            "content mode {mode} is not available yet; a project keeps no content until it is"
        ))
        .into());
    }
    let organization_id = &acting.organization_id;

    let mut tx = project_scope(&state.db, project_id)
        .await?
        .bind_organization(organization_id)
        .await?;
    // Nothing is written on a refusal, and the chain lock is let go before
    // the answer rather than whenever the pool next sees the connection.
    let switched =
        match switch_content_mode(&*state.auth, &acting, project_id, &mut tx, &headers, mode).await
        {
            Ok(switched) => switched,
            Err(refused) => {
                if let Err(e) = tx.rollback().await {
                    tracing::warn!(
                        project_id = %project_id,
                        error = %e,
                        "telemetry: a refused content mode change could not be rolled back; \
                         its connection drops it"
                    );
                }
                return Err(refused);
            }
        };
    tx.commit().await?;
    if let Some(filed_under) = &switched.refiled_from {
        // The settling's repair, which records nothing on a chain, as the
        // settling itself does not; logged as its failure was.
        tracing::info!(
            project_id = %project_id,
            organization_id = %organization_id,
            filed_under = %filed_under,
            "telemetry: a project's settings filed under the organization that holds it"
        );
    }
    if let Some(from) = switched.from {
        tracing::info!(
            project_id = %project_id,
            organization_id = %organization_id,
            from = %from,
            content_mode = %mode,
            "telemetry: a project's content mode changed"
        );
    }
    Ok(Json(ContentModeSetting::of(mode)))
}

/// The change itself, inside the transaction [`put_content_mode`] opened:
/// decided under the organization's chain lock, written where it changes
/// something, and recorded where it changes the mode.
async fn switch_content_mode(
    auth: &dyn Auth,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    headers: &HeaderMap,
    mode: ContentMode,
) -> Result<Switched, TelmoniError> {
    let organization_id = &acting.organization_id;

    // ⚠ The organization's chain lock before anything is read. A transfer's
    // accept holds it from its audit rows to its commit and moves this row
    // in between, as an ownership hand-over and a deletion hold it over
    // theirs: under it, the answer auth gives again is the one that stands,
    // and a change resolved before one of them committed is refused rather
    // than written under an organization that no longer holds the project.
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(format!("{}ms", CHAIN_LOCK_WAIT.as_millis()))
        .execute(tx.conn())
        .await?;
    if let Err(e) = lock_chain(tx.conn(), organization_id).await {
        let timed_out = e
            .as_database_error()
            .and_then(|db| db.code())
            .is_some_and(|code| code == LOCK_NOT_AVAILABLE);
        if !timed_out {
            return Err(e.into());
        }
        tracing::warn!(
            project_id = %project_id,
            organization_id = %organization_id,
            "telemetry: a content mode change gave up on the organization's chain lock"
        );
        return Err(AuthError::Conflict(
            "another change to this project's organization is under way; try again".into(),
        )
        .into());
    }
    let Ok(now) = tokio::time::timeout(READ_AGAIN_BUDGET, auth.resolve_again(acting)).await else {
        tracing::warn!(
            project_id = %project_id,
            organization_id = %organization_id,
            "telemetry: auth did not answer a content mode change again in time"
        );
        return Err(AuthError::IdentityUnavailable.into());
    };
    let now = now?;
    if now.organization_id != *organization_id {
        return Err(AuthError::Conflict(
            "the project has moved to another organization; reload and try again".into(),
        )
        .into());
    }
    now.require_project_owner()?;

    let (filed_under, held) = db::settings_for_update(tx, project_id).await?.unzip();
    let from = held.unwrap_or_default();
    let refiled_from = filed_under.filter(|filed_under| filed_under != organization_id);
    let changed = from != mode;
    if changed || refiled_from.is_some() {
        db::set_content_mode(tx, project_id, organization_id, mode).await?;
    }
    if changed {
        emit_audit(
            tx,
            AuditEvent {
                organization_id,
                in_project: Some(project_id),
                actor: Actor::User(acting.user_id.as_str()),
                action: AuditAction::Updated,
                resource_kind: TelmoniResourceKind::Project,
                resource_id: Some(project_id.as_str()),
                request_id: correlation_id(headers),
                ip_address: None,
                user_agent: None,
                metadata: Some(json!({ "content_mode": { "from": from, "to": mode } })),
            },
        )
        .await?;
    }
    Ok(Switched {
        from: changed.then_some(from),
        refiled_from,
    })
}
