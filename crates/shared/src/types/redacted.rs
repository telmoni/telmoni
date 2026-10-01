//! [`Redacted`] — a `String` newtype whose `Debug`/`Display` are masked and
//! whose backing bytes are wiped on drop.
//!
//! Redaction is a property of the TYPE, not of remembering at each call site:
//! a struct holding one can derive `Debug` freely. There is deliberately no
//! `Deref` or `AsRef<str>`, so a secret never silently coerces to `&str` and
//! every real read goes through [`Redacted::expose`], which grep can find.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

/// A secret string that never prints itself and wipes its bytes on drop.
#[derive(Clone)]
pub struct Redacted(Zeroizing<String>);

impl Redacted {
    /// The underlying secret. Named `expose` so every deliberate read of the
    /// plaintext is easy to audit by grep.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Redacted(\"***\")")
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl From<String> for Redacted {
    fn from(s: String) -> Self {
        Self(Zeroizing::new(s))
    }
}

impl From<&str> for Redacted {
    fn from(s: &str) -> Self {
        Self(Zeroizing::new(s.to_owned()))
    }
}

impl From<Redacted> for String {
    fn from(r: Redacted) -> Self {
        (*r.0).clone()
    }
}

impl From<Redacted> for Arc<str> {
    fn from(r: Redacted) -> Self {
        Self::from(r.0.as_str())
    }
}

impl<'de> Deserialize<'de> for Redacted {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_are_masked_but_expose_is_not() {
        let secret: Redacted = "super-secret-value".into();
        assert_eq!(format!("{secret:?}"), "Redacted(\"***\")");
        assert_eq!(format!("{secret}"), "***");
        assert!(!format!("{secret:?} {secret}").contains("super-secret-value"));
        assert_eq!(secret.expose(), "super-secret-value");
    }

    #[test]
    fn deserializes_from_a_string_and_stays_masked() {
        #[derive(serde::Deserialize)]
        struct Body {
            value: Redacted,
        }
        let body: Body = serde_json::from_str(r#"{"value":"sk-live-abc"}"#).unwrap();
        assert_eq!(body.value.expose(), "sk-live-abc");
        assert!(!format!("{:?}", body.value).contains("sk-live-abc"));
    }

    #[test]
    fn owned_conversions_round_trip_the_plaintext() {
        let s: String = Redacted::from("sk-live-owned").into();
        assert_eq!(s, "sk-live-owned");
        let a: Arc<str> = Redacted::from("telmoni_owned").into();
        assert_eq!(&*a, "telmoni_owned");
    }

    #[test]
    fn masked_even_inside_a_derived_debug() {
        #[derive(Debug)]
        struct Holder {
            #[expect(
                dead_code,
                reason = "read only through the derived Debug this test exercises"
            )]
            token: Redacted,
        }
        let h = Holder {
            token: "telmoni_live_abc".into(),
        };
        assert!(!format!("{h:?}").contains("telmoni_live_abc"));
    }
}
