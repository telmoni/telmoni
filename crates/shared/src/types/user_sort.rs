//! [`UserSort`] — closed vocabulary of the columns the users page sorts by.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// A column of the console's users page, named in the URL so a link holds
/// the sort: by tokens first, until a call is priced, so the users an agent
/// spends the most on stand at the top. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSort {
    /// The end user's id, as stored.
    User,
    /// How many runs were theirs in the window.
    Traces,
    /// How many of those runs had a span that failed.
    Errors,
    /// How many steps their runs took, summed.
    Steps,
    /// Their runs' input tokens, summed.
    InputTokens,
    /// Their runs' output tokens, summed.
    OutputTokens,
    /// Their runs' input and output tokens, summed: the default.
    Tokens,
    /// When their first run in the window started.
    FirstSeen,
    /// When their last run in the window ended.
    LastSeen,
}

impl UserSort {
    /// Every column, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 9] {
        [
            Self::User,
            Self::Traces,
            Self::Errors,
            Self::Steps,
            Self::InputTokens,
            Self::OutputTokens,
            Self::Tokens,
            Self::FirstSeen,
            Self::LastSeen,
        ]
    }
}

impl fmt::Display for UserSort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Traces => write!(f, "traces"),
            Self::Errors => write!(f, "errors"),
            Self::Steps => write!(f, "steps"),
            Self::InputTokens => write!(f, "input_tokens"),
            Self::OutputTokens => write!(f, "output_tokens"),
            Self::Tokens => write!(f, "tokens"),
            Self::FirstSeen => write!(f, "first_seen"),
            Self::LastSeen => write!(f, "last_seen"),
        }
    }
}

impl FromStr for UserSort {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "traces" => Ok(Self::Traces),
            "errors" => Ok(Self::Errors),
            "steps" => Ok(Self::Steps),
            "input_tokens" => Ok(Self::InputTokens),
            "output_tokens" => Ok(Self::OutputTokens),
            "tokens" => Ok(Self::Tokens),
            "first_seen" => Ok(Self::FirstSeen),
            "last_seen" => Ok(Self::LastSeen),
            _ => Err(ParseEnumError::new(
                s,
                "user, traces, errors, steps, input_tokens, output_tokens, tokens, first_seen, \
                 last_seen",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [UserSort; 9] = UserSort::all();

    /// `all()` is a hand-written array, so nothing but a `match` forces it to
    /// stay complete: a new variant fails to compile here until it is listed.
    #[test]
    fn all_lists_every_variant() {
        for sort in ALL {
            match sort {
                UserSort::User
                | UserSort::Traces
                | UserSort::Errors
                | UserSort::Steps
                | UserSort::InputTokens
                | UserSort::OutputTokens
                | UserSort::Tokens
                | UserSort::FirstSeen
                | UserSort::LastSeen => {}
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
            assert_eq!(v.to_string().parse::<UserSort>().unwrap(), *v);
        }
        assert!("Tokens".parse::<UserSort>().is_err());
        assert!("firstSeen".parse::<UserSort>().is_err());
        assert!("".parse::<UserSort>().is_err());
    }

    #[test]
    fn serde_matches_display() {
        for v in ALL {
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
    }
}
