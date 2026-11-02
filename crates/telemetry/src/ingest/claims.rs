//! The claim on an export's body: its SHA-256, under the project, set in
//! Redis if absent before the rows are written, so a retry with the same
//! bytes is answered as the first was. Pending for a minute while they are
//! written, and let go where the write fails, since the insert's token, the
//! same digest, keeps ClickHouse from writing a copy twice; then for an hour
//! the answer's mode, and what its plan held it to, some 200 bytes whatever
//! it said, the retry's own bytes giving the rest. Without Redis, no claim.

use telmoni_shared::digest::sha256_hex;
use telmoni_shared::{ContentMode, ProjectId};

use crate::capacity::Held;
use crate::contract::{CLAIM_ANSWER_SECS, CLAIM_PENDING_SECS};
use crate::replica::Redis;

/// The marker a claim holds while the rows are being written.
const PENDING: &[u8] = b"pending";
/// What an answer is held under, before its mode.
const ANSWERED: &str = "answered:";
/// The line after it where spans were sampled out past the plan, before the
/// sentence they were told with.
const SAMPLED: &str = "sampled:";
/// The line after it where content was past the ceiling.
const CONTENT_FULL: &str = "content_full";

/// The key a body's claim is held under.
fn key(project_id: &ProjectId, digest: &str) -> String {
    format!("telemetry:ingest:body:{}:{digest}", project_id.as_str())
}

/// What a claim holds once its body is written: its mode, then a line for
/// each thing its plan held it to.
fn answered(content_mode: ContentMode, held: &Held) -> String {
    let mut value = format!("{ANSWERED}{content_mode}");
    if let Some(detail) = &held.sampled {
        value.push_str(&format!("\n{SAMPLED}{}", detail.replace('\n', " ")));
    }
    if held.content_full {
        value.push_str(&format!("\n{CONTENT_FULL}"));
    }
    value
}

/// What the claim found.
#[derive(Debug)]
pub enum Claim {
    /// This body is new, or Redis does not answer: write it. The guard holds
    /// the answer's mode, or the claim while the rows may yet land, or lets it go.
    Fresh(Guard),
    /// A copy of this body is being written now: answer `503`, retry in a
    /// second.
    Pending,
    /// This body was written within the hour, answered under this mode and
    /// held to this.
    Answered(ContentMode, Held),
}

/// A claim held on a body, to be answered or released.
#[derive(Debug)]
pub struct Guard {
    key: Option<String>,
}

impl Guard {
    /// The rows are written: hold the answer's mode for the hour.
    pub async fn answer(self, redis: &Redis, content_mode: ContentMode, held: &Held) {
        let Some(key) = self.key else {
            return;
        };
        let Some(mut connection) = redis.connection().await else {
            return;
        };
        let stored: Result<(), redis::RedisError> = redis::cmd("SET")
            .arg(&key)
            .arg(answered(content_mode, held))
            .arg("EX")
            .arg(CLAIM_ANSWER_SECS)
            .query_async(&mut connection)
            .await;
        if let Err(error) = stored {
            redis.stood_in("holding an answer", &error);
        }
    }

    /// The write failed: let the body be sent again.
    pub async fn release(self, redis: &Redis) {
        let Some(key) = self.key else {
            return;
        };
        let Some(mut connection) = redis.connection().await else {
            return;
        };
        let released: Result<(), redis::RedisError> = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut connection)
            .await;
        if let Err(error) = released {
            redis.stood_in("releasing a claim", &error);
        }
    }
}

/// The digest a body is claimed by: its SHA-256, in hex.
#[must_use]
pub fn digest(body: &[u8]) -> String {
    sha256_hex(body)
}

/// Claim `digest` for `project_id`, or learn what became of its first copy.
pub async fn claim(redis: &Redis, project_id: &ProjectId, digest: &str) -> Claim {
    let Some(mut connection) = redis.connection().await else {
        return Claim::Fresh(Guard { key: None });
    };
    let key = key(project_id, digest);
    let set: Result<Option<String>, redis::RedisError> = redis::cmd("SET")
        .arg(&key)
        .arg(PENDING)
        .arg("NX")
        .arg("EX")
        .arg(CLAIM_PENDING_SECS)
        .query_async(&mut connection)
        .await;
    match set {
        Ok(Some(_)) => Claim::Fresh(Guard { key: Some(key) }),
        Ok(None) => {
            let held: Result<Option<Vec<u8>>, redis::RedisError> = redis::cmd("GET")
                .arg(&key)
                .query_async(&mut connection)
                .await;
            match held {
                Ok(Some(value)) => {
                    parse(&value).map_or(Claim::Pending, |(mode, held)| Claim::Answered(mode, held))
                }
                // Expired between the two calls: the first copy's minute ran
                // out with no answer, so this copy is the one to write.
                Ok(None) => Claim::Fresh(Guard { key: None }),
                Err(error) => {
                    redis.stood_in("reading a claim", &error);
                    Claim::Fresh(Guard { key: None })
                }
            }
        }
        Err(error) => {
            redis.stood_in("claiming a body", &error);
            Claim::Fresh(Guard { key: None })
        }
    }
}

/// A held value as the mode its answer named and what it was held to, or
/// `None` for the pending marker.
fn parse(value: &[u8]) -> Option<(ContentMode, Held)> {
    let mut lines = std::str::from_utf8(value).ok()?.lines();
    let mode = lines.next()?.strip_prefix(ANSWERED)?.parse().ok()?;
    let mut held = Held::default();
    for line in lines {
        if let Some(detail) = line.strip_prefix(SAMPLED) {
            held.sampled = Some(detail.to_owned());
        } else if line == CONTENT_FULL {
            held.content_full = true;
        }
    }
    Some((mode, held))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A claim holds the answer's mode, and what its plan held it to, and
    /// no byte of its body.
    #[test]
    fn an_answer_is_held_as_its_mode_and_what_it_was_held_to() {
        let plain = Held::default();
        for mode in ContentMode::all() {
            assert_eq!(answered(mode, &plain), format!("answered:{mode}"));
            assert_eq!(
                parse(answered(mode, &plain).as_bytes()),
                Some((mode, plain.clone()))
            );
        }
        let held = Held {
            sampled: Some("Move to Pro for $20 a month on its billing page.".into()),
            content_full: true,
        };
        let value = answered(ContentMode::On, &held);
        assert_eq!(parse(value.as_bytes()), Some((ContentMode::On, held)));
        assert_eq!(parse(PENDING), None);
        assert_eq!(parse(b"answered:"), None, "no mode, no answer");
    }

    #[test]
    fn the_digest_is_the_sha256_of_the_decompressed_bytes() {
        assert_eq!(
            digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let project = ProjectId::new();
        let key = key(&project, &digest(b"body"));
        assert!(key.starts_with("telemetry:ingest:body:"), "{key}");
        assert!(key.contains(project.as_str()));
    }
}
