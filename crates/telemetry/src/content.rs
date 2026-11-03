//! What a project whose content mode is `on` keeps of its spans' content:
//! each field compressed, then sealed under the project's content key, the
//! span's id and the field's name as associated data, in `plain_content`, and
//! its sealed bytes on its organization's row of `content_held`. The key is a
//! data key of the project's own, made with the first content it keeps,
//! wrapped under `CONTENT_KEK`, never the hashing key's `TELEMETRY_KEK`, and
//! held in memory once opened, as the hashing key is ([`crate::hashing`]).
//! Deleting the settings row deletes the key, so nothing reads the content
//! from that commit, but a backup taken before. Content is read only through
//! [`open`], which files each read in the organization's audit chain.

use std::io::Read;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain_in_time};
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::digest::{HmacKey, hex_decode};
use telmoni_shared::envelope::{Dek, KEK_VERSION, Sealed, SealedRef, Vault};
use telmoni_shared::{
    AuditAction, OrganizationId, ProjectId, Redacted, TelemetryError, TelmoniError,
    TelmoniResourceKind,
};

use crate::AppState;
use crate::contract::BATCH_MAX_BYTES;
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

/// `text` compressed and sealed under `key`, bound to `aad`: a ciphertext does
/// not compress, so a field is compressed before it is sealed.
pub(crate) fn seal_field(
    vault: &Vault,
    key: &Dek,
    text: &Redacted,
    aad: &str,
) -> Result<Sealed, TelmoniError> {
    let packed = zstd::bulk::compress(text.expose().as_bytes(), zstd::DEFAULT_COMPRESSION_LEVEL)
        .map_err(|_| TelmoniError::Internal("content: a field would not compress".into()))?;
    vault.seal_bytes(key, &packed, aad.as_bytes())
}

/// The most a field decompresses to: one export's body, the JSON ingest
/// writes of its events, arrays and maps a few times longer at most.
const FIELD_MAX: usize = 16 * BATCH_MAX_BYTES;

/// A field [`keep`] sealed, opened under `key` and `aad` and decompressed back
/// to its text, never past `FIELD_MAX`.
///
/// # Errors
///
/// The field does not open under them, or does not decompress to text.
pub fn open_field(
    vault: &Vault,
    key: &Dek,
    sealed: SealedRef<'_>,
    aad: &str,
) -> Result<String, TelmoniError> {
    let packed = vault.open_bytes(key, sealed, aad.as_bytes())?;
    let cap = u64::try_from(FIELD_MAX).unwrap_or(u64::MAX);
    let mut text = String::new();
    zstd::Decoder::with_buffer(packed.as_slice())
        .and_then(|decoder| {
            decoder
                .take(cap.saturating_add(1))
                .read_to_string(&mut text)
        })
        .map_err(|_| TelmoniError::Internal("content: a field would not decompress".into()))?;
    if text.len() > FIELD_MAX {
        return Err(TelmoniError::Internal(
            "content: a field decompresses past its cap".into(),
        ));
    }
    Ok(text)
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
    let (wrapped, key) = opened_key(state, project_id).await?;
    let words_key = words_key(&key);
    let mut sealed = SealedFields::default();
    let mut named: (Vec<String>, Vec<Uuid>) = (Vec::new(), Vec::new());
    for span in content {
        // A comment or an example names no file: a hash in it would hold one
        // from its sweep.
        let names_files = !span.span_id.starts_with(crate::scores::COMMENT_SPAN_PREFIX)
            && !span
                .span_id
                .starts_with(crate::evaluators::examples::SPAN_PREFIX);
        for (field, text) in span.fields {
            for hash in files_named(&text).into_iter().filter(|_| names_files) {
                named.0.push(hash);
                named.1.push(span.run_id);
            }
            let (words, pairs) = hashed_words(&words_key, &text);
            sealed.words.push(json!(words).to_string());
            sealed.pairs.push(json!(pairs).to_string());
            let aad = field_aad(&span.span_id, &field);
            let Sealed { ciphertext, nonce } =
                seal_field(&state.vault, &key, &Redacted::from(text), &aad)?;
            let bytes = i32::try_from(ciphertext.len()).unwrap_or(i32::MAX);
            sealed.run_ids.push(span.run_id);
            sealed.trace_ids.push(span.trace_id.clone());
            sealed.span_ids.push(span.span_id.clone());
            sealed.fields.push(field);
            sealed.ciphertexts.push(ciphertext);
            sealed.nonces.push(nonce);
            sealed.bytes.push(bytes);
        }
    }
    let (tx, organization_id) = held_in_place(state, project_id).await?;
    chain::settle(tx, project_id, WHAT, async |tx| {
        still_under(tx, project_id, &wrapped.wrapped_dek, "its content").await?;
        db::keep_content(
            tx,
            project_id,
            &organization_id,
            received_at,
            wrapped.id,
            &sealed,
        )
        .await?;
        if !named.0.is_empty() {
            db::name_media(tx, project_id, received_at, (&named.0, &named.1)).await?;
        }
        Ok(())
    })
    .await
}

/// A score's comment kept in place of the one it had, sealed as a span's
/// field is under the score's run ([`crate::scores::comment_span`]), so a
/// trace's deletion, an end user's erasure and retention take it as they take
/// the run's content, and opened only with the run's, as its read; none kept
/// for `None`, the last one taken back.
///
/// # Errors
///
/// As [`keep`].
pub async fn keep_comment(
    state: &AppState,
    project_id: &ProjectId,
    (run_id, trace_id): (Uuid, &str),
    score_id: Uuid,
    comment: Option<&str>,
) -> Result<(), TelmoniError> {
    forget_comments(state, project_id, run_id, &[score_id]).await?;
    // A run deleted as it was judged or scored keeps nothing it said after.
    let Some(comment) = comment else {
        return Ok(());
    };
    if crate::erasure::was_deleted(state, project_id, run_id).await {
        return Ok(());
    }
    keep(
        state,
        project_id,
        Utc::now(),
        vec![SpanContent {
            run_id,
            trace_id: trace_id.to_owned(),
            span_id: crate::scores::comment_span(score_id),
            fields: vec![(crate::scores::COMMENT_FIELD.to_owned(), comment.to_owned())],
        }],
    )
    .await
}

/// The project's fields under the spans `spans` names of `run_id` deleted,
/// their bytes given back.
///
/// # Errors
///
/// As [`keep`].
pub async fn forget_named(
    state: &AppState,
    project_id: &ProjectId,
    run_id: Uuid,
    spans: &[String],
) -> Result<(), TelmoniError> {
    let mut tx =
        telmoni_shared::db::tenant_session::maintenance_scope(&state.db, crate::db::TelemetryLane)
            .await?;
    db::forget_fields(tx.conn(), project_id, db::Spans::Named(run_id, spans)).await?;
    tx.commit().await?;
    Ok(())
}

/// The comments of `score_ids` deleted, their bytes given back.
///
/// # Errors
///
/// As [`keep`].
pub async fn forget_comments(
    state: &AppState,
    project_id: &ProjectId,
    run_id: Uuid,
    score_ids: &[Uuid],
) -> Result<(), TelmoniError> {
    let spans: Vec<String> = score_ids
        .iter()
        .copied()
        .map(crate::scores::comment_span)
        .collect();
    forget_named(state, project_id, run_id, &spans).await
}

/// The project's fields named `field` under every span whose id begins with
/// `prefix` deleted, their bytes given back.
///
/// # Errors
///
/// As [`keep`].
pub async fn forget_prefixed(
    state: &AppState,
    project_id: &ProjectId,
    field: &str,
    prefix: &str,
) -> Result<(), TelmoniError> {
    let mut tx =
        telmoni_shared::db::tenant_session::maintenance_scope(&state.db, crate::db::TelemetryLane)
            .await?;
    db::forget_fields(tx.conn(), project_id, db::Spans::Prefixed(field, prefix)).await?;
    tx.commit().await?;
    Ok(())
}

/// The newest `limit` fields named `field` under spans whose id begins with
/// `prefix`, opened for an evaluation: its read recorded on its own row, as
/// [`open_for_evaluation`]'s is. Newest first.
///
/// # Errors
///
/// As [`keep`].
pub async fn open_prefixed_for_evaluation(
    state: &AppState,
    project_id: &ProjectId,
    field: &str,
    prefix: &str,
    limit: i64,
) -> Result<Vec<String>, TelmoniError> {
    let Some((mut tx, _)) = held_if_kept(state, project_id).await? else {
        return Ok(Vec::new());
    };
    let kept = db::read_prefixed(&mut tx, project_id, field, prefix, limit).await?;
    tx.commit().await?;
    if kept.is_empty() {
        return Ok(Vec::new());
    }
    let key = key_for(state, project_id).await?;
    kept.into_iter()
        .map(|(_, _, span_id, field, ciphertext, nonce)| {
            open_field(
                &state.vault,
                &key,
                SealedRef {
                    ciphertext: &ciphertext,
                    nonce: &nonce,
                },
                &field_aad(&span_id, &field),
            )
        })
        .collect()
}

/// Refuses what was sealed under `wrapped` once the project's content key is
/// another: one deleted meanwhile, as the project went `off`, or made again,
/// and the exporter's retry meets the mode. `kept` names what was sealed.
async fn still_under(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    project_id: &ProjectId,
    wrapped: &[u8],
    kept: &str,
) -> Result<(), TelmoniError> {
    let now = db::content_key(tx, project_id).await?;
    if now.is_none_or(|now| now.wrapped_dek != wrapped) {
        return Err(TelemetryError::StoreUnavailable(format!(
            "the project's content key changed as {kept} was kept; try again"
        ))
        .into());
    }
    // The mode too, under the same lock: an export that read `on` before a
    // switch to `off` committed would otherwise keep its content, under a key
    // minted for an `off` project, which the retry's answer then refuses.
    if db::content_mode(tx, project_id).await? != telmoni_shared::ContentMode::On {
        return Err(TelemetryError::StoreUnavailable(format!(
            "the project's content mode changed as {kept} was kept; try again"
        ))
        .into());
    }
    Ok(())
}

/// The runs' content, or that of the spans `spans` names, opened for
/// `actor`, whom the caller has let read the project: the one way content is
/// read, so every read is a `Viewed` row in the organization's chain,
/// committed before a field is opened. The row names the traces read, never
/// what they hold; a read that finds nothing writes none.
///
/// # Errors
///
/// As [`keep`], or the chain's lock was not had in time: a conflict to retry.
pub async fn open(
    state: &AppState,
    actor: Actor<'_>,
    project_id: &ProjectId,
    runs: &[Uuid],
    spans: Option<&[String]>,
) -> Result<Vec<SpanContent>, TelmoniError> {
    // No settings row, no content: a project that never kept any.
    let Some((tx, organization_id)) = held_if_kept(state, project_id).await? else {
        return Ok(Vec::new());
    };
    let kept = chain::settle(tx, project_id, WHAT, async |tx| {
        file_the_read(tx, &organization_id, actor, project_id, runs, spans).await
    })
    .await?;
    if kept.is_empty() {
        return Ok(Vec::new());
    }

    opened(state, project_id, kept).await
}

/// The runs' content, or that of the spans `spans` names, opened for an
/// evaluation: its read recorded on the evaluation's own row by the caller,
/// never a chain row each, which a machine's load would hold the
/// organization's chain under.
///
/// # Errors
///
/// As [`keep`].
pub async fn open_for_evaluation(
    state: &AppState,
    project_id: &ProjectId,
    runs: &[Uuid],
    spans: Option<&[String]>,
) -> Result<Vec<SpanContent>, TelmoniError> {
    let Some((mut tx, _)) = held_if_kept(state, project_id).await? else {
        return Ok(Vec::new());
    };
    let kept = db::read_content(&mut tx, project_id, runs, spans).await?;
    tx.commit().await?;
    if kept.is_empty() {
        return Ok(Vec::new());
    }
    opened(state, project_id, kept).await
}

/// `kept`, each field opened under the project's key, grouped by span.
async fn opened(
    state: &AppState,
    project_id: &ProjectId,
    kept: Vec<db::KeptField>,
) -> Result<Vec<SpanContent>, TelmoniError> {
    let key = key_for(state, project_id).await?;
    let mut opened: Vec<SpanContent> = Vec::new();
    for (run_id, trace_id, span_id, field, ciphertext, nonce) in kept {
        let text = open_field(
            &state.vault,
            &key,
            SealedRef {
                ciphertext: &ciphertext,
                nonce: &nonce,
            },
            &field_aad(&span_id, &field),
        )?;
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

/// The runs' sealed fields, and where there are any, the `Viewed` row that
/// files their read, its chain lock waited on no longer than a lane's.
async fn file_the_read(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
    actor: Actor<'_>,
    project_id: &ProjectId,
    runs: &[Uuid],
    spans: Option<&[String]>,
) -> Result<Vec<db::KeptField>, TelmoniError> {
    let kept = db::read_content(tx, project_id, runs, spans).await?;
    if kept.is_empty() {
        return Ok(kept);
    }
    // The traces read, not those asked for: kept is ordered by trace.
    let mut read: Vec<&str> = kept
        .iter()
        .map(|(_, trace_id, ..)| trace_id.as_str())
        .collect();
    read.dedup();
    let read = json!({ "trace_ids": read, "fields": kept.len() });
    file_viewed(tx, organization_id, actor, project_id, read).await?;
    Ok(kept)
}

/// The `Viewed` row that files a read of the project's content, `read`
/// naming what was read and never what it holds, its chain lock waited on no
/// longer than a lane's.
async fn file_viewed(
    tx: &mut Scoped<'_, ProjectAndOrganization>,
    organization_id: &OrganizationId,
    actor: Actor<'_>,
    project_id: &ProjectId,
    read: serde_json::Value,
) -> Result<(), TelmoniError> {
    lock_chain_in_time(tx.conn(), organization_id).await?;
    emit_audit(
        tx,
        AuditEvent::new(
            organization_id,
            actor,
            AuditAction::Viewed,
            TelmoniResourceKind::Project,
        )
        .in_project(project_id)
        .on(project_id.as_str())
        .details(read),
    )
    .await?;
    Ok(())
}

/// The project's content key, opened: read from its settings row each time,
/// the key service asked only when the row holds a key not yet opened.
///
/// # Errors
///
/// As [`keep`].
pub async fn key_for(state: &AppState, project_id: &ProjectId) -> Result<Arc<Dek>, TelmoniError> {
    Ok(opened_key(state, project_id).await?.1)
}

/// The project's content key as its settings row holds it now, wrapped, and
/// opened — made and wrapped where the row has none. The opened key comes
/// from the cache only while the row still holds the key it was opened from,
/// so one deleted as content went `off`, or replaced since, is used by no
/// process: the row is read every time, the key service only on a change.
async fn opened_key(
    state: &AppState,
    project_id: &ProjectId,
) -> Result<(WrappedContentKey, Arc<Dek>), TelmoniError> {
    let aad = key_aad(project_id);
    let mut tx = project_scope(&state.db, project_id).await?;
    let read = db::content_key(&mut tx, project_id).await?;
    tx.commit().await?;
    let wrapped = match read {
        Some(wrapped) => wrapped,
        None => make_key(state, project_id, aad.as_bytes()).await?,
    };
    let key = unwrapped(state, project_id, &wrapped).await?;
    Ok((wrapped, key))
}

/// `wrapped`, opened: from the cache while it holds the key `wrapped` is,
/// else by the key service, then cached.
async fn unwrapped(
    state: &AppState,
    project_id: &ProjectId,
    wrapped: &WrappedContentKey,
) -> Result<Arc<Dek>, TelmoniError> {
    if let Some((held, key)) = state.content_keys.get(project_id)
        && held == wrapped.wrapped_dek
    {
        return Ok(key);
    }
    let aad = key_aad(project_id);
    let key = state
        .content_kek
        .unwrap_dek(
            &state.http,
            &wrapped.wrapped_dek,
            aad.as_bytes(),
            wrapped.key_version,
        )
        .await
        .map_err(|e| kek_refused(&e, WHAT))?;
    let key = Arc::new(key);
    state.content_keys.put(
        project_id.clone(),
        (wrapped.wrapped_dek.clone(), Arc::clone(&key)),
    );
    Ok(key)
}

/// What a message's `file` part names a file kept here by: this, then the
/// SHA-256 of its bytes in lowercase hex.
pub const FILE_ID_PREFIX: &str = "telmoni:";

/// The largest file a project keeps, in bytes.
pub const MEDIA_MAX: usize = 5 * 1024 * 1024;

/// Whether `text` names a file as one is kept here: the SHA-256 of its bytes
/// in lowercase hex, as `sha256_hex` writes it.
#[must_use]
pub fn is_file_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The hashes of the files a field's `file` parts name as kept here, each
/// once, in order.
#[must_use]
pub fn files_named(text: &str) -> Vec<String> {
    let mut hashes: Vec<String> = Vec::new();
    for after in text.split(FILE_ID_PREFIX).skip(1) {
        let hash: String = after.chars().take(64).collect();
        if is_file_hash(&hash) && !hashes.contains(&hash) {
            hashes.push(hash);
        }
    }
    hashes
}

/// A file a message carried, kept for an `on` project: sealed under the
/// project's content key, its hash as associated data, once however many
/// spans send it. Nothing for a project that keeps no content.
///
/// # Errors
///
/// As [`keep`].
pub async fn keep_media(
    state: &AppState,
    project_id: &ProjectId,
    (sha256, mime_type): (&str, &str),
    body: &[u8],
) -> Result<(), TelmoniError> {
    if db::content_mode_of(&state.db, project_id).await? != telmoni_shared::ContentMode::On {
        return Ok(());
    }
    let (wrapped, key) = opened_key(state, project_id).await?;
    let Sealed { ciphertext, nonce } =
        state
            .vault
            .seal_bytes(&key, body, media_aad(sha256).as_bytes())?;
    let bytes = i32::try_from(body.len()).unwrap_or(i32::MAX);
    let (tx, organization_id) = held_in_place(state, project_id).await?;
    chain::settle(tx, project_id, WHAT, async |tx| {
        still_under(tx, project_id, &wrapped.wrapped_dek, "the file").await?;
        db::keep_media(
            tx,
            (project_id, &organization_id),
            sha256,
            mime_type,
            (&ciphertext, &nonce),
            bytes,
            wrapped.id,
        )
        .await?;
        Ok(())
    })
    .await
}

/// What a file is sealed under, beside the key: its hash, so bytes moved to
/// another row open as nothing.
fn media_aad(sha256: &str) -> String {
    format!("media/{sha256}")
}

/// A file the project keeps, opened for `actor`, its read filed in the
/// organization's chain before it is opened, as a field's is ([`open`]): its
/// type and bytes, or none where the project keeps no such file.
///
/// # Errors
///
/// As [`open`].
pub async fn open_media(
    state: &AppState,
    actor: Actor<'_>,
    project_id: &ProjectId,
    sha256: &str,
) -> Result<Option<(String, Vec<u8>)>, TelmoniError> {
    let Some((tx, organization_id)) = held_if_kept(state, project_id).await? else {
        return Ok(None);
    };
    let kept = chain::settle(tx, project_id, WHAT, async |tx| {
        let kept = db::read_media(tx, project_id, sha256).await?;
        if kept.is_some() {
            let read = json!({ "media": sha256 });
            file_viewed(tx, &organization_id, actor, project_id, read).await?;
        }
        Ok(kept)
    })
    .await?;
    let Some((mime_type, ciphertext, nonce)) = kept else {
        return Ok(None);
    };
    unsealed_media(state, project_id, sha256, (mime_type, &ciphertext, &nonce))
        .await
        .map(Some)
}

/// A file the project keeps, opened for an evaluation with no chain row: the
/// evaluation's row records the read of its target, the files it names among
/// it, as [`open_for_evaluation`]'s.
///
/// # Errors
///
/// As [`open_for_evaluation`].
pub async fn open_media_for_evaluation(
    state: &AppState,
    project_id: &ProjectId,
    sha256: &str,
) -> Result<Option<(String, Vec<u8>)>, TelmoniError> {
    let Some((mut tx, _)) = held_if_kept(state, project_id).await? else {
        return Ok(None);
    };
    let kept = db::read_media(&mut tx, project_id, sha256).await?;
    tx.commit().await?;
    let Some((mime_type, ciphertext, nonce)) = kept else {
        return Ok(None);
    };
    unsealed_media(state, project_id, sha256, (mime_type, &ciphertext, &nonce))
        .await
        .map(Some)
}

/// A kept file's type and size, none where the project keeps no such file
/// or no content: what a judgement weighs before it opens one.
///
/// # Errors
///
/// Postgres did not answer.
pub async fn media_kind(
    state: &AppState,
    project_id: &ProjectId,
    sha256: &str,
) -> Result<Option<(String, usize)>, TelmoniError> {
    let Some((mut tx, _)) = held_if_kept(state, project_id).await? else {
        return Ok(None);
    };
    let kind = db::media_kind(&mut tx, project_id, sha256).await?;
    tx.commit().await?;
    Ok(kind.map(|(mime_type, bytes)| (mime_type, usize::try_from(bytes).unwrap_or(usize::MAX))))
}

/// A kept file's bytes opened under the project's key, its hash the associated data.
async fn unsealed_media(
    state: &AppState,
    project_id: &ProjectId,
    sha256: &str,
    (mime_type, ciphertext, nonce): (String, &[u8], &[u8]),
) -> Result<(String, Vec<u8>), TelmoniError> {
    let key = key_for(state, project_id).await?;
    let body = state.vault.open_bytes(
        &key,
        SealedRef { ciphertext, nonce },
        media_aad(sha256).as_bytes(),
    )?;
    Ok((mime_type, body))
}

/// The most fields a search reads to find its runs, newest first.
pub const SEARCH_FIELDS_MAX: i64 = 5_000;

/// The most distinct words a field is found by: a document longer than this
/// is found by its first ones.
const WORDS_MAX: usize = 4_096;

/// The longest word kept, in characters; a longer one is cut to it, as its
/// query is.
const WORD_MAX: usize = 64;

/// What the key words are hashed under is derived for: one key a purpose.
const WORDS_LABEL: &[u8] = b"telmoni content words";

/// The key a project's words are hashed under, derived from its content key,
/// so deleting that key leaves every stored hash matching nothing.
fn words_key(key: &Dek) -> HmacKey {
    HmacKey::new(HmacKey::new(key.as_bytes()).hex(WORDS_LABEL).as_bytes())
}

/// A field's words in order, lower-cased, each a run of letters and digits:
/// a JSON field's string values alone, so a message's own keys (`role`,
/// `parts`) find nothing, and any other text whole.
#[must_use]
pub fn words_of(text: &str) -> Vec<String> {
    fn strings<'a>(value: &'a serde_json::Value, into: &mut Vec<&'a str>) {
        match value {
            serde_json::Value::String(s) => into.push(s),
            serde_json::Value::Array(items) => items.iter().for_each(|v| strings(v, into)),
            serde_json::Value::Object(map) => map.values().for_each(|v| strings(v, into)),
            _ => {}
        }
    }
    let parsed: Option<serde_json::Value> = serde_json::from_str(text).ok();
    let mut sources = Vec::new();
    match &parsed {
        Some(value @ (serde_json::Value::Array(_) | serde_json::Value::Object(_))) => {
            strings(value, &mut sources);
        }
        _ => sources.push(text),
    }
    sources
        .iter()
        .flat_map(|source| source.split(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase().chars().take(WORD_MAX).collect())
        .collect()
}

/// A field's words and each pair of neighbouring words, hashed for its
/// `words` and `pairs` columns: the first [`WORDS_MAX`] distinct of each, in
/// its order.
fn hashed_words(key: &HmacKey, text: &str) -> (Vec<i64>, Vec<i64>) {
    let words = words_of(text);
    let pairs = pairs_of(&words);
    let hashed = |items: Vec<String>| {
        first_distinct(items)
            .iter()
            .map(|item| hash_word(key, item))
            .collect()
    };
    (hashed(words), hashed(pairs))
}

/// What a search asks of a field: each word of `query` among its `words`, and
/// each pair of its neighbours among its `pairs`, so a phrase is matched by
/// hashes alone.
fn query_hashes(key: &HmacKey, query: &[String]) -> (Vec<i64>, Vec<i64>) {
    let hashed = |items: &[String]| {
        let mut hashed: Vec<i64> = items.iter().map(|item| hash_word(key, item)).collect();
        hashed.sort_unstable();
        hashed.dedup();
        hashed
    };
    (hashed(query), hashed(&pairs_of(query)))
}

/// Each word with the one after it. A word holds no space, so a pair names
/// its two words and no others.
fn pairs_of(words: &[String]) -> Vec<String> {
    words
        .windows(2)
        .filter_map(|pair| match pair {
            [first, second] => Some(format!("{first} {second}")),
            _ => None,
        })
        .collect()
}

/// The first [`WORDS_MAX`] distinct of `items`, in their order.
fn first_distinct(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .take(WORDS_MAX)
        .collect()
}

/// A word's, or a pair's, first 64 bits of keyed hash: enough to name it within
/// a project, where a collision only finds a field that lacks it.
fn hash_word(key: &HmacKey, word: &str) -> i64 {
    let bytes = hex_decode(&key.prefix_hex(word.as_bytes(), 8)).unwrap_or_default();
    <[u8; 8]>::try_from(bytes.as_slice()).map_or(0, i64::from_be_bytes)
}

/// The runs whose kept content holds every word of `query`, and each of its
/// words beside the next where it has more than one, newest first: what the
/// trace list narrows to. Matched by keyed hashes alone, so no field is opened
/// and nothing is filed; none where the project has no content key.
///
/// # Errors
///
/// Postgres or the key service did not answer.
pub async fn search(
    state: &AppState,
    project_id: &ProjectId,
    query: &str,
) -> Result<Vec<Uuid>, TelmoniError> {
    let phrase = words_of(query);
    if phrase.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = project_scope(&state.db, project_id).await?;
    let Some(wrapped) = db::content_key(&mut tx, project_id).await? else {
        tx.commit().await?;
        return Ok(Vec::new());
    };
    let key = unwrapped(state, project_id, &wrapped).await?;
    let (words, pairs) = query_hashes(&words_key(&key), &phrase);
    let found =
        db::search_content(&mut tx, project_id, (&words, &pairs), SEARCH_FIELDS_MAX).await?;
    tx.commit().await?;
    let mut runs: Vec<Uuid> = Vec::new();
    for run_id in found {
        if !runs.contains(&run_id) {
            runs.push(run_id);
        }
    }
    Ok(runs)
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
        id: Uuid::new_v4(),
        wrapped_dek: state
            .content_kek
            .wrap(&state.http, &dek, aad)
            .await
            .map_err(|e| kek_refused(&e, WHAT))?,
        key_version: KEK_VERSION,
    };
    let (tx, _) = held_in_place(state, project_id).await?;
    let written = chain::settle(tx, project_id, WHAT, async |tx| {
        db::set_content_key_if_none(tx, project_id, &wrapped).await?;
        Ok(db::content_key(tx, project_id).await?)
    })
    .await?;
    // No row took the key: the project went `off` as it was made, so the
    // export is refused and retried, to be answered under the mode then.
    written.ok_or_else(|| {
        TelemetryError::StoreUnavailable(
            "the project's content mode changed as its key was made; try again".into(),
        )
        .into()
    })
}

/// The project's scope bound to the organization its settings row is filed
/// under, the project held in place so no move passes before the commit; none
/// where it has no settings row, which holds no content then.
async fn held_if_kept<'c>(
    state: &'c AppState,
    project_id: &ProjectId,
) -> Result<Option<(Scoped<'c, ProjectAndOrganization>, OrganizationId)>, TelmoniError> {
    let mut tx = chain::hold_project(&state.db, project_id, WHAT).await?;
    let Some(organization_id) = db::settings_organization(&mut tx, project_id).await? else {
        chain::rolled_back(tx.rollback().await, project_id, WHAT);
        return Ok(None);
    };
    let tx = tx.bind_organization(&organization_id).await?;
    Ok(Some((tx, organization_id)))
}

/// [`held_if_kept`] for a write: the row is the export's own, made by its
/// hashing key before any content, so its absence is a purge that passed.
async fn held_in_place<'c>(
    state: &'c AppState,
    project_id: &ProjectId,
) -> Result<(Scoped<'c, ProjectAndOrganization>, OrganizationId), TelmoniError> {
    held_if_kept(state, project_id).await?.ok_or_else(|| {
        TelemetryError::StoreUnavailable(
            "the project's settings were deleted as it was reached; try again".into(),
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field's words are its letters and digits, lower-cased, in order; a
    /// JSON field's string values alone, never its keys.
    #[test]
    fn words_are_read_from_text_and_from_a_json_fields_values_alone() {
        assert_eq!(
            words_of("Refund the ORDER, #4521!"),
            ["refund", "the", "order", "4521"]
        );
        let messages =
            r#"[{"role":"user","parts":[{"type":"text","content":"Where is my order?"}]}]"#;
        // An object's values by its keys' order, each value's words in theirs.
        assert_eq!(
            words_of(messages),
            ["where", "is", "my", "order", "text", "user"]
        );
        assert_eq!(words_of("naïve café"), ["naïve", "café"]);
        assert!(words_of("  ,;  ").is_empty());
        assert_eq!(words_of(&"x".repeat(100))[0].chars().count(), WORD_MAX);
    }

    /// The files a field names as kept here, each once: a `telmoni:` and a
    /// whole lowercase hash, never a part of one.
    #[test]
    fn a_field_names_the_files_kept_here_by_their_whole_hash() {
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let text = format!(
            r#"[{{"file_id":"telmoni:{a}"}},{{"file_id":"telmoni:{a}"}},"telmoni:{b}","telmoni:{}","file-x"]"#,
            "c".repeat(10)
        );
        assert_eq!(files_named(&text), [a, b]);
        assert!(files_named(&format!("telmoni:{}", "A".repeat(64))).is_empty());
        assert!(!is_file_hash(&"A".repeat(64)));
    }

    /// ⚠ What the SDKs put and name (`sdk/fixtures/media.json`) is what is
    /// kept here: at the lane's path, under its cap, each file named in what
    /// they sent under this prefix.
    #[test]
    fn the_sdks_files_are_put_and_named_as_kept_here() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/fixtures/media.json");
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("media.json"))
                .expect("media.json is JSON");
        assert_eq!(fixture["max_bytes"], MEDIA_MAX);
        let lane = crate::handler::v1::V1_LANES
            .iter()
            .find(|lane| lane.op == crate::handler::v1::V1Op::Media)
            .map(|lane| lane.route.path.trim_end_matches("{sha256}"))
            .expect("the media lane");
        let mut put: Vec<String> = fixture["uploads"]
            .as_array()
            .expect("uploads")
            .iter()
            .map(|upload| upload["path"].as_str().and_then(|p| p.strip_prefix(lane)))
            .map(|hash| hash.expect("an upload at the lane").to_owned())
            .collect();
        assert!(put.iter().all(|hash| is_file_hash(hash)), "{put:?}");
        let mut named = files_named(&fixture["sent"].to_string());
        put.sort();
        named.sort();
        assert_eq!(named, put);
    }

    /// ⚠ A field opens as it was sealed, compressed between: an empty one, a
    /// line, and one past an export's body, as ingest's JSON of events may be;
    /// one past the cap does not, nor bytes sealed without being compressed.
    #[test]
    fn a_field_opens_as_it_was_sealed() {
        let vault = Vault::new();
        let key = vault.new_dek().expect("a key");
        let open = |sealed: &Sealed| {
            open_field(
                &vault,
                &key,
                SealedRef {
                    ciphertext: &sealed.ciphertext,
                    nonce: &sealed.nonce,
                },
                "span/field",
            )
        };
        let seal = |text: &str| {
            seal_field(&vault, &key, &Redacted::from(text), "span/field").expect("sealed")
        };
        let long = "event ".repeat(BATCH_MAX_BYTES / 2);
        for text in ["", "Where is my refund?", long.as_str()] {
            assert_eq!(open(&seal(text)).expect("opened"), text);
        }
        assert!(open(&seal(&"a".repeat(FIELD_MAX + 1))).is_err());
        let raw = vault
            .seal_bytes(&key, b"Where is my refund?", b"span/field")
            .expect("sealed");
        assert!(open(&raw).is_err());
    }

    /// A phrase is found where each of its words stands beside the next, in
    /// that order, by the field's hashes alone.
    #[test]
    fn a_phrase_is_found_by_its_neighbouring_pairs_alone() {
        let vault = telmoni_shared::envelope::Vault::new();
        let key = words_key(&vault.new_dek().expect("a key"));
        let (words, pairs) = hashed_words(&key, "where is my order");
        let finds = |query: &str| {
            let (asked, asked_pairs) = query_hashes(&key, &words_of(query));
            asked.iter().all(|hash| words.contains(hash))
                && asked_pairs.iter().all(|hash| pairs.contains(hash))
        };
        assert!(finds("my order") && finds("where is my"));
        assert!(!finds("order my"), "out of order");
        assert!(!finds("where order"), "not side by side");
    }

    /// ⚠ A field longer than the cap is found by its first distinct words and
    /// pairs, as it reads, never by the alphabet's.
    #[test]
    fn a_long_field_is_found_by_its_first_words() {
        let vault = telmoni_shared::envelope::Vault::new();
        let key = words_key(&vault.new_dek().expect("a key"));
        let mut text: Vec<String> = (0..WORDS_MAX).map(|n| format!("w{n:04}")).collect();
        text.push("a".to_owned());
        let (words, pairs) = hashed_words(&key, &text.join(" "));
        assert_eq!((words.len(), pairs.len()), (WORDS_MAX, WORDS_MAX));
        assert!(words.contains(&hash_word(&key, &format!("w{:04}", WORDS_MAX - 1))));
        assert!(
            !words.contains(&hash_word(&key, "a")),
            "a word past the cap"
        );
    }

    /// Words are hashed under a key of the content key's, never alike under
    /// two, and each once.
    #[test]
    fn words_are_hashed_once_under_a_key_derived_from_the_content_key() {
        let vault = telmoni_shared::envelope::Vault::new();
        let (one, two) = (
            vault.new_dek().expect("a key"),
            vault.new_dek().expect("a key"),
        );
        let (words, pairs) = hashed_words(&words_key(&one), "order the order");
        // `order` and `the`; the pairs `order the` and `the order`.
        assert_eq!((words.len(), pairs.len()), (2, 2));
        assert_ne!(
            (words, pairs),
            hashed_words(&words_key(&two), "order the order")
        );
    }
}
