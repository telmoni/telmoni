//! The ping keys' rotation. Two keys may be live at once, so a project moves
//! its crontab lines over one at a time, and the older is retired a week
//! after its last ping once the newer stands — by a loop of this module's
//! own, a tick a minute on every replica, each tick safe beside the others'
//! since a key is retired once and recorded once
//! ([`crate::handler::ping_keys::retire_stale_keys`]). A tick that fails is
//! logged, and the next one runs.

use std::sync::Arc;
use std::time::Duration;

use crate::AppState;
use crate::handler::ping_keys;

/// How long after its last ping a replaced key is retired: a week, since a
/// crontab line is edited by hand and a weekly job pings once in it.
pub const REPLACED_AFTER_S: i64 = 7 * 24 * 60 * 60;

/// How often a tick looks.
const EVERY: Duration = Duration::from_secs(60);

/// How long after boot the first tick runs: past the startup probes.
const START_DELAY: Duration = Duration::from_secs(60);

/// The rotation, forever: the first tick after `START_DELAY`, then one
/// every `EVERY`; a tick that overruns delays the next rather than
/// stacking.
pub async fn run(state: Arc<AppState>) {
    tokio::time::sleep(START_DELAY).await;
    let mut ticker = tokio::time::interval(EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match ping_keys::retire_stale_keys(&state).await {
            Ok(0) => {}
            Ok(retired) => {
                tracing::info!(retired, "telemetry: ping keys retired by their rotation");
            }
            Err(e) => tracing::warn!(
                error = %e,
                "telemetry: the ping keys' rotation tick failed; the next one retries"
            ),
        }
    }
}
