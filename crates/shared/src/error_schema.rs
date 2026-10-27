//! RFC 9457 (obsoletes RFC 7807) problem-details schema — the customer-facing
//! wire shape of every service error. Tagged `[PUBLIC-API]`: field names and
//! the `type` vocabulary are a contract.

use serde::{Deserialize, Serialize};

/// RFC 9457 problem-details payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProblemDetails {
    /// Stable URI identifying the error category, e.g.
    #[serde(rename = "type")]
    pub type_uri: String,

    /// Short, human-readable title for the error. Constant per `type_uri`.
    pub title: String,

    /// HTTP status code — duplicates the response status.
    pub status: u16,

    /// Longer human-readable explanation; may include caller-supplied input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,

    /// Optional URI identifying this specific occurrence (request id, audit id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,

    /// Arbitrary extension members (RFC 9457 §3.2) — flattened in JSON.
    #[serde(flatten)]
    pub extensions: serde_json::Map<String, serde_json::Value>,
}

/// The extension that also rides as the answer's `Retry-After`.
pub const RETRY_AFTER_SECS: &str = "retry_after_secs";

impl ProblemDetails {
    /// A problem of `type_uri`, before its detail and extensions.
    #[must_use]
    pub fn new(type_uri: impl Into<String>, title: impl Into<String>, status: u16) -> Self {
        Self {
            type_uri: type_uri.into(),
            title: title.into(),
            status,
            ..Self::default()
        }
    }

    /// This occurrence's sentence.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// One extension member.
    #[must_use]
    pub fn extension(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.extensions.insert(key.into(), value.into());
        self
    }

    /// The `Retry-After` this problem asks for, whichever error wrote it.
    #[must_use]
    pub fn retry_after(&self) -> Option<u64> {
        self.extensions
            .get(RETRY_AFTER_SECS)
            .and_then(serde_json::Value::as_u64)
    }
}
