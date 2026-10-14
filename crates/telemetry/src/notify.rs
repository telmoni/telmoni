//! The notices this module raises — a monitor its first ping made — written
//! here as auth's and billing's are in theirs, and handed
//! to notifications through the outbox ([`crate::outbox`]) rather than the
//! seam directly, so a notice is never lost between a ping's commit and the
//! seam's answer, and never told twice: each carries a dedup key the seam
//! answers a replay of with its first row.

use serde_json::json;

use telmoni_shared::NotificationKind;

use crate::db::{MonitorRow, ResolvedPingKey};
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
