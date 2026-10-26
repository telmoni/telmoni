//! [`IncidentStatus`] — where an incident stands.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// Where an incident stands: `open` from its breach, `acknowledged` once a
/// member has taken it, and `resolved` at its recovery or by a member's
/// hand. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    /// Breached, and nobody has taken it.
    #[default]
    Open,
    /// Taken by a member: no more reminders.
    Acknowledged,
    /// Recovered, or resolved by a member.
    Resolved,
}

impl IncidentStatus {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Open, Self::Acknowledged, Self::Resolved]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Acknowledged => "acknowledged",
            Self::Resolved => "resolved",
        }
    }
}

impl fmt::Display for IncidentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IncidentStatus {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(Self::Open),
            "acknowledged" => Ok(Self::Acknowledged),
            "resolved" => Ok(Self::Resolved),
            _ => Err(ParseEnumError::new(s, "open, acknowledged, resolved")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [IncidentStatus; 3] = IncidentStatus::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                IncidentStatus::Open | IncidentStatus::Acknowledged | IncidentStatus::Resolved => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<IncidentStatus>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["closed", "Open", ""] {
            assert!(
                given.parse::<IncidentStatus>().is_err(),
                "parsed: {given:?}"
            );
        }
    }
}
