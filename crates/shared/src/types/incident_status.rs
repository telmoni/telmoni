//! [`IncidentStatus`] — where an incident stands.

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

crate::wire_enum!(IncidentStatus {
    Open => "open",
    Acknowledged => "acknowledged",
    Resolved => "resolved",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<IncidentStatus>();
    }

    #[test]
    fn from_str_rejects_the_rest() {
        for given in ["closed", "Open", ""] {
            assert!(
                given.parse::<IncidentStatus>().is_err(),
                "parsed: {given:?}"
            );
        }
    }
}
