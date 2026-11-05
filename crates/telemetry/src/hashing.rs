//! What a `user_id` and a `session_id` are stored as: each its HMAC-SHA-256
//! under a key of the project's own, the first 16 bytes in hex. The key is
//! made with the project's first export, sealed under
//! `TELEMETRY_KEK` as a connector's grant is under `CONNECTOR_KEK`
//! (`telmoni_shared::envelope`), and held in memory once opened, so the key
//! service is asked once per project per process.
//!
//! ⚠ The key, not the hash, is what erasure deletes: with the row
//! gone, no stored id can be matched against a list of guessed ones.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use telmoni_shared::acting::ActingKey;
use telmoni_shared::db::tenant_session::project_scope;
use telmoni_shared::digest::{HmacKey, hex_decode, hex_lower};
use telmoni_shared::envelope::{KEK_VERSION, KekError, Sealed};
use telmoni_shared::{AuthError, ProjectId, Redacted, TelemetryError, TelmoniError};

use crate::AppState;
use crate::db::{self, SealedHashingKey};
use crate::handler::chain;

/// How much of the HMAC is kept: 16 bytes, 32 hex characters, which no two
/// users of one project share in practice and nobody reverses.
pub const HASH_BYTES: usize = 16;

/// How a project stores the user and session ids its spans carry: hashed
/// under the project's own key, always.
#[derive(Clone)]
pub struct UserIdStorage(Arc<HmacKey>);

impl UserIdStorage {
    /// `raw`, as the row holds it.
    #[must_use]
    pub fn store(&self, raw: &str) -> String {
        self.0.prefix_hex(raw.as_bytes(), HASH_BYTES)
    }
}

impl std::fmt::Debug for UserIdStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UserIdStorage(***)")
    }
}

/// Each project's key, once opened: a process asks the key service once per
/// project. Never emptied: a project's key never changes while its row
/// stands, and a purged project's entry opens nothing again.
pub struct Cache<T = UserIdStorage>(RwLock<HashMap<ProjectId, T>>);

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self(RwLock::default())
    }
}

impl<T: Clone> Cache<T> {
    pub(crate) fn get(&self, project_id: &ProjectId) -> Option<T> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(project_id)
            .cloned()
    }

    pub(crate) fn put(&self, project_id: ProjectId, storage: T) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(project_id, storage);
    }

    /// An entry kept ahead of the work it stands for, let go when that failed;
    /// never a key's.
    pub(crate) fn forget(&self, project_id: &ProjectId) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(project_id);
    }
}

/// How the key's project stores user ids: from the cache, else from its
/// settings row — made, and its key sealed, where the project has none yet,
/// which also files the row under the organization that holds the project
/// now — and then cached.
///
/// # Errors
///
/// Postgres or the key service did not answer, a move or a purge held the
/// project, or auth could not say where a new project is: each one an
/// exporter retries. The project is gone, refused as its key now is; or the
/// row's key will not open, an outage of ours, logged by project.
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
    let read = db::hashing_key(&mut tx, project_id).await?;
    tx.commit().await?;
    let sealed = match read {
        Some(sealed) => sealed,
        None => key_project(state, project_id, aad).await?,
    };
    let storage = UserIdStorage(Arc::new(open_key(state, &sealed, aad).await?));
    state.hashing_keys.put(project_id.clone(), storage.clone());
    Ok(storage)
}

/// Makes the project's hashing key, and its settings row where it has none.
/// The key is sealed first, holding nothing, since the key service is
/// across the network. Then, with the project held in place so that no
/// transfer commits meanwhile, the row is filed under the organization that
/// holds the project now — the row's own, or auth's answer — never the one
/// the export's key was resolved to, which a transfer may have left: that
/// organization's purge would mark the moved project's spans deleted. The
/// key is ours, or the one a racing export wrote first.
async fn key_project(
    state: &AppState,
    project_id: &ProjectId,
    aad: &[u8],
) -> Result<SealedHashingKey, TelmoniError> {
    let sealed = seal_new_key(state, aad).await?;
    let mut tx = chain::hold_project(&state.db, project_id, "hashing key").await?;
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
    let written = db::hashing_key(&mut tx, project_id).await?;
    tx.commit().await?;
    written.ok_or_else(|| {
        TelmoniError::Internal("telemetry: the hashing key just written is not there".into())
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
        .map_err(|e| kek_refused(&e, "hashing key"))?;
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
        .map_err(|e| kek_refused(&e, "hashing key"))?;
    let secret = state.vault.open(
        &dek,
        telmoni_shared::envelope::SealedRef {
            ciphertext: &sealed.ciphertext,
            nonce: &sealed.nonce,
        },
        aad,
    )?;
    let bytes = hex_decode(secret.expose()).ok_or_else(|| {
        TelmoniError::Internal("telemetry: a hashing key opened to something other than hex".into())
    })?;
    Ok(HmacKey::new(&bytes))
}

/// The KEK did not give a key back: an outage to retry, or a row that is
/// broken, which retrying will not mend. `what` names the key, as a log reads it.
pub(crate) fn kek_refused(error: &KekError, what: &str) -> TelmoniError {
    match error {
        KekError::Unavailable(_) => {
            tracing::warn!(error = %error, "telemetry: the key service did not answer");
            TelemetryError::StoreUnavailable(format!(
                "the key service that seals a project's {what} did not answer"
            ))
            .into()
        }
        KekError::Invalid(_) => {
            tracing::error!(error = %error, key = what, "telemetry: a project's key will not open");
            TelmoniError::Internal(format!("telemetry: a project's {what} will not open"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hashed_id_is_sixteen_bytes_of_hex_the_same_each_time_and_per_key() {
        let one = UserIdStorage(Arc::new(HmacKey::new(&[1; 32])));
        let two = UserIdStorage(Arc::new(HmacKey::new(&[2; 32])));
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
    }

    #[test]
    fn storage_prints_no_key() {
        let hashed = UserIdStorage(Arc::new(HmacKey::new(&[7; 32])));
        assert_eq!(format!("{hashed:?}"), "UserIdStorage(***)");
    }
}
