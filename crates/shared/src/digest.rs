//! Deterministic content digests.

use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;

/// SHA-256 of `data`, lowercase hex (64 chars).
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    hex_lower(&Sha256::digest(data))
}

/// Deterministic JSON: object keys sorted recursively, compact. Independent of a
/// map's insertion order, so re-serializing at verify time reproduces the exact
/// bytes that were hashed at write time.
#[must_use]
pub fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body = keys
                .iter()
                .map(|k| {
                    let key = Value::String((*k).clone()).to_string();
                    format!("{key}:{}", canonical_json(&map[*k]))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(items) => {
            let body = items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        scalar => scalar.to_string(),
    }
}

/// The bytes `hex` spells, in either case; `None` for anything else.
#[must_use]
pub fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| match pair {
            [hi, lo] => {
                let byte = (char::from(*hi).to_digit(16)? << 4) | char::from(*lo).to_digit(16)?;
                u8::try_from(byte).ok()
            }
            _ => None,
        })
        .collect()
}

/// Lowercase hex, inlined to avoid a one-line crate dependency.
#[must_use]
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(hex_digit(b >> 4));
        out.push(hex_digit(b));
    }
    out
}

/// The lowercase hex digit for the low four bits of `n`.
#[must_use]
pub const fn hex_digit(n: u8) -> char {
    match n & 0x0f {
        d @ 0..=9 => (b'0' + d) as char,
        d => (b'a' + d - 10) as char,
    }
}

/// A key for HMAC-SHA-256, held by `ring`: the material never leaves it, and
/// `Debug` prints none of it.
pub struct HmacKey(ring::hmac::Key);

impl HmacKey {
    /// A key over `secret`, whatever its length.
    #[must_use]
    pub fn new(secret: &[u8]) -> Self {
        Self(ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret))
    }

    /// HMAC-SHA-256 of `data` under this key, its first `bytes` bytes — at
    /// most all 32 — as lowercase hex.
    #[must_use]
    pub fn prefix_hex(&self, data: &[u8], bytes: usize) -> String {
        let tag = ring::hmac::sign(&self.0, data);
        hex_lower(tag.as_ref().get(..bytes.min(32)).unwrap_or_default())
    }

    /// HMAC-SHA-256 of `data` under this key, whole, as lowercase hex.
    #[must_use]
    pub fn hex(&self, data: &[u8]) -> String {
        self.prefix_hex(data, 32)
    }

    /// Whether `tag` is the HMAC-SHA-256 of `data` under this key, compared
    /// in constant time.
    #[must_use]
    pub fn verify(&self, data: &[u8], tag: &[u8]) -> bool {
        ring::hmac::verify(&self.0, data, tag).is_ok()
    }
}

/// Why a signed message was refused: one 401 on the wire, so a probe learns
/// nothing, and the variant in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// A header missing or unparseable.
    Malformed,
    /// The timestamp is outside the replay window.
    Stale,
    /// No signature matches.
    Mismatch,
}

/// How far a signature's timestamp may stray from the receiver's clock before
/// the message is a replay: five minutes, Stripe's and Slack's number, and
/// what the book tells a webhook's receiver.
pub const REPLAY_TOLERANCE_SECS: i64 = 300;

/// How many signatures one `t=…,v1=…` header may carry: one, and two while
/// a secret rolls; the header is otherwise a caller's to fill, and each
/// candidate is a comparison the receiver pays for.
pub const MAX_SIGNATURES: usize = 8;

/// `t` within [`REPLAY_TOLERANCE_SECS`] of `now`, either way.
pub fn in_replay_window(t: i64, now: i64) -> Result<(), SignatureError> {
    let age = now.saturating_sub(t);
    if (-REPLAY_TOLERANCE_SECS..=REPLAY_TOLERANCE_SECS).contains(&age) {
        Ok(())
    } else {
        Err(SignatureError::Stale)
    }
}

/// The tag over `"{t}.{body}"`, the body never copied.
fn timestamped_tag(key: &HmacKey, t: i64, body: &[u8]) -> ring::hmac::Tag {
    let mut context = ring::hmac::Context::with_key(&key.0);
    context.update(t.to_string().as_bytes());
    context.update(b".");
    context.update(body);
    context.sign()
}

/// `t=<t>,v1=<hex>`, a `v1` per secret in order: HMAC-SHA-256 over
/// `"{t}.{body}"`, Stripe's shape, so an integrator already knows it.
#[must_use]
pub fn sign_timestamped(secrets: &[&str], body: &[u8], t: i64) -> String {
    secrets.iter().fold(format!("t={t}"), |header, secret| {
        let tag = timestamped_tag(&HmacKey::new(secret.as_bytes()), t, body);
        format!("{header},v1={}", hex_lower(tag.as_ref()))
    })
}

/// A `t=<unix>,v1=<hex>` header verified against `body` under `secret`:
/// `t` within the replay window of `now`, and one of its `v1`s — at most
/// [`MAX_SIGNATURES`], each compared in constant time against one tag over
/// the body — the HMAC-SHA-256 over `"{t}.{body}"`.
pub fn verify_timestamped(
    secret: &str,
    header: &str,
    body: &[u8],
    now: i64,
) -> Result<(), SignatureError> {
    let mut t: Option<i64> = None;
    let mut candidates: Vec<Vec<u8>> = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", v)) => t = v.parse().ok(),
            Some(("v1", v)) => candidates.extend(hex_decode(v)),
            _ => {}
        }
    }
    let t = t.ok_or(SignatureError::Malformed)?;
    if candidates.is_empty() || candidates.len() > MAX_SIGNATURES {
        return Err(SignatureError::Malformed);
    }
    in_replay_window(t, now)?;
    let tag = timestamped_tag(&HmacKey::new(secret.as_bytes()), t, body);
    if candidates
        .iter()
        .any(|candidate| bool::from(tag.as_ref().ct_eq(candidate.as_slice())))
    {
        Ok(())
    } else {
        Err(SignatureError::Mismatch)
    }
}

impl std::fmt::Debug for HmacKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HmacKey(***)")
    }
}

#[cfg(test)]
mod hmac_tests {
    use super::*;

    /// RFC 4231, test case 2.
    #[test]
    fn hmac_sha256_matches_the_rfc_vector_and_is_cut_to_its_prefix() {
        let key = HmacKey::new(b"Jefe");
        let data = b"what do ya want for nothing?";
        assert_eq!(
            key.prefix_hex(data, 32),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(key.prefix_hex(data, 16), "5bdcc146bf60754e6a042426089575c7");
        assert_eq!(
            key.prefix_hex(data, 64).len(),
            64,
            "never more than the whole tag"
        );
        assert_eq!(format!("{key:?}"), "HmacKey(***)");
    }

    #[test]
    fn a_tag_verifies_whole_and_hex_reads_either_case() {
        let key = HmacKey::new(b"Jefe");
        let data = b"what do ya want for nothing?";
        let tag = hex_decode(&key.hex(data)).expect("hex");
        assert!(key.verify(data, &tag));
        assert!(!key.verify(b"what do ya want for something?", &tag));
        assert_eq!(hex_decode("5BdC"), Some(vec![0x5b, 0xdc]));
        for bad in ["5", "zz", "5bd", "5b "] {
            assert_eq!(hex_decode(bad), None, "{bad:?}");
        }
    }

    // One header, one secret rolling: the receiver takes whichever `v1`
    // verifies, within the window, and nothing a stuffed header could buy.
    #[test]
    fn a_timestamped_header_verifies_under_either_secret_and_in_its_window() {
        let body = br#"{"id":"evt_1"}"#;
        let header = sign_timestamped(&["new", "old"], body, 1_700_000_000);
        assert!(header.starts_with("t=1700000000,v1="));
        assert_eq!(header.matches(",v1=").count(), 2);
        for secret in ["new", "old"] {
            assert_eq!(
                verify_timestamped(secret, &header, body, 1_700_000_000),
                Ok(())
            );
        }
        assert_eq!(
            verify_timestamped("other", &header, body, 1_700_000_000),
            Err(SignatureError::Mismatch)
        );
        assert_eq!(
            verify_timestamped("new", &header, b"{}", 1_700_000_000),
            Err(SignatureError::Mismatch)
        );
        assert_eq!(
            verify_timestamped("new", &header, body, 1_700_000_000 + REPLAY_TOLERANCE_SECS),
            Ok(())
        );
        assert_eq!(
            verify_timestamped(
                "new",
                &header,
                body,
                1_700_000_000 + REPLAY_TOLERANCE_SECS + 1
            ),
            Err(SignatureError::Stale)
        );
        let only_sig = header
            .split_once(',')
            .map(|(_, rest)| rest)
            .unwrap_or_default();
        for malformed in [
            "",
            "t=1700000000",
            "t=,v1=",
            "t=soon,v1=00",
            "v1=deadbeef",
            "nonsense",
            only_sig,
        ] {
            assert_eq!(
                verify_timestamped("new", malformed, body, 1_700_000_000),
                Err(SignatureError::Malformed),
                "{malformed:?}"
            );
        }
    }

    // A header is a caller's to fill: past the roll's two signatures and a
    // margin it is refused before anything is compared, a part of another
    // name is passed over, and a timestamp at either end of the range is
    // stale rather than an overflow.
    #[test]
    fn a_header_is_bounded_in_signatures_and_in_time() {
        let body = b"{}";
        let now = 1_700_000_000;
        let valid = sign_timestamped(&["s"], body, now);
        let stuffed = format!("{valid}{}", ",v1=00".repeat(MAX_SIGNATURES));
        assert_eq!(
            verify_timestamped("s", &stuffed, body, now),
            Err(SignatureError::Malformed)
        );
        let within = format!("{valid}{}", ",v1=00".repeat(MAX_SIGNATURES - 1));
        assert_eq!(verify_timestamped("s", &within, body, now), Ok(()));
        let noisy = format!("{valid},v0=deadbeef,x=y");
        assert_eq!(verify_timestamped("s", &noisy, body, now), Ok(()));
        for t in [i64::MIN, i64::MAX, -1] {
            assert_eq!(
                verify_timestamped("s", &format!("t={t},v1=deadbeef"), body, now),
                Err(SignatureError::Stale),
                "t={t}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let a = canonical_json(&json!({ "b": 1, "a": { "y": 2, "x": 1 } }));
        let b = canonical_json(&json!({ "a": { "x": 1, "y": 2 }, "b": 1 }));
        assert_eq!(a, b);
        assert_eq!(a, r#"{"a":{"x":1,"y":2},"b":1}"#);
    }

    #[test]
    fn sha256_hex_is_lowercase_64() {
        let h = sha256_hex(b"");
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn cross_language_golden_vectors_match_canonical_json() {
        let stub = json!({
            "schema_version": 1,
            "network_flows": [],
            "egress_denials": [],
            "notable_syscalls": [],
            "resource_summary": {},
        });
        assert_eq!(
            canonical_json(&stub),
            r#"{"egress_denials":[],"network_flows":[],"notable_syscalls":[],"resource_summary":{},"schema_version":1}"#
        );

        let tricky = json!({
            "b": "snowman ☃ quote\" backslash\\ tab\t nul",
            "a": [1, true, null, -7],
            "nested": { "z": 0.0001, "y": [1.5, -2.5e-7, 1e21, 1.0] },
        });
        assert_eq!(
            canonical_json(&tricky),
            "{\"a\":[1,true,null,-7],\"b\":\"snowman ☃ quote\\\" backslash\\\\ tab\\t nul\",\"nested\":{\"y\":[1.5,-2.5e-7,1e+21,1.0],\"z\":0.0001}}"
        );
    }
}
