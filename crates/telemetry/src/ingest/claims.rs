//! The claim on an export's body: its SHA-256, under the project, set in
//! Redis if absent before the rows are written, so a retry with the same
//! bytes is answered as the first was, not written again. Pending for a
//! minute, then for an hour the answer's mode alone, some 200 bytes whatever
//! it said, the retry's own bytes giving the rest. Without Redis, no claim.

use telmoni_shared::digest::sha256_hex;
use telmoni_shared::{ContentMode, ProjectId};

use crate::contract::{CLAIM_ANSWER_SECS, CLAIM_PENDING_SECS};
use crate::replica::Redis;

/// The marker a claim holds while the rows are being written.
const PENDING: &[u8] = b"pending";
/// What an answer is held under, before its mode.
const ANSWERED: &str = "answered:";

/// The key a body's claim is held under.
fn key(project_id: &ProjectId, digest: &str) -> String {
    format!("telemetry:ingest:body:{}:{digest}", project_id.as_str())
}

/// What a claim holds once its body is written.
fn answered(content_mode: ContentMode) -> String {
    format!("{ANSWERED}{content_mode}")
}

/// What the claim found.
#[derive(Debug)]
pub enum Claim {
    /// This body is new, or Redis does not answer: write it. The guard
    /// holds the answer's mode, or releases the claim if the write fails.
    Fresh(Guard),
    /// A copy of this body is being written now: answer `503`, retry in a
    /// second.
    Pending,
    /// This body was written within the hour, answered under this mode.
    Answered(ContentMode),
}

/// A claim held on a body, to be answered or released.
#[derive(Debug)]
pub struct Guard {
    key: Option<String>,
}

impl Guard {
    /// The rows are written: hold the answer's mode for the hour.
    pub async fn answer(self, redis: &Redis, content_mode: ContentMode) {
        let Some(key) = self.key else {
            return;
        };
        let Some(mut connection) = redis.connection().await else {
            return;
        };
        let stored: Result<(), redis::RedisError> = redis::cmd("SET")
            .arg(&key)
            .arg(answered(content_mode))
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
                Ok(Some(value)) => parse(&value).map_or(Claim::Pending, Claim::Answered),
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

/// A held value as the mode its answer named, or `None` for the pending
/// marker.
fn parse(value: &[u8]) -> Option<ContentMode> {
    std::str::from_utf8(value)
        .ok()?
        .strip_prefix(ANSWERED)?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A claim holds the answer's mode and no byte of its body.
    #[test]
    fn an_answer_is_held_as_its_mode_alone() {
        for mode in ContentMode::all() {
            assert_eq!(answered(mode), format!("answered:{mode}"));
            assert_eq!(parse(answered(mode).as_bytes()), Some(mode));
        }
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
