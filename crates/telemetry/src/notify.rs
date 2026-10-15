//! The notices this module raises — a monitor its first ping made, a ping
//! key its rotation retired — written here as auth's are in its own, and
//! handed to notifications through the outbox ([`crate::outbox`]) rather
//! than the seam directly, so a notice is never lost between its
//! transaction's commit and the seam's answer, and never told twice: each
//! carries a dedup key the seam answers a replay of with its first row.

use serde_json::json;

use telmoni_shared::NotificationKind;

use crate::db::{MonitorRow, PingKeyRow, ResolvedPingKey};
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
