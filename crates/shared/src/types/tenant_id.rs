//! [`OrganizationId`], [`ProjectId`] and [`UserId`] — the validated identifier
//! newtypes, stamped out by one macro so they **do not convert into each other**.
//!
//! That incompatibility is the reason this module exists. All three are opaque
//! strings at runtime, and each keys a different boundary: an organization id
//! binds `app.organization_id`, a project id `app.project_id`, and a user id —
//! a person, the identity provider's subject — `app.user_id`. A `From` between
//! any pair would let a caller satisfy one boundary with whatever they put in
//! another header.
//!
//! ⚠ **An organization id is minted and carries `org_`; a user id never does.**
//! The prefix prevents accidentally binding a person's id as an organization:
//! it fails here, loudly,
//! rather than matching no row under row security.
//!
//! Otherwise validation is deliberately permissive: anything stricter than
//! non-empty, capped and free of whitespace would reject a valid third-party
//! OIDC `sub`.

use serde::{Deserialize, Serialize};

use uuid::Uuid;

/// Maximum length, in chars. Generous, because third-party OIDC providers can
/// emit long `sub`s.
const MAX_LEN: usize = 256;

/// Stamp out one validated tenant-id newtype plus its error enum. `$shape` is
/// the id's own rule beyond the shared ones, and `$shape_msg` finishes the
/// sentence that names it.
macro_rules! tenant_id_newtype {
    (
        $(#[$id_doc:meta])*
        $name:ident,
        $(#[$err_doc:meta])*
        $error:ident,
        $label:literal,
        $shape:path,
        $shape_msg:literal
    ) => {
        $(#[$err_doc])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $error {
            /// Input was empty.
            Empty,
            /// Input exceeded the 256-character cap.
            TooLong,
            /// Input contained whitespace or a control character.
            Invalid,
            /// Input did not have this id's shape.
            Shape,
        }

        impl std::fmt::Display for $error {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Empty => write!(f, "{} must not be empty", $label),
                    Self::TooLong => write!(f, "{} exceeds {}-character cap", $label, MAX_LEN),
                    Self::Invalid => {
                        write!(f, "{} contains whitespace or control character", $label)
                    }
                    Self::Shape => write!(f, "{} {}", $label, $shape_msg),
                }
            }
        }

        impl std::error::Error for $error {}

        $(#[$id_doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
        #[serde(try_from = "String")]
        #[sqlx(transparent)]
        pub struct $name(String);

        impl TryFrom<String> for $name {
            type Error = $error;

            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::try_new(s)
            }
        }

        impl $name {
            /// Validated public constructor. Use at every external boundary —
            /// HTTP handlers, CLI arguments, webhook bodies.
            pub fn try_new(s: impl Into<String>) -> Result<Self, $error> {
                let s = s.into();
                if s.is_empty() {
                    return Err($error::Empty);
                }
                if s.chars().count() > MAX_LEN {
                    return Err($error::TooLong);
                }
                if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
                    return Err($error::Invalid);
                }
                if !$shape(s.as_str()) {
                    return Err($error::Shape);
                }
                Ok(Self(s))
            }

            /// Unvalidated constructor for trusted sources only: DB decodes,
            /// tests, constants. Debug builds assert the same invariants.
            #[must_use]
            pub fn from_trusted(s: impl Into<String>) -> Self {
                let s = s.into();
                debug_assert!(
                    !s.is_empty()
                        && s.chars().count() <= MAX_LEN
                        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
                        && $shape(s.as_str()),
                    "trusted string violated {} invariants",
                    $label
                );
                Self(s)
            }

            /// Borrow the inner string. Prefer this over `.0` access.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume the newtype and return the inner `String`.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

/// A project id has no shape of its own beyond the shared rules.
const fn any_shape(_: &str) -> bool {
    true
}

/// An organization id is one this platform minted.
fn is_minted_organization(s: &str) -> bool {
    s.starts_with(ORGANIZATION_ID_PREFIX)
}

/// A person's id is the provider's subject, and never an organization's.
fn is_not_an_organization(s: &str) -> bool {
    !s.starts_with(ORGANIZATION_ID_PREFIX)
}

tenant_id_newtype!(
    /// Project identifier — a project inside an organization, the second level
    /// of tenancy under it.
    ProjectId,
    /// Reasons [`ProjectId::try_new`] can reject input.
    ProjectIdError,
    "project id",
    any_shape,
    "has an unexpected shape"
);

/// The prefix every minted project id carries — `project_`.
pub const PROJECT_ID_PREFIX: &str = "project_";

/// The prefix every minted organization id carries — `org_`.
pub const ORGANIZATION_ID_PREFIX: &str = "org_";

/// The prefix every minted API token carries — `telmoni_`.
pub const API_TOKEN_PREFIX: &str = "telmoni_";

/// The alphabet the random half is spelled in.
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// How many base62 characters follow the prefix of a minted id.
pub const MINTED_ID_RANDOM_LEN: usize = 16;

/// The rejection threshold, `4 * 62`.
const BASE62_REJECT_AT: u8 = 248;

/// `prefix` and 16 random base62 characters, drawn from v4 UUIDs.
fn mint(prefix: &str) -> String {
    let mut id = String::with_capacity(prefix.len() + MINTED_ID_RANDOM_LEN);
    id.push_str(prefix);

    let mut taken = 0usize;
    while taken < MINTED_ID_RANDOM_LEN {
        for (i, &byte) in Uuid::new_v4().as_bytes().iter().enumerate() {
            if taken == MINTED_ID_RANDOM_LEN {
                break;
            }
            if i == 6 || i == 8 || byte >= BASE62_REJECT_AT {
                continue;
            }
            if let Some(&c) = BASE62.get(usize::from(byte % 62)) {
                id.push(char::from(c));
                taken += 1;
            }
        }
    }

    id
}

impl ProjectId {
    /// Mint a fresh project id — the prefix and 16 random base62 characters.
    #[must_use]
    pub fn new() -> Self {
        Self(mint(PROJECT_ID_PREFIX))
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

tenant_id_newtype!(
    /// User identifier — the provider `sub` of the acting person.
    UserId,
    /// Reasons [`UserId::try_new`] can reject input.
    UserIdError,
    "user id",
    is_not_an_organization,
    "must not start with `org_` — that is an organization id"
);

/// The prefix every user id the built-in provider mints carries — `user_`.
/// An external provider's subjects are its own, and carry whatever it chose.
pub const USER_ID_PREFIX: &str = "user_";

impl UserId {
    /// Mint a fresh user id for a person the built-in provider signs up —
    /// `user_` and 16 random base62 characters.
    #[must_use]
    pub fn new() -> Self {
        Self(mint(USER_ID_PREFIX))
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

tenant_id_newtype!(
    /// Organization identifier — minted when an organization is founded,
    /// provisioned or created, never a person's id.
    OrganizationId,
    /// Reasons [`OrganizationId::try_new`] can reject input.
    OrganizationIdError,
    "organization id",
    is_minted_organization,
    "must start with `org_`"
);

impl OrganizationId {
    /// Mint a fresh organization id — `org_` and 16 random base62 characters.
    #[must_use]
    pub fn new() -> Self {
        Self(mint(ORGANIZATION_ID_PREFIX))
    }
}

impl Default for OrganizationId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minted id is the prefix and exactly `MINTED_ID_RANDOM_LEN`
    #[test]
    fn a_minted_project_id_is_the_prefix_and_sixteen_base62_characters() {
        let id = ProjectId::new();
        let s = id.as_str();
        assert!(s.starts_with(PROJECT_ID_PREFIX), "no prefix: {s}");
        let random = s.trim_start_matches(PROJECT_ID_PREFIX);
        assert_eq!(random.chars().count(), MINTED_ID_RANDOM_LEN, "{s}");
        assert!(
            random.bytes().all(|b| BASE62.contains(&b)),
            "characters outside base62 alphabet: {s}"
        );
    }

    #[test]
    fn a_minted_user_id_is_user_and_sixteen_base62_characters() {
        let id = UserId::new();
        let s = id.as_str();
        let random = s
            .strip_prefix(USER_ID_PREFIX)
            .unwrap_or_else(|| panic!("no prefix: {s}"));
        assert_eq!(random.chars().count(), MINTED_ID_RANDOM_LEN, "{s}");
        assert!(
            random.bytes().all(|b| BASE62.contains(&b)),
            "characters outside base62 alphabet: {s}"
        );
        assert!(
            UserId::try_new(s).is_ok(),
            "a minted id must pass its own gate"
        );
    }

    #[test]
    fn a_minted_organization_id_is_org_and_sixteen_base62_characters() {
        let id = OrganizationId::new();
        let s = id.as_str();
        let random = s
            .strip_prefix(ORGANIZATION_ID_PREFIX)
            .unwrap_or_else(|| panic!("no prefix: {s}"));
        assert_eq!(random.chars().count(), MINTED_ID_RANDOM_LEN, "{s}");
        assert!(
            random.bytes().all(|b| BASE62.contains(&b)),
            "characters outside base62 alphabet: {s}"
        );
        assert_eq!(OrganizationId::try_new(s), Ok(id));
    }

    /// The two halves of the old conflation, each refused at the boundary: a
    /// person's id is not an organization, and an organization is not a person.
    #[test]
    fn a_user_id_is_never_an_organization_id_and_back() {
        assert_eq!(
            OrganizationId::try_new("user_01J8XYZ"),
            Err(OrganizationIdError::Shape)
        );
        assert_eq!(
            UserId::try_new("org_4cG1a6Qx0Rz7PpLm"),
            Err(UserIdError::Shape)
        );
        assert!(UserId::try_new("user_01J8XYZ").is_ok());
        assert!(OrganizationId::try_new("org_acme").is_ok());
    }
}
