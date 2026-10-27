//! [`SpanStatus`] — closed vocabulary of how a span ended.

use serde::{Deserialize, Serialize};

/// How a finished span ended — a run's outcome, on the run's own span. The
/// stable string a span's row carries. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    /// It finished.
    Ok,
    /// Its status was `ERROR`; its `error_type` names the exception's class.
    Error,
    /// It was stopped before it finished (`telmoni.cancelled`), which no
    /// alert counts as a failure.
    Cancelled,
}

crate::wire_enum!(SpanStatus {
    Ok => "ok",
    Error => "error",
    Cancelled => "cancelled",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<SpanStatus>();
    }

    /// OpenTelemetry's own spellings are what ingest reads, never what a row
    /// carries.
    #[test]
    fn from_str_rejects_opentelemetrys_status_codes() {
        for code in ["STATUS_CODE_OK", "ERROR", "unset", "canceled", ""] {
            assert!(code.parse::<SpanStatus>().is_err(), "parsed: {code:?}");
        }
    }
}
