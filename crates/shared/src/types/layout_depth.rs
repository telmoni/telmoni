//! [`LayoutDepth`] — how far a trace opens in a session's or a trace's layout.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// How far a trace opens: its line alone, its steps, or its steps and the
/// calls made inside them, nested as the spans recorded them. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutDepth {
    /// The trace's line alone.
    #[default]
    Trace,
    /// Its steps.
    Steps,
    /// Its steps and the calls inside them.
    Calls,
}

impl LayoutDepth {
    /// Every depth, in wire order: the shallowest first.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Trace, Self::Steps, Self::Calls]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Steps => "steps",
            Self::Calls => "calls",
        }
    }
}

impl fmt::Display for LayoutDepth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LayoutDepth {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trace" => Ok(Self::Trace),
            "steps" => Ok(Self::Steps),
            "calls" => Ok(Self::Calls),
            _ => Err(ParseEnumError::new(s, "trace, steps, calls")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [LayoutDepth; 3] = LayoutDepth::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for depth in ALL {
            match depth {
                LayoutDepth::Trace | LayoutDepth::Steps | LayoutDepth::Calls => {}
            }
        }
        assert_eq!(LayoutDepth::default(), LayoutDepth::Trace);
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<LayoutDepth>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["Trace", "spans", ""] {
            assert!(given.parse::<LayoutDepth>().is_err(), "parsed: {given:?}");
        }
    }
}
