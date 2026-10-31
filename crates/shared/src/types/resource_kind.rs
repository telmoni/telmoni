//! [`TelmoniResourceKind`] — the closed vocabulary of platform-side resource types.

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
    /// A `tm_` API token — minted, rotated, or revoked.
    Token,
    /// A browser sign-in, ended from another device. Distinct from `Token`, or
    /// "signed out my old laptop" would be filed under API-key activity.
    Session,
    /// A project's Slack or signed-webhook connection. The row names
    /// the vendor and channel or the endpoint's host — never the grant, the URL
    /// or the secret, because the chain is append-only.
    Connector,
    /// A project's alert: made, saved over, paused, resumed, a connector
    /// muted for it heard again, or deleted.
    Alert,
    /// A maintenance window over a project's alerts, opened or ended early.
    MaintenanceWindow,
    /// An alert's incident, acknowledged or resolved by a member.
    Incident,
    /// One trace of a project, deleted by a person.
    Trace,
    /// One end user of a project, erased: named by the keyed hash ingest
    /// stored, never the id the project sent.
    EndUser,
}

crate::wire_enum!(TelmoniResourceKind {
    Organization => "organization",
    Project => "project",
    Member => "member",
    Token => "token",
    Session => "session",
    Connector => "connector",
    Alert => "alert",
    MaintenanceWindow => "maintenance_window",
    Incident => "incident",
    Trace => "trace",
    EndUser => "end_user",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<TelmoniResourceKind>();
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
        assert!("monitor".parse::<TelmoniResourceKind>().is_err());
        assert!("ping_key".parse::<TelmoniResourceKind>().is_err());
        assert!("account".parse::<TelmoniResourceKind>().is_err());
        assert!("not_a_resource".parse::<TelmoniResourceKind>().is_err());
    }
}
