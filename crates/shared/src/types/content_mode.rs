//! [`ContentMode`] — what a project keeps of the content its spans carry.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// What a project keeps of prompts, completions and tool payloads: nothing
/// unless an organization owner says otherwise. Spans are kept in every mode;
/// content never reaches ClickHouse in any. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ContentMode {
    /// Every content field is dropped at the door, the span kept. A project
    /// with no settings of its own reads as this.
    #[default]
    Off,
    /// Content is kept as it arrives, in Postgres alone.
    On,
    /// Only content the SDK encrypted under the customer's key is kept,
    /// in Postgres alone; the key never reaches the server.
    Sealed,
}

impl ContentMode {
    /// Every mode, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Off, Self::On, Self::Sealed]
    }
}

impl fmt::Display for ContentMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => write!(f, "off"),
            Self::On => write!(f, "on"),
            Self::Sealed => write!(f, "sealed"),
        }
    }
}

impl FromStr for ContentMode {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" => Ok(Self::Off),
            "on" => Ok(Self::On),
            "sealed" => Ok(Self::Sealed),
            _ => Err(ParseEnumError::new(s, "off, on, sealed")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ContentMode; 3] = ContentMode::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for mode in ALL {
            match mode {
                ContentMode::Off | ContentMode::On | ContentMode::Sealed => {}
            }
        }
        let mut seen: Vec<String> = ALL.iter().map(ToString::to_string).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len(), "all() repeats a variant");
    }

    /// ⚠ Off until an owner changes it: the default is what a project with
    /// no settings row reads as.
    #[test]
    fn the_default_keeps_no_content() {
        assert_eq!(ContentMode::default(), ContentMode::Off);
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in &ALL {
            assert_eq!(v.to_string().parse::<ContentMode>().unwrap(), *v);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        for mode in ["Off", "plain", "encrypted", "true", ""] {
            assert!(mode.parse::<ContentMode>().is_err(), "parsed: {mode:?}");
        }
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
