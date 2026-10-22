//! The notices this module raises — a monitor its first ping made, a ping
//! key its rotation retired, an incident opened or resolved — written here
//! as auth's are in its own, and handed to notifications through the outbox
//! ([`crate::outbox`]) rather than the seam directly, so a notice is never
//! lost between its transaction's commit and the seam's answer, and never
//! told twice: each carries a dedup key the seam answers a replay of with
//! its first row.

use chrono::Duration;
use serde_json::json;

use telmoni_shared::{MonitorState, NotificationKind};

use crate::db::{IncidentRow, MonitorRow, PingKeyRow, ResolvedPingKey};
use crate::incidents::{Resolution, Trigger};
use crate::outbox::QueuedNotice;

/// The owner told of a monitor its first ping made, once a monitor: the slug
/// and which key sent it, in the body and the metadata, for the monitor's
/// page to be led to once it has one.
#[must_use]
pub fn monitor_discovered(monitor: &MonitorRow, key: &ResolvedPingKey) -> QueuedNotice {
    QueuedNotice {
        kind: NotificationKind::MonitorDiscovered,
        title: format!("New monitor: {}", monitor.slug),
        body: format!(
            "A ping under the key {}… named a monitor this project did not have, so it was \
             made and paused as new: its runs are kept, and it watches for nothing until you \
             set it up.",
            key.prefix
        ),
        metadata: json!({
            "slug": monitor.slug,
            "monitor_id": monitor.id,
            "ping_key_id": key.id,
            "ping_key_prefix": key.prefix,
            "made": true,
        }),
        dedup_key: format!("telemetry:monitor:{}:discovered", monitor.id),
    }
}

/// The project told of a monitor its agent's first run made, once a monitor,
/// as a ping's first is told: the agent in the body and the metadata, so its
/// owner can find what is sending and set it up.
#[must_use]
pub fn monitor_discovered_by_run(monitor: &MonitorRow) -> QueuedNotice {
    QueuedNotice {
        kind: NotificationKind::MonitorDiscovered,
        title: format!("New monitor: {}", monitor.slug),
        body: format!(
            "A run of the agent {} came with no monitor watching it, so one was made and paused \
             as new: its runs are kept, and it watches for nothing until you set it up.",
            monitor.agent
        ),
        metadata: json!({
            "slug": monitor.slug,
            "monitor_id": monitor.id,
            "agent": monitor.agent,
            "made": true,
        }),
        dedup_key: format!("telemetry:monitor:{}:discovered", monitor.id),
    }
}

/// The project told of a key its rotation retired, once a key: the key's
/// prefix in the body and the metadata, so the crontab line still carrying
/// it, refused from now on, can be found from the feed rather than from
/// cron's mail.
#[must_use]
pub fn ping_key_retired(key: &PingKeyRow) -> QueuedNotice {
    let reason = if key.last_ping_at.is_some() {
        "had not pinged in over a week"
    } else {
        "was minted over a week ago and never pinged"
    };
    QueuedNotice {
        kind: NotificationKind::PingKeyRetired,
        title: format!("Ping key retired: {}…", key.prefix),
        body: format!(
            "The rotation retired the ping key {}…: a newer key of this project stands, and \
             this one {reason}. A line still using it is refused from now on; move it to the \
             newer key.",
            key.prefix
        ),
        metadata: json!({
            "ping_key_id": key.id,
            "ping_key_prefix": key.prefix,
        }),
        dedup_key: format!("telemetry:ping-key:{}:retired", key.id),
    }
}

/// The project told of an incident, once: the monitor, what moved it, and
/// whether it is flapping, for the alerts page to lead to once it has one.
#[must_use]
pub fn incident_opened(
    monitor: &MonitorRow,
    incident: &IncidentRow,
    trigger: Trigger,
) -> QueuedNotice {
    let flapping = if incident.flapping {
        " It has flipped in and out of trouble more than six times in an hour, so this incident \
         stays open until it has held one state for an hour."
    } else {
        ""
    };
    let exit_code = match trigger {
        Trigger::RunFailed { exit_code } => exit_code,
        _ => None,
    };
    QueuedNotice {
        kind: NotificationKind::IncidentOpened,
        title: format!("{} {}", monitor.slug, said(incident.cause)),
        body: format!(
            "The monitor {} needs someone: {}.{flapping}",
            monitor.slug,
            trigger.clause()
        ),
        metadata: json!({
            "incident_id": incident.id,
            "monitor_id": monitor.id,
            "slug": monitor.slug,
            "cause": incident.cause,
            "detail": trigger.as_str(),
            "exit_code": exit_code,
            "flapping": incident.flapping,
            "opened_at": incident.opened_at,
        }),
        dedup_key: format!("telemetry:incident:{}:opened", incident.id),
    }
}

/// The project told an incident closed, once: how, and how long it stood.
#[must_use]
pub fn incident_resolved(
    monitor: &MonitorRow,
    incident: &IncidentRow,
    resolution: Resolution,
) -> QueuedNotice {
    let resolved_at = incident.resolved_at.unwrap_or(incident.opened_at);
    let lasted = resolved_at - incident.opened_at;
    let when = Trigger::from_word(&incident.detail)
        .map_or_else(|| incident.cause.to_string(), Trigger::clause);
    let (title, body) = match resolution {
        Resolution::Recovered => (
            format!("{} recovered", monitor.slug),
            format!(
                "The monitor {} is healthy again after {}; the incident opened when {when}.",
                monitor.slug,
                words(lasted)
            ),
        ),
        Resolution::Paused => (
            format!("{} paused", monitor.slug),
            format!(
                "The monitor {} was paused by its owner after {}, closing the incident that \
                 opened when {when}.",
                monitor.slug,
                words(lasted)
            ),
        ),
    };
    QueuedNotice {
        kind: NotificationKind::IncidentResolved,
        title,
        body,
        metadata: json!({
            "incident_id": incident.id,
            "monitor_id": monitor.id,
            "slug": monitor.slug,
            "cause": incident.cause,
            "detail": incident.detail,
            "resolution": resolution.as_str(),
            "opened_at": incident.opened_at,
            "resolved_at": resolved_at,
            "duration_s": lasted.num_seconds().max(0),
            "flapping": incident.flapping,
        }),
        dedup_key: format!("telemetry:incident:{}:resolved", incident.id),
    }
}

/// The state in words, after the slug.
const fn said(state: MonitorState) -> &'static str {
    match state {
        MonitorState::Failed => "failed",
        MonitorState::Stalled => "stalled",
        MonitorState::OverBound => "is over a bound",
        MonitorState::Late => "is late",
        MonitorState::Running
        | MonitorState::New
        | MonitorState::Paused
        | MonitorState::Healthy => "needs someone",
    }
}

/// A span of time in words: `42 s`, `4 min`, `1 h 12 min`, `2 d 3 h`.
fn words(lasted: Duration) -> String {
    let secs = lasted.num_seconds().max(0);
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h {} min", s / 3600, (s % 3600) / 60),
        s => format!("{} d {} h", s / 86_400, (s % 86_400) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_of_time_in_words() {
        assert_eq!(words(Duration::seconds(42)), "42 s");
        assert_eq!(words(Duration::seconds(4 * 60 + 59)), "4 min");
        assert_eq!(words(Duration::seconds(3600 + 12 * 60)), "1 h 12 min");
        assert_eq!(words(Duration::seconds(2 * 86_400 + 3 * 3600)), "2 d 3 h");
        assert_eq!(
            words(Duration::seconds(-5)),
            "0 s",
            "clocks apart read as none"
        );
    }
}
