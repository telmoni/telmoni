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
