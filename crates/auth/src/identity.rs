//! What an organization is called, and the address an owner shares with.

use telmoni_shared::{TelmoniError, slug};

/// The display cap. A name rides a table cell and the foot of the sidebar.
const MAX_DISPLAY_NAME_LEN: usize = 120;

/// The address cap: RFC 5321 caps a path at 256 octets, so this is longer than
/// any real mailbox and shorter than a payload worth storing.
pub const MAX_EMAIL_LEN: usize = 254;

/// True for a character that carries no glyph but changes how the rest reads:
/// bidi overrides, zero-width characters, the BOM. Two names identical on
/// screen can differ by these, which is how one person is made to look like another.
fn is_invisible_formatting(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// The gate every address an inviter types passes through, on its way to an
/// invitation. Returns the normalised address, so a caller cannot check one
/// string and store another.
///
/// **Deliberately a SHAPE check and nothing more.** Whether the address exists,
/// or belongs to a Telmoni organization, must not be answerable by a stranger:
/// the invitation is written either way and the reply reads the same.
pub fn validate_email(candidate: &str) -> Result<String, TelmoniError> {
    let email = candidate.trim().to_lowercase();

    if email.is_empty() {
        return Err(TelmoniError::BadRequest("enter an email address".into()));
    }
    if email.chars().count() > MAX_EMAIL_LEN {
        return Err(TelmoniError::BadRequest(format!(
            "an email address is at most {MAX_EMAIL_LEN} characters"
        )));
    }
    if email
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || is_invisible_formatting(c))
    {
        return Err(TelmoniError::BadRequest(
            "an email address cannot contain spaces".into(),
        ));
    }

    let mut parts = email.split('@');
    let ok = match (parts.next(), parts.next(), parts.next()) {
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
    if !ok {
        return Err(TelmoniError::BadRequest(
            "that is not an email address".into(),
        ));
    }
    Ok(email)
}

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
pub fn display_for(display_name: Option<&str>, email: &str) -> String {
    display_name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(email)
        .to_string()
}

/// What a person's first organization is called when it is provisioned —
/// named after its holder, the first word of their name, "Ada's organization"
/// — and the slugs it may go by, best first. Somebody who gave no name gets
/// "My organization". Nothing is read
/// off their address: the name is shown to everyone the owner invites, and
/// the slug that follows it is in every path. The owner renames it, and moves
/// its URL, on Settings.
///
/// No slugs when the holder's own name gives none: "李明's organization"
/// would read as `s-organization`, so it takes a placeholder instead.
#[must_use]
pub fn default_organization(display_name: Option<&str>) -> (String, Vec<slug::Slug>) {
    const POSSESSIVE: &str = "'s organization";
    let room = crate::db::organizations::MAX_ORGANIZATION_NAME - POSSESSIVE.chars().count();
    let Some(first) = display_name.and_then(|name| name.split_whitespace().next()) else {
        let name = "My organization".to_owned();
        let slugs = slug::candidates(slug::Scope::Organization, &name);
        return (name, slugs);
    };
    let first: String = first.chars().take(room).collect();
    let name = format!("{first}{POSSESSIVE}");
    let slugs = if slug::slugify(&first).is_some() {
        slug::candidates(slug::Scope::Organization, &name)
    } else {
        Vec::new()
    };
    (name, slugs)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn an_address_is_normalised_to_one_spelling() {
        assert_eq!(
            validate_email("  Dana@Example.COM ").unwrap(),
            "dana@example.com"
        );
        assert_eq!(
            validate_email("a+tag@sub.example.co.uk").unwrap(),
            "a+tag@sub.example.co.uk"
        );
    }

    #[test]
    fn a_string_that_cannot_be_an_address_is_refused() {
        for bad in [
            "",
            "   ",
            "dana",
            "@example.com",
            "dana@",
            "dana@example",
            "dana@.com",
            "dana@example..com",
            "dana@example.com.",
            "one@two@example.com",
            "da na@example.com",
            "dana\u{200B}@example.com",
        ] {
            assert!(validate_email(bad).is_err(), "`{bad}` must be refused");
        }
    }

    /// The cap is on the NORMALISED address, so trimming cannot smuggle a
    /// longer one past it.
    #[test]
    fn an_absurd_address_is_refused_by_length() {
        let long = format!("{}@example.com", "a".repeat(MAX_EMAIL_LEN));
        assert!(validate_email(&long).is_err());
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
        assert_eq!(display_for(Some("Dana"), "dana@example.com"), "Dana");
        assert_eq!(display_for(None, "dana@example.com"), "dana@example.com");
        assert_eq!(
            display_for(Some("  "), "dana@example.com"),
            "dana@example.com"
        );
    }

    #[test]
    fn a_first_organization_is_named_after_its_holder_not_their_address() {
        let (name, slugs) = default_organization(Some("Ada Lovelace"));
        assert_eq!(name, "Ada's organization");
        assert_eq!(
            slugs.first().map(telmoni_shared::slug::Slug::as_str),
            Some("adas-organization")
        );

        for nobody in [None, Some("   ")] {
            let (name, slugs) = default_organization(nobody);
            assert_eq!(name, "My organization");
            assert_eq!(
                slugs.first().map(telmoni_shared::slug::Slug::as_str),
                Some("my-organization")
            );
        }

        let (long, _) = default_organization(Some(&"a".repeat(300)));
        assert_eq!(
            long.chars().count(),
            crate::db::organizations::MAX_ORGANIZATION_NAME
        );
        assert!(long.ends_with("'s organization"), "{long}");
    }

    // The possessive alone would read `s-organization` for every such name.
    #[test]
    fn a_name_with_no_latin_letter_leaves_the_slug_to_a_placeholder() {
        let (name, slugs) = default_organization(Some("李明"));
        assert_eq!(name, "李明's organization");
        assert!(slugs.is_empty(), "{slugs:?}");
    }
}
