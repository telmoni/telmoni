//! `POST /internal/v1/telemetry/stored-id` — an end user's id, or a
//! session's, as the project stores it ([`crate::hashing`]), from the id its
//! application sent: what a list's filter takes, since no page shows an id
//! as sent. Anyone who reads the project's traces asks; the id rides in the
//! body, never a URL, and is neither logged nor kept.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use telmoni_shared::extract::Json;
use telmoni_shared::{AuthError, Redacted, TelmoniError};

use crate::AppState;
use crate::contract::{SESSION_ID_MAX, USER_ID_MAX};
use crate::handler::traces::reader;
use crate::hashing;

/// The id asked about, as sent: one of the two, by its name.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sent {
    UserId(Redacted),
    SessionId(Redacted),
}

impl Sent {
    /// The id, what it names, and the most characters ingest keeps of one.
    fn parts(&self) -> (&str, &'static str, usize) {
        match self {
            Self::UserId(id) => (id.expose(), "an end user's id", USER_ID_MAX),
            Self::SessionId(id) => (id.expose(), "a session's id", SESSION_ID_MAX),
        }
    }
}

/// The id as the project's spans hold it.
#[derive(Debug, Serialize)]
pub struct Stored {
    pub stored_id: String,
}

/// `POST /internal/v1/telemetry/stored-id` — hashed as ingest hashes it,
/// whether or not a span carries it: the list it narrows says which. `404`
/// for a project that has stored no id yet, whose key a read never makes.
pub async fn stored_id(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(sent): Json<Sent>,
) -> Result<Json<Stored>, TelmoniError> {
    let reader = reader(&state, &headers).await?;
    let (raw, what, max) = sent.parts();
    let id = hashing::id_as_sent(raw, what, max)?;
    let Some(storage) = hashing::storage_where_keyed(&state, &reader.project_id).await? else {
        return Err(AuthError::NotFound("this project has stored no id yet".into()).into());
    };
    Ok(Json(Stored {
        stored_id: storage.store(&id),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body names one id by what it is, and nothing else beside it.
    #[test]
    fn the_body_is_one_id_named_by_what_it_is() {
        let user: Sent = serde_json::from_str(r#"{"user_id":"u-1"}"#).expect("a user's");
        assert_eq!(user.parts(), ("u-1", "an end user's id", USER_ID_MAX));
        let session: Sent = serde_json::from_str(r#"{"session_id":"s-1"}"#).expect("a session's");
        assert_eq!(session.parts(), ("s-1", "a session's id", SESSION_ID_MAX));
        for refused in [
            "{}",
            r#"{"trace_id":"t"}"#,
            r#"{"user_id":"u","session_id":"s"}"#,
            r#"{"user_id":7}"#,
        ] {
            assert!(serde_json::from_str::<Sent>(refused).is_err(), "{refused}");
        }
    }

    /// ⚠ The id never reaches a log: the body's `Debug` masks it.
    #[test]
    fn the_id_is_masked_in_debug() {
        let sent: Sent = serde_json::from_str(r#"{"user_id":"ada@example.com"}"#).expect("a body");
        assert!(!format!("{sent:?}").contains("ada"));
    }

    /// An id is read as ingest read it: its NULs dropped, then held to its
    /// length, and a refusal never says it back.
    #[test]
    fn an_id_is_read_as_ingest_reads_one() {
        assert_eq!(
            hashing::id_as_sent("ac\0me-42", "an end user's id", USER_ID_MAX).expect("an id"),
            "acme-42"
        );
        for (raw, max) in [("", 8), ("\0", 8), ("too-long-an-id", 8)] {
            let refused = hashing::id_as_sent(raw, "a session's id", max).expect_err("refused");
            let detail = refused.to_problem_details().detail.unwrap_or_default();
            assert_eq!(
                detail,
                format!("a session's id is 1 to {max} characters, as its spans carry it")
            );
        }
    }
}
