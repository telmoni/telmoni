//! [`wire_enum!`](crate::wire_enum): a closed vocabulary's wire words, written once.

/// What [`wire_enum!`](crate::wire_enum) gives a vocabulary, for code generic
/// over one.
pub trait WireEnum: Copy + 'static {
    /// Every variant, in wire order.
    fn variants() -> &'static [Self];
    /// The variant's wire word.
    fn word(self) -> &'static str;
}

/// A fieldless enum's wire words from one table: `all`, `as_str`, `Display`
/// and `FromStr`, whose refusal lists every word. A variant left out does not
/// compile, and a word given twice is an unreachable pattern; the serde and
/// sqlx derives stay on the enum, held to the table by
/// [`assert_wire_words`](crate::test_util::assert_wire_words).
#[macro_export]
macro_rules! wire_enum {
    (@words $first:literal $(, $rest:literal)*) => {
        concat!($first $(, ", ", $rest)*)
    };
    ($ty:ident { $($variant:ident => $word:literal),+ $(,)? }) => {
        impl $ty {
            /// Every variant, in wire order.
            #[must_use]
            pub const fn all() -> [Self; [$($word),+].len()] {
                [$(Self::$variant),+]
            }

            /// The wire word.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $word,)+
                }
            }
        }

        impl ::core::fmt::Display for $ty {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::core::str::FromStr for $ty {
            type Err = $crate::types::ParseEnumError;

            fn from_str(s: &str) -> ::core::result::Result<Self, Self::Err> {
                match s {
                    $($word => ::core::result::Result::Ok(Self::$variant),)+
                    _ => ::core::result::Result::Err($crate::types::ParseEnumError::new(
                        s,
                        $crate::wire_enum!(@words $($word),+),
                    )),
                }
            }
        }

        impl $crate::types::WireEnum for $ty {
            fn variants() -> &'static [Self] {
                &[$(Self::$variant),+]
            }

            fn word(self) -> &'static str {
                self.as_str()
            }
        }
    };
}
