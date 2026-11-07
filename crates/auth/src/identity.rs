//! What an organization is called, and the address an owner shares with.

use telmoni_shared::Email;
use telmoni_shared::text::is_invisible_formatting;
use telmoni_shared::{TelmoniError, slug};

/// The display cap. A name rides a table cell and the foot of the sidebar.
const MAX_DISPLAY_NAME_LEN: usize = 120;

/// `raw` without its control and invisible formatting characters, trimmed:
/// what two names identical on screen have in common.
fn printable(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && !is_invisible_formatting(*c))
        .collect();
    cleaned.trim().to_owned()
}

/// Sanitise the name the identity provider gave us. `None` when it gave
/// nothing usable, so the console falls back to the address rather than to
/// something invented here.
#[must_use]
pub fn sanitize_display_name(raw: Option<&str>) -> Option<String> {
    let cleaned: String = printable(raw?).chars().take(MAX_DISPLAY_NAME_LEN).collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

/// A name a person types and others read — a project's, an organization's,
/// an API key's — or the 400 that says why not. Each is printed in mail or
/// in every member's console, so each is held to what a display name is.
pub fn name(raw: &str, what: &str, max: usize) -> Result<String, TelmoniError> {
    let name = printable(raw);
    if name.is_empty() {
        return Err(TelmoniError::BadRequest(format!("give the {what} a name")));
    }
    if name.chars().count() > max {
        return Err(TelmoniError::BadRequest(format!(
            "the {what}'s name is at most {max} characters"
        )));
    }
    Ok(name)
}

/// Free text a person adds beside a name, such as an API key's description:
/// printable, and at most `max` characters; `None` when nothing is left.
pub fn note(raw: Option<&str>, what: &str, max: usize) -> Result<Option<String>, TelmoniError> {
    let Some(note) = raw.map(printable).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    if note.chars().count() > max {
        return Err(TelmoniError::BadRequest(format!(
            "the {what} is at most {max} characters"
        )));
    }
    Ok(Some(note))
}

/// What a page prints for a person: their name when the provider gave one,
/// otherwise their address. One function, because three copies of a decision
/// are how one of them starts printing something else.
#[must_use]
pub fn display_for(display_name: Option<&str>, email: &Email) -> String {
    display_name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(email.as_str())
        .to_string()
}

/// What a person's first organization is called when it is provisioned, and
/// the slugs it may go by, best first: the part of their address before the
/// `@`, less any `+tag`, as `khe4r6` for `khe4r6+test@gmail.com`. The owner
/// renames it, and moves its URL, on Settings.
///
/// No slugs when that part gives none (`李明`): it takes a placeholder.
#[must_use]
pub fn default_organization(email: &Email) -> (String, Vec<slug::Slug>) {
    let local = email
        .as_str()
        .rsplit_once('@')
        .map_or(email.as_str(), |(local, _)| local);
    let local = local.split_once('+').map_or(local, |(before, _)| before);
    let name: String = local
        .chars()
        .take(crate::db::organizations::MAX_ORGANIZATION_NAME)
        .collect();
    let slugs = slug::candidates(slug::Scope::Organization, &name);
    (name, slugs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dana() -> Email {
        Email::from_trusted("dana@example.com")
    }

    /// A typed name loses what would let it pass for another, and is counted
    /// in characters, whoever calls the lane.
    #[test]
    fn a_typed_name_is_printable_and_bounded() {
        assert_eq!(
            name("  Pay\u{202E}ments\u{200B} ", "key", 100).unwrap(),
            "Payments"
        );
        assert_eq!(
            name(&"🦀".repeat(100), "key", 100).unwrap(),
            "🦀".repeat(100)
        );
        let too_long = name(&"a".repeat(101), "key", 100).unwrap_err();
        assert_eq!(
            too_long.to_problem_details().detail.as_deref(),
            Some("the key's name is at most 100 characters")
        );
        let blank = name(" \u{200B}\u{FEFF} ", "project", 100).unwrap_err();
        assert_eq!(
            blank.to_problem_details().detail.as_deref(),
            Some("give the project a name")
        );
    }

    #[test]
    fn a_note_beside_a_name_is_optional_and_bounded() {
        assert_eq!(note(None, "key's description", 500).unwrap(), None);
        assert_eq!(
            note(Some("  \u{200B} "), "key's description", 500).unwrap(),
            None
        );
        assert_eq!(
            note(Some(" for the deploy "), "key's description", 500).unwrap(),
            Some("for the deploy".into())
        );
        assert!(note(Some(&"a".repeat(501)), "key's description", 500).is_err());
    }

    #[test]
    fn a_display_name_is_sanitised_and_never_invented() {
        assert_eq!(
            sanitize_display_name(Some(" Kendrick ")).as_deref(),
            Some("Kendrick")
        );
        assert_eq!(
            sanitize_display_name(Some("Ken\u{202E}drick")).as_deref(),
            Some("Kendrick")
        );
        assert_eq!(sanitize_display_name(Some("   ")), None);
        assert_eq!(sanitize_display_name(None), None);
    }

    /// The fallback is the address itself, shown whole, never cut to "dana".
    #[test]
    fn a_person_with_no_name_is_shown_their_address() {
        assert_eq!(display_for(Some("Dana"), &dana()), "Dana");
        assert_eq!(display_for(None, &dana()), "dana@example.com");
        assert_eq!(display_for(Some("  "), &dana()), "dana@example.com");
    }

    /// Named the part of the address before the `@`, less any `+tag`, and
    /// nothing else: no domain, and no word added.
    #[test]
    fn the_first_organization_is_named_the_address_before_its_at() {
        let named = |address: &str| default_organization(&Email::from_trusted(address));
        for (address, name) in [
            ("khe4r6@gmail.com", "khe4r6"),
            ("kendrick@acme.com", "kendrick"),
            ("khe4r6+test@gmail.com", "khe4r6"),
            ("ada.lovelace@example.com", "ada.lovelace"),
        ] {
            assert_eq!(named(address).0, name, "{address}");
        }
        let (_, slugs) = named("khe4r6@gmail.com");
        assert_eq!(slugs.first().map(slug::Slug::as_str), Some("khe4r6"));
        assert!(
            slugs.iter().skip(1).all(|s| s
                .as_str()
                .strip_prefix("khe4r6-")
                .is_some_and(|tail| tail.len() == 6)),
            "{slugs:?}"
        );
    }

    /// One that gives no slug leaves the URL to a placeholder, and keeps its
    /// name.
    #[test]
    fn a_name_with_no_latin_letter_leaves_the_slug_to_a_placeholder() {
        let (name, slugs) = default_organization(&Email::from_trusted("李明@example.com"));
        assert_eq!(name, "李明");
        assert!(slugs.is_empty());
    }
}
