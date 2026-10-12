//! Redis, as one replica holds it: the one `ConnectionManager` a replica
//! runs on, which reconnects on its own, made the first time it is asked for
//! rather than at boot — Redis may come up after the server, and a replica
//! that cannot reach it serves on, each limit held in the process instead
//! and the fact logged, as the console's limiter falls back.

use std::time::{Duration, Instant};

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use tokio::sync::Mutex;

/// How long one call waits for Redis to answer before the replica stands in.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long the first connection may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the replica stands in after a connection failed before it tries
/// again: a Redis that is down must not cost every export and every machine
/// request a dial of its own while it is.
const STAND_IN_FOR: Duration = Duration::from_secs(5);

/// Redis for this replica.
pub struct Redis {
    /// The URL, parsed; dials nothing by itself.
    client: redis::Client,
    /// The manager, once made, and when the last dial failed. Behind an
    /// async lock, so one replica makes one, and the callers that find it
    /// being made wait for it.
    manager: Mutex<Slot>,
    /// Until when the replica stands in because a call on a live connection
    /// found Redis gone: the manager redials on its own, but without this
    /// every caller would wait out a response timeout of its own, and log
    /// it, for as long as the outage lasts.
    outage_until: std::sync::Mutex<Option<Instant>>,
}

/// What the replica holds of its connection.
#[derive(Default)]
struct Slot {
    manager: Option<ConnectionManager>,
    /// The last dial that failed, while the replica stands in.
    failed_at: Option<Instant>,
}

impl Redis {
    /// Redis at `url` (`REDIS_URL`), not yet dialled.
    ///
    /// # Errors
    ///
    /// The URL is not a Redis URL.
    pub fn connect(url: &str) -> anyhow::Result<Self> {
        Ok(Self {
            client: redis::Client::open(url)?,
            manager: Mutex::new(Slot::default()),
            outage_until: std::sync::Mutex::new(None),
        })
    }

    /// The replica's connection, made on first use; `None`, logged, when
    /// Redis does not answer, which every caller takes as "stand in" — and
    /// again, unlogged and without a dial, for `STAND_IN_FOR` after that, or
    /// after a call on the connection found Redis gone (`stood_in`).
    pub async fn connection(&self) -> Option<ConnectionManager> {
        if self
            .outage_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|until| Instant::now() < until)
        {
            return None;
        }
        let mut slot = self.manager.lock().await;
        if let Some(manager) = slot.manager.as_ref() {
            return Some(manager.clone());
        }
        if slot
            .failed_at
            .is_some_and(|failed_at| failed_at.elapsed() < STAND_IN_FOR)
        {
            return None;
        }
        let config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(CONNECT_TIMEOUT))
            .set_response_timeout(Some(RESPONSE_TIMEOUT))
            .set_number_of_retries(1);
        match ConnectionManager::new_with_config(self.client.clone(), config).await {
            Ok(manager) => {
                tracing::info!("telemetry: connected to Redis");
                slot.manager = Some(manager.clone());
                slot.failed_at = None;
                Some(manager)
            }
            Err(error) => {
                // The error names the host, never a password: `redis` strips
                // the URL's credentials from what it reports.
                tracing::warn!(
                    error = %error.kind_description(),
                    "telemetry: Redis did not answer; this replica stands in for it"
                );
                slot.failed_at = Some(Instant::now());
                None
            }
        }
    }

    /// A call on the connection failed, and its caller stands in. Where the
    /// failure says Redis is unreachable, rather than that one command was
    /// refused, the replica stands in for `STAND_IN_FOR` before it asks
    /// again, and the outage is logged once for the window, not once a call.
    pub(crate) fn stood_in(&self, what: &str, error: &redis::RedisError) {
        let unreachable = error.is_io_error()
            || error.is_timeout()
            || error.is_connection_dropped()
            || error.is_connection_refusal();
        if unreachable {
            let mut until = self
                .outage_until
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let already = until.is_some_and(|until| now < until);
            *until = Some(now + STAND_IN_FOR);
            if already {
                return;
            }
        }
        tracing::warn!(
            error = %error.kind_description(),
            "telemetry: Redis failed on {what}; this replica stands in for it"
        );
    }
}

/// What a Redis error is, for a log line that must not carry its text: a
/// connection refused reads differently from a script that failed.
trait KindDescription {
    fn kind_description(&self) -> String;
}

impl KindDescription for redis::RedisError {
    fn kind_description(&self) -> String {
        format!("{:?}", self.kind())
    }
}
