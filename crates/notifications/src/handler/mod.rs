//! The BFF-facing feed under `/internal/v1/notifications`, and what the modules
//! beside this one call in process: the emit, the purges and the redaction.
//! Emitting is not itself audited: the module that caused the event audits it.

pub mod connectors;
pub mod slack_events;

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::IntoResponse};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use telmoni_shared::acting::Acting;
use telmoni_shared::db::tenant_session::{
    Binding, Scoped, maintenance_scope, organization_scope, project_scope,
};
use telmoni_shared::extract::Json;
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam::{Emitted, Notice, Route, Routed, resolve_again_in_time};
use telmoni_shared::{NotificationKind, OrganizationId, ProjectId, TelmoniError, UserId};
use uuid::Uuid;

use crate::AppState;
use crate::db::{self, NotificationsLane};
use crate::{delivery, retention, uninstall};

/// Most feed rows a read returns (newest first; a dropdown, not an export).
const FEED_LIMIT: i64 = 50;

/// The organization's audit chain lock, before any row: a transfer holds it
/// while it awaits the purge, which must never wait on a lane waiting on it.
pub(crate) async fn lock_audit_chain<B: Binding>(
    tx: &mut Scoped<'_, B>,
    organization_id: &OrganizationId,
) -> Result<(), TelmoniError> {
    Ok(telmoni_shared::audit::lock_chain_in_time(tx.conn(), organization_id).await?)
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
    resolve_again_in_time(state.auth.as_ref(), &identity.acting)
        .await?
        .require_project(verb, resource)?;
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
        .map_err(|e| TelmoniError::BadRequest(format!("invalid x-project-id: {e}")))
}

/// The bounds a notice's text fields carry.
const TITLE_MAX_CHARS: usize = 200;
const BODY_MAX_CHARS: usize = 4_000;
/// `<producer>:<id>` — the shape `Notice::dedup_key` documents.
const DEDUP_KEY_MAX_CHARS: usize = 200;
/// A route's key, as its column holds it.
const ROUTE_MAX_CHARS: usize = 200;
/// The one bound in BYTES: `metadata` is served to a browser verbatim, so a
/// producer must not be able to make a notice carry a megabyte.
const METADATA_MAX_BYTES: u64 = 4_096;

/// One trimmed, length-checked text field, or a 400 that names it.
fn bounded<'a>(value: &'a str, field: &str, max: usize) -> Result<&'a str, TelmoniError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max {
        return Err(TelmoniError::BadRequest(format!(
            "{field} is 1-{max} characters"
        )));
    }
    Ok(trimmed)
}

/// A notice held to an emit's bounds, checked once for every way it is written.
struct Checked<'a> {
    kind: NotificationKind,
    subject_user_id: Option<&'a UserId>,
    title: &'a str,
    body: &'a str,
    metadata: serde_json::Value,
    dedup_key: Option<&'a str>,
}

impl<'a> Checked<'a> {
    fn new(notice: Notice<'a>) -> Result<Self, TelmoniError> {
        let metadata = if notice.metadata.is_null() {
            json!({})
        } else {
            notice.metadata
        };
        if !metadata.is_object() {
            return Err(TelmoniError::BadRequest(
                "metadata must be a JSON object".into(),
            ));
        }
        if serde_json::to_string(&metadata).map_or(0, |s| s.len()) as u64 > METADATA_MAX_BYTES {
            return Err(TelmoniError::PayloadTooLarge {
                max_bytes: METADATA_MAX_BYTES,
            });
        }
        Ok(Self {
            kind: notice.kind,
            subject_user_id: notice.subject_user_id,
            title: bounded(notice.title, "title", TITLE_MAX_CHARS)?,
            body: bounded(notice.body, "body", BODY_MAX_CHARS)?,
            metadata,
            dedup_key: notice
                .dedup_key
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(|key| bounded(key, "dedup_key", DEDUP_KEY_MAX_CHARS))
                .transpose()?,
        })
    }

    /// The feed row it is written as.
    fn item(&self) -> db::NewFeedItem<'_> {
        db::NewFeedItem {
            subject_user_id: self.subject_user_id,
            kind: self.kind,
            title: self.title,
            body: self.body,
            metadata: &self.metadata,
            dedup_key: self.dedup_key,
        }
    }
}

/// Where a fresh row's deliveries go: every active connection of the feed's
/// whole, or the ones a route names.
enum Fanout<'a> {
    All,
    Routed {
        project_id: &'a ProjectId,
        key: &'a str,
        connections: &'a [Uuid],
        most: usize,
    },
}

/// What an emit's transaction came to: the write, and for a fresh row the
/// deliveries queued and, where a route named them, the connections.
struct Written {
    emitted: Emitted,
    deliveries: Vec<Uuid>,
    connections: Vec<Uuid>,
}

/// The feed write and the fan-out, in one transaction committed here. Generic
/// over the binding because a project notice and an organization notice open
/// different scopes for the same body.
async fn write_notice<B: db::EmitBinding>(
    mut tx: Scoped<'_, B>,
    project_id: Option<&ProjectId>,
    organization_id: &OrganizationId,
    notice: &Checked<'_>,
    fanout: Fanout<'_>,
) -> Result<Written, TelmoniError> {
    let item = notice.item();
    let Some(feed_id) = db::insert_feed(&mut tx, project_id, organization_id, &item).await? else {
        let feed_id = db::feed_id_for_dedup_key(
            &mut tx,
            organization_id,
            project_id,
            item.dedup_key.unwrap_or_default(),
        )
        .await?
        .ok_or_else(|| TelmoniError::Internal("dedup conflict with no surviving row".into()))?;
        tx.commit().await?;
        return Ok(Written {
            emitted: Emitted::Replayed { feed_id },
            deliveries: Vec::new(),
            connections: Vec::new(),
        });
    };
    let (deliveries, connections) = match fanout {
        Fanout::All => (
            db::enqueue_deliveries(
                &mut tx,
                organization_id,
                project_id,
                item.kind,
                item.title,
                item.body,
                item.subject_user_id,
            )
            .await?,
            Vec::new(),
        ),
        Fanout::Routed {
            project_id,
            key,
            connections,
            most,
        } => db::enqueue_routed(
            &mut tx,
            organization_id,
            project_id,
            item.kind,
            item.title,
            item.body,
            key,
            connections,
            most,
        )
        .await?
        .into_iter()
        .unzip(),
    };
    tx.commit().await?;
    Ok(Written {
        emitted: Emitted::Fresh { feed_id },
        deliveries,
        connections,
    })
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
    let notice = Checked::new(notice)?;
    let written = match project_id {
        Some(project) => {
            let tx = project_scope(&state.db, project).await?;
            write_notice(tx, Some(project), organization_id, &notice, Fanout::All).await?
        }
        None => {
            let tx = organization_scope(&state.db, organization_id).await?;
            write_notice(tx, None, organization_id, &notice, Fanout::All).await?
        }
    };
    if let Emitted::Fresh { .. } = written.emitted {
        tracing::info!(
            project_id = project_id.map(AsRef::as_ref),
            organization_id = %organization_id,
            kind = %notice.kind,
            queued = written.deliveries.len(),
            "notification emitted"
        );
        delivery::spawn_first_attempts(state, written.deliveries);
    }
    Ok(written.emitted)
}

/// A notice written to the project's feed and queued to the connections its
/// route names ([`telmoni_shared::seam::Notifications::emit_routed`]), each
/// active, taking its kind, and not muted for the route; a dedup key written
/// before answers its first row and queues nothing, as an emit does.
pub async fn emit_routed_notice(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    notice: Notice<'_>,
    route: Route<'_>,
) -> Result<Routed, TelmoniError> {
    let notice = Checked::new(notice)?;
    let key = bounded(route.key, "route", ROUTE_MAX_CHARS)?;
    let tx = project_scope(&state.db, project_id).await?;
    let written = write_notice(
        tx,
        Some(project_id),
        organization_id,
        &notice,
        Fanout::Routed {
            project_id,
            key,
            connections: route.connections,
            most: route.most,
        },
    )
    .await?;
    if let Emitted::Fresh { .. } = written.emitted {
        tracing::info!(
            project_id = %project_id,
            organization_id = %organization_id,
            kind = %notice.kind,
            queued = written.deliveries.len(),
            "notification emitted to its route"
        );
        delivery::spawn_first_attempts(state, written.deliveries);
    }
    Ok(Routed {
        emitted: written.emitted,
        connections: written.connections,
    })
}

/// A notice queued to the connections its route names alone, as
/// [`emit_routed_notice`] picks them, with no feed row: a reminder, or what
/// an incident whose feed rows are written already says again
/// ([`telmoni_shared::seam::Notifications::remind`]). Its title and body are
/// held to an emit's bounds; it has no metadata, which only the feed keeps.
pub async fn remind_notice(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    notice: Notice<'_>,
    route: Route<'_>,
) -> Result<Vec<Uuid>, TelmoniError> {
    let title = bounded(notice.title, "title", TITLE_MAX_CHARS)?;
    let body = bounded(notice.body, "body", BODY_MAX_CHARS)?;
    let route_key = bounded(route.key, "route", ROUTE_MAX_CHARS)?;
    let mut tx = project_scope(&state.db, project_id).await?;
    let queued = db::enqueue_routed(
        &mut tx,
        organization_id,
        project_id,
        notice.kind,
        title,
        body,
        route_key,
        route.connections,
        route.most,
    )
    .await?;
    tx.commit().await?;
    tracing::info!(
        project_id = %project_id,
        organization_id = %organization_id,
        kind = %notice.kind,
        queued = queued.len(),
        "notification told the connectors again"
    );
    let (deliveries, connections) = queued.into_iter().unzip();
    delivery::spawn_first_attempts(state, deliveries);
    Ok(connections)
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

/// `GET /internal/v1/notifications/feed` — the recent feed, in ONE scope.
pub async fn get_feed(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, TelmoniError> {
    let (items, unread) = match feed_scope(&state, &headers).await? {
        FeedScope::Project(project_id) => {
            read_feed(project_scope(&state.db, &project_id).await?, &project_id).await?
        }
        FeedScope::Organization(organization_id) => {
            read_feed(
                organization_scope(&state.db, &organization_id).await?,
                &organization_id,
            )
            .await?
        }
    };
    Ok(Json(json!({ "items": items, "unread": unread })))
}

/// The feed and its unread count, read together.
async fn read_feed<S: db::FeedScope>(
    mut tx: Scoped<'_, S>,
    key: &S::Key,
) -> Result<(Vec<db::FeedItem>, i64), TelmoniError> {
    let items = db::list_feed(&mut tx, key, FEED_LIMIT).await?;
    let unread = db::count_unread(&mut tx, key).await?;
    tx.commit().await?;
    Ok((items, unread))
}

/// What a mark-read asks: the feed read up to `before`, the newest notice the
/// page drew, so one that landed since stays unread for the next draw.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkRead {
    pub before: DateTime<Utc>,
}

/// `POST /internal/v1/notifications/read` — mark the feed read up to `before`,
/// with the same predicate as the list and count; three readers over three
/// sets would be a badge nothing can clear.
pub async fn mark_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(MarkRead { before }): Json<MarkRead>,
) -> Result<impl IntoResponse, TelmoniError> {
    let marked = match feed_scope(&state, &headers).await? {
        FeedScope::Project(project_id) => {
            mark_feed_read(
                project_scope(&state.db, &project_id).await?,
                &project_id,
                before,
            )
            .await?
        }
        FeedScope::Organization(organization_id) => {
            mark_feed_read(
                organization_scope(&state.db, &organization_id).await?,
                &organization_id,
                before,
            )
            .await?
        }
    };
    Ok(Json(json!({ "marked": marked })))
}

/// The feed marked read up to `before`, committed.
async fn mark_feed_read<S: db::FeedScope>(
    mut tx: Scoped<'_, S>,
    key: &S::Key,
    before: DateTime<Utc>,
) -> Result<u64, TelmoniError> {
    let marked = db::mark_all_read(&mut tx, key, before).await?;
    tx.commit().await?;
    Ok(marked)
}

/// Everything held for the organization, before its row goes, and an uninstall
/// filed for each workspace its connectors reached. What auth's finalize calls.
pub async fn purge_organization(
    state: &Arc<AppState>,
    organization_id: &OrganizationId,
) -> Result<u64, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    let purged = db::purge_connections(&mut tx, db::FeedOf::Organization(organization_id)).await?;
    let filed = uninstall::file(&mut tx, &purged.connections).await?;
    tx.commit().await?;
    uninstall::settle_soon(state, filed);
    let rows = purged.rows + purge_feed(state, db::FeedOf::Organization(organization_id)).await?;
    tracing::info!(
        organization_id = %organization_id,
        purged = rows,
        "notifications purge: the organization's feeds removed"
    );
    Ok(rows)
}

/// Everything held for one project, and an uninstall filed for each workspace
/// its connectors reached: before a transfer, as the old owner's grants must not
/// post the new owner's events, after a delete, and again by the sweep. The
/// connections go in a transaction of their own, committed before the feed.
pub async fn purge_project(
    state: &Arc<AppState>,
    project_id: &ProjectId,
) -> Result<u64, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    db::wait_no_longer(&mut tx).await?;
    let purged = db::purge_connections(&mut tx, db::FeedOf::Project(project_id)).await?;
    let filed = uninstall::file(&mut tx, &purged.connections).await?;
    tx.commit().await?;
    uninstall::settle_soon(state, filed);
    let rows = purged.rows + purge_feed(state, db::FeedOf::Project(project_id)).await?;
    tracing::info!(
        project_id = %project_id,
        purged = rows,
        "notifications purge: the project's rows removed"
    );
    Ok(rows)
}

/// Everything held for one project before it is handed to another organization,
/// in one transaction — the feed in bounded statements, before the connections, the
/// order an erasure and the retention sweep write them, and once more after, for a
/// notice raised meanwhile — so a move refused leaves it as it was; the caller's
/// budget bounds it, and the uninstalls filed are settled after the commit.
pub async fn purge_project_for_move(
    state: &Arc<AppState>,
    project_id: &ProjectId,
) -> Result<u64, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
    db::wait_no_longer(&mut tx).await?;
    let mut rows = 0;
    loop {
        let removed = db::purge_feed_chunk(
            &mut tx,
            db::FeedOf::Project(project_id),
            retention::SWEEP_CHUNK,
        )
        .await?;
        rows += removed;
        if removed < feed_chunk() {
            break;
        }
    }
    let purged = db::purge_connections(&mut tx, db::FeedOf::Project(project_id)).await?;
    let filed = uninstall::file(&mut tx, &purged.connections).await?;
    rows += purged.rows;
    // A notice raised while the feed went — a seat's emitted after auth's
    // commit — goes last; one an erasure holds costs a wait the
    // deadlock detector ends, and the move is tried again.
    rows += db::purge_feed_chunk(
        &mut tx,
        db::FeedOf::Project(project_id),
        retention::SWEEP_CHUNK,
    )
    .await?;
    tx.commit().await?;
    uninstall::settle_soon(state, filed);
    tracing::info!(
        project_id = %project_id,
        purged = rows,
        "notifications purge: the project's rows removed before its move"
    );
    Ok(rows)
}

/// A feed, a chunk a transaction, so no statement outgrows the role's five seconds
/// and a purge cut short keeps what it did. One that outruns the retention sweep's
/// rounds is an `Err`, for its caller to try again where it stopped.
async fn purge_feed(state: &AppState, of: db::FeedOf<'_>) -> Result<u64, TelmoniError> {
    let mut rows = 0;
    for _ in 0..retention::SWEEP_MAX_ROUNDS {
        let mut tx = maintenance_scope(&state.db, NotificationsLane).await?;
        db::wait_no_longer(&mut tx).await?;
        let removed = db::purge_feed_chunk(&mut tx, of, retention::SWEEP_CHUNK).await?;
        tx.commit().await?;
        rows += removed;
        if removed < feed_chunk() {
            return Ok(rows);
        }
    }
    Err(TelmoniError::Internal(format!(
        "the feed outran the purge's rounds after {rows} rows, kept; the next try goes on"
    )))
}

/// The retention sweep's chunk, as the rows a statement answers are counted.
fn feed_chunk() -> u64 {
    u64::try_from(retention::SWEEP_CHUNK).unwrap_or(u64::MAX)
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
