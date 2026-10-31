//! What a project whose content mode is `on` keeps of its spans' content:
//! each field sealed under the project's content key, the span's id and the
//! field's name as associated data, in `plain_content`, and its bytes on its
//! organization's row of `content_held`. The key is a data key of the
//! project's own, made with the first content it keeps, wrapped under
//! `CONTENT_KEK`, never the hashing key's `TELEMETRY_KEK`, and held in
//! memory once opened, as the hashing key is ([`crate::hashing`]). Deleting
//! the settings row deletes the key, so every copy of the content, backups
//! among them, is unreadable from that commit. Content is read only through
//! [`open`], which files each read in the organization's audit chain.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use telmoni_shared::audit::{Actor, AuditEvent, emit_audit, lock_chain_in_time};
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::{ProjectAndOrganization, Scoped, project_scope};
use telmoni_shared::digest::HmacKey;
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
    let (wrapped, key) = opened_key(state, project_id).await?;
    let words_key = words_key(&key);
    let mut sealed = SealedFields::default();
    for span in content {
        for (field, text) in span.fields {
            let bytes = i32::try_from(text.len()).unwrap_or(i32::MAX);
            sealed
                .words
                .push(json!(hashed_words(&words_key, &text)).to_string());
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
    // Still the key they were sealed under: one deleted meanwhile, as the
    // project went `off`, refuses them, and the exporter's retry meets the mode.
    let now = db::content_key(&mut tx, project_id).await?;
    if now.is_none_or(|now| now.wrapped_dek != wrapped) {
        chain::rolled_back(tx.rollback().await, project_id, WHAT);
        return Err(TelemetryError::StoreUnavailable(
            "the project's content key changed as its content was kept; try again".into(),
        )
        .into());
    }
    db::keep_content(&mut tx, project_id, &organization_id, received_at, &sealed).await?;
    tx.commit().await?;
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
    let Some((mut tx, organization_id)) = held_if_kept(state, project_id).await? else {
        return Ok(Vec::new());
    };
    let read = file_the_read(&mut tx, &organization_id, actor, project_id, runs, spans).await;
    let kept = match read {
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
) -> Result<(Vec<u8>, Arc<Dek>), TelmoniError> {
    let aad = key_aad(project_id);
    let mut tx = project_scope(&state.db, project_id).await?;
    let read = db::content_key(&mut tx, project_id).await?;
    tx.commit().await?;
    let wrapped = match read {
        Some(wrapped) => wrapped,
        None => make_key(state, project_id, aad.as_bytes()).await?,
    };
    let key = unwrapped(state, project_id, &wrapped).await?;
    Ok((wrapped.wrapped_dek, key))
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

/// A field's distinct words, hashed for its `words` column.
fn hashed_words(key: &HmacKey, text: &str) -> Vec<String> {
    let mut words = words_of(text);
    words.sort_unstable();
    words.dedup();
    words.truncate(WORDS_MAX);
    words.iter().map(|word| hash_word(key, word)).collect()
}

fn hash_word(key: &HmacKey, word: &str) -> String {
    key.prefix_hex(word.as_bytes(), 16)
}

/// The runs whose kept content holds every word of `query`, and its words in
/// that order where it has more than one, newest first: what the trace list
/// narrows to. A field is opened in the process to match a phrase, never
/// shown, so nothing is filed; none where the project has no content key.
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
    let words_key = words_key(&key);
    let mut hashed: Vec<String> = phrase.iter().map(|w| hash_word(&words_key, w)).collect();
    hashed.sort_unstable();
    hashed.dedup();
    let found = db::search_content(&mut tx, project_id, &hashed, SEARCH_FIELDS_MAX).await?;
    tx.commit().await?;
    let mut runs: Vec<Uuid> = Vec::new();
    for (run_id, _, span_id, field, ciphertext, nonce) in found {
        if runs.contains(&run_id) {
            continue;
        }
        let holds = phrase.len() == 1 || {
            let aad = field_aad(&span_id, &field);
            let sealed = SealedRef {
                ciphertext: &ciphertext,
                nonce: &nonce,
            };
            state
                .vault
                .open(&key, sealed, aad.as_bytes())
                .ok()
                .is_some_and(|text| holds_in_order(&words_of(text.expose()), &phrase))
        };
        if holds {
            runs.push(run_id);
        }
    }
    Ok(runs)
}

/// Whether `words` hold `phrase` as one run, in order.
fn holds_in_order(words: &[String], phrase: &[String]) -> bool {
    words.windows(phrase.len()).any(|window| window == phrase)
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
            .content_kek
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
/// under, the project held in place so no move passes before the commit; none
/// where it has no settings row, which holds no content then.
async fn held_if_kept<'c>(
    state: &'c AppState,
    project_id: &ProjectId,
) -> Result<Option<(Scoped<'c, ProjectAndOrganization>, OrganizationId)>, TelmoniError> {
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

    /// A phrase holds where its words run in order, and nowhere else.
    #[test]
    fn a_phrase_holds_in_order_alone() {
        let words = words_of("where is my order");
        assert!(holds_in_order(&words, &words_of("my order")));
        assert!(!holds_in_order(&words, &words_of("order my")));
        assert!(!holds_in_order(&words, &words_of("where order")));
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
        let hashed = hashed_words(&words_key(&one), "order the order");
        assert_eq!(hashed.len(), 2);
        assert!(!hashed.iter().any(|h| h.contains("order")));
        assert_ne!(hashed, hashed_words(&words_key(&two), "order the order"));
    }
}
