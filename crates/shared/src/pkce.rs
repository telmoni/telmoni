//! RFC 7636's proof of possession, S256 alone: the challenge a sign-in sends
//! ahead with the browser, and the verifier only the console that started it
//! holds. Auth checks it for the codes it mints, and hands it to a provider
//! for the codes the provider mints.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use subtle::ConstantTimeEq as _;

/// Whether `challenge` is an S256 challenge: a SHA-256, 43 base64url
/// characters.
#[must_use]
pub fn is_challenge(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `verifier` is one RFC 7636 allows (§ 4.1): 43 to 128 characters
/// of its unreserved set.
#[must_use]
pub fn is_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// The S256 challenge of `verifier`: base64url of its SHA-256 (§ 4.2).
#[must_use]
pub fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(ring::digest::digest(
        &ring::digest::SHA256,
        verifier.as_bytes(),
    ))
}

/// Whether `verifier` is the one `challenge` was made from, compared in
/// constant time.
#[must_use]
pub fn answers(verifier: &str, challenge: &str) -> bool {
    is_verifier(verifier)
        && bool::from(
            challenge_of(verifier)
                .as_bytes()
                .ct_eq(challenge.as_bytes()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636, Appendix B: the specification's own example.
    #[test]
    fn the_rfcs_example_answers_its_challenge_and_nothing_else_does() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(challenge_of(verifier), challenge);
        assert!(is_challenge(challenge) && is_verifier(verifier));
        assert!(answers(verifier, challenge));
        assert!(!answers(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXK",
            challenge
        ));
        assert!(
            !answers("short", &challenge_of("short")),
            "too short a verifier"
        );
        assert!(!is_challenge(
            "not/base64url+challenge=========================="
        ));
    }
}
