//! Rate limits over Redis: GCRA on one timestamp per subject — the time its
//! next request is theoretically allowed — so a limit of 600 a minute costs
//! one key, not 600 entries; held in this process, per subject, while Redis
//! does not answer. Ingest holds one per project, the door the binary puts
//! in front of the machine lanes one per source and one per credential, and
//! the ping lane one per organization at the rate its plan sets
//! ([`RateLimit::wait_at`]), each a [`RateLimit`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use telmoni_shared::digest::sha256_hex;
use telmoni_shared::{TelmoniError, TenantError};

use crate::replica::Redis;

/// GCRA in one round trip: `KEYS[1]` the subject's key, `ARGV[1]` the
/// emission interval, `ARGV[2]` the burst and `ARGV[3]` now, all in
/// milliseconds. Answers `{1, 0}` to admit, `{0, wait_ms}` to refuse. The key
/// expires with the window it describes, so an idle subject costs nothing.
const GCRA: &str = r"
local tat = tonumber(redis.call('GET', KEYS[1]) or ARGV[3])
local now = tonumber(ARGV[3])
local interval = tonumber(ARGV[1])
local burst = tonumber(ARGV[2])
local new_tat = math.max(tat, now) + interval
local allow_at = new_tat - burst
if allow_at <= now then
  redis.call('SET', KEYS[1], new_tat, 'PX', new_tat - now)
  return {1, 0}
end
return {0, allow_at - now}
";

/// One limit, so many a minute per subject, as one replica holds it: the
/// script, and the timestamps it keeps itself while Redis does not answer.
pub struct RateLimit {
    /// What every key of this limit starts with, so two limits on one
    /// subject never share a timestamp.
    prefix: &'static str,
    /// How many a minute each subject may make.
    per_minute: u32,
    script: redis::Script,
    local: Mutex<HashMap<String, u64>>,
}

impl RateLimit {
    /// A limit of `per_minute` a minute per subject, its keys under `prefix`.
    /// A limit of nothing a minute would admit nothing ever, so it is one.
    #[must_use]
    pub fn new(prefix: &'static str, per_minute: u32) -> Self {
        Self {
            prefix,
            per_minute: per_minute.max(1),
            script: redis::Script::new(GCRA),
            local: Mutex::new(HashMap::new()),
        }
    }

    /// A credential's bucket, which never holds the credential: the first
    /// 32 hex characters of its SHA-256.
    #[must_use]
    pub fn digest(credential: &str) -> String {
        let mut hex = sha256_hex(credential.as_bytes());
        hex.truncate(32);
        hex
    }

    /// The key `subject`'s limit is held under.
    fn key(&self, subject: &str) -> String {
        format!("{}:{subject}", self.prefix)
    }

    /// Let `subject`'s request in, or refuse it with how long to wait.
    ///
    /// # Errors
    ///
    /// `/errors/tenant/rate-limited`, with `Retry-After`.
    pub async fn admit(&self, redis: &Redis, subject: &str) -> Result<(), TelmoniError> {
        match self
            .wait_at(redis, subject, self.per_minute, self.per_minute)
            .await
        {
            None => Ok(()),
            Some(wait_ms) => Err(TenantError::RateLimited {
                retry_after_secs: wait_ms.div_ceil(1_000).max(1),
            }
            .into()),
        }
    }

    /// `subject`'s request against `per_minute` a minute, `burst` of them at
    /// once, rather than the limit's own: `None` to let it in, or how long
    /// to wait, in milliseconds. For a rate each subject has its own of, as
    /// each organization's plan sets its pings.
    pub async fn wait_at(
        &self,
        redis: &Redis,
        subject: &str,
        per_minute: u32,
        burst: u32,
    ) -> Option<u64> {
        let interval = emission_interval_ms(per_minute);
        let burst = interval * u64::from(burst.max(1));
        let now = now_ms();
        let decision = match redis.connection().await {
            Some(mut connection) => {
                let answer: Result<(i64, i64), redis::RedisError> = self
                    .script
                    .key(self.key(subject))
                    .arg(interval)
                    .arg(burst)
                    .arg(now)
                    .invoke_async(&mut connection)
                    .await;
                match answer {
                    Ok((1, _)) => Decision::Admit,
                    Ok((_, wait_ms)) => Decision::Wait(u64::try_from(wait_ms).unwrap_or(0)),
                    Err(error) => {
                        redis.stood_in("a rate limit", &error);
                        self.admit_locally(subject, now, interval, burst)
                    }
                }
            }
            None => self.admit_locally(subject, now, interval, burst),
        };
        match decision {
            Decision::Admit => None,
            Decision::Wait(wait_ms) => Some(wait_ms),
        }
    }

    /// The same decision, from this process's own timestamps.
    fn admit_locally(&self, subject: &str, now: u64, interval: u64, burst: u64) -> Decision {
        let mut local = self
            .local
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tat = local.get(subject).copied().unwrap_or(now);
        let (decision, new_tat) = gcra(tat, now, interval, burst);
        if let Some(new_tat) = new_tat {
            local.insert(subject.to_owned(), new_tat);
        }
        // Nothing expires a process's own map, so what has lapsed is swept
        // as it grows, the way the key's own expiry would have.
        if local.len() > 10_000 {
            local.retain(|_, tat| *tat > now);
        }
        decision
    }
}

/// Between two requests at `per_minute` a minute, in milliseconds; a rate of
/// nothing a minute would admit nothing ever, so it is one.
fn emission_interval_ms(per_minute: u32) -> u64 {
    60_000 / u64::from(per_minute.max(1))
}

/// What the limit said.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Admit,
    /// Refused: how long to wait, in milliseconds.
    Wait(u64),
}

/// The script's arithmetic, in Rust, for the local map and the tests.
fn gcra(tat: u64, now: u64, interval_ms: u64, burst_ms: u64) -> (Decision, Option<u64>) {
    let new_tat = tat.max(now) + interval_ms;
    let allow_at = new_tat.saturating_sub(burst_ms);
    if allow_at <= now {
        (Decision::Admit, Some(new_tat))
    } else {
        (Decision::Wait(allow_at - now), None)
    }
}

/// Now, in milliseconds since the epoch: the clock the limit runs on.
fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PER_MINUTE: u32 = 600;

    fn limit() -> RateLimit {
        RateLimit::new("test:limit", PER_MINUTE)
    }

    /// A minute's worth goes through at once; the next waits for one
    /// emission interval, and a minute later the burst is whole again.
    #[test]
    fn gcra_admits_a_burst_then_meters() {
        let interval = emission_interval_ms(PER_MINUTE);
        let burst = interval * u64::from(PER_MINUTE);
        let now = 1_000_000;
        let mut tat = now;
        for n in 0..PER_MINUTE {
            let (decision, new_tat) = gcra(tat, now, interval, burst);
            assert_eq!(decision, Decision::Admit, "request {n} of the burst");
            tat = new_tat.expect("admitted");
        }
        let (decision, _) = gcra(tat, now, interval, burst);
        assert_eq!(decision, Decision::Wait(interval));
        let (decision, _) = gcra(tat, now + interval, interval, burst);
        assert_eq!(decision, Decision::Admit, "one interval later, one more");
        let (decision, _) = gcra(tat, now + 60_000, interval, burst);
        assert_eq!(decision, Decision::Admit);
    }

    #[test]
    fn the_local_map_refuses_as_the_script_would_and_names_whole_seconds() {
        let limit = limit();
        let now = now_ms();
        let interval = emission_interval_ms(PER_MINUTE);
        let burst = interval * u64::from(PER_MINUTE);
        for _ in 0..PER_MINUTE {
            assert_eq!(
                limit.admit_locally("one", now, interval, burst),
                Decision::Admit
            );
        }
        let Decision::Wait(wait_ms) = limit.admit_locally("one", now, interval, burst) else {
            panic!("the burst was not spent");
        };
        assert_eq!(wait_ms, interval);
        assert_eq!(
            wait_ms.div_ceil(1_000).max(1),
            1,
            "told to retry after a second"
        );
        assert_eq!(
            limit.admit_locally("another", now, interval, burst),
            Decision::Admit,
            "another subject's limit is its own"
        );
    }

    /// A plan's rate with a burst of its own: two a minute, twelve at once,
    /// then one each thirty seconds.
    #[test]
    fn a_rate_and_burst_of_the_subjects_own_meter_as_given() {
        let limit = RateLimit::new("test:plan", 600);
        let interval = emission_interval_ms(2);
        let burst = interval * 12;
        let now = now_ms();
        for n in 0..12 {
            assert_eq!(
                limit.admit_locally("org", now, interval, burst),
                Decision::Admit,
                "ping {n} of the burst"
            );
        }
        assert_eq!(
            limit.admit_locally("org", now, interval, burst),
            Decision::Wait(30_000)
        );
        assert_eq!(emission_interval_ms(0), 60_000, "nothing a minute is one");
    }

    /// A smaller limit meters sooner, and two limits on one subject keep
    /// their keys apart.
    #[test]
    fn each_limit_has_its_own_rate_and_keys() {
        let small = RateLimit::new("test:small", 2);
        let now = now_ms();
        let (interval, burst) = (emission_interval_ms(2), emission_interval_ms(2) * 2);
        assert_eq!(
            small.admit_locally("s", now, interval, burst),
            Decision::Admit
        );
        assert_eq!(
            small.admit_locally("s", now, interval, burst),
            Decision::Admit
        );
        assert!(matches!(
            small.admit_locally("s", now, interval, burst),
            Decision::Wait(_)
        ));
        assert_ne!(small.key("s"), limit().key("s"));
        assert_eq!(
            RateLimit::new("x", 0).per_minute,
            1,
            "nothing a minute is one"
        );
    }

    #[test]
    fn a_digest_names_a_credential_without_holding_it() {
        let digest = RateLimit::digest("Bearer telmoni_secret");
        assert_eq!(digest.len(), 32);
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!digest.contains("telmoni"));
        assert_eq!(digest, RateLimit::digest("Bearer telmoni_secret"));
        assert_ne!(digest, RateLimit::digest("Bearer telmoni_other"));
    }
}
