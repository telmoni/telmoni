//! What a `user_id` is stored as: its HMAC-SHA-256 under a key of the
//! project's own, the first 16 bytes in hex, unless the project keeps it
//! plain. The key is made with the project's first export, sealed under
//! `TELEMETRY_KEK` as a connector's grant is under `CONNECTOR_KEK`
//! (`telmoni_shared::envelope`), and held in memory once opened, so the key
//! service is asked once per project per process.
//!
//! ⚠ The key, not the hash, is what erasure (4.4) deletes: with the row
//! gone, no stored id can be matched against a list of guessed ones.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use telmoni_shared::acting::ActingKey;
use telmoni_shared::db::project_move;
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::digest::{HmacKey, hex_lower};
use telmoni_shared::envelope::{KEK_VERSION, KekError, Sealed};
use telmoni_shared::{AuthError, ProjectId, Redacted, TelemetryError, TelmoniError};

use crate::AppState;
use crate::db::{self, HashingSettings, SealedHashingKey};
use crate::handler::chain;

/// How much of the HMAC is kept: 16 bytes, 32 hex characters, which no two
/// users of one project share in practice and nobody reverses.
pub const HASH_BYTES: usize = 16;

/// How a project stores the user ids its spans carry.
#[derive(Clone)]
pub enum UserIdStorage {
    /// Hashed under the project's own key.
    Hashed(Arc<HmacKey>),
    /// As sent: the project chose to keep them plain.
    Plain,
}

impl UserIdStorage {
    /// `raw`, as the row holds it.
    #[must_use]
    pub fn store(&self, raw: &str) -> String {
        match self {
            Self::Hashed(key) => key.prefix_hex(raw.as_bytes(), HASH_BYTES),
            Self::Plain => raw.to_owned(),
        }
    }
}

impl std::fmt::Debug for UserIdStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Hashed(_) => "UserIdStorage::Hashed(***)",
            Self::Plain => "UserIdStorage::Plain",
        })
    }
}

/// Each project's storage, once read: a process asks the key service once
/// per project. Never emptied: a project's key never changes while its row
/// stands, and a purged project's entry hashes nothing again.
#[derive(Default)]
pub struct Cache(RwLock<HashMap<ProjectId, UserIdStorage>>);

impl Cache {
    fn get(&self, project_id: &ProjectId) -> Option<UserIdStorage> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .cloned()
    }

    fn put(&self, project_id: ProjectId, storage: UserIdStorage) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(project_id, storage);
    }
}

/// How the key's project stores user ids: from the cache, else from its
/// settings row — made, and its key sealed, where the project has none yet,
/// which also files the row under the organization that holds the project
/// now — and then cached.
///
/// # Errors
///
/// Postgres did not answer; the key service did not answer, or a move or a
/// purge of the project held it as its first export went to key it, which
/// an exporter retries; auth did not say, in time or at all, which
/// organization holds a project with no settings row yet, which it retries
/// too; the project is gone since its key was read, refused as its key now
/// is; or the row's key will not open, which is an outage of ours, logged
/// by project.
pub async fn storage_for(state: &AppState, key: &ActingKey) -> Result<UserIdStorage, TelmoniError> {
    storage_for_project(state, &key.project_id).await
}

/// [`storage_for`], for a project named by itself, with no key in hand.
///
/// # Errors
///
/// As [`storage_for`].
pub async fn storage_for_project(
    state: &AppState,
    project_id: &ProjectId,
) -> Result<UserIdStorage, TelmoniError> {
    if let Some(storage) = state.hashing_keys.get(project_id) {
        return Ok(storage);
    }
    let aad = project_id.as_str().as_bytes();
    let mut tx = project_scope(&state.db, project_id).await?;
    let read = db::hashing_settings(&mut tx, project_id).await?;
    tx.commit().await?;
    let settings = match read {
        Some(settings) if settings.key.is_some() => settings,
        _ => key_project(state, project_id, aad).await?,
    };
    let storage = if settings.user_id_hashed {
        let sealed = settings.key.ok_or_else(|| {
            TelmoniError::Internal("telemetry: the hashing key just written is not there".into())
        })?;
        UserIdStorage::Hashed(Arc::new(open_key(state, &sealed, aad).await?))
    } else {
        UserIdStorage::Plain
    };
    state.hashing_keys.put(project_id.clone(), storage.clone());
    Ok(storage)
}

/// The project's hashing key made, and its settings row with it where it
/// has none. The key is sealed first, holding nothing, since the key
/// service is asked across the network; then, the project held in place as
/// a ping holds it, so that no transfer of it commits meanwhile, the row is
/// filed under the organization that holds the project now — the one the
/// row names, where it has one, which a move takes with the project, or,
/// where it has none, auth's answer, asked under the hold
/// ([`chain::home`]) — and never under the organization the export's key
/// was resolved to before, which a transfer committed since would have the
/// row filed back under: that organization's purge, which finds its
/// projects by their settings rows, would mark the moved project's spans
/// deleted, and the new one's never find them. Kept out by a move or a
/// purge holding the project, the export is refused as one an exporter
/// tries again; for a project auth no longer has, as its key now is,
/// deleted with it, rather than with a status the lane gives nothing else.
/// The key is ours, or the one a racing export wrote first.
async fn key_project(
    state: &AppState,
    project_id: &ProjectId,
    aad: &[u8],
) -> Result<HashingSettings, TelmoniError> {
    let sealed = seal_new_key(state, aad).await?;
    let mut tx = project_scope(&state.db, project_id).await?;
    if !project_move::try_hold_in_place(tx.conn(), project_id).await? {
        chain::rolled_back(tx.rollback().await, project_id, "hashing key");
        return Err(TelemetryError::StoreUnavailable(
            "the project is moving to another organization, or being deleted; the export is \
             tried again"
                .into(),
        )
        .into());
    }
    let organization_id = match db::settings_organization(&mut tx, project_id).await? {
        Some(filed) => filed,
        None => match chain::home(state.auth.as_ref(), project_id).await {
            Ok(Some(holds)) => holds,
            Ok(None) => {
                chain::rolled_back(tx.rollback().await, project_id, "hashing key");
                return Err(AuthError::InvalidToken.into());
            }
            Err(unanswered) => {
                chain::rolled_back(tx.rollback().await, project_id, "hashing key");
                return Err(unanswered);
            }
        },
    };
    let mut tx = tx.bind_organization(&organization_id).await?;
    db::ensure_settings(&mut tx, project_id, &organization_id).await?;
    db::set_hashing_key_if_none(&mut tx, project_id, &sealed).await?;
    let settings = db::hashing_settings(&mut tx, project_id).await?;
    tx.commit().await?;
    settings.ok_or_else(|| {
        TelmoniError::Internal("telemetry: the settings row just written is not there".into())
    })
}

/// A fresh key for one project, sealed: the data key wrapped by the KEK, and
/// 256 random bits — as hex, since the vault seals text — under it.
async fn seal_new_key(state: &AppState, aad: &[u8]) -> Result<SealedHashingKey, TelmoniError> {
    let dek = state.vault.new_dek()?;
    let wrapped_dek = state
        .kek
        .wrap(&state.http, &dek, aad)
        .await
        .map_err(|e| kek_refused(&e))?;
    let material = state.vault.new_dek()?;
    let secret = Redacted::from(hex_lower(material.as_bytes()));
    let Sealed { ciphertext, nonce } = state.vault.seal(&dek, &secret, aad)?;
    Ok(SealedHashingKey {
        ciphertext,
        nonce,
        wrapped_dek,
        key_version: KEK_VERSION,
    })
}

/// A project's key, opened: the data key unwrapped by the KEK, the hex under
/// it, and the HMAC key over its bytes.
async fn open_key(
    state: &AppState,
    sealed: &SealedHashingKey,
    aad: &[u8],
) -> Result<HmacKey, TelmoniError> {
    let dek = state
        .kek
        .unwrap_dek(&state.http, &sealed.wrapped_dek, aad, sealed.key_version)
        .await
        .map_err(|e| kek_refused(&e))?;
    let secret = state.vault.open(
        &dek,
        &Sealed {
            ciphertext: sealed.ciphertext.clone(),
            nonce: sealed.nonce.clone(),
        },
        aad,
    )?;
    let bytes = hex_bytes(secret.expose()).ok_or_else(|| {
        TelmoniError::Internal("telemetry: a hashing key opened to something other than hex".into())
    })?;
    Ok(HmacKey::new(&bytes))
}

/// The KEK did not give a key back: an outage to retry, or a row that is
/// broken, which retrying will not mend.
fn kek_refused(error: &KekError) -> TelmoniError {
    match error {
        KekError::Unavailable(_) => {
            tracing::warn!(error = %error, "telemetry: the key service did not answer");
            TelemetryError::StoreUnavailable(
                "the key service that seals a project's hashing key did not answer".into(),
            )
            .into()
        }
        KekError::Invalid(_) => {
            tracing::error!(error = %error, "telemetry: a project's hashing key will not open");
            TelmoniError::Internal("telemetry: a project's hashing key will not open".into())
        }
    }
}

/// Hex back into bytes, or `None` for anything that is not hex.
fn hex_bytes(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|digits| u8::from_str_radix(digits, 16).ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hashed_id_is_sixteen_bytes_of_hex_the_same_each_time_and_per_key() {
        let one = UserIdStorage::Hashed(Arc::new(HmacKey::new(&[1; 32])));
        let two = UserIdStorage::Hashed(Arc::new(HmacKey::new(&[2; 32])));
        let stored = one.store("acme-42");
        assert_eq!(stored.len(), HASH_BYTES * 2);
        assert!(stored.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(stored, one.store("acme-42"), "the same id hashes the same");
        assert_ne!(stored, one.store("acme-43"));
        assert_ne!(
            stored,
            two.store("acme-42"),
            "another project's key, another hash"
        );
        assert_eq!(UserIdStorage::Plain.store("acme-42"), "acme-42");
    }

    #[test]
    fn hex_round_trips_and_refuses_what_is_not_hex() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(hex_bytes(&hex_lower(&bytes)), Some(bytes));
        assert_eq!(hex_bytes("abc"), None, "an odd length");
        assert_eq!(hex_bytes("zz"), None);
        assert_eq!(hex_bytes(""), Some(Vec::new()));
    }

    #[test]
    fn storage_prints_no_key() {
        let hashed = UserIdStorage::Hashed(Arc::new(HmacKey::new(&[7; 32])));
        assert_eq!(format!("{hashed:?}"), "UserIdStorage::Hashed(***)");
    }
}
