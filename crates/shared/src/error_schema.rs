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
