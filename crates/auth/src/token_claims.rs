//! Reading a claim out of a token an identity provider just handed this
//! service, without verifying it.
//!
//! Safe for what these are used for, and nothing else: the token came from
//! our own confidential exchange over TLS. The OpenID Connect provider reads
//! an id token's claims here once [`telmoni_shared::oidc::IdTokenVerifier`]
//! has verified it, and an adapter for a provider with a management API
//! reads the session id its logout page names. Nothing here decides who
//! somebody is, and no token this service mints is read here: its own are
//! opaque.

use serde::Deserialize;

/// The `sid` claim, when the token carries a non-empty one.
#[must_use]
pub fn session_id_of(token: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Sid {
        sid: Option<String>,
    }
    payload::<Sid>(token)?.sid.filter(|s| !s.is_empty())
}

/// The payload segment of a JWT, decoded and parsed as `T`.
pub(crate) fn payload<T: serde::de::DeserializeOwned>(token: &str) -> Option<T> {
    use base64::Engine;
    let payload_b64 = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_segment_is_read_as_json() {
        let payload_b64 = "eyJleHAiOjk5OTk5OTk5OTl9";
        let v: serde_json::Value = payload(&format!("h.{payload_b64}.s")).expect("decodes");
        assert_eq!(v["exp"], 9_999_999_999u64);
    }

    #[test]
    fn a_session_id_is_read_and_an_empty_one_is_none() {
        use base64::Engine as _;
        let with = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({ "sid": "session_1" }).to_string());
        assert_eq!(
            session_id_of(&format!("h.{with}.s")).as_deref(),
            Some("session_1")
        );
        let empty = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({ "sid": "" }).to_string());
        assert_eq!(session_id_of(&format!("h.{empty}.s")), None);
        assert_eq!(session_id_of("h.!!!.s"), None);
    }
}
