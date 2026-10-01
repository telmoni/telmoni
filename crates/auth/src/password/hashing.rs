//! Argon2id for the built-in provider's passwords, at the crate's defaults,
//! which are OWASP's: 19 MiB, two passes, one lane. Each hash and each
//! check runs off the async runtime, since it is meant to take a while.

use std::sync::OnceLock;

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

use telmoni_shared::{AuthError, TelmoniError};

/// The floor NIST SP 800-63B sets, and no composition rule: length is what
/// resists guessing, and a rule about digits mostly decides which digit.
pub const MIN_PASSWORD_CHARS: usize = 8;

/// The ceiling, so a hash is bounded work. Well above anything a manager
/// generates.
pub const MAX_PASSWORD_BYTES: usize = 256;

/// The shape check every new password passes.
pub fn validate(password: &str) -> Result<(), TelmoniError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(
            AuthError::BadRequest(format!("use at least {MIN_PASSWORD_CHARS} characters")).into(),
        );
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(AuthError::BadRequest(format!(
            "a password is at most {MAX_PASSWORD_BYTES} characters"
        ))
        .into());
    }
    Ok(())
}

/// The PHC string to store for `password`.
pub async fn hash(password: String) -> Result<String, TelmoniError> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
    })
    .await
    .map_err(|e| TelmoniError::internal("password hashing task", e))?
    .map_err(|_| TelmoniError::Internal("password hashing failed".into()))
}

/// Whether `password` is the one `phc` was made from. The parameters come
/// from the stored string, so a hash made under yesterday's defaults still
/// verifies.
pub async fn verify(password: String, phc: String) -> Result<bool, TelmoniError> {
    tokio::task::spawn_blocking(move || verify_now(&password, &phc))
        .await
        .map_err(|e| TelmoniError::internal("password check task", e))?
}

fn verify_now(password: &str, phc: &str) -> Result<bool, TelmoniError> {
    let parsed = PasswordHash::new(phc)
        .map_err(|_| TelmoniError::Internal("a stored password hash is unreadable".into()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Spend the time a real check takes, against a hash nobody's password
/// made: a sign-in for an address with no account answers as slowly as one
/// with the wrong password, so the answer's timing says nothing about which.
/// The decoy is made on the first call, on the same blocking thread.
pub async fn burn(password: String) {
    let _ = tokio::task::spawn_blocking(move || verify_now(&password, decoy())).await;
}

fn decoy() -> &'static str {
    static DECOY: OnceLock<String> = OnceLock::new();
    DECOY.get_or_init(|| {
        Argon2::default()
            .hash_password(b"nobody's password")
            .map(|h| h.to_string())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_held_to_its_length_and_nothing_else() {
        assert!(validate("hunter22").is_ok());
        assert!(validate("correct horse battery staple").is_ok());
        assert!(validate("hunter2").is_err(), "seven characters");
        assert!(validate("ünïcödé!").is_ok(), "eight characters, more bytes");
        assert!(validate(&"x".repeat(MAX_PASSWORD_BYTES + 1)).is_err());
    }

    #[tokio::test]
    async fn a_hash_verifies_its_own_password_and_no_other() {
        let phc = hash("correct horse battery staple".into()).await.unwrap();
        assert!(phc.starts_with("$argon2id$v=19$"), "{phc}");
        assert!(
            verify("correct horse battery staple".into(), phc.clone())
                .await
                .unwrap()
        );
        assert!(
            !verify("correct horse battery stapler".into(), phc)
                .await
                .unwrap()
        );
        assert!(
            verify("anything".into(), "not a hash".into())
                .await
                .is_err()
        );
    }

    /// The decoy is a real hash, so burning a guess against it costs what a
    /// real check costs, and a guess never verifies against it.
    #[tokio::test]
    async fn the_decoy_is_a_real_hash_that_no_guess_matches() {
        assert!(decoy().starts_with("$argon2id$v=19$"), "{}", decoy());
        assert!(!verify("anything".into(), decoy().to_owned()).await.unwrap());
        burn("anything".into()).await;
    }
}
