//! [`Email`] — an address as every table stores it: trimmed, lowercased and
//! shaped like one, checked once where it arrives.
//!
//! It is personal data, so its `Debug` is masked and it has no `Display`:
//! the address is read only through [`Email::as_str`], which grep can find.

use serde::{Deserialize, Serialize};

use crate::text::is_invisible_formatting;

/// The address cap: RFC 5321 caps a path at 256 octets, so this is longer than
/// any real mailbox and shorter than a payload worth storing.
pub const MAX_EMAIL_LEN: usize = 254;

/// An address that passed [`Email::parse`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Email(String);

/// Why [`Email::parse`] refused, in the words an inviter reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EmailError {
    /// Nothing was typed.
    #[error("enter an email address")]
    Empty,
    /// Longer than [`MAX_EMAIL_LEN`].
    #[error("an email address is at most {MAX_EMAIL_LEN} characters")]
    TooLong,
    /// A space, a control or an invisible formatting character.
    #[error("an email address cannot contain spaces")]
    Spaced,
    /// Not one local part, an `@` and a dotted domain.
    #[error("that is not an email address")]
    Shape,
}

impl Email {
    /// `candidate` trimmed and lowercased, if it is shaped like an address.
    ///
    /// **Deliberately a SHAPE check and nothing more.** Whether the address
    /// exists, or belongs to anybody here, must not be answerable by a
    /// stranger.
    ///
    /// # Errors
    ///
    /// [`EmailError`], naming what is wrong with it.
    pub fn parse(candidate: &str) -> Result<Self, EmailError> {
        let email = candidate.trim().to_lowercase();
        if email.is_empty() {
            return Err(EmailError::Empty);
        }
        if email.chars().count() > MAX_EMAIL_LEN {
            return Err(EmailError::TooLong);
        }
        if email
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || is_invisible_formatting(c))
        {
            return Err(EmailError::Spaced);
        }
        let mut parts = email.split('@');
        let shaped = match (parts.next(), parts.next(), parts.next()) {
            (Some(local), Some(domain), None) => {
                !local.is_empty()
                    && domain.len() >= 3
                    && domain.contains('.')
                    && !domain.starts_with('.')
                    && !domain.ends_with('.')
                    && !domain.contains("..")
            }
            _ => false,
        };
        if !shaped {
            return Err(EmailError::Shape);
        }
        Ok(Self(email))
    }

    /// Unchecked, for tests and constants; debug builds assert the shape.
    #[must_use]
    pub fn from_trusted(s: impl Into<String>) -> Self {
        let s = s.into();
        debug_assert!(
            Self::parse(&s).is_ok_and(|e| e.0 == s),
            "not an address as stored: {s}"
        );
        Self(s)
    }

    /// The address.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The part after the `@`.
    #[must_use]
    pub fn domain(&self) -> &str {
        self.0.rsplit_once('@').map_or("", |(_, domain)| domain)
    }
}

impl std::fmt::Debug for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Email(***)")
    }
}

impl TryFrom<String> for Email {
    type Error = EmailError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<Email> for String {
    fn from(email: Email) -> Self {
        email.0
    }
}

impl From<EmailError> for crate::TelmoniError {
    fn from(e: EmailError) -> Self {
        Self::BadRequest(e.to_string())
    }
}

impl sqlx::Type<sqlx::Postgres> for Email {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl sqlx::postgres::PgHasArrayType for Email {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::postgres::PgHasArrayType>::array_type_info()
    }
}

/// A row's address is held to [`Email::parse`], as an input's is.
impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Email {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        Ok(Self::parse(&<String as sqlx::Decode<
            'r,
            sqlx::Postgres,
        >>::decode(value)?)?)
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Email {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as sqlx::Encode<'_, sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }

    fn size_hint(&self) -> usize {
        <String as sqlx::Encode<'_, sqlx::Postgres>>::size_hint(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_trimmed_and_lowercased() {
        assert_eq!(
            Email::parse("  Dana@Example.COM ").unwrap().as_str(),
            "dana@example.com"
        );
        assert_eq!(
            Email::parse("a+tag@sub.example.co.uk").unwrap().domain(),
            "sub.example.co.uk"
        );
    }

    #[test]
    fn each_refusal_says_what_is_wrong() {
        assert_eq!(Email::parse("  "), Err(EmailError::Empty));
        assert_eq!(Email::parse("a b@example.com"), Err(EmailError::Spaced));
        assert_eq!(
            Email::parse("a\u{200B}@example.com"),
            Err(EmailError::Spaced)
        );
        for bad in [
            "dana",
            "@example.com",
            "dana@",
            "dana@example",
            "dana@.com",
            "dana@example..com",
            "dana@example.com.",
            "one@two@example.com",
        ] {
            assert_eq!(Email::parse(bad), Err(EmailError::Shape), "{bad}");
        }
        let long = format!("{}@example.com", "a".repeat(MAX_EMAIL_LEN));
        assert_eq!(Email::parse(&long), Err(EmailError::TooLong));
    }

    #[test]
    fn debug_prints_no_address_and_serde_round_trips_it() {
        let email = Email::from_trusted("dana@example.com");
        assert!(!format!("{email:?}").contains("dana"));
        let wire = serde_json::to_string(&email).unwrap();
        assert_eq!(wire, "\"dana@example.com\"");
        assert_eq!(serde_json::from_str::<Email>(&wire).unwrap(), email);
        assert!(serde_json::from_str::<Email>("\"nope\"").is_err());
    }
}
