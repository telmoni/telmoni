//! [`Verdict`] — a member's word on an incident.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// A member's word on an incident: it was worth being told, or it was
/// noise. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Worth being told.
    Useful,
    /// Not worth being told.
    Noise,
}

impl Verdict {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::Useful, Self::Noise]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Useful => "useful",
            Self::Noise => "noise",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Verdict {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "useful" => Ok(Self::Useful),
            "noise" => Ok(Self::Noise),
            _ => Err(ParseEnumError::new(s, "useful, noise")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Verdict; 2] = Verdict::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                Verdict::Useful | Verdict::Noise => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<Verdict>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["Useful", "spam", ""] {
            assert!(given.parse::<Verdict>().is_err(), "parsed: {given:?}");
        }
    }
}
