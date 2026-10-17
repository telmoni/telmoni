//! The BFF-facing feed under `/internal/notifications`, and what the modules
//! beside this one call in process: the emit, the purges and the redaction.
//! Emitting is not itself audited: the module that caused the event audits it.

pub mod connectors;
pub mod slack_events;

use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, http::HeaderMap, response::IntoResponse};
use serde_json::json;
use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{
    Binding, Scoped, maintenance_scope, organization_scope, project_scope,
};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::{Emitted, Notice};
use telmoni_shared::{AuthError, OrganizationId, ProjectId, TelmoniError, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::db::{self, NotificationsLane};
use crate::{delivery, uninstall};

/// Most feed rows a read returns (newest first; a dropdown, not an export).
const FEED_LIMIT: i64 = 50;

/// The longest a lane waits for its organization's chain lock, and for any lock
/// after it: the role's own `lock_timeout` on a tier, set for roles with none.
const CHAIN_LOCK_WAIT: &str = "2000ms";

/// Postgres's `lock_not_available`: a `lock_timeout` ran out.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// How long a lane under the chain lock waits for auth's second answer: well
/// inside the two seconds the organization's other writers wait for that lock.
const ANSWER_BUDGET: Duration = Duration::from_secs(1);

/// The organization's audit chain lock, before any row: a transfer holds it
/// while it awaits the purge, which must never wait on a lane waiting on it.
pub(crate) async fn lock_audit_chain<B: Binding>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(CHAIN_LOCK_WAIT)
        .execute(tx.conn())
        .await?;
    match telmoni_shared::audit::lock_chain(tx.conn(), organization_id).await {
        Ok(()) => Ok(()),
        Err(e)
            if e.as_database_error()
                .and_then(|db| db.code())
                .is_some_and(|code| code == LOCK_NOT_AVAILABLE) =>
        {
            tracing::warn!(
                organization_id = %organization_id,
                "notifications: a change gave up on the organization's chain lock"
            );
            Err(AuthError::Conflict(
                "another change to this organization is under way; try again".into(),
            )
            .into())
        }
        Err(e) => Err(e.into()),
    }
}

/// The chain lock, then auth's answer again, which stands to the commit as a transfer
/// holds that lock to its own: a project moved since, or a role lost, is refused.
pub(crate) async fn hold_chain<B: Binding>(
    state: &AppState,
    tx: &mut Scoped<'_, B>,
    identity: &ProjectIdentity,
    verb: Verb,
    resource: Resource,
) -> Result<(), TelmoniError> {
    lock_audit_chain(tx, &identity.organization_id).await?;
    let Ok(now) =
        tokio::time::timeout(ANSWER_BUDGET, state.auth.resolve_again(&identity.acting)).await
    else {
        tracing::warn!(
            project_id = %identity.project_id,
            organization_id = %identity.organization_id,
            "notifications: auth did not answer a change to a project again in time"
        );
        return Err(AuthError::IdentityUnavailable.into());
    };
    let now = now?;
    if now.organization_id != identity.organization_id {
        return Err(AuthError::Conflict(
            "the project has moved to another organization; reload and try again".into(),
        )
        .into());
    }
    now.require_project(verb, resource)?;
    Ok(())
}

/// Who is behind this request's bearer, as auth resolves it for the
/// organization and project the headers name. This module cannot decide
/// that itself: it holds no grant on `auth.project_members`.
pub(crate) async fn resolve(state: &AppState, headers: &HeaderMap) -> Result<Acting, TelmoniError> {
    state.auth.resolve(headers).await
}

/// A person acting on a project, at a role the matrix admits for `verb` on
/// `resource`. What every connector lane opens with.
pub(crate) struct ProjectIdentity {
    /// Who acts (audit actor, `installed_by`).
    pub user_id: UserId,
    /// The project acted on — the tenancy boundary every scoped query binds.
    pub project_id: ProjectId,
    /// The organization that OWNS that project, as auth resolved it.
    pub organization_id: OrganizationId,
    /// Auth's whole answer, for asking it again under the chain lock ([`hold_chain`]).
    pub acting: Acting,
}

pub(crate) async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    verb: Verb,
    resource: Resource,
) -> Result<ProjectIdentity, TelmoniError> {
    let acting = resolve(state, headers).await?;
    let project_id = acting.require_project(verb, resource)?.project_id.clone();
    Ok(ProjectIdentity {
        project_id,
        user_id: acting.user_id.clone(),
        organization_id: acting.organization_id.clone(),
        acting,
    })
}

/// The PROJECT a feed read names, from `x-project-id`.
fn project_from_header(headers: &HeaderMap) -> Result<Option<ProjectId>, TelmoniError> {
    let Some(raw) = headers
        .get("x-project-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    ProjectId::try_new(raw)
        .map(Some)
        .map_err(|e| AuthError::BadRequest(format!("invalid x-project-id: {e}")).into())
}

/// The bounds a notice's text fields carry.
const TITLE_MAX_CHARS: usize = 200;
const BODY_MAX_CHARS: usize = 4_000;
/// `<producer>:<id>` — the shape `Notice::dedup_key` documents.
const DEDUP_KEY_MAX_CHARS: usize = 200;
/// The one bound in BYTES: `metadata` is served to a browser verbatim, so a
/// producer must not be able to make a notice carry a megabyte.
const METADATA_MAX_BYTES: u64 = 4_096;

/// One trimmed, length-checked text field, or a 400 that names it.
fn bounded<'a>(value: &'a str, field: &str, max: usize) -> Result<&'a str, TelmoniError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max {
        return Err(AuthError::BadRequest(format!("{field} is 1-{max} characters")).into());
    }
    Ok(trimmed)
}

/// What an emit's transaction came to, for the answer built after its commit.
enum Written {
    /// A new row, and the deliveries queued for it.
    Fresh { feed_id: Uuid, queued: Vec<Uuid> },
    /// A replay: the id the first emit minted for this key, and nothing queued.
    Replayed { feed_id: Uuid },
}

/// The feed write and the fan-out, in one transaction committed here. Generic
/// over the binding because a project notice and an organization notice open
/// different scopes for the same body.
async fn write_notice<B: db::EmitBinding>(
    mut tx: Scoped<'_, B>,
    project_id: Option<&ProjectId>,
    organization_id: &OrganizationId,
    item: &db::NewFeedItem<'_>,
) -> Result<Written, TelmoniError> {
    let Some(feed_id) = db::insert_feed(&mut tx, project_id, organization_id, item).await? else {
        let feed_id = db::feed_id_for_dedup_key(
            &mut tx,
            organization_id,
            project_id,
            item.dedup_key.unwrap_or_default(),
        )
        .await?
        .ok_or_else(|| TelmoniError::Internal("dedup conflict with no surviving row".into()))?;
        tx.commit().await?;
        return Ok(Written::Replayed { feed_id });
    };
    let queued = db::enqueue_deliveries(
        &mut tx,
        organization_id,
        project_id,
        item.kind,
        item.title,
        item.body,
        item.subject_user_id,
    )
    .await?;
    tx.commit().await?;
    Ok(Written::Fresh { feed_id, queued })
}

/// Write the feed row and queue one delivery per active connection, in one
/// transaction: the project's when one is named, else the organization's.
/// First attempts spawn AFTER the commit, so the answer never waits on a
/// vendor. What a module beside this one calls, through
/// [`crate::seam::Notifier`].
pub async fn emit_notice(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
    project_id: Option<&ProjectId>,
    notice: Notice<'_>,
) -> Result<Emitted, TelmoniError> {
    let kind = notice.kind;
    let title = bounded(notice.title, "title", TITLE_MAX_CHARS)?;
    let body = bounded(notice.body, "body", BODY_MAX_CHARS)?;
    let dedup_key = match notice.dedup_key.map(str::trim).filter(|s| !s.is_empty()) {
        Some(key) => Some(bounded(key, "dedup_key", DEDUP_KEY_MAX_CHARS)?),
        None => None,
    };
    let metadata = if notice.metadata.is_null() {
        json!({})
    } else {
        notice.metadata
    };
    if !metadata.is_object() {
        return Err(AuthError::BadRequest("metadata must be a JSON object".into()).into());
    }
    if serde_json::to_string(&metadata).map_or(0, |s| s.len()) as u64 > METADATA_MAX_BYTES {
        return Err(TelmoniError::PayloadTooLarge {
            max_bytes: METADATA_MAX_BYTES,
        });
    }

    let item = db::NewFeedItem {
        subject_user_id: notice.subject_user_id,
        kind,
        title,
        body,
        metadata: &metadata,
        dedup_key,
    };
    let written = match project_id {
        Some(project) => {
            let tx = project_scope(&state.db, project).await?;
            write_notice(tx, Some(project), organization_id, &item).await?
        }
        None => {
            let tx = organization_scope(&state.db, organization_id).await?;
            write_notice(tx, None, organization_id, &item).await?
        }
    };
    let (feed_id, queued) = match written {
        Written::Replayed { feed_id } => {
            return Ok(Emitted {
                feed_id,
                deduplicated: true,
            });
        }
        Written::Fresh { feed_id, queued } => (feed_id, queued),
    };

    tracing::info!(
        project_id = project_id.map(AsRef::as_ref),
        organization_id = %organization_id,
        kind = %kind,
        queued = queued.len(),
        "notification emitted"
    );
    delivery::spawn_first_attempts(state, queued);
    Ok(Emitted {
        feed_id,
        deduplicated: false,
    })
}

/// Which feed a read names: the project in `x-project-id`, at any role auth
/// admits the person to, or the organization, owner and admins.
enum FeedScope {
    Project(ProjectId),
    Organization(OrganizationId),
}

async fn feed_scope(state: &AppState, headers: &HeaderMap) -> Result<FeedScope, TelmoniError> {
    let acting = resolve(state, headers).await?;
    match project_from_header(headers)? {
        Some(_) => Ok(FeedScope::Project(
            acting.project_or_bad_request()?.project_id.clone(),
        )),
        None => Ok(FeedScope::Organization(
            acting.require_organization_admin()?.organization_id.clone(),
        )),
    }
}

/// `GET /internal/notifications/feed` — the recent feed, in ONE scope.
pub async fn get_feed(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let (items, unread) = match feed_scope(&state, &headers).await? {
        FeedScope::Project(project_id) => {
            let mut tx = project_scope(&state.db, &project_id).await?;
            let items = db::list_feed(&mut tx, &project_id, FEED_LIMIT).await?;
            let unread = db::count_unread(&mut tx, &project_id).await?;
            tx.commit().await?;
            (items, unread)
        }
        FeedScope::Organization(organization_id) => {
            let mut tx = organization_scope(&state.db, &organization_id).await?;
            let items = db::list_organization_feed(&mut tx, &organization_id, FEED_LIMIT).await?;
            let unread = db::count_organization_unread(&mut tx, &organization_id).await?;
            tx.commit().await?;
            (items, unread)
        }
    };
    Ok(Json(json!({ "items": items, "unread": unread })))
}

/// `POST /internal/notifications/read` — mark the feed read with the same
/// predicate as the list and count; three readers over three sets would be a
/// badge nothing can clear.
pub async fn mark_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let marked = match feed_scope(&state, &headers).await? {
        FeedScope::Project(project_id) => {
            let mut tx = project_scope(&state.db, &project_id).await?;
            let marked = db::mark_all_read(&mut tx, &project_id).await?;
            tx.commit().await?;
            marked
        }
        FeedScope::Organization(organization_id) => {
            let mut tx = organization_scope(&state.db, &organization_id).await?;
            let marked = db::mark_organization_read(&mut tx, &organization_id).await?;
            tx.commit().await?;
            marked
        }
    };
    Ok(Json(json!({ "marked": marked })))
}

/// Everything held for the organization, before its row goes, and an uninstall
/// filed for each workspace its connectors reached. What auth's finalize calls.
pub async fn purge_organization(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
) -> Result<u64, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let purged = db::purge_organization(&mut tx, organization_id).await?;
    let filed = uninstall::file(&mut tx, &purged.connections).await?;
    tx.commit().await?;
    uninstall::settle_soon(state, filed);
    tracing::info!(
        organization_id = %organization_id,
        purged = purged.rows,
        "notifications purge: the organization's feeds removed"
    );
    Ok(purged.rows)
}

/// Everything held for one project, and an uninstall filed for each workspace
/// its connectors reached: before a transfer, as the old owner's grants must not
/// post the new owner's events, after a delete, and again by the sweep.
pub async fn purge_project(
    state: &Arc<AppState>,
    project_id: &ProjectId,
) -> Result<u64, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    db::wait_no_longer(&mut tx).await?;
    let purged = db::purge_project(&mut tx, project_id).await?;
    let filed = uninstall::file(&mut tx, &purged.connections).await?;
    tx.commit().await?;
    uninstall::settle_soon(state, filed);
    tracing::info!(
        project_id = %project_id,
        purged = purged.rows,
        "notifications purge: the project's rows removed"
    );
    Ok(purged.rows)
}

/// A person's account erasure: every notice that named them, on every feed
/// and every delivery row still holding the text, rewritten to name a former
/// member. Called by auth's `erase_person` before the identity goes;
/// idempotent, and a failure stops that erasure for the sweep to retry.
///
/// What a connected Slack channel, or a webhook, already received
/// is not reached from here and cannot be: those copies are the vendor's.
pub async fn redact_person(state: &AppState, user_id: &UserId) -> Result<u64, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let redacted = db::redact_person(&mut tx, user_id).await?;
    tx.commit().await?;
    tracing::info!(user_id = %user_id, redacted, "notifications redaction: the person's name rewritten");
    Ok(redacted)
}
