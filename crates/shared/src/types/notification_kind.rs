//! [`NotificationKind`] — closed vocabulary of notification kinds.

use serde::{Deserialize, Serialize};

/// Closed set of notification kinds, the stable string a feed row carries.
/// `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// An alert for the organization's owner and admins, raised by a service
    /// beside this repository's, which writes its own title and body. It lands
    /// on the organization's feed, never a project's.
    OrganizationAlert,
    /// Somebody joined a project: accepted an invitation to it, or was given
    /// a seat on it from the organization's roster.
    MemberAdded,
    /// A member left or was removed from a project.
    MemberLeft,
    /// A Slack channel or a webhook was connected to the project.
    ConnectorConnected,
    /// A connection stopped: the vendor answered that the installation is gone
    /// or the target refuses posts. The feed carries the news because the
    /// channel that would have carried it is the one that stopped.
    ConnectorDisconnected,
    /// An alert breached: an incident opened, or one that recovered within
    /// the hour reopened; a reminder of one held is told under it too, marked
    /// as one in its metadata.
    IncidentOpened,
    /// An alert recovered, or a member resolved its incident.
    IncidentResolved,
}

crate::wire_enum!(NotificationKind {
    OrganizationAlert => "organization_alert",
    MemberAdded => "member_added",
    MemberLeft => "member_left",
    ConnectorConnected => "connector_connected",
    ConnectorDisconnected => "connector_disconnected",
    IncidentOpened => "incident_opened",
    IncidentResolved => "incident_resolved",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<NotificationKind>();
    }

    #[test]
    fn from_str_rejects_legacy_string_values() {
        for legacy in [
            "welcome",
            "deploy_failure",
            "schedule_failed",
            "run_egress_denied",
            "run_policy_violation",
            "run_failed",
            "welcome_email",
            "organization_deleted",
            "token_rotated",
            "heartbeat_down",
            "heartbeat_recovered",
            "monitor_discovered",
            "ping_key_retired",
        ] {
            assert!(
                legacy.parse::<NotificationKind>().is_err(),
                "stale value parsed: {legacy}"
            );
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("WelcomeEmail".parse::<NotificationKind>().is_err());
        assert!("welcome".parse::<NotificationKind>().is_err());
        assert!("".parse::<NotificationKind>().is_err());
    }
}
