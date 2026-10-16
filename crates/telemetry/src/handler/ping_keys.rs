//! A project's ping keys, under [`Resource::Token`] as its API keys are: a
//! credential a machine holds, shown once and kept hashed. `GET
//! /internal/telemetry/ping-keys`, which lists them; `POST`, which mints one
//! — two may be live at once, so a rotation moves the crontab lines over one
//! at a time; and `DELETE /ping-keys/{id}`, which retires one now. The
//! rotation retires the older of two a week after its last ping
//! ([`crate::rotation`]), through [`retire_stale_keys`]. Each minting and
//! retiring is recorded on the chain as [`TelmoniResourceKind::PingKey`],
//! since a key is what lets a line fake a ping.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::acting::Acting;
use telmoni_shared::audit::{Actor, AuditEvent, emit_audit};
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{
    ProjectAndOrganization, Scoped, maintenance_scope, project_scope,
};
use telmoni_shared::digest::sha256_hex;
use telmoni_shared::extract::{Json, correlation_id};
use telmoni_shared::rbac::{Resource, Verb};
use telmoni_shared::seam;
use telmoni_shared::types::{PING_KEY_PREFIX, mint_ping_key};
use telmoni_shared::{
    AuditAction, AuthError, OrganizationId, ProjectId, TelmoniError, TelmoniResourceKind,
};

use crate::AppState;
use crate::db::{self, PingKeyRow, ResolvedPingKey, TelemetryLane};
use crate::handler::chain;
use crate::rotation::REPLACED_AFTER_S;
use crate::{notify, outbox};

/// How many of a project's keys may be live at once: the one its lines
/// carry, and the one they are moving to.
pub const LIVE_MAX: i64 = 2;
/// How much of a key the list shows: its prefix and the first characters
/// after it, enough to tell two keys apart and nothing a guesser gains by.
const SHOWN: usize = PING_KEY_PREFIX.len() + 8;
/// How many stale keys one tick of the rotation retires.
const RETIRE_BATCH: i64 = 100;

/// One ping key, as the lanes answer it: never the key.
#[derive(Debug, Serialize)]
pub struct PingKey {
    pub id: Uuid,
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    pub last_ping_at: Option<DateTime<Utc>>,
    pub retired_at: Option<DateTime<Utc>>,
}

impl From<PingKeyRow> for PingKey {
    fn from(row: PingKeyRow) -> Self {
        Self {
            id: row.id,
            prefix: row.prefix,
            created_at: row.created_at,
            last_ping_at: row.last_ping_at,
            retired_at: row.retired_at,
        }
    }
}

/// What the list answers: the live keys first, each set newest first.
#[derive(Debug, Serialize)]
pub struct PingKeyList {
    pub keys: Vec<PingKey>,
}

/// A key as it is minted: the key itself, shown this once, and its row.
#[derive(Debug, Serialize)]
pub struct MintedPingKey {
    pub key: String,
    #[serde(flatten)]
    pub ping_key: PingKey,
}

/// A key the project does not have.
fn not_found() -> TelmoniError {
    AuthError::NotFound("no ping key by that id in this project".into()).into()
}

/// `GET /internal/telemetry/ping-keys` — the project's keys, as its API keys
/// are listed: what each shows of itself, when it was made, its last ping
/// and when it was retired.
pub async fn list_ping_keys(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<PingKeyList>, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = &acting
        .require_project(Verb::Read, Resource::Token)?
        .project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    let rows = db::ping_keys(&mut tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(PingKeyList {
        keys: rows.into_iter().map(PingKey::from).collect(),
    }))
}

/// `POST /internal/telemetry/ping-keys` — a new key of the project, shown
/// this once, made under the organization's chain lock: the second live key
/// beside the first for a rotation, and never a third.
pub async fn create_ping_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<MintedPingKey>), TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Create, Resource::Token)?
        .project_id
        .clone();
    seam::admit_new(state.entitlements.as_ref(), &acting.organization_id).await?;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let minted = match mint_key(&state, &acting, &project_id, &mut tx, &headers).await {
        Ok(minted) => minted,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "ping key");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, ping_key_id = %minted.ping_key.id, "telemetry: a ping key minted");
    Ok((StatusCode::CREATED, Json(minted)))
}

/// The minting itself, inside the transaction [`create_ping_key`] opened:
/// under the organization's chain lock, the role checked again on auth's
/// second answer, the project's settings row made or re-filed, the live
/// keys counted, and the key written as its hash and recorded.
async fn mint_key(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    headers: &HeaderMap,
) -> Result<MintedPingKey, TelmoniError> {
    let organization_id = &acting.organization_id;
    let now = chain::hold(&*state.auth, acting, project_id, tx).await?;
    now.require_project(Verb::Create, Resource::Token)?;
    db::ensure_settings(tx, project_id, organization_id).await?;
    let live = db::live_ping_key_count(tx, project_id).await?;
    if live >= LIVE_MAX {
        return Err(AuthError::Conflict(format!(
            "this project holds {LIVE_MAX} live ping keys already, the most it may: retire one \
             first"
        ))
        .into());
    }
    let key = mint_ping_key();
    let shown: String = key.chars().take(SHOWN).collect();
    let row = db::insert_ping_key(
        tx,
        project_id,
        organization_id,
        Uuid::new_v4(),
        &sha256_hex(key.as_bytes()),
        &shown,
    )
    .await?;
    emit_audit(
        tx,
        AuditEvent {
            organization_id,
            in_project: Some(project_id),
            actor: Actor::User(acting.user_id.as_str()),
            action: AuditAction::Created,
            resource_kind: TelmoniResourceKind::PingKey,
            resource_id: Some(&row.id.to_string()),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "prefix": row.prefix })),
        },
    )
    .await?;
    Ok(MintedPingKey {
        key,
        ping_key: PingKey::from(row),
    })
}

/// `DELETE /internal/telemetry/ping-keys/{id}` — the key retired now: every
/// ping under it a 404 from here on. One retired already is left as it is.
pub async fn delete_ping_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, TelmoniError> {
    let acting = state.auth.resolve(&headers).await?;
    let project_id = acting
        .require_project(Verb::Delete, Resource::Token)?
        .project_id
        .clone();
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let mut tx = chain::scoped(&state.db, &acting, &project_id).await?;
    let row = match retire_key(&state, &acting, &project_id, &mut tx, &headers, id).await {
        Ok(row) => row,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, &project_id, "ping key");
            return Err(refused);
        }
    };
    tx.commit().await?;
    tracing::info!(project_id = %project_id, ping_key_id = %row.id, "telemetry: a ping key retired");
    Ok(StatusCode::NO_CONTENT)
}

/// The retirement itself, inside the transaction [`delete_ping_key`]
/// opened: under the organization's chain lock, a live key put out of use
/// and recorded; one retired already is answered as it is, and nothing is
/// recorded.
async fn retire_key(
    state: &AppState,
    acting: &Acting,
    project_id: &ProjectId,
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    headers: &HeaderMap,
    id: Uuid,
) -> Result<PingKeyRow, TelmoniError> {
    let now = chain::hold(&*state.auth, acting, project_id, tx).await?;
    now.require_project(Verb::Delete, Resource::Token)?;
    let before = db::ping_key(tx, project_id, id)
        .await?
        .ok_or_else(not_found)?;
    if before.retired_at.is_some() {
        return Ok(before);
    }
    let row = db::retire_ping_key(tx, project_id, id)
        .await?
        .ok_or_else(not_found)?;
    emit_audit(
        tx,
        AuditEvent {
            organization_id: &acting.organization_id,
            in_project: Some(project_id),
            actor: Actor::User(acting.user_id.as_str()),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::PingKey,
            resource_id: Some(&row.id.to_string()),
            request_id: correlation_id(headers),
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "prefix": row.prefix })),
        },
    )
    .await?;
    Ok(row)
}

/// What one tick of the rotation did: the keys it retired, and those it left
/// for a later tick.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Rotated {
    pub retired: u32,
    pub left: u32,
}

/// What became of one stale key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Retirement {
    /// Retired, recorded and told.
    Retired,
    /// Retired already — by another replica's tick, or a person — or gone
    /// with its project: nothing to do.
    Gone,
    /// Its project is held alone, by a move to another organization or a
    /// purge: left for a later tick, which finds it where it went, or gone.
    KeptOut,
    /// Its organization's chain lock was not had in time: left for a later
    /// tick, as every other key the tick then finds standing under that
    /// organization is.
    Busy(OrganizationId),
    /// Its organization's chain lock was found busy earlier in the tick:
    /// left for a later tick without waiting on that lock again.
    PassedOver,
}

/// The rotation's tick: every live key a week past its last ping with a
/// newer live key of its project standing, retired ([`retire_each`]). Safe
/// on every replica at once: a key two ticks find is retired by the first,
/// and the second finds it retired and records and tells nothing.
pub async fn retire_stale_keys(state: &AppState) -> Result<Rotated, TelmoniError> {
    let mut tx = maintenance_scope(&state.db, TelemetryLane).await?;
    let stale = db::stale_ping_keys(&mut tx, REPLACED_AFTER_S, RETIRE_BATCH).await?;
    tx.commit().await?;
    retire_each(state, &stale).await
}

/// The tick's work on the stale keys it read, in turn: each retired in its
/// project's own scope, recorded on the chain as the module's own act and
/// told to the project. Each key on its own: one that cannot be retired now
/// — its project moving or being purged, its organization's chain lock busy
/// past its wait, its retirement refused — is logged and left for a later
/// tick, and the tick goes on to the rest, so one organization holds up its
/// own keys alone; and an organization whose lock was busy is passed over
/// for the rest of the tick, so its keys cost the tick one wait, not one
/// each. A database that stops answering ends the tick, as a failed read of
/// the keys does, since every key after it would wait it out in turn.
pub async fn retire_each(
    state: &AppState,
    stale: &[ResolvedPingKey],
) -> Result<Rotated, TelmoniError> {
    let mut rotated = Rotated::default();
    let mut busy: HashSet<OrganizationId> = HashSet::new();
    for key in stale {
        match retire_stale_key(state, key, &busy).await {
            Ok(Retirement::Retired) => rotated.retired += 1,
            Ok(Retirement::Gone) => {}
            Ok(Retirement::KeptOut | Retirement::PassedOver) => rotated.left += 1,
            Ok(Retirement::Busy(organization_id)) => {
                tracing::warn!(
                    organization_id = %organization_id,
                    "telemetry: the ping keys' rotation passed over an organization whose chain \
                     lock was busy; its stale keys wait for the next tick"
                );
                busy.insert(organization_id);
                rotated.left += 1;
            }
            Err(e) if unanswered(&e) => return Err(e),
            Err(e) => {
                tracing::warn!(
                    project_id = %key.project_id,
                    ping_key_id = %key.id,
                    error = %e,
                    "telemetry: the ping keys' rotation could not retire a key; the next tick \
                     tries again"
                );
                rotated.left += 1;
            }
        }
    }
    Ok(rotated)
}

/// Whether a key's retirement failed for want of the database — no
/// connection to be had, or the one it had lost — rather than for anything
/// about the key.
fn unanswered(error: &TelmoniError) -> bool {
    matches!(
        error,
        TelmoniError::Database(
            sqlx::Error::PoolTimedOut
                | sqlx::Error::PoolClosed
                | sqlx::Error::WorkerCrashed
                | sqlx::Error::Io(_)
                | sqlx::Error::Tls(_)
                | sqlx::Error::Protocol(_)
        )
    )
}

/// One stale key retired, in a transaction of its own: its project held in
/// place, so no move or purge of it commits meanwhile, and the organization
/// the key stands under read there — not the one the tick's read named,
/// which a move committed since would have the retirement filed under and
/// the row policy refuse — then, unless the tick found that organization's
/// chain lock busy already (`passed_over`), under that lock, retired,
/// recorded, and the project's notice put in the outbox. Told, since a line
/// not moved over to the newer key is refused from now on, and its owner
/// would otherwise learn of it from cron's mail.
async fn retire_stale_key(
    state: &AppState,
    key: &ResolvedPingKey,
    passed_over: &HashSet<OrganizationId>,
) -> Result<Retirement, TelmoniError> {
    let project_id = &key.project_id;
    let mut tx = project_scope(&state.db, project_id).await?;
    if !project_move::try_hold_in_place(tx.conn(), project_id).await? {
        chain::rolled_back(tx.rollback().await, project_id, "ping key rotation");
        tracing::debug!(
            project_id = %project_id,
            ping_key_id = %key.id,
            "telemetry: a stale ping key's project is moving to another organization or being \
             purged; a later tick retires the key where it went, or finds it gone"
        );
        return Ok(Retirement::KeptOut);
    }
    let Some(organization_id) = db::live_ping_key_organization(&mut tx, project_id, key.id).await?
    else {
        chain::rolled_back(tx.rollback().await, project_id, "ping key rotation");
        return Ok(Retirement::Gone);
    };
    if passed_over.contains(&organization_id) {
        chain::rolled_back(tx.rollback().await, project_id, "ping key rotation");
        return Ok(Retirement::PassedOver);
    }
    let mut tx = tx.bind_organization(&organization_id).await?;
    match retire_in_place(&mut tx, project_id, &organization_id, key.id).await {
        Ok(Retirement::Retired) => {
            tx.commit().await?;
            tracing::info!(
                project_id = %project_id,
                ping_key_id = %key.id,
                "telemetry: a ping key retired by its rotation, a week past its last ping"
            );
            Ok(Retirement::Retired)
        }
        Ok(left) => {
            chain::rolled_back(tx.rollback().await, project_id, "ping key rotation");
            Ok(left)
        }
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, project_id, "ping key rotation");
            Err(refused)
        }
    }
}

/// The retirement itself, inside the transaction [`retire_stale_key`]
/// opened, the project held in place and bound to the organization its key
/// stands under now.
async fn retire_in_place(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    organization_id: &OrganizationId,
    id: Uuid,
) -> Result<Retirement, TelmoniError> {
    if !chain::lock_in_time(organization_id, tx).await? {
        return Ok(Retirement::Busy(organization_id.clone()));
    }
    let Some(row) = db::retire_ping_key(tx, project_id, id).await? else {
        return Ok(Retirement::Gone);
    };
    emit_audit(
        tx,
        AuditEvent {
            organization_id,
            in_project: Some(project_id),
            actor: Actor::Service("telemetry"),
            action: AuditAction::Deleted,
            resource_kind: TelmoniResourceKind::PingKey,
            resource_id: Some(&row.id.to_string()),
            request_id: None,
            ip_address: None,
            user_agent: None,
            metadata: Some(json!({ "prefix": row.prefix, "rotation": true })),
        },
    )
    .await?;
    outbox::enqueue_notice(
        tx,
        project_id,
        organization_id,
        &notify::ping_key_retired(&row),
    )
    .await?;
    Ok(Retirement::Retired)
}
