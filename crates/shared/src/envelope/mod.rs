//! Envelope encryption for a reversible secret at rest: a data key per row,
//! wrapped by a key this process never holds.
//!
//! For a secret a service must give BACK in the clear at the moment of use — a
//! webhook URL, a bot token. A hashed credential needs none of this.
//!
//! ⚠ **A library, never a service-to-service lane.** Internal hops are gated by
//! the shared service secret, which says nothing about who is asking; a
//! seal-and-open oracle over it would let any secret holder open every row,
//! and the audit log would name the oracle instead of the real caller.
//!
//! [`Vault`] seals each field under the row's own [`Dek`], a fresh nonce per
//! field and the row id as associated data; [`Kek`] wraps the DEK, again with
//! the row id. A dump yields ciphertext and wrapped keys; disabling the KMS key
//! version renders every row inert at once. What envelope does NOT buy:
//! compromising the running service still decrypts, because it is allowed to.
//! What it stops is anything portable — there is no key to walk out with.
//!
//! Plaintext leaves only as a [`Redacted`], wiped on drop; `grep '\.expose()'`
//! finds every read.

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use zeroize::Zeroizing;

use crate::{Redacted, TelmoniError};

mod kek;

pub use kek::{KEK_VERSION, Kek, KekError, KmsKek, LocalKek, is_loopback_origin, on_deployed_tier};

/// A data key is AES-256: 32 bytes, always.
const DEK_LEN: usize = 32;

/// AES-256-GCM's nonce length: 96 bits, one fresh random nonce per field.
const NONCE_LEN: usize = 12;

/// One row's data key: wiped on drop, masked in `Debug`.
pub struct Dek(Zeroizing<[u8; DEK_LEN]>);

impl Dek {
    pub(crate) fn from_bytes(bytes: [u8; DEK_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The raw key, for the cipher. Every read is greppable by this name.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; DEK_LEN] {
        &self.0
    }
}

impl std::fmt::Debug for Dek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Dek(***)")
    }
}

/// One ciphertext with the nonce it was sealed under. `Debug` prints the
/// lengths only: not a secret, but bytes in a log line are noise.
#[derive(Clone)]
pub struct Sealed {
    /// The field, encrypted, with the tag appended.
    pub ciphertext: Vec<u8>,
    /// The 96-bit nonce this field was sealed under.
    pub nonce: Vec<u8>,
}

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sealed")
            .field("ciphertext_len", &self.ciphertext.len())
            .field("nonce_len", &self.nonce.len())
            .finish()
    }
}

/// The cipher and the randomness behind it. Stateless: every seal and every
/// open is handed the row's own data key by the caller.
pub struct Vault {
    rng: SystemRandom,
}

impl Default for Vault {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Vault")
    }
}

impl Vault {
    /// A vault over the OS randomness source.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rng: SystemRandom::new(),
        }
    }

    /// A fresh data key for one row, from the OS.
    pub fn new_dek(&self) -> Result<Dek, TelmoniError> {
        let mut bytes = [0u8; DEK_LEN];
        self.rng
            .fill(&mut bytes)
            .map_err(|_| TelmoniError::Internal("vault: no randomness for a data key".into()))?;
        Ok(Dek::from_bytes(bytes))
    }

    fn cipher(dek: &Dek) -> Result<LessSafeKey, TelmoniError> {
        UnboundKey::new(&AES_256_GCM, dek.as_bytes())
            .map(LessSafeKey::new)
            .map_err(|_| {
                TelmoniError::Internal("vault: the data key was refused by the cipher".into())
            })
    }

    /// Seal `plaintext` under `dek` and a fresh random nonce, bound to `aad` —
    /// the row's id, so a ciphertext copied to another row will not open there.
    pub fn seal(
        &self,
        dek: &Dek,
        plaintext: &Redacted,
        aad: &[u8],
    ) -> Result<Sealed, TelmoniError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce_bytes)
            .map_err(|_| TelmoniError::Internal("vault: no randomness for a nonce".into()))?;
        let mut in_out = plaintext.expose().as_bytes().to_vec();
        Self::cipher(dek)?
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad),
                &mut in_out,
            )
            .map_err(|_| TelmoniError::Internal("vault: seal failed".into()))?;
        Ok(Sealed {
            ciphertext: in_out,
            nonce: nonce_bytes.to_vec(),
        })
    }

    /// Open a row's ciphertext under its `dek` back into the secret it holds.
    pub fn open(&self, dek: &Dek, sealed: &Sealed, aad: &[u8]) -> Result<Redacted, TelmoniError> {
        let nonce: [u8; NONCE_LEN] = sealed
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| TelmoniError::Internal("vault: nonce is not 96 bits".into()))?;
        let mut in_out = sealed.ciphertext.clone();
        let len = Self::cipher(dek)?
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                &mut in_out,
            )
            .map_err(|_| TelmoniError::Internal("vault: ciphertext did not verify".into()))?
            .len();
        // Truncate rather than copy, so the one allocation that ever holds the
        // plaintext is the one `Redacted` wipes on drop.
        in_out.truncate(len);
        let text = String::from_utf8(in_out)
            .map_err(|_| TelmoniError::Internal("vault: plaintext is not UTF-8".into()))?;
        Ok(Redacted::from(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_secret_opens_back_to_itself_under_its_own_key() {
        let v = Vault::new();
        let dek = v.new_dek().unwrap();
        let secret = Redacted::from("https://hooks.slack.com/services/T/B/x");
        let sealed = v.seal(&dek, &secret, b"row-1").unwrap();
        assert!(
            !sealed.ciphertext.windows(5).any(|w| w == b"hooks"),
            "the ciphertext carries the plaintext"
        );
        let opened = v.open(&dek, &sealed, b"row-1").unwrap();
        assert_eq!(opened.expose(), secret.expose());
    }

    #[test]
    fn another_rows_key_does_not_open_it() {
        let v = Vault::new();
        let sealed = v
            .seal(&v.new_dek().unwrap(), &Redacted::from("secret"), b"row-1")
            .unwrap();
        assert!(v.open(&v.new_dek().unwrap(), &sealed, b"row-1").is_err());
    }

    /// The property the AAD buys: a ciphertext moved between rows fails.
    #[test]
    fn a_ciphertext_moved_to_another_row_does_not_open() {
        let v = Vault::new();
        let dek = v.new_dek().unwrap();
        let sealed = v.seal(&dek, &Redacted::from("secret"), b"row-1").unwrap();
        assert!(v.open(&dek, &sealed, b"row-2").is_err());
    }

    #[test]
    fn two_seals_of_one_value_never_share_a_nonce_or_a_ciphertext() {
        let v = Vault::new();
        let dek = v.new_dek().unwrap();
        let a = v.seal(&dek, &Redacted::from("same"), b"row").unwrap();
        let b = v.seal(&dek, &Redacted::from("same"), b"row").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn every_data_key_is_fresh_and_masked() {
        let v = Vault::new();
        let a = v.new_dek().unwrap();
        let b = v.new_dek().unwrap();
        assert_ne!(a.as_bytes(), b.as_bytes());
        assert_ne!(a.as_bytes(), &[0u8; DEK_LEN]);
        assert_eq!(format!("{a:?}"), "Dek(***)");
    }
}
