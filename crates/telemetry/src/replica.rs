//! Redis, as one replica holds it: the one `ConnectionManager` a replica
//! runs on, which reconnects on its own, made the first time it is asked for
//! rather than at boot — Redis may come up after the server, and a replica
//! that cannot reach it serves on, each limit held in the process instead
//! and the fact logged, as the console's limiter falls back.

use std::sync::OnceLock;
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
    /// The manager, once made, read without a lock from then on.
    manager: OnceLock<ConnectionManager>,
    /// When the last dial failed, held by the one dial under way: a caller
    /// that finds one stands in rather than wait out a Redis that is down.
    dial: Mutex<Option<Instant>>,
    /// Until when the replica stands in because a call on a live connection
    /// found Redis gone: the manager redials on its own, but without this
    /// every caller would wait out a response timeout of its own, and log
    /// it, for as long as the outage lasts.
    outage_until: std::sync::Mutex<Option<Instant>>,
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
            manager: OnceLock::new(),
            dial: Mutex::new(None),
            outage_until: std::sync::Mutex::new(None),
        })
    }

    /// The replica's connection, made on first use; `None`, logged, when
    /// Redis does not answer, which every caller takes as "stand in" — and,
    /// unlogged and dialling nothing, while another dials, for `STAND_IN_FOR`
    /// after a failed dial, or after a call found Redis gone (`stood_in`).
    pub async fn connection(&self) -> Option<ConnectionManager> {
        if self
            .outage_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|until| Instant::now() < until)
        {
            return None;
        }
        if let Some(manager) = self.manager.get() {
            return Some(manager.clone());
        }
        let Ok(mut failed_at) = self.dial.try_lock() else {
            return None;
        };
        // Made by a dial that ended as this caller came in.
        if let Some(manager) = self.manager.get() {
            return Some(manager.clone());
        }
        if failed_at.is_some_and(|failed_at| failed_at.elapsed() < STAND_IN_FOR) {
            return None;
        }
        let config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(CONNECT_TIMEOUT))
            .set_response_timeout(Some(RESPONSE_TIMEOUT))
            .set_number_of_retries(1);
        match ConnectionManager::new_with_config(self.client.clone(), config).await {
            Ok(manager) => {
                tracing::info!("telemetry: connected to Redis");
                *failed_at = None;
                // Only the dial's holder sets it, so it is never set already.
                let _ = self.manager.set(manager.clone());
                Some(manager)
            }
            Err(error) => {
                // The error names the host, never a password: `redis` strips
                // the URL's credentials from what it reports.
                tracing::warn!(
                    error = %error.kind_description(),
                    "telemetry: Redis did not answer; this replica stands in for it"
                );
                *failed_at = Some(Instant::now());
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tokio::net::TcpListener;

    use super::Redis;

    /// A caller that comes in while a dial is under way stands in at once.
    #[tokio::test]
    async fn a_caller_finding_a_dial_under_way_stands_in_at_once() {
        // Takes connections and never answers, as a Redis gone quiet.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let address = listener.local_addr().expect("its address");
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let redis = Arc::new(Redis::connect(&format!("redis://{address}")).expect("a Redis URL"));
        let dial_started = Instant::now();
        let dialling = tokio::spawn({
            let redis = Arc::clone(&redis);
            async move { redis.connection().await.is_some() }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let started = Instant::now();
        assert!(redis.connection().await.is_none());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "it waited on the dial"
        );

        let dialled = tokio::time::timeout(Duration::from_secs(20), dialling)
            .await
            .expect("the dial ends")
            .expect("the dial's task");
        assert!(!dialled);
        assert!(
            dial_started.elapsed() > Duration::from_millis(500),
            "the dial was not held, so the test proves nothing"
        );
    }
}
