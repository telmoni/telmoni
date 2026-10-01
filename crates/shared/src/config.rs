//! Reading the process environment at startup.

use std::env;

/// The value of `key`, refusing "unset" and "set but blank" alike.
pub fn require(key: &str) -> anyhow::Result<String> {
    required(key, env::var(key).ok())
}

/// The value of `key`, or `None` when it is unset OR empty.
#[must_use]
pub fn optional(key: &str) -> Option<String> {
    present(env::var(key).ok())
}

/// [`optional`], with any trailing `/` removed.
#[must_use]
pub fn optional_base_url(key: &str) -> Option<String> {
    optional(key).map(|s| s.trim_end_matches('/').to_owned())
}

/// `key` parsed into `T`, or `default` when the var is unset or blank.
pub fn env_parse<T>(key: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    parsed(key, optional(key), default)
}

fn required(key: &str, raw: Option<String>) -> anyhow::Result<String> {
    let Some(value) = raw else {
        anyhow::bail!("missing required env var: {key}");
    };
    if value.trim().is_empty() {
        anyhow::bail!("required env var {key} is set but empty");
    }
    Ok(value)
}

fn present(raw: Option<String>) -> Option<String> {
    raw.filter(|s| !s.trim().is_empty())
}

fn parsed<T>(key: &str, raw: Option<String>, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match raw {
        Some(s) => s.parse().map_err(|e| {
            anyhow::anyhow!("{key} must be a valid {}: {e}", std::any::type_name::<T>())
        }),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::{parsed, present, required};

    #[test]
    fn a_required_var_that_is_set_but_blank_is_refused() {
        assert!(required("K", Some("   ".to_owned())).is_err());
        assert!(required("K", Some(String::new())).is_err());
    }

    #[test]
    fn an_unset_required_var_is_refused() {
        assert!(required("K", None).is_err());
    }

    #[test]
    fn a_required_var_keeps_its_value_verbatim() {
        assert_eq!(
            required("K", Some(" s3cret ".to_owned())).unwrap(),
            " s3cret "
        );
    }

    #[test]
    fn an_empty_optional_var_reads_the_same_as_an_absent_one() {
        assert_eq!(present(Some(String::new())), None);
        assert_eq!(present(Some("  ".to_owned())), None);
        assert_eq!(present(None), None);
        assert_eq!(present(Some("v".to_owned())).as_deref(), Some("v"));
    }

    #[test]
    fn an_unparseable_value_is_an_error_not_the_default() {
        assert!(parsed::<u16>("PORT", Some("eight".to_owned()), 8080).is_err());
    }

    #[test]
    fn a_blank_numeric_var_takes_the_default_rather_than_crashing() {
        assert_eq!(
            parsed::<u16>("PORT", present(Some(String::new())), 8080).unwrap(),
            8080
        );
    }

    #[test]
    fn an_unset_value_takes_the_default() {
        assert_eq!(parsed::<u16>("PORT", None, 8080).unwrap(), 8080);
    }
}
