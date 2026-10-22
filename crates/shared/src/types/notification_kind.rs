//! [`NotificationKind`] — closed vocabulary of notification kinds.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

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
    /// A monitor made by its first ping, before its owner set it up.
    /// Telemetry raises it on the project's feed, once a monitor, with the
    /// slug and which key sent it in the metadata.
    MonitorDiscovered,
    /// A ping key its rotation retired, over a week past its last ping — or
    /// its minting, where none came — with a newer key of its project
    /// standing. Telemetry raises it on the project's feed, once a key, with
    /// the key's prefix in the metadata, so a line still carrying the key,
    /// refused from then on, can be found from the feed rather than from
    /// cron's mail.
    PingKeyRetired,
    /// An incident opened: a monitor moved into failed, stalled, over a bound
    /// or late, and needs someone. Telemetry raises it on the project's feed,
    /// once an incident, with the monitor, the cause and what moved it in the
    /// metadata; a flapping monitor's says so.
    IncidentOpened,
    /// An incident resolved: the monitor is healthy again, or its owner
    /// paused it. Telemetry raises it on the project's feed, once an
    /// incident, with how it closed and how long it stood in the metadata.
    IncidentResolved,
}

impl NotificationKind {
    /// Every kind, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 9] {
        [
            Self::OrganizationAlert,
            Self::MemberAdded,
            Self::MemberLeft,
            Self::ConnectorConnected,
            Self::ConnectorDisconnected,
            Self::MonitorDiscovered,
            Self::PingKeyRetired,
            Self::IncidentOpened,
            Self::IncidentResolved,
        ]
    }
}

impl fmt::Display for NotificationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OrganizationAlert => write!(f, "organization_alert"),
            Self::MemberAdded => write!(f, "member_added"),
            Self::MemberLeft => write!(f, "member_left"),
            Self::ConnectorConnected => write!(f, "connector_connected"),
            Self::ConnectorDisconnected => write!(f, "connector_disconnected"),
            Self::MonitorDiscovered => write!(f, "monitor_discovered"),
            Self::PingKeyRetired => write!(f, "ping_key_retired"),
            Self::IncidentOpened => write!(f, "incident_opened"),
            Self::IncidentResolved => write!(f, "incident_resolved"),
        }
    }
}

impl FromStr for NotificationKind {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "organization_alert" => Ok(Self::OrganizationAlert),
            "member_added" => Ok(Self::MemberAdded),
            "member_left" => Ok(Self::MemberLeft),
            "connector_connected" => Ok(Self::ConnectorConnected),
            "connector_disconnected" => Ok(Self::ConnectorDisconnected),
            "monitor_discovered" => Ok(Self::MonitorDiscovered),
            "ping_key_retired" => Ok(Self::PingKeyRetired),
            "incident_opened" => Ok(Self::IncidentOpened),
            "incident_resolved" => Ok(Self::IncidentResolved),
            _ => Err(ParseEnumError::new(
                s,
                "organization_alert, member_added, member_left, connector_connected, \
                 connector_disconnected, monitor_discovered, ping_key_retired, \
                 incident_opened, incident_resolved",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vocabulary, read from the enum rather than restated here.
    const ALL: [NotificationKind; 9] = NotificationKind::all();

    /// `all()` is a hand-written array, so nothing but a `match` forces it to
    /// stay complete: a new variant fails to compile here until it is listed.
    #[test]
    fn all_lists_every_variant() {
        for kind in ALL {
            match kind {
                NotificationKind::OrganizationAlert
                | NotificationKind::MemberAdded
                | NotificationKind::MemberLeft
                | NotificationKind::ConnectorConnected
                | NotificationKind::ConnectorDisconnected
                | NotificationKind::MonitorDiscovered
                | NotificationKind::PingKeyRetired
                | NotificationKind::IncidentOpened
                | NotificationKind::IncidentResolved => {}
            }
        }
        let mut seen: Vec<String> = ALL.iter().map(ToString::to_string).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len(), "all() repeats a variant");
    }

    #[test]
    fn display_matches_snake_case() {
        assert_eq!(NotificationKind::MemberAdded.to_string(), "member_added");
        assert_eq!(
            NotificationKind::OrganizationAlert.to_string(),
            "organization_alert"
        );
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in &ALL {
            assert_eq!(v.to_string().parse::<NotificationKind>().unwrap(), *v);
        }
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

    #[test]
    fn serde_uses_snake_case_wire() {
        assert_eq!(
            serde_json::to_string(&NotificationKind::MemberAdded).unwrap(),
            "\"member_added\""
        );
        let back: NotificationKind = serde_json::from_str("\"organization_alert\"").unwrap();
        assert_eq!(back, NotificationKind::OrganizationAlert);
    }
}
