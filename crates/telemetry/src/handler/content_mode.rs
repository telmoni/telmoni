//! A project's content mode: `GET /internal/v1/telemetry/content-mode`, which
//! every role on the project reads, and `PUT`, which its organization's owner
//! alone may send, since what a project keeps changes what its organization
//! is liable for: leaving `off` takes the code `POST …/content-mode/code`
//! mailed them, and going back may delete what the project kept, its key
//! first. The project is the one `x-project-id` names.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::tenant_session::{
    ProjectAndOrganization, Scoped, maintenance_scope, project_scope,
};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::{Auth, Confirmation, Receipt};
use telmoni_shared::{
    AuditAction, AuthError, ContentMode, OrganizationId, ProjectId, Redacted, TelmoniError,
    TelmoniResourceKind,
};

use crate::db::TelemetryLane;
use crate::handler::chain;
use crate::retention::CONTENT_BATCH;
use crate::{AppState, db};

/// The modes an owner may switch a project to. A mode with nowhere to keep
/// content would tell the SDK, in every answer, to send what ingest must then
/// drop, so a mode joins only once ingest keeps what it promises.
pub const OFFERED: &[ContentMode] = &[ContentMode::Off, ContentMode::On];

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
    /// The code mailed to the owner, which a move out of `off` spends.
    #[serde(default)]
    pub code: Option<Redacted>,
    /// With `off`: delete what the project kept, its key at once.
    #[serde(default)]
    pub delete_content: bool,
}

/// What a change came to, logged once it has committed.
struct Switched {
    /// The mode it replaced, when it changed the mode.
    from: Option<ContentMode>,
    /// The organization a stray row was filed under, when it re-filed one.
    refiled_from: Option<OrganizationId>,
    /// The project's content key it deleted, by the id its rows are purged by.
    deleted: Option<Uuid>,
}

/// `GET /internal/v1/telemetry/content-mode` — the project's mode.
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

/// `PUT /internal/v1/telemetry/content-mode` — switch the project to one of the
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
    if change.delete_content && mode != ContentMode::Off {
        return Err(TelmoniError::BadRequest(
            "a project's content is deleted only as its mode goes off".into(),
        ));
    }
    let organization_id = &acting.organization_id;
    if mode != ContentMode::Off && current_mode(&state, project_id).await? == ContentMode::Off {
        // Spent before the switch: a switch that then fails leaves the code
        // spent, and the owner asks for another.
        let Some(code) = &change.code else {
            return Err(TelmoniError::BadRequest(
                "keeping content takes the code mailed to the organization's owner".into(),
            ));
        };
        state
            .auth
            .spend_confirmation(
                &acting,
                Confirmation::ContentMode(project_id),
                code.expose(),
            )
            .await?;
    }

    let delete = change.delete_content;
    let switched = chain::write(&state.db, &acting, project_id, "content mode", async |tx| {
        switch_content_mode(&*state.auth, &acting, project_id, tx, (mode, delete)).await
    })
    .await?;
    if let Some(key) = switched.deleted {
        purge_deleted(&state, &acting.user_id, project_id, Utc::now(), key);
    }
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
    (mode, delete): (ContentMode, bool),
) -> Result<Switched, TelmoniError> {
    let organization_id = &acting.organization_id;

    // ⚠ The organization's chain lock before anything is read, and the role
    // checked again on auth's second answer, the one that stands.
    let now = chain::hold(auth, acting, tx).await?;
    now.require_project_owner()?;

    let (filed_under, held) = db::settings_for_update(tx, project_id).await?.unzip();
    let from = held.unwrap_or_default();
    let refiled_from = filed_under.filter(|filed_under| filed_under != organization_id);
    let changed = from != mode;
    if changed || refiled_from.is_some() {
        db::set_content_mode(tx, project_id, organization_id, mode).await?;
    }
    // In the switch's own commit: from it, nothing the project kept opens,
    // on any process, whatever rows the purge after has yet to take.
    let deleted = if delete {
        db::delete_content_key(tx, project_id).await?
    } else {
        None
    };
    if changed || deleted.is_some() {
        let details = if deleted.is_some() {
            json!({ "content_mode": { "from": from, "to": mode }, "content_deleted": true })
        } else {
            json!({ "content_mode": { "from": from, "to": mode } })
        };
        emit_audit(
            tx,
            AuditEvent::new(
                organization_id,
                Actor::User(acting.user_id.as_str()),
                AuditAction::Updated,
                TelmoniResourceKind::Project,
            )
            .in_project(project_id)
            .on(project_id.as_str())
            .details(details),
        )
        .await?;
    }
    Ok(Switched {
        from: changed.then_some(from),
        refiled_from,
        deleted,
    })
}

/// The project's mode as its row holds it, for the switch to know whether it
/// leaves `off`.
async fn current_mode(
    state: &AppState,
    project_id: &ProjectId,
) -> Result<ContentMode, TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    let mode = db::content_mode(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(mode)
}

/// `POST /internal/v1/telemetry/content-mode/code` — mail the organization's
/// owner the code that lets the project keep content.
pub async fn request_content_mode_code(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting.require_project_owner()?.project_id;
    state
        .auth
        .mail_confirmation(&acting, Confirmation::ContentMode(project_id))
        .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "sent": true }))))
}

/// The rows sealed under `key`, the one the switch deleted, purged after its
/// commit, a batch a transaction, and the receipt mailed to `asker`; content
/// kept under a key made since, the project `on` again, is left alone. Apart
/// from the answer: nothing they hold opens any more, so a slow purge keeps
/// nobody waiting, and one that fails leaves rows nothing reads, which
/// retention takes in its time.
fn purge_deleted(
    state: &Arc<AppState>,
    asker: &telmoni_shared::UserId,
    project_id: &ProjectId,
    unreadable_since: DateTime<Utc>,
    key: Uuid,
) {
    let (state, asker, project_id) = (Arc::clone(state), asker.clone(), project_id.clone());
    tokio::spawn(async move {
        let purged = async {
            let mut gone = 0u64;
            loop {
                let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
                let batch = db::delete_key_content(
                    tx.conn(),
                    &project_id,
                    key,
                    unreadable_since,
                    CONTENT_BATCH,
                )
                .await?;
                tx.commit().await?;
                gone += batch;
                if batch < CONTENT_BATCH.unsigned_abs() {
                    return Ok::<_, TelmoniError>(gone);
                }
            }
        };
        match purged.await {
            Ok(rows) => tracing::info!(project_id = %project_id, rows,
                "telemetry: a project's deleted content purged"),
            Err(e) => tracing::warn!(project_id = %project_id, error = %e,
                "telemetry: a project's deleted content was not all purged; nothing reads what is left, and retention takes it"),
        }
        state
            .auth
            .mail_receipt(
                &asker,
                Receipt::Content {
                    project_id: &project_id,
                    unreadable_since,
                },
            )
            .await;
    });
}
