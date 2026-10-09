//! Slugs: the readable names organizations and projects go by in the
//! console's paths — `/{organization}` and `/{organization}/{project}`, with
//! an organization's own pages beside its projects at `/{organization}/{page}`,
//! each a word no project goes by ([`PROJECT_RESERVED`]).
//!
//! ⚠ **A slug is never the row's identity.** A project's follows its name:
//! auth derives it whenever the name is set — creation, a rename, a move into
//! another organization — so a rename moves the URL. An organization's is
//! derived once, from its first name, and is a setting of its own after that
//! (`PATCH /internal/organization`), so only a URL change moves it. Ids key
//! lanes, headers, cookies and foreign keys; every link a person is shown —
//! an index entry's URL and a citation included — is spelled with slugs, and
//! dies when the slug moves, as a Vercel link does. The console redirects an
//! id in a path to wherever the slug is now.
//!
//! An organization's slug is unique across every organization, being the
//! path's first segment; a project's only within its organization.
//!
//! And no slug of either holds a [`BLOCKED`] word ([`offends`]): a URL is
//! carried into every link, invitation and address bar it is shared to, so a
//! name that would make one takes a placeholder when its row is made, a
//! project renamed to one keeps the URL it had, and a URL asked for with one
//! is refused.

use uuid::Uuid;

/// The longest a slug may be, in bytes; every slug is ASCII. Room for a name,
/// short enough that a path still reads in an address bar.
pub const MAX_LEN: usize = 48;

/// Words no organization may go by: the console's own top-level paths, the
/// ones a console built on it serves or may yet, and the ones its framework
/// answers itself (`/404`, `/500`, `/index`). An organization holding one
/// would shadow that page or be shadowed by it. Sorted, and published in the
/// wire contract, which the console's copy is pinned to.
pub const ORGANIZATION_RESERVED: &[&str] = &[
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
    "errors",
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
    "otlp",
    "ping",
    "pings",
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
    "webhooks",
    "www",
];

/// Words no project may go by: its organization's own pages, which sit beside
/// its projects (`/{organization}/settings`), and the ones a console built on
/// it serves there or may yet. A project holding one would be shadowed by that
/// page. Sorted, and published in the wire contract, which the console's copy
/// is pinned to: the console tells a page from a project by it.
pub const PROJECT_RESERVED: &[&str] = &[
    "activity",
    "api-keys",
    "audit-log",
    "billing",
    "connectors",
    "integrations",
    "members",
    "new",
    "notifications",
    "plans",
    "projects",
    "security",
    "settings",
    "sso",
    "support",
    "usage",
];

/// Words no slug of either scope may hold: profanity and slurs. Matched as
/// whole words — one of a slug's hyphen-separated runs, the slug with its
/// hyphens taken out (with or without the number on its end), or single
/// letters spelled apart within it ([`offends`]) — never inside a longer word,
/// so "Scunthorpe", "assessment" and "Dickens" pass. A word that is also a name, a place or an
/// ordinary word ("dick", "cock", "ass", "dyke", "niger", "retard", which is
/// French for a delay) is left off: a person called Dick still gets their own
/// organization's name as its URL. English, plurals and endings spelled out.
/// Sorted.
pub const BLOCKED: &[&str] = &[
    "arsehole",
    "arseholes",
    "asshole",
    "assholes",
    "beaner",
    "beaners",
    "bitch",
    "bitches",
    "bullshit",
    "chink",
    "chinks",
    "cocksucker",
    "cocksuckers",
    "cunt",
    "cunts",
    "dickhead",
    "dickheads",
    "fag",
    "faggot",
    "faggots",
    "fags",
    "fuck",
    "fucked",
    "fucker",
    "fuckers",
    "fuckface",
    "fuckin",
    "fucking",
    "fuckoff",
    "fucks",
    "fuckyou",
    "gook",
    "gooks",
    "jizz",
    "kaffir",
    "kike",
    "kikes",
    "motherfucker",
    "motherfuckers",
    "motherfucking",
    "nigga",
    "niggas",
    "niggaz",
    "nigger",
    "niggers",
    "paki",
    "pakis",
    "raghead",
    "ragheads",
    "retarded",
    "shit",
    "shithead",
    "shitheads",
    "shithole",
    "shits",
    "shitting",
    "shitty",
    "slut",
    "sluts",
    "spic",
    "spics",
    "towelhead",
    "towelheads",
    "trannies",
    "tranny",
    "twat",
    "twats",
    "wanker",
    "wankers",
    "wetback",
    "wetbacks",
    "whore",
    "whores",
];

/// Whether `slug` holds a [`BLOCKED`] word: as one of its runs; as the whole
/// of it with the hyphens out (`f-u-c-k`), and again without the number on its
/// end (`f-u-c-k-2`), since numbering is how a taken slug is told apart and
/// must not unblock one; or as single letters spelled apart inside it
/// (`my-f-u-c-k`), with or without that number too — each read as written
/// and with the digits a word is
/// disguised with read as the letters they stand in for (`sh1t`, `5lut`).
#[must_use]
pub fn offends(slug: &str) -> bool {
    let runs: Vec<&str> = slug.split('-').collect();
    let numbered = runs
        .iter()
        .rev()
        .take_while(|run| run.bytes().all(|b| b.is_ascii_digit()))
        .count();
    let unnumbered = runs
        .get(..runs.len().saturating_sub(numbered))
        .unwrap_or_default();
    let mut words = vec![runs.concat(), unnumbered.concat()];
    words.extend(runs.iter().map(|run| (*run).to_owned()));
    words.extend(spelled_apart(&runs));
    words.extend(spelled_apart(unnumbered));
    words
        .iter()
        .any(|word| BLOCKED.iter().any(|blocked| reads_as(word, blocked)))
}

/// Each stretch of two or more single-letter runs in `runs`, joined: the word
/// they spell apart (`my-f-u-c-k`). A digit counts as a letter here, since one
/// stands in for a letter in a stretch as in a word (`s-h-1-t`).
fn spelled_apart(runs: &[&str]) -> Vec<String> {
    let mut stretches = Vec::new();
    let mut letters = String::new();
    for run in runs {
        if run.len() == 1 {
            letters.push_str(run);
        } else if letters.len() > 1 {
            stretches.push(std::mem::take(&mut letters));
        } else {
            letters.clear();
        }
    }
    if letters.len() > 1 {
        stretches.push(letters);
    }
    stretches
}

/// Whether `word` spells `blocked`, letter for letter, or with a digit in the
/// place of the letter it stands in for — a one for an `i` or an `l`, each in
/// its own place (`bu11sh1t`). A word mostly of digits is a number rather than
/// a word in disguise (`600k`, `n1994`), so its digits stay digits.
fn reads_as(word: &str, blocked: &str) -> bool {
    if word.len() != blocked.len() {
        return false;
    }
    let digits = word.bytes().filter(u8::is_ascii_digit).count();
    let disguised = digits.saturating_mul(2) <= word.len();
    word.bytes().zip(blocked.bytes()).all(|(w, b)| {
        w == b
            || (disguised
                && match w {
                    b'0' => b == b'o',
                    b'1' => b == b'i' || b == b'l',
                    b'3' => b == b'e',
                    b'4' => b == b'a',
                    b'5' => b == b's',
                    b'6' | b'9' => b == b'g',
                    b'7' => b == b't',
                    b'8' => b == b'b',
                    _ => false,
                })
    })
}

/// How many numbered slugs follow a name's own. Within an organization names
/// collide only through punctuation and accents ("Web App", "web-app",
/// "Web_App"), so a handful is plenty for its projects.
const NUMBERED: u32 = 20;

/// How long the random tail is on an organization's last candidate.
const TAIL_LEN: usize = 6;

/// The namespace a slug is minted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// An organization's: unique everywhere, and never an
    /// [`ORGANIZATION_RESERVED`] word.
    Organization,
    /// A project's: unique within its organization, and never a
    /// [`PROJECT_RESERVED`] word.
    Project,
}

impl Scope {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Organization => "org",
            Self::Project => "project",
        }
    }

    /// Whether no row in this scope may go by `slug`.
    #[must_use]
    pub fn reserves(self, slug: &str) -> bool {
        let reserved = match self {
            Self::Organization => ORGANIZATION_RESERVED,
            Self::Project => PROJECT_RESERVED,
        };
        reserved.binary_search(&slug).is_ok()
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
        let inside_a_word = slug.as_bytes().get(MAX_LEN).is_some_and(|&b| b != b'-');
        slug.truncate(MAX_LEN);
        slug.truncate(whole_words(&slug, inside_a_word).len());
    }
    (!slug.is_empty()).then_some(slug)
}

/// The slugs a row called `name` may take, best first: the name's own, then
/// numbered from `-2`. The caller writes the first no other row in `scope`
/// holds. Empty when the name gives no slug, or only one that [`offends`]:
/// numbering a blocked word does not unblock it.
///
/// An organization's end with the name and a random tail. Its namespace is
/// everybody's, so the numbers of a common name ("Personal", "Test") run out,
/// and the slug should still read as the name it follows.
#[must_use]
pub fn candidates(scope: Scope, name: &str) -> Vec<String> {
    let Some(base) = slugify(name) else {
        return Vec::new();
    };
    // Checked on the base, not only on each candidate: a word spelled apart
    // (`f-u-c-k`) reads whole only while nothing follows it, so its numbers
    // and its random tail would otherwise slip past.
    if offends(&base) {
        return Vec::new();
    }
    let tail = match scope {
        Scope::Organization => Some(suffixed(&base, &random(TAIL_LEN))),
        Scope::Project => None,
    };
    std::iter::once(base.clone())
        .chain((2..=NUMBERED).map(|n| suffixed(&base, &n.to_string())))
        .chain(tail)
        .filter(|slug| !scope.reserves(slug) && !offends(slug))
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
    let inside_a_word = base.as_bytes().get(room).is_some_and(|&b| b != b'-');
    let cut = whole_words(base.get(..room).unwrap_or(base), inside_a_word);
    format!("{cut}-{suffix}")
}

/// A slug cut short, without a dangling hyphen, and back to its last whole
/// word when the cut fell `inside_a_word`: half a word can read as another,
/// a blocked one among them. A cut inside the first word keeps what it cut.
fn whole_words(cut: &str, inside_a_word: bool) -> &str {
    let whole = if inside_a_word {
        cut.rfind('-').and_then(|at| cut.get(..at)).unwrap_or(cut)
    } else {
        cut
    };
    whole.trim_end_matches('-')
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
        for reserved in [ORGANIZATION_RESERVED, PROJECT_RESERVED] {
            assert!(reserved.windows(2).all(|w| w[0] < w[1]));
            assert!(reserved.iter().all(|w| is_slug(w)));
        }
        assert!(Scope::Organization.reserves("account"));
        assert!(Scope::Organization.reserves("404"));
        assert!(!Scope::Organization.reserves("acme"));
        assert!(Scope::Project.reserves("settings"));
        assert!(!Scope::Project.reserves("acme"));
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

    // A project sits beside its organization's own pages, so it skips their
    // words where an organization need not.
    #[test]
    fn a_project_skips_its_organizations_page_names() {
        let all = candidates(Scope::Project, "Members");
        assert_eq!(all.first().map(String::as_str), Some("members-2"));
        assert_eq!(all.len(), 19);
        assert_eq!(
            candidates(Scope::Organization, "Members")
                .first()
                .map(String::as_str),
            Some("members")
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
        assert!(
            is_slug(last) && !Scope::Organization.reserves(last),
            "{last}"
        );
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
        assert!(org.starts_with("org-") && is_slug(&org) && !Scope::Organization.reserves(&org));
        assert!(
            project.starts_with("project-")
                && is_slug(&project)
                && !Scope::Project.reserves(&project)
        );
        assert_ne!(org, placeholder(Scope::Organization));
    }

    #[test]
    fn the_blocked_words_are_sorted_single_words() {
        assert!(BLOCKED.windows(2).all(|w| w[0] < w[1]));
        assert!(
            BLOCKED
                .iter()
                .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()))
        );
    }

    #[test]
    fn a_blocked_word_offends_however_it_is_spelled() {
        for bad in [
            "fuck",
            "acme-shit",
            "shit-show",
            "motherfucker-inc",
            "sh1t-show",
            "5lut",
            "s1ut",
            "f-u-c-k",
            "sh-it",
            // A number on the end, as a taken slug is numbered, unblocks nothing.
            "f-u-c-k-2",
            "ass-hole-2",
            // Letters spelled apart inside a longer slug, numbered or not.
            "my-f-u-c-k",
            "my-f-u-c-k-2",
            "acme-s-h-1-t-2",
            // A one for an `i` in one place and an `l` in another.
            "bu11sh1t",
            "sh1tho1e",
        ] {
            assert!(offends(bad), "{bad}");
        }
    }

    // A run mostly of digits is a number, not a word in disguise, and a
    // single letter beside words spells nothing.
    #[test]
    fn a_number_or_a_lone_letter_does_not_offend() {
        for fine in [
            "600k",
            "road-to-600k",
            "600-k",
            "n1994",
            "model-f465",
            "go-ok-labs",
            "s-hit-records",
            "a-team",
            "j-k-rowling",
        ] {
            assert!(!offends(fine), "{fine}");
        }
    }

    // A cut that falls inside a word goes back to the last whole one: half a
    // word can read as another, a blocked one among them.
    #[test]
    fn a_long_name_is_cut_back_to_a_whole_word() {
        assert_eq!(
            slugify("Engineering Council and Software Houses of Pakistan").as_deref(),
            Some("engineering-council-and-software-houses-of")
        );
        assert_eq!(
            suffixed("abcdefghij-abcdefghij-abcdefghij-abcdefghij-abcd", "20"),
            "abcdefghij-abcdefghij-abcdefghij-abcdefghij-20"
        );
    }

    // Whole words only: the Scunthorpe problem is a filter matching inside
    // words, and a name or a place that is also slang is somebody's own.
    #[test]
    fn a_word_inside_another_or_a_name_does_not_offend() {
        for fine in [
            "acme",
            "scunthorpe-united",
            "assessment",
            "class-act",
            "cocktail-bar",
            "shiitake",
            "dickens-and-sons",
            "dicks-organization",
            "hancock",
            "van-dyke",
            "niger-delta",
            "gestion-du-retard",
            "the-sh-it",
            "web-app-2",
        ] {
            assert!(!offends(fine), "{fine}");
        }
    }

    #[test]
    fn a_name_with_a_blocked_word_gives_no_slug_of_its_own() {
        assert!(candidates(Scope::Project, "Shit Tracker").is_empty());
        assert!(candidates(Scope::Organization, "Fuck this").is_empty());
        // Spelled apart, the word reads whole only with nothing after it:
        // neither its numbers nor an organization's random tail get through.
        assert!(candidates(Scope::Project, "F U C K").is_empty());
        assert!(candidates(Scope::Organization, "Ass Hole").is_empty());
        assert_eq!(
            candidates(Scope::Organization, "Dick's organization")
                .first()
                .map(String::as_str),
            Some("dicks-organization")
        );
    }
}
