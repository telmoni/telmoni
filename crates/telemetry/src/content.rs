//! What a project whose content mode is `on` keeps of its spans' content:
//! each field sealed under the project's content key, the span's id and the
//! field's name as associated data, in `plain_content`, and its bytes on its
//! organization's row of `content_held`. The key is a data key of the
//! project's own, made with the first content it keeps, wrapped under
//! `TELEMETRY_KEK` beside the hashing key ([`crate::hashing`]), and held in
//! memory once opened, as that one is. Content is read only through
//! [`open`], which files each read in the organization's audit chain.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain_in_time};
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::envelope::{Dek, KEK_VERSION, Sealed, SealedRef};
use telmoni_shared::{
    AuditAction, OrganizationId, ProjectId, Redacted, TelemetryError, TelmoniError,
    TelmoniResourceKind,
};

use crate::AppState;
use crate::db::{self, SealedFields, WrappedContentKey};
use crate::handler::chain;
use crate::hashing::kek_refused;
use crate::ingest::batch::SpanContent;

/// What the key's log lines and refusals call it.
const WHAT: &str = "content key";

/// What a project's content key is wrapped under, beside its id: never the
/// hashing key's, so neither wrapped key opens in the other's place.
fn key_aad(project_id: &ProjectId) -> String {
    format!("{project_id}/content")
}

/// What a field is sealed under: the span it came on and its name, so a
/// ciphertext moved to another span or field will not open there.
#[must_use]
pub fn field_aad(span_id: &str, field: &str) -> String {
    format!("{span_id}/{field}")
}

/// An export's content, sealed and kept, its bytes counted; nothing where it
/// carries none.
///
/// # Errors
///
/// Postgres or the key service did not answer, or a move or a purge held the
/// project: each one an exporter retries. A key that will not open is ours.
pub async fn keep(
    state: &AppState,
    project_id: &ProjectId,
    received_at: DateTime<Utc>,
    content: Vec<SpanContent>,
) -> Result<(), TelmoniError> {
    if content.is_empty() {
        return Ok(());
    }
    let key = key_for(state, project_id).await?;
    let mut sealed = SealedFields::default();
    for span in content {
        for (field, text) in span.fields {
            let bytes = i32::try_from(text.len()).unwrap_or(i32::MAX);
            let aad = field_aad(&span.span_id, &field);
            let Sealed { ciphertext, nonce } =
                state
                    .vault
                    .seal(&key, &Redacted::from(text), aad.as_bytes())?;
            sealed.run_ids.push(span.run_id);
            sealed.trace_ids.push(span.trace_id.clone());
            sealed.span_ids.push(span.span_id.clone());
            sealed.fields.push(field);
            sealed.ciphertexts.push(ciphertext);
            sealed.nonces.push(nonce);
            sealed.bytes.push(bytes);
        }
    }
    let (mut tx, organization_id) = held_in_place(state, project_id).await?;
    db::keep_content(&mut tx, project_id, &organization_id, received_at, &sealed).await?;
    tx.commit().await?;
    Ok(())
}

/// The traces' content, opened for `actor`, whom the caller has let read the
/// project: the one way content is read, so every read is a `Viewed` row in
/// the organization's chain, committed before a field is opened. The row
/// names the traces read, never what they hold; a read that finds nothing writes
/// none.
///
/// # Errors
///
/// As [`keep`], or the chain's lock was not had in time: a conflict to retry.
pub async fn open(
    state: &AppState,
    actor: Actor<'_>,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<SpanContent>, TelmoniError> {
    let (mut tx, organization_id) = held_in_place(state, project_id).await?;
    let kept = match file_the_read(&mut tx, &organization_id, actor, project_id, trace_ids).await {
        Ok(kept) => kept,
        Err(refused) => {
            chain::rolled_back(tx.rollback().await, project_id, WHAT);
            return Err(refused);
        }
    };
    tx.commit().await?;
    if kept.is_empty() {
        return Ok(Vec::new());
    }

    let key = key_for(state, project_id).await?;
    let mut opened: Vec<SpanContent> = Vec::new();
    for (run_id, trace_id, span_id, field, ciphertext, nonce) in kept {
        let text = state
            .vault
            .open(
                &key,
                SealedRef {
                    ciphertext: &ciphertext,
                    nonce: &nonce,
                },
                field_aad(&span_id, &field).as_bytes(),
            )?
            .expose()
            .to_owned();
        match opened.last_mut() {
            Some(span) if span.trace_id == trace_id && span.span_id == span_id => {
                span.fields.push((field, text));
            }
            _ => opened.push(SpanContent {
                run_id,
                trace_id,
                span_id,
                fields: vec![(field, text)],
            }),
        }
    }
    Ok(opened)
}

/// The traces' sealed fields, and where there are any, the `Viewed` row that
/// files their read, its chain lock waited on no longer than a lane's.
async fn file_the_read(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
    actor: Actor<'_>,
    project_id: &ProjectId,
    trace_ids: &[String],
) -> Result<Vec<db::KeptField>, TelmoniError> {
    let kept = db::read_content(tx, project_id, trace_ids).await?;
    if kept.is_empty() {
        return Ok(kept);
    }
    lock_chain_in_time(tx.conn(), organization_id).await?;
    // The traces read, not those asked for: kept is ordered by trace.
    let mut read: Vec<&str> = kept
        .iter()
        .map(|(_, trace_id, ..)| trace_id.as_str())
        .collect();
    read.dedup();
    let project = project_id.to_string();
    emit_audit(
        tx,
        AuditEvent::new(
            organization_id,
            actor,
            AuditAction::Viewed,
            TelmoniResourceKind::Project,
        )
        .in_project(project_id)
        .on(&project)
        .details(json!({ "trace_ids": read, "fields": kept.len() })),
    )
    .await?;
    Ok(kept)
}

/// The project's content key, opened: from the cache, else from its settings
/// row, made and wrapped where the row has none, and then cached.
///
/// # Errors
///
/// As [`keep`].
pub async fn key_for(state: &AppState, project_id: &ProjectId) -> Result<Arc<Dek>, TelmoniError> {
    if let Some(key) = state.content_keys.get(project_id) {
        return Ok(key);
    }
    let aad = key_aad(project_id);
    let mut tx = project_scope(&state.db, project_id).await?;
    let read = db::content_key(&mut tx, project_id).await?;
    tx.commit().await?;
    let wrapped = match read {
        Some(wrapped) => wrapped,
        None => make_key(state, project_id, aad.as_bytes()).await?,
    };
    let key = state
        .kek
        .unwrap_dek(
            &state.http,
            &wrapped.wrapped_dek,
            aad.as_bytes(),
            wrapped.key_version,
        )
        .await
        .map_err(|e| kek_refused(&e, WHAT))?;
    let key = Arc::new(key);
    state.content_keys.put(project_id.clone(), Arc::clone(&key));
    Ok(key)
}

/// A fresh data key for the project, wrapped first, holding nothing, since the
/// key service is across the network; then written where the row has none.
/// The key is ours, or the one a racing export wrote first.
async fn make_key(
    state: &AppState,
    project_id: &ProjectId,
    aad: &[u8],
) -> Result<WrappedContentKey, TelmoniError> {
    let dek = state.vault.new_dek()?;
    let wrapped = WrappedContentKey {
        wrapped_dek: state
            .kek
            .wrap(&state.http, &dek, aad)
            .await
            .map_err(|e| kek_refused(&e, WHAT))?,
        key_version: KEK_VERSION,
    };
    let (mut tx, _) = held_in_place(state, project_id).await?;
    db::set_content_key_if_none(&mut tx, project_id, &wrapped).await?;
    let written = db::content_key(&mut tx, project_id).await?;
    tx.commit().await?;
    written.ok_or_else(|| {
        TelmoniError::Internal("telemetry: the content key just written is not there".into())
    })
}

/// The project's scope bound to the organization its settings row is filed
/// under, the project held in place so no move passes before the commit. The
/// row is the export's own, made by its hashing key before any content.
async fn held_in_place<'c>(
    state: &'c AppState,
    project_id: &ProjectId,
) -> Result<(Scoped<'c, ProjectAndOrganization>, OrganizationId), TelmoniError> {
    let mut tx = project_scope(&state.db, project_id).await?;
    if !project_move::try_hold_in_place(tx.conn(), project_id).await? {
        chain::rolled_back(tx.rollback().await, project_id, WHAT);
        return Err(TelemetryError::StoreUnavailable(
            "the project is moving to another organization, or being deleted; try again".into(),
        )
        .into());
    }
    let Some(organization_id) = db::settings_organization(&mut tx, project_id).await? else {
        chain::rolled_back(tx.rollback().await, project_id, WHAT);
        return Err(TelemetryError::StoreUnavailable(
            "the project's settings were deleted as it was reached; try again".into(),
        )
        .into());
    };
    let tx = tx.bind_organization(&organization_id).await?;
    Ok((tx, organization_id))
}
