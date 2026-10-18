//! [`LayoutOpen`] — which traces, and which branches of them, start open.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// Which of a session's traces, and which branches inside a trace, start
/// open: none, those that failed, or all of them. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutOpen {
    /// Every one shut until opened.
    #[default]
    None,
    /// Those holding a failed span open.
    Failed,
    /// Every one open.
    All,
}

impl LayoutOpen {
    /// Every choice, in wire order: the least open first.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::None, Self::Failed, Self::All]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Failed => "failed",
            Self::All => "all",
        }
    }
}

impl fmt::Display for LayoutOpen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LayoutOpen {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "failed" => Ok(Self::Failed),
            "all" => Ok(Self::All),
            _ => Err(ParseEnumError::new(s, "none, failed, all")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [LayoutOpen; 3] = LayoutOpen::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for open in ALL {
            match open {
                LayoutOpen::None | LayoutOpen::Failed | LayoutOpen::All => {}
            }
        }
        assert_eq!(LayoutOpen::default(), LayoutOpen::None);
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<LayoutOpen>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["None", "errors", ""] {
            assert!(given.parse::<LayoutOpen>().is_err(), "parsed: {given:?}");
        }
    }
}
