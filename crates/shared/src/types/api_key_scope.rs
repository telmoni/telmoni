//! [`ApiKeyScope`] — what an API key may be used for.

use serde::{Deserialize, Serialize};

/// One thing an API key opens. A key holds one scope or both, chosen when it
/// is minted, so a key baked into an agent's process can send its spans and
/// read nothing. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyScope {
    /// The read lanes under `/v1`.
    Read,
    /// Sending spans to `/otlp/v1/traces`.
    Ingest,
}

crate::wire_enum!(ApiKeyScope {
    Read => "read",
    Ingest => "ingest",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<ApiKeyScope>();
    }

    #[test]
    fn from_str_rejects_unknown() {
        for scope in ["Read", "write", "admin", "ingest ", ""] {
            assert!(scope.parse::<ApiKeyScope>().is_err(), "parsed: {scope:?}");
        }
    }
}
