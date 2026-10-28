//! The tag-cardinality cap: in a project, a tag's key takes up to
//! [`TAG_VALUES_PER_DAY`] values a UTC day, and the project up to
//! [`TAG_KEYS_PER_DAY`] keys; a tag past either is dropped as `cardinality`,
//! the span kept. The day's keys, and each key's values, are sets in Redis of
//! 64-bit digests, which Redis keeps as intsets of a few KiB, gone once the
//! day ends. Without Redis every tag is kept: a replica's own count would
//! hold each to the cap once per replica.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, Utc};
use sha2::{Digest as _, Sha256};

use telmoni_shared::ProjectId;

use crate::contract::{TAG_KEYS_PER_DAY, TAG_PREFIX, TAG_VALUES_PER_DAY};
use crate::ingest::report::Drops;
use crate::ingest::span::{self, Cap, DropReason};
use crate::replica::Redis;
use crate::store::SpanRow;

/// The cap's script, hashed once rather than for every export.
static CAP_SCRIPT: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(CAP));

/// The cap, in one round trip. `KEYS[1]` is the project's keys of the day,
/// `KEYS[1 + i]` the values of the export's `i`th key; `ARGV` the values'
/// cap, the keys' cap, when the day's sets expire in Unix seconds, each key's
/// digest, then each tag as its key's `i` and its value's digest. Answers
/// each tag's verdict in order: 0 kept, 1 past the values' cap, 2 past the
/// keys'.
const CAP: &str = r"
local values_cap = tonumber(ARGV[1])
local keys_cap = tonumber(ARGV[2])
local expire_at = tonumber(ARGV[3])
local keys = #KEYS - 1
local open = {}
for i = 1, keys do
  if redis.call('SISMEMBER', KEYS[1], ARGV[3 + i]) == 1 then
    open[i] = true
  elseif redis.call('SCARD', KEYS[1]) < keys_cap then
    redis.call('SADD', KEYS[1], ARGV[3 + i])
    open[i] = true
  end
end
redis.call('EXPIREAT', KEYS[1], expire_at)
local verdicts = {}
for n = 4 + keys, #ARGV, 2 do
  local i = tonumber(ARGV[n])
  local verdict = 2
  if open[i] then
    if redis.call('SISMEMBER', KEYS[1 + i], ARGV[n + 1]) == 1 then
      verdict = 0
    elseif redis.call('SCARD', KEYS[1 + i]) < values_cap then
      redis.call('SADD', KEYS[1 + i], ARGV[n + 1])
      redis.call('EXPIREAT', KEYS[1 + i], expire_at)
      verdict = 0
    else
      verdict = 1
    end
  end
  verdicts[#verdicts + 1] = verdict
end
return verdicts
";

/// How long a day's sets outlive its midnight, for a replica whose clock is
/// a little behind.
const KEPT_PAST_MIDNIGHT_S: i64 = 5 * 60;

/// What became of one tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Kept,
    PastValues,
    PastKeys,
}

impl Verdict {
    fn of(answer: i64) -> Self {
        match answer {
            1 => Self::PastValues,
            2 => Self::PastKeys,
            _ => Self::Kept,
        }
    }

    fn reason(self) -> Option<DropReason> {
        match self {
            Self::Kept => None,
            Self::PastValues => Some(DropReason::Cardinality(Cap::TagValuesPerDay)),
            Self::PastKeys => Some(DropReason::Cardinality(Cap::TagKeysPerDay)),
        }
    }
}

/// `rows`' tags held to their project's cap for the day `received_at` falls
/// in: each past it taken off its row, which is counted again, and pushed to
/// `dropped`.
pub async fn hold(
    redis: &Redis,
    project_id: &ProjectId,
    received_at: DateTime<Utc>,
    rows: &mut [SpanRow],
    dropped: &mut Drops,
) {
    // The export's keys and tags, each once, in the order they came, and
    // each row's tags as places among them.
    let mut keys: Vec<&str> = Vec::new();
    let mut tags: Vec<(usize, &str)> = Vec::new();
    let mut places: Vec<Vec<usize>> = Vec::with_capacity(rows.len());
    let mut key_at: HashMap<&str, usize> = HashMap::new();
    let mut tag_at: HashMap<(usize, &str), usize> = HashMap::new();
    for row in rows.iter() {
        let mut place = Vec::with_capacity(row.tags.len());
        for (key, value) in &row.tags {
            let k = *key_at.entry(key.as_str()).or_insert_with(|| {
                keys.push(key);
                keys.len() - 1
            });
            let t = *tag_at.entry((k, value.as_str())).or_insert_with(|| {
                tags.push((k, value));
                tags.len() - 1
            });
            place.push(t);
        }
        places.push(place);
    }
    if tags.is_empty() {
        return;
    }
    let Some(verdicts) = ask(redis, project_id, received_at.date_naive(), &keys, &tags).await
    else {
        return;
    };

    for (row, place) in rows.iter_mut().zip(places) {
        let mut place = place.into_iter();
        let before = row.tags.len();
        let span_id = &row.span_id;
        row.tags.retain(|(key, _)| {
            let verdict = place
                .next()
                .and_then(|t| verdicts.get(t).copied())
                .unwrap_or(Verdict::Kept);
            let Some(reason) = verdict.reason() else {
                return true;
            };
            dropped.push(span_id, &format!("{TAG_PREFIX}{key}"), reason);
            false
        });
        if row.tags.len() < before {
            row.units = span::units_of(row);
        }
    }
}

/// Each tag's verdict, in order; `None`, every tag kept, where Redis does not
/// answer.
async fn ask(
    redis: &Redis,
    project_id: &ProjectId,
    day: NaiveDate,
    keys: &[&str],
    tags: &[(usize, &str)],
) -> Option<Vec<Verdict>> {
    let mut connection = redis.connection().await?;
    let day_key = day_key(project_id, day);
    let hashed: Vec<String> = keys.iter().map(|key| digest(key)).collect();
    let mut invocation = CAP_SCRIPT.prepare_invoke();
    invocation.key(&day_key);
    for key in &hashed {
        invocation.key(format!("{day_key}:{key}"));
    }
    invocation
        .arg(TAG_VALUES_PER_DAY)
        .arg(TAG_KEYS_PER_DAY)
        .arg(expire_at(day));
    for key in &hashed {
        invocation.arg(key);
    }
    for (k, value) in tags {
        invocation.arg(k + 1).arg(digest(value));
    }
    let answer: Result<Vec<i64>, redis::RedisError> =
        invocation.invoke_async(&mut connection).await;
    match answer {
        Ok(verdicts) => Some(verdicts.into_iter().map(Verdict::of).collect()),
        Err(error) => {
            redis.stood_in("the tag-cardinality cap", &error);
            None
        }
    }
}

/// The set of the project's keys of the day; each key's values sit under it,
/// by the key's digest.
fn day_key(project_id: &ProjectId, day: NaiveDate) -> String {
    format!("telemetry:ingest:tags:{project_id}:{day}")
}

/// A 64-bit digest of `text`, in decimal, which Redis keeps in an intset.
fn digest(text: &str) -> String {
    let hash = Sha256::digest(text.as_bytes());
    let first = hash
        .as_slice()
        .first_chunk::<8>()
        .copied()
        .unwrap_or_default();
    i64::from_be_bytes(first).to_string()
}

/// When the day's sets go: its midnight, and a few minutes past.
fn expire_at(day: NaiveDate) -> i64 {
    day.succ_opt()
        .and_then(|next| next.and_hms_opt(0, 0, 0))
        .map_or(0, |midnight| midnight.and_utc().timestamp())
        + KEPT_PAST_MIDNIGHT_S
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ Past Redis's 512 an intset becomes a hash table of ten times the
    /// bytes, and Redis holds 64 MB on a tier.
    #[test]
    fn a_keys_values_stay_an_intset() {
        const { assert!(TAG_VALUES_PER_DAY <= 512 && TAG_KEYS_PER_DAY <= 512) };
        assert!(digest("search").parse::<i64>().is_ok());
        assert_eq!(digest("search"), digest("search"));
        assert_ne!(digest("search"), digest("Search"));
    }

    #[test]
    fn a_days_sets_go_after_its_midnight() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 26).expect("a date");
        let midnight = DateTime::parse_from_rfc3339("2026-10-27T00:00:00Z")
            .expect("an instant")
            .timestamp();
        assert_eq!(expire_at(day), midnight + KEPT_PAST_MIDNIGHT_S);
        assert!(day_key(&ProjectId::new(), day).ends_with(":2026-10-26"));
    }
}
