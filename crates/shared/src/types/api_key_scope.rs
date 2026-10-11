//! [`ApiKeyScope`] — what an API key may be used for.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

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

impl ApiKeyScope {
    /// Every scope, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::Read, Self::Ingest]
    }
}

impl fmt::Display for ApiKeyScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => write!(f, "read"),
            Self::Ingest => write!(f, "ingest"),
        }
    }
}

impl FromStr for ApiKeyScope {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read" => Ok(Self::Read),
            "ingest" => Ok(Self::Ingest),
            _ => Err(ParseEnumError::new(s, "read, ingest")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ApiKeyScope; 2] = ApiKeyScope::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for scope in ALL {
            match scope {
                ApiKeyScope::Read | ApiKeyScope::Ingest => {}
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
            assert_eq!(v.to_string().parse::<ApiKeyScope>().unwrap(), *v);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        for scope in ["Read", "write", "admin", "ingest ", ""] {
            assert!(scope.parse::<ApiKeyScope>().is_err(), "parsed: {scope:?}");
        }
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
