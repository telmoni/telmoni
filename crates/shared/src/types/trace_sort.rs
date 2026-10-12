//! [`TraceSort`] — closed vocabulary of the columns the trace list sorts by.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// A column of the console's trace list, named in the URL so a link holds
/// the sort: by user and by session above all, so one user's traces, and one
/// session's, stand together. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceSort {
    /// When the run started: newest first by default.
    Started,
    /// The run's agent.
    Agent,
    /// The end user, as stored.
    User,
    /// The session the run belongs to.
    Session,
    /// How the run ended.
    Status,
    /// How many steps the run took.
    Steps,
    /// Its input and output tokens, summed.
    Tokens,
    /// How long it ran.
    Duration,
}

impl TraceSort {
    /// Every column, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::Started,
            Self::Agent,
            Self::User,
            Self::Session,
            Self::Status,
            Self::Steps,
            Self::Tokens,
            Self::Duration,
        ]
    }
}

impl fmt::Display for TraceSort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started => write!(f, "started"),
            Self::Agent => write!(f, "agent"),
            Self::User => write!(f, "user"),
            Self::Session => write!(f, "session"),
            Self::Status => write!(f, "status"),
            Self::Steps => write!(f, "steps"),
            Self::Tokens => write!(f, "tokens"),
            Self::Duration => write!(f, "duration"),
        }
    }
}

impl FromStr for TraceSort {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "started" => Ok(Self::Started),
            "agent" => Ok(Self::Agent),
            "user" => Ok(Self::User),
            "session" => Ok(Self::Session),
            "status" => Ok(Self::Status),
            "steps" => Ok(Self::Steps),
            "tokens" => Ok(Self::Tokens),
            "duration" => Ok(Self::Duration),
            _ => Err(ParseEnumError::new(
                s,
                "started, agent, user, session, status, steps, tokens, duration",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [TraceSort; 8] = TraceSort::all();

    /// `all()` is a hand-written array, so nothing but a `match` forces it to
    /// stay complete: a new variant fails to compile here until it is listed.
    #[test]
    fn all_lists_every_variant() {
        for sort in ALL {
            match sort {
                TraceSort::Started
                | TraceSort::Agent
                | TraceSort::User
                | TraceSort::Session
                | TraceSort::Status
                | TraceSort::Steps
                | TraceSort::Tokens
                | TraceSort::Duration => {}
            }
        }
        let mut seen: Vec<String> = ALL.iter().map(ToString::to_string).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len(), "all() repeats a variant");
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for v in &ALL {
            assert_eq!(v.to_string().parse::<TraceSort>().unwrap(), *v);
        }
        assert!("Started".parse::<TraceSort>().is_err());
        assert!("".parse::<TraceSort>().is_err());
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
