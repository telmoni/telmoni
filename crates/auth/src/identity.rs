//! What an organization is called, and the address an owner shares with.

use telmoni_shared::{AuthError, TelmoniError};

/// The display cap. A name rides a table cell and the foot of the sidebar.
const MAX_DISPLAY_NAME_LEN: usize = 120;

/// The address cap: RFC 5321 caps a path at 256 octets, so this is longer than
/// any real mailbox and shorter than a payload worth storing.
const MAX_EMAIL_LEN: usize = 254;

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
        return Err(AuthError::BadRequest("enter an email address".into()).into());
    }
    if email.chars().count() > MAX_EMAIL_LEN {
        return Err(AuthError::BadRequest(format!(
            "an email address is at most {MAX_EMAIL_LEN} characters"
        ))
        .into());
    }
    if email
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || is_invisible_formatting(c))
    {
        return Err(AuthError::BadRequest("an email address cannot contain spaces".into()).into());
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
        return Err(AuthError::BadRequest(format!("`{email}` is not an email address")).into());
    }
    Ok(email)
}

/// Sanitise the name the identity provider gave us. `None` when it gave
/// nothing usable, so the console falls back to the address rather than to
/// something invented here.
#[must_use]
pub fn sanitize_display_name(raw: Option<&str>) -> Option<String> {
    let cleaned: String = raw?
        .chars()
        .filter(|c| !c.is_control() && !is_invisible_formatting(*c))
        .take(MAX_DISPLAY_NAME_LEN)
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

/// An organization's name, held to what a display name is held to: it is
/// printed in mail to any address an owner or admin invites, and in every
/// member's console. `None` when nothing printable is left.
#[must_use]
pub fn sanitize_organization_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && !is_invisible_formatting(*c))
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
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

/// What a page prints for an organization: the name it was given — the
/// console's `organizationLabel` makes the same choice. Never the owner's
/// address: an organization is named before anyone but its owner sees it, and
/// the fallback covers only the owner's own unnamed one, read on a lane the
/// console does not show them before they have named it.
#[must_use]
pub fn organization_label(name: Option<&str>) -> String {
    name.map(str::trim).filter(|n| !n.is_empty()).map_or_else(
        || format!("A {} organization", telmoni_shared::PRODUCT_NAME),
        str::to_string,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
