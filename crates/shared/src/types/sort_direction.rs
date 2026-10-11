//! [`SortDirection`] — closed vocabulary of the two ways a list is sorted.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// Which way a console list is sorted, named in the URL beside the column so
/// a link holds both. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    /// Smallest, earliest or first in the alphabet first.
    Asc,
    /// Largest, latest or last in the alphabet first.
    Desc,
}

impl SortDirection {
    /// Both directions, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::Asc, Self::Desc]
    }
}

impl fmt::Display for SortDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Asc => write!(f, "asc"),
            Self::Desc => write!(f, "desc"),
        }
    }
}

impl FromStr for SortDirection {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "asc" => Ok(Self::Asc),
            "desc" => Ok(Self::Desc),
            _ => Err(ParseEnumError::new(s, "asc, desc")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [SortDirection; 2] = SortDirection::all();

    #[test]
    fn all_lists_every_variant() {
        for direction in ALL {
            match direction {
                SortDirection::Asc | SortDirection::Desc => {}
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
            assert_eq!(v.to_string().parse::<SortDirection>().unwrap(), *v);
        }
        assert!("ascending".parse::<SortDirection>().is_err());
        assert!("DESC".parse::<SortDirection>().is_err());
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
