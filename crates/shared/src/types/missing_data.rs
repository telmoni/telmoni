//! [`MissingData`] — what an alert does with a window that holds no span.

use std::{fmt, str::FromStr};

use crate::types::ParseEnumError;

use serde::{Deserialize, Serialize};

/// What an alert does with a window that holds no span: reads it as 0,
/// keeps the severity it had, shows `no_data`, or shows it and tells the
/// alert's channels once it lasts. `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MissingData {
    /// The value is 0, compared as any other.
    #[default]
    Zero,
    /// The severity stays as it was.
    Keep,
    /// The severity is `no_data`, and nobody is told.
    NoData,
    /// The severity is `no_data`, told once it has lasted a window.
    NotifyNoData,
}

impl MissingData {
    /// Every one, in wire order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Zero, Self::Keep, Self::NoData, Self::NotifyNoData]
    }

    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::Keep => "keep",
            Self::NoData => "no_data",
            Self::NotifyNoData => "notify_no_data",
        }
    }
}

impl fmt::Display for MissingData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MissingData {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "zero" => Ok(Self::Zero),
            "keep" => Ok(Self::Keep),
            "no_data" => Ok(Self::NoData),
            "notify_no_data" => Ok(Self::NotifyNoData),
            _ => Err(ParseEnumError::new(
                s,
                "zero, keep, no_data, notify_no_data",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [MissingData; 4] = MissingData::all();

    /// A new variant fails to compile here until `all()` lists it.
    #[test]
    fn all_lists_every_variant() {
        for v in ALL {
            match v {
                MissingData::Zero
                | MissingData::Keep
                | MissingData::NoData
                | MissingData::NotifyNoData => {}
            }
        }
    }

    #[test]
    fn from_str_round_trips_every_variant_and_rejects_the_rest() {
        for v in ALL {
            assert_eq!(v.as_str().parse::<MissingData>().unwrap(), v);
            assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{v}\""));
        }
        for given in ["none", "NoData", ""] {
            assert!(given.parse::<MissingData>().is_err(), "parsed: {given:?}");
        }
    }
}
