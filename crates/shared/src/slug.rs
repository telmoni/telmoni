//! Slugs: the readable names organizations and projects go by in the
//! console's paths — `/{organization}` and `/{organization}/{project}`, with
//! an organization's own pages under `/{organization}/~/…`, where no project's
//! slug can land on them.
//!
//! ⚠ **A slug follows its row's name and is never the row's identity.** Auth
//! derives it whenever the name is set — creation, a rename, a project's move
//! into another organization — so a rename moves the URL. Anything kept longer
//! than a page (an index entry, a notice's link) names the row by its id, which
//! the console redirects to wherever the slug is now.
//!
//! An organization's slug is unique across every organization, being the
//! path's first segment; a project's only within its organization.

use uuid::Uuid;

/// The longest a slug may be, in bytes; every slug is ASCII. Room for a name,
/// short enough that a path still reads in an address bar.
pub const MAX_LEN: usize = 48;

/// Words no organization may go by: the console's own top-level paths, the
/// ones a console built on it serves or may yet, and the ones its framework
/// answers itself (`/404`, `/500`, `/index`). An organization holding one
/// would shadow that page or be shadowed by it. Sorted, and published in the
/// wire contract, which the console's copy is pinned to.
pub const RESERVED: &[&str] = &[
    "404",
    "500",
    "about",
    "account",
    "accounts",
    "admin",
    "api",
    "app",
    "apple-icon",
    "apps",
    "assets",
    "auth",
    "billing",
    "blog",
    "blueprints",
    "careers",
    "changelog",
    "cli",
    "community",
    "connect",
    "console",
    "contact",
    "cookbook",
    "dashboard",
    "developers",
    "device",
    "docs",
    "download",
    "enterprise",
    "favicon",
    "health",
    "help",
    "home",
    "icon",
    "index",
    "install",
    "internal",
    "invite",
    "invites",
    "legal",
    "login",
    "logout",
    "manifest",
    "metrics",
    "new",
    "null",
    "oauth",
    "opengraph-image",
    "org",
    "organization",
    "organizations",
    "plans",
    "pricing",
    "privacy",
    "project",
    "projects",
    "robots",
    "security",
    "settings",
    "sign-in",
    "sign-out",
    "sign-up",
    "signin",
    "signout",
    "signup",
    "sitemap",
    "sso",
    "static",
    "status",
    "support",
    "team",
    "teams",
    "terms",
    "twitter-image",
    "undefined",
    "user",
    "users",
    "v1",
    "v2",
    "www",
];

/// How many numbered slugs follow a name's own. Within an organization names
/// collide only through punctuation and accents ("Web App", "web-app",
/// "Web_App"), so a handful is plenty for its projects.
const NUMBERED: u32 = 20;

/// How long the random tail is on an organization's last candidate.
const TAIL_LEN: usize = 6;

/// The namespace a slug is minted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// An organization's: unique everywhere, and never a [`RESERVED`] word.
    Organization,
    /// A project's: unique within its organization.
    Project,
}

impl Scope {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Organization => "org",
            Self::Project => "project",
        }
    }

    fn allows(self, slug: &str) -> bool {
        match self {
            Self::Organization => !is_reserved(slug),
            Self::Project => true,
        }
    }
}

/// Whether `s` has a slug's shape: lowercase ASCII letters and digits in runs
/// joined by single hyphens, at most [`MAX_LEN`] long.
#[must_use]
pub fn is_slug(s: &str) -> bool {
    s.len() <= MAX_LEN
        && s.split('-').all(|run| {
            !run.is_empty()
                && run
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Whether no organization may go by `s`.
#[must_use]
pub fn is_reserved(s: &str) -> bool {
    RESERVED.binary_search(&s).is_ok()
}

/// The slug `name` reads as: Latin letters without their accents, digits, and
/// a hyphen wherever anything else stood, cut to [`MAX_LEN`]. `None` when
/// nothing in it survives — a name in a script with no Latin letters.
#[must_use]
pub fn slugify(name: &str) -> Option<String> {
    let mut slug = String::new();
    let mut gap = false;
    for c in name.chars().flat_map(char::to_lowercase) {
        let mut ascii = [0u8; 4];
        let piece: &str = if c.is_ascii_alphanumeric() {
            c.encode_utf8(&mut ascii)
        } else if let Some(folded) = fold(c) {
            folded
        } else if is_silent(c) {
            continue;
        } else {
            gap = true;
            continue;
        };
        if gap && !slug.is_empty() {
            slug.push('-');
        }
        gap = false;
        slug.push_str(piece);
    }
    if slug.len() > MAX_LEN {
        slug.truncate(MAX_LEN);
        slug.truncate(slug.trim_end_matches('-').len());
    }
    (!slug.is_empty()).then_some(slug)
}

/// The slugs a row called `name` may take, best first: the name's own, then
/// numbered from `-2`. The caller writes the first no other row in `scope`
/// holds. Empty when the name gives no slug.
///
/// An organization's end with the name and a random tail. Its namespace is
/// everybody's, so the numbers of a common name ("Personal", "Test") run out,
/// and the slug should still read as the name it follows.
#[must_use]
pub fn candidates(scope: Scope, name: &str) -> Vec<String> {
    let Some(base) = slugify(name) else {
        return Vec::new();
    };
    let tail = match scope {
        Scope::Organization => Some(suffixed(&base, &random(TAIL_LEN))),
        Scope::Project => None,
    };
    std::iter::once(base.clone())
        .chain((2..=NUMBERED).map(|n| suffixed(&base, &n.to_string())))
        .chain(tail)
        .filter(|slug| scope.allows(slug))
        .collect()
}

/// The slug a row takes when its name gives none — an organization nobody
/// has named yet, or every candidate already taken: the scope's word and ten
/// random base-36 characters.
#[must_use]
pub fn placeholder(scope: Scope) -> String {
    const RANDOM_LEN: usize = 10;
    format!("{}-{}", scope.prefix(), random(RANDOM_LEN))
}

/// `len` random base-36 characters.
fn random(len: usize) -> String {
    const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    // 252 is 7 × 36: a byte at or above it would favour the first digits.
    const REJECT_AT: u8 = 252;

    let mut out = String::with_capacity(len);
    while out.len() < len {
        for (i, &byte) in Uuid::new_v4().as_bytes().iter().enumerate() {
            if out.len() == len {
                break;
            }
            // Bytes 6 and 8 carry the UUID's version and variant, not chance.
            if i == 6 || i == 8 || byte >= REJECT_AT {
                continue;
            }
            if let Some(&c) = BASE36.get(usize::from(byte % 36)) {
                out.push(char::from(c));
            }
        }
    }
    out
}

/// `base` with `-{suffix}` on the end, cut so the whole stays within
/// [`MAX_LEN`].
fn suffixed(base: &str, suffix: &str) -> String {
    let room = MAX_LEN.saturating_sub(suffix.len() + 1);
    let cut = base.get(..room).unwrap_or(base).trim_end_matches('-');
    format!("{cut}-{suffix}")
}

/// What a Latin letter with a mark reads as in a slug; ligatures and the sharp
/// s spell themselves out. Lowercase only: [`slugify`] lowercases first.
fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// Characters that vanish rather than split a word: apostrophes, so
/// "Ada's Lab" reads `adas-lab`, and the combining marks lowercasing can leave
/// behind (`İ` lowercases to `i` and a dot above).
fn is_silent(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '\u{2bc}' | '\u{300}'..='\u{36f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_reads_as_lowercase_words_joined_by_hyphens() {
        assert_eq!(slugify("Acme").as_deref(), Some("acme"));
        assert_eq!(
            slugify("Default Project").as_deref(),
            Some("default-project")
        );
        assert_eq!(slugify("  Web -- App!  ").as_deref(), Some("web-app"));
        assert_eq!(slugify("api_v2.1").as_deref(), Some("api-v2-1"));
    }

    #[test]
    fn accents_fold_and_apostrophes_vanish() {
        assert_eq!(slugify("Müller GmbH").as_deref(), Some("muller-gmbh"));
        assert_eq!(slugify("Ærø Straße").as_deref(), Some("aero-strasse"));
        assert_eq!(slugify("Ada's Lab").as_deref(), Some("adas-lab"));
        assert_eq!(slugify("İstanbul").as_deref(), Some("istanbul"));
    }

    #[test]
    fn a_name_with_nothing_latin_gives_no_slug() {
        assert_eq!(slugify("株式会社"), None);
        assert_eq!(slugify("🚀 — ✨"), None);
        assert_eq!(slugify(""), None);
    }

    #[test]
    fn a_long_name_is_cut_without_a_dangling_hyphen() {
        let slug = slugify(&format!("{} tail", "a".repeat(MAX_LEN - 1)));
        assert_eq!(slug.as_deref(), Some("a".repeat(MAX_LEN - 1).as_str()));
    }

    #[test]
    fn what_slugify_gives_is_a_slug() {
        let long = "z".repeat(90);
        for name in ["Acme", "Müller GmbH", "  x  ", "a-b", long.as_str()] {
            let slug = slugify(name).unwrap();
            assert!(is_slug(&slug), "{slug}");
        }
    }

    #[test]
    fn the_shape_refuses_what_a_path_could_misread() {
        assert!(is_slug("acme"));
        assert!(is_slug("a"));
        assert!(is_slug("web-app-2"));
        for bad in [
            "", "-acme", "acme-", "web--app", "Acme", "org_abc", "a.b", "~",
        ] {
            assert!(!is_slug(bad), "{bad}");
        }
        assert!(!is_slug(&"a".repeat(MAX_LEN + 1)));
    }

    #[test]
    fn the_reserved_words_are_sorted_slugs() {
        assert!(RESERVED.windows(2).all(|w| w[0] < w[1]));
        assert!(RESERVED.iter().all(|w| is_slug(w)));
        assert!(is_reserved("account"));
        assert!(is_reserved("404"));
        assert!(!is_reserved("acme"));
    }

    #[test]
    fn candidates_run_from_the_name_through_its_numbers() {
        let all = candidates(Scope::Project, "Web App");
        assert_eq!(all.first().map(String::as_str), Some("web-app"));
        assert_eq!(all.get(1).map(String::as_str), Some("web-app-2"));
        assert_eq!(all.len(), 20);
        assert!(candidates(Scope::Project, "株式会社").is_empty());
    }

    #[test]
    fn an_organization_skips_a_reserved_word_but_not_its_numbers() {
        let all = candidates(Scope::Organization, "Account");
        assert_eq!(all.first().map(String::as_str), Some("account-2"));
        assert_eq!(
            candidates(Scope::Project, "Account")
                .first()
                .map(String::as_str),
            Some("account")
        );
    }

    // Twenty organizations called "Personal" is a Tuesday. The twenty-first
    // still gets a slug that reads as its name.
    #[test]
    fn an_organizations_candidates_end_with_the_name_and_a_random_tail() {
        let all = candidates(Scope::Organization, "Personal");
        assert_eq!(all.len(), 21);
        assert_eq!(all.get(19).map(String::as_str), Some("personal-20"));
        let last = all.last().unwrap();
        let tail = last.strip_prefix("personal-").unwrap();
        assert_eq!(tail.len(), TAIL_LEN);
        assert!(is_slug(last) && !is_reserved(last), "{last}");
        assert_ne!(
            candidates(Scope::Organization, "Personal").last(),
            Some(last)
        );

        let long = candidates(Scope::Organization, &"c".repeat(90));
        assert!(long.iter().all(|slug| is_slug(slug)), "{long:?}");
    }

    #[test]
    fn a_suffixed_slug_stays_within_the_limit() {
        let base = "b".repeat(MAX_LEN);
        let slug = suffixed(&base, "20");
        assert_eq!(slug.len(), MAX_LEN);
        assert!(slug.ends_with("-20"));
        assert!(is_slug(&suffixed("web-", "2")));
    }

    #[test]
    fn a_placeholder_is_a_slug_of_its_scope() {
        let org = placeholder(Scope::Organization);
        let project = placeholder(Scope::Project);
        assert!(org.starts_with("org-") && is_slug(&org) && !is_reserved(&org));
        assert!(project.starts_with("project-") && is_slug(&project));
        assert_ne!(org, placeholder(Scope::Organization));
    }
}
