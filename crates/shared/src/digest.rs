//! Deterministic content digests.

use serde_json::Value;
use sha2::{Digest, Sha256};

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
