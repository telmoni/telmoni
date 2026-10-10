//! [`SpanStatus`] — closed vocabulary of how a span ended.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// How a finished span ended — a run's outcome, on the run's own span. The
/// stable string a span's row carries. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl SpanStatus {
    /// Every status, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Ok, Self::Error, Self::Cancelled]
    }
}

impl fmt::Display for SpanStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ok => write!(f, "ok"),
            Self::Error => write!(f, "error"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl FromStr for SpanStatus {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ok" => Ok(Self::Ok),
            "error" => Ok(Self::Error),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(ParseEnumError::new(s, "ok, error, cancelled")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [SpanStatus; 3] = SpanStatus::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for status in ALL {
            match status {
                SpanStatus::Ok | SpanStatus::Error | SpanStatus::Cancelled => {}
            }
        }
        let mut seen: Vec<String> = ALL.iter().map(ToString::to_string).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len(), "all() repeats a variant");
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in &ALL {
            assert_eq!(v.to_string().parse::<SpanStatus>().unwrap(), *v);
        }
    }

    /// OpenTelemetry's own spellings are what ingest reads, never what a row
    /// carries.
    #[test]
    fn from_str_rejects_opentelemetrys_status_codes() {
        for code in ["STATUS_CODE_OK", "ERROR", "unset", "canceled", ""] {
            assert!(code.parse::<SpanStatus>().is_err(), "parsed: {code:?}");
        }
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
