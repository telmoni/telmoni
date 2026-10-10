//! [`TelmoniResourceKind`] — the closed vocabulary of platform-side resource types.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// The closed set of platform-side resource kinds the audit log writes.
/// `[PUBLIC-API]`: a rename breaks SDK consumers and serialized audit rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TelmoniResourceKind {
    /// An organization — the top-level tenant boundary.
    Organization,
    /// A project — one container owned by an organization.
    Project,
    /// A project membership row or invite (role assignment within one project).
    Member,
    /// A `telmoni_` API token — minted, rotated, or revoked.
    Token,
    /// A browser sign-in, ended from another device. Distinct from `Token`, or
    /// "signed out my old laptop" would be filed under API-key activity.
    Session,
    /// A project's Slack, Discord or signed-webhook connection. The row names
    /// the vendor and channel or the endpoint's host — never the grant, the URL
    /// or the secret, because the chain is append-only.
    Connector,
}

impl TelmoniResourceKind {
    /// Every kind, in declaration order.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [
            Self::Organization,
            Self::Project,
            Self::Member,
            Self::Token,
            Self::Session,
            Self::Connector,
        ]
    }
}

impl fmt::Display for TelmoniResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Organization => write!(f, "organization"),
            Self::Project => write!(f, "project"),
            Self::Token => write!(f, "token"),
            Self::Session => write!(f, "session"),
            Self::Member => write!(f, "member"),
            Self::Connector => write!(f, "connector"),
        }
    }
}

impl FromStr for TelmoniResourceKind {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "organization" => Ok(Self::Organization),
            "project" => Ok(Self::Project),
            "member" => Ok(Self::Member),
            "token" => Ok(Self::Token),
            "session" => Ok(Self::Session),
            "connector" => Ok(Self::Connector),
            _ => Err(ParseEnumError::new(
                s,
                "organization, project, member, token, session, connector",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, for the round-trip below.
    const ALL: &[TelmoniResourceKind] = &TelmoniResourceKind::all();

    /// `ALL` covers the enum, and says so by count.
    #[test]
    fn all_lists_every_variant() {
        fn named(kind: TelmoniResourceKind) -> &'static str {
            match kind {
                TelmoniResourceKind::Organization => "organization",
                TelmoniResourceKind::Project => "project",
                TelmoniResourceKind::Member => "member",
                TelmoniResourceKind::Token => "token",
                TelmoniResourceKind::Session => "session",
                TelmoniResourceKind::Connector => "connector",
            }
        }
        assert_eq!(
            ALL.len(),
            6,
            "a variant reached the enum without reaching ALL"
        );
        for kind in ALL {
            assert_eq!(named(*kind), kind.to_string());
        }
    }

    #[test]
    fn display_matches_snake_case() {
        assert_eq!(TelmoniResourceKind::Project.to_string(), "project");
        assert_eq!(TelmoniResourceKind::Session.to_string(), "session");
    }

    /// The organization and project kinds are distinct spellings, and an audit
    /// reader depends on that: collapsing a pair would lose which one changed.
    #[test]
    fn the_two_levels_have_distinct_spellings() {
        let spellings = [
            TelmoniResourceKind::Organization,
            TelmoniResourceKind::Project,
            TelmoniResourceKind::Member,
        ]
        .map(|kind| kind.to_string());

        let mut unique = spellings.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            3,
            "two levels collapsed to one: {spellings:?}"
        );
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in ALL {
            assert_eq!(v.to_string().parse::<TelmoniResourceKind>().unwrap(), *v);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("PROJECT".parse::<TelmoniResourceKind>().is_err());
        assert!("projects".parse::<TelmoniResourceKind>().is_err());
        assert!("".parse::<TelmoniResourceKind>().is_err());
        assert!("subscription".parse::<TelmoniResourceKind>().is_err());
        // The retired observability product's kinds. They guard against its
        // stale values, not against the names: one that comes back as a
        // variant leaves this list in the same change.
        assert!("heartbeat".parse::<TelmoniResourceKind>().is_err());
        assert!("service".parse::<TelmoniResourceKind>().is_err());
        assert!("slo".parse::<TelmoniResourceKind>().is_err());
        assert!("alert_rule".parse::<TelmoniResourceKind>().is_err());
        assert!("incident".parse::<TelmoniResourceKind>().is_err());
        assert!("account".parse::<TelmoniResourceKind>().is_err());
        assert!("not_a_resource".parse::<TelmoniResourceKind>().is_err());
    }

    #[test]
    fn serde_uses_snake_case_wire() {
        assert_eq!(
            serde_json::to_string(&TelmoniResourceKind::Session).unwrap(),
            "\"session\""
        );
        let back: TelmoniResourceKind = serde_json::from_str("\"session\"").unwrap();
        assert_eq!(back, TelmoniResourceKind::Session);
    }
}
