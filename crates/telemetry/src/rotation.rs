//! The ping keys' rotation. Two keys may be live at once, so a project moves
//! its crontab lines over one at a time, and the older is retired a week
//! after its last ping once the newer stands — by a loop of this module's
//! own, a tick a minute on every replica, each tick safe beside the others'
//! since a key is retired, recorded and told to its project once
//! ([`crate::handler::ping_keys::retire_stale_keys`]). A key that cannot be
//! retired now is logged and left for a later tick, and the tick goes on to
//! the rest. A tick that fails — its read of the stale keys, or a database
//! that stops answering — is logged, and the next one runs; so is a tick
//! that panics, each tick being a task of its own, so that a key whose
//! retirement panics ends that tick alone — never the loop, which a restart
//! would not mend, since the next loop meets the same key, and never the
//! server.
//!
//! ⚠ The loop's handle is raced against the server in the binary's `serve`,
//! as the outbox's is, for a loop that ends all the same: a dead loop would
//! retire no key on its replica for as long as the pod ran, and no sweep or
//! command retires one in its place, so the process exits and the restart
//! is the recovery.

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
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + START_DELAY, EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let tick = tokio::spawn({
            let state = state.clone();
            async move { ping_keys::retire_stale_keys(&state).await }
        });
        match tick.await {
            Ok(Ok(ping_keys::Rotated { retired: 0, .. })) => {}
            Ok(Ok(ping_keys::Rotated { retired, left })) => {
                tracing::info!(
                    retired,
                    left,
                    "telemetry: ping keys retired by their rotation"
                );
            }
            Ok(Err(e)) => tracing::warn!(
                error = %e,
                "telemetry: the ping keys' rotation tick failed; the next one retries"
            ),
            Err(e) => tracing::error!(
                error = %e,
                "telemetry: the ping keys' rotation tick died; the next one retries"
            ),
        }
    }
}
