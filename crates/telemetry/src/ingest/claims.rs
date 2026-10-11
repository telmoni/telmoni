//! The claim on an export's body: its SHA-256, under the project, set in
//! Redis only if absent before the rows are written, so a retry with the
//! same bytes — the one way a body arrives twice — is answered as the first
//! was rather than written again. Pending for a minute while the rows are
//! written; then the answer itself, for an hour. Without Redis, no claim:
//! the export is written, and a retry may be too, which the log says.

use telmoni_shared::ProjectId;
use telmoni_shared::digest::sha256_hex;

use crate::contract::{CLAIM_ANSWER_SECS, CLAIM_PENDING_SECS};
use crate::replica::{Redis, stood_in};

/// The marker a claim holds while the rows are being written.
const PENDING: &[u8] = b"pending";
/// What an answer is stored under, before the mode and the body.
const ANSWERED: &[u8] = b"answered:";

/// The key a body's claim is held under.
fn key(project_id: &ProjectId, digest: &str) -> String {
    format!("telemetry:ingest:body:{}:{digest}", project_id.as_str())
}

/// The answer an export got, as a retry is given it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAnswer {
    /// The project's content mode at the time, for the header.
    pub content_mode: String,
    /// The response body, in the request's encoding.
    pub body: Vec<u8>,
}

/// What the claim found.
#[derive(Debug)]
pub enum Claim {
    /// This body is new, or Redis does not answer: write it. The guard
    /// stores the answer, or releases the claim if the write fails.
    Fresh(Guard),
    /// A copy of this body is being written now: answer `503`, retry in a
    /// second.
    Pending,
    /// This body was written within the hour: its answer again.
    Answered(StoredAnswer),
}

/// A claim held on a body, to be answered or released.
#[derive(Debug)]
pub struct Guard {
    key: Option<String>,
}

impl Guard {
    /// The rows are written: hold the answer for the hour.
    pub async fn answer(self, redis: &Redis, answer: &StoredAnswer) {
        let Some(key) = self.key else {
            return;
        };
        let Some(mut connection) = redis.connection().await else {
            return;
        };
        let mut value = ANSWERED.to_vec();
        value.extend_from_slice(answer.content_mode.as_bytes());
        value.push(b':');
        value.extend_from_slice(&answer.body);
        let stored: Result<(), redis::RedisError> = redis::cmd("SET")
            .arg(&key)
            .arg(value)
            .arg("EX")
            .arg(CLAIM_ANSWER_SECS)
            .query_async(&mut connection)
            .await;
        if let Err(error) = stored {
            stood_in("holding an answer", &error);
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
            stood_in("releasing a claim", &error);
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
                    stood_in("reading a claim", &error);
                    Claim::Fresh(Guard { key: None })
                }
            }
        }
        Err(error) => {
            stood_in("claiming a body", &error);
            Claim::Fresh(Guard { key: None })
        }
    }
}

/// A held value as an answer, or `None` for the pending marker.
fn parse(value: &[u8]) -> Option<StoredAnswer> {
    let rest = value.strip_prefix(ANSWERED)?;
    let colon = rest.iter().position(|b| *b == b':')?;
    let (mode, body) = rest.split_at(colon);
    Some(StoredAnswer {
        content_mode: String::from_utf8(mode.to_vec()).ok()?,
        body: body.get(1..)?.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_stored_with_its_mode_and_read_back_whole() {
        let answer = StoredAnswer {
            content_mode: "off".into(),
            body: b"{\"partialSuccess\":{\"rejectedSpans\":1,\"errorMessage\":\"a:b:c\"}}".to_vec(),
        };
        let mut value = ANSWERED.to_vec();
        value.extend_from_slice(b"off:");
        value.extend_from_slice(&answer.body);
        assert_eq!(
            parse(&value),
            Some(answer),
            "colons in the body are the body's"
        );
        assert_eq!(parse(PENDING), None);
        assert_eq!(parse(b"answered:off"), None, "no body, no answer");
        assert_eq!(
            parse(b"answered:off:"),
            Some(StoredAnswer {
                content_mode: "off".into(),
                body: Vec::new()
            })
        );
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
