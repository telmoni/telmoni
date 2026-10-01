//! The audit-chain row hash.
//!
//! ⚠ **A frozen contract.** Once rows are chained, changing the inputs or their
//! order invalidates every historical `row_hash`; the golden vector below pins
//! the bytes. Each field is length-prefixed so no boundary is ambiguous
//! (`"a" + "bc"` is not `"ab" + "c"`), and an ABSENT field is prefixed
//! `u64::MAX` so a NULL column and an empty one are different inputs.
//!
//! One segment is the exception: `in_project` is appended only
//! when present, so a row outside any project hashes consistently
//! and older chains stay valid without a rehash.

use crate::digest::{canonical_json, hex_lower};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Compute a row's chain hash. `prev_hash` is the previous (per-ORGANIZATION)
/// row's `row_hash`, or `None` for an organization's genesis row.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the row hash covers every column of the audit row, by design"
)]
pub fn row_hash(
    id: Uuid,
    organization_id: &str,
    actor_id: &str,
    action: &str,
    resource_kind: &str,
    resource_id: Option<&str>,
    metadata: Option<&Value>,
    created_at: DateTime<Utc>,
    prev_hash: Option<&str>,
    in_project: Option<&str>,
) -> String {
    let id = id.to_string();
    // Absent metadata renders `""`, which no JSON value canonicalizes to, so it
    // needs no presence tag.
    let meta = metadata.map(canonical_json).unwrap_or_default();
    // Microsecond precision, always `Z`: the exact string the writer stores.
    let ts = created_at.to_rfc3339_opts(SecondsFormat::Micros, true);

    let mut hasher = Sha256::new();
    for seg in [
        Some(id.as_str()),
        Some(organization_id),
        Some(actor_id),
        Some(action),
        Some(resource_kind),
        resource_id,
        Some(meta.as_str()),
        Some(ts.as_str()),
        prev_hash,
    ] {
        match seg {
            Some(s) => {
                hasher.update((s.len() as u64).to_le_bytes());
                hasher.update(s.as_bytes());
            }
            None => hasher.update(u64::MAX.to_le_bytes()),
        }
    }
    // After the link, and NOTHING when absent, so pre-column rows keep their
    // hash. Unambiguous: `prev_hash` is always tagged, so a segment after it can
    // only be this one, and the newtype behind it rejects the empty string.
    if let Some(project) = in_project {
        hasher.update((project.len() as u64).to_le_bytes());
        hasher.update(project.as_bytes());
    }
    hex_lower(&hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixed() -> (Uuid, DateTime<Utc>) {
        let id = Uuid::parse_str("018f0000-0000-7000-8000-000000000001").unwrap();
        let ts = DateTime::parse_from_rfc3339("2026-06-05T12:00:00.000000Z")
            .unwrap()
            .with_timezone(&Utc);
        (id, ts)
    }

    /// Golden vector — freezes the algorithm. If this flips, the chain
    /// contract changed and every historical `row_hash` is invalidated: do NOT
    /// "just update" it. In production, changing this requires every row
    /// rehashed under a new algorithm version.
    ///
    /// Every field is a DISTINCT value, or a swap between two adjacent
    /// segments would go uncaught.
    #[test]
    fn golden_vector_is_stable() {
        let (id, ts) = fixed();
        let h = row_hash(
            id,
            "user-1",
            "actor-1",
            "created",
            "token",
            Some("token-9"),
            Some(&json!({ "rotated": true })),
            ts,
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
            None,
        );
        assert_eq!(
            h,
            "8bbe14eb0626568a7b96c5619f5bb07fffb7b4f940a361e8372a1c0cc1620624"
        );
    }

    #[test]
    fn changing_any_field_changes_the_hash() {
        let (id, ts) = fixed();
        let base = row_hash(
            id,
            "o",
            "a",
            "created",
            "token",
            Some("r"),
            None,
            ts,
            None,
            None,
        );

        for altered in [
            row_hash(
                id,
                "o2",
                "a",
                "created",
                "token",
                Some("r"),
                None,
                ts,
                None,
                None,
            ),
            row_hash(
                id,
                "o",
                "a2",
                "created",
                "token",
                Some("r"),
                None,
                ts,
                None,
                None,
            ),
            row_hash(
                id,
                "o",
                "a",
                "updated",
                "token",
                Some("r"),
                None,
                ts,
                None,
                None,
            ),
            row_hash(
                id,
                "o",
                "a",
                "created",
                "secret",
                Some("r"),
                None,
                ts,
                None,
                None,
            ),
            row_hash(
                id,
                "o",
                "a",
                "created",
                "token",
                Some("r2"),
                None,
                ts,
                None,
                None,
            ),
            row_hash(id, "o", "a", "created", "token", None, None, ts, None, None),
            row_hash(
                id,
                "o",
                "a",
                "created",
                "token",
                Some("r"),
                Some(&json!({ "k": 1 })),
                ts,
                None,
                None,
            ),
            row_hash(
                id,
                "o",
                "a",
                "created",
                "token",
                Some("r"),
                None,
                ts,
                Some("x"),
                None,
            ),
            row_hash(
                id,
                "o",
                "a",
                "created",
                "token",
                Some("r"),
                None,
                ts,
                None,
                Some("project_7bQx2mNv9BcK4dLp"),
            ),
        ] {
            assert_ne!(base, altered);
        }
        assert_eq!(base.len(), 64, "sha256 hex is 64 chars");
    }

    /// Present-but-empty and absent are different facts, so they hash
    /// differently.
    #[test]
    fn an_absent_segment_and_an_empty_one_differ() {
        let (id, ts) = fixed();
        assert_ne!(
            row_hash(id, "o", "a", "created", "token", None, None, ts, None, None),
            row_hash(
                id,
                "o",
                "a",
                "created",
                "token",
                Some(""),
                None,
                ts,
                None,
                None
            ),
        );
    }

    /// A boundary between two segments cannot be moved: `"ab" + "c"` and
    /// `"a" + "bc"` are different inputs.
    #[test]
    fn a_segment_boundary_cannot_be_shifted() {
        let (id, ts) = fixed();
        let a = row_hash(id, "o", "ab", "c", "token", None, None, ts, None, None);
        let b = row_hash(id, "o", "a", "bc", "token", None, None, ts, None, None);
        assert_ne!(a, b);
    }

    /// The project segment. A row inside a project hashes
    /// differently from the same row outside one or inside another, and a row
    /// outside one hashes consistently.
    #[test]
    fn golden_vector_with_a_project_is_stable() {
        let (id, ts) = fixed();
        let h = row_hash(
            id,
            "user-1",
            "actor-1",
            "created",
            "token",
            Some("token-9"),
            Some(&json!({ "rotated": true })),
            ts,
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
            // Frozen literal, deliberately still `proj_…`: this vector pins the
            // algorithm, so its input must never move with a rename.
            Some("proj_7bQx2mNv9BcK4dLp"),
        );
        assert_eq!(
            h,
            "19fbd5f0c37a2ec34f7f0c15216f337e2739fa849bebeed9b931ff0c70d57e05"
        );
    }

    /// A row's project cannot be read as its link, nor its link as its
    /// project.
    #[test]
    fn a_project_and_a_link_do_not_trade_places() {
        let (id, ts) = fixed();
        let linked = row_hash(
            id,
            "o",
            "a",
            "created",
            "token",
            None,
            None,
            ts,
            Some("x"),
            None,
        );
        let placed = row_hash(
            id,
            "o",
            "a",
            "created",
            "token",
            None,
            None,
            ts,
            None,
            Some("x"),
        );
        assert_ne!(linked, placed);
        let both = row_hash(
            id,
            "o",
            "a",
            "created",
            "token",
            None,
            None,
            ts,
            Some("x"),
            Some("y"),
        );
        let swapped = row_hash(
            id,
            "o",
            "a",
            "created",
            "token",
            None,
            None,
            ts,
            Some("y"),
            Some("x"),
        );
        assert_ne!(both, swapped);
    }

    #[test]
    fn metadata_key_order_does_not_matter() {
        let (id, ts) = fixed();
        let a = row_hash(
            id,
            "o",
            "a",
            "created",
            "token",
            Some("r"),
            Some(&json!({ "x": 1, "y": 2 })),
            ts,
            None,
            None,
        );
        let b = row_hash(
            id,
            "o",
            "a",
            "created",
            "token",
            Some("r"),
            Some(&json!({ "y": 2, "x": 1 })),
            ts,
            None,
            None,
        );
        assert_eq!(a, b, "canonical JSON must sort keys");
    }
}
