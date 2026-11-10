//! The scrubbers over content where it leaves: secrets, then PII, rule by rule,
//! each match replaced by its kind's marker and counted (`sdk/fixtures/canary.json`).
//! Hand-written over bytes, every class ASCII, and linear on any text, since an
//! agent's content may be crafted to stall the process that sends it.

use std::borrow::Cow;
use std::collections::BTreeMap;

/// Which scrubbers run.
#[derive(Debug, Clone, Copy)]
pub struct Switches {
    /// Replace recognized credentials.
    pub secrets: bool,
    /// Replace recognized personal data.
    pub pii: bool,
}

impl Switches {
    /// Whether any scanner is enabled.
    #[must_use]
    pub fn any(self) -> bool {
        self.secrets || self.pii
    }
}

/// What the scrubbers took out of a span's content, by `<family>:<kind>`, and
/// the fields one failed on, by `dropped:<field>`.
pub type Counts = BTreeMap<String, u64>;

/// Where a match starts and ends.
type Span = (usize, usize);

/// `text` as the scrubbers `on` leave it, and what they replaced; `None` where
/// a rule went wrong, which fails the field.
#[must_use]
pub fn scrub(text: &str, on: Switches) -> Option<(String, Counts)> {
    let mut counts = Counts::new();
    let mut text = Cow::Borrowed(text);
    if on.secrets {
        for (kind, rule) in SECRETS {
            let found = rule.matches(text.as_bytes())?;
            text = replace(
                text,
                "secret",
                found.into_iter().map(|span| (span, kind)),
                &mut counts,
            )?;
        }
        let found = keywords(text.as_bytes());
        text = replace(text, "secret", found, &mut counts)?;
    }
    if on.pii {
        for (kind, rule) in PII {
            let found = rule.matches(text.as_bytes())?;
            text = replace(
                text,
                "pii",
                found.into_iter().map(|span| (span, kind)),
                &mut counts,
            )?;
        }
    }
    Some((text.into_owned(), counts))
}

/// `text` with each match in `found`, left to right, replaced by its marker and counted.
fn replace<'a>(
    text: Cow<'a, str>,
    family: &str,
    found: impl IntoIterator<Item = (Span, &'static str)>,
    counts: &mut Counts,
) -> Option<Cow<'a, str>> {
    let mut out = String::new();
    let mut from = 0;
    for ((start, end), kind) in found {
        out.push_str(text.get(from..start)?);
        out.extend(["[", family, ": ", kind, "]"]);
        *counts.entry(format!("{family}:{kind}")).or_default() += 1;
        from = end;
    }
    if from == 0 {
        return Some(text);
    }
    out.push_str(text.get(from..)?);
    Some(Cow::Owned(out))
}

/// How a rule finds its matches, each search going on where the last match ended.
#[derive(Clone, Copy)]
enum Rule {
    /// All of them in one pass, which keeps what it has read.
    All(fn(&[u8]) -> Vec<Span>),
    /// The next one from a position.
    Next(fn(&[u8], usize) -> Option<Span>),
    /// Where the match at a start ends, or the next start that could begin one;
    /// no ASCII letter, digit or `_` may touch a match.
    Bounded(fn(&[u8], usize) -> Result<usize, usize>),
}

impl Rule {
    /// Every match in `bytes`, left to right; `None` for one behind the search, or empty.
    fn matches(self, bytes: &[u8]) -> Option<Vec<Span>> {
        match self {
            Self::All(all) => Some(all(bytes)),
            Self::Next(next) => {
                let mut found = Vec::new();
                let mut from = 0;
                while let Some((start, end)) = next(bytes, from) {
                    if start < from || end <= start {
                        return None;
                    }
                    found.push((start, end));
                    from = end;
                }
                Some(found)
            }
            Self::Bounded(at) => Some(bounded(bytes, at)),
        }
    }
}

/// Every match of a bounded rule, tried only at a start no word byte precedes.
fn bounded(bytes: &[u8], at: fn(&[u8], usize) -> Result<usize, usize>) -> Vec<Span> {
    let mut found = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        // Every head opens with a letter.
        if !bytes.get(start).is_some_and(u8::is_ascii_alphabetic)
            || byte_before(bytes, start).is_some_and(word_byte)
        {
            start += 1;
            continue;
        }
        let next = match at(bytes, start) {
            Ok(end) if !word(bytes, end) => {
                found.push((start, end));
                end
            }
            // Every later start inside a match touched after ends there, touched too.
            Ok(end) | Err(end) => end,
        };
        start = next.max(start + 1);
    }
    found
}

/// The secrets' rules, in their order; the keywords' values come after them.
const SECRETS: [(&str, Rule); 11] = [
    ("private_key", Rule::All(private_keys)),
    ("connection_string", Rule::Next(connection_string)),
    ("jwt", Rule::Bounded(jwt)),
    ("anthropic_key", Rule::Bounded(anthropic_key)),
    ("openai_key", Rule::Bounded(openai_key)),
    ("stripe_key", Rule::Bounded(stripe_key)),
    ("github_token", Rule::Bounded(github_token)),
    ("slack_token", Rule::Bounded(slack_token)),
    ("aws_key", Rule::Bounded(aws_key)),
    ("gcp_key", Rule::Bounded(gcp_key)),
    ("telmoni_key", Rule::Bounded(telmoni_key)),
];

/// PII's rules, in their order.
const PII: [(&str, Rule); 7] = [
    ("email", Rule::Next(email)),
    ("iban", Rule::Next(iban)),
    ("card", Rule::Next(card)),
    ("ssn", Rule::Next(ssn)),
    ("phone", Rule::Next(phone)),
    ("ip", Rule::Next(ip)),
    ("home_path", Rule::Next(home_path)),
];

// ── Secrets ──────────────────────────────────────────────────────────────────

/// Each private key: from a BEGIN to the first of its END, a `"` no backslash
/// escapes, which ends a JSON string, and the text's end.
fn private_keys(bytes: &[u8]) -> Vec<Span> {
    let mut found = Vec::new();
    let (mut ends, mut quotes) = (None, None);
    let mut from = 0;
    while let Some((start, begun)) = armor(bytes, from, b"-----BEGIN ") {
        let quote = cached(
            &mut quotes,
            begun,
            |quote| quote,
            || unescaped_quote(bytes, begun),
        );
        let ended = cached(
            &mut ends,
            begun,
            |(at, _)| at,
            || armor(bytes, begun, b"-----END "),
        );
        let end = match ended {
            Some((_, end)) if quote.is_none_or(|quote| end <= quote) => end,
            _ => quote.unwrap_or(bytes.len()),
        };
        found.push((start, end));
        from = end;
    }
    found
}

/// The last search's answer where it stands for one from `from`, as no later
/// find lies between; else `search`'s, kept. Many keys before one find read to it once.
fn cached<T: Copy>(
    last: &mut Option<(usize, Option<T>)>,
    from: usize,
    at: impl Fn(T) -> usize,
    search: impl FnOnce() -> Option<T>,
) -> Option<T> {
    if let Some((searched, found)) = *last
        && searched <= from
        && found.is_none_or(|found| from <= at(found))
    {
        return found;
    }
    let found = search();
    *last = Some((from, found));
    found
}

/// The first `head`, `[A-Z0-9 ]*` and `PRIVATE KEY-----` from `from`: its start
/// and end. The run is whole, the dashes after it not of its class.
fn armor(bytes: &[u8], from: usize, head: &[u8]) -> Option<Span> {
    (from..bytes.len()).find_map(|start| {
        if !holds(bytes, start, head) {
            return None;
        }
        let label = start + head.len();
        let end = run(bytes, label, armor_byte);
        (end >= label + 11 && holds(bytes, end - 11, b"PRIVATE KEY") && holds(bytes, end, b"-----"))
            .then_some((start, end + 5))
    })
}

/// The first `"` from `from` after an even run of backslashes, none among it.
fn unescaped_quote(bytes: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    loop {
        let quote = at + bytes.get(at..)?.iter().position(|&b| b == b'"')?;
        let escapes = bytes
            .get(..quote)?
            .iter()
            .rev()
            .take_while(|&&b| b == b'\\')
            .count();
        if escapes.is_multiple_of(2) {
            return Some(quote);
        }
        at = quote + 1;
    }
}

/// The next connection string, found by its `://`: a scheme of
/// `[A-Za-z][A-Za-z0-9+.-]{1,20}` before it, its credentials and the rest after.
fn connection_string(bytes: &[u8], from: usize) -> Option<Span> {
    let mut at = from;
    loop {
        let slashes = find(bytes, at, b"://")?;
        at = slashes + 1;
        // The scheme ends at the colon, not of its class: the run back from it.
        let reach = (slashes.saturating_sub(21).max(from)..slashes)
            .rev()
            .take_while(|&i| bytes.get(i).copied().is_some_and(scheme_byte))
            .last();
        let start = reach.and_then(|reach| {
            (reach..slashes.saturating_sub(1))
                .find(|&i| bytes.get(i).is_some_and(u8::is_ascii_alphabetic))
        });
        if let Some(start) = start
            && let Some(end) = credentials(bytes, slashes + 3)
        {
            return Some((start, end));
        }
    }
}

/// Where a user, `:`, a password, `@` and the rest from `at` end, each ended by
/// whitespace, a quote or a backslash, the user by `/@:` too, the password by `/@`.
fn credentials(bytes: &[u8], at: usize) -> Option<usize> {
    let user = run(bytes, at, |b| {
        !ends_run(b) && !matches!(b, b'/' | b'@' | b':')
    });
    if user == at || bytes.get(user) != Some(&b':') {
        return None;
    }
    let password = run(bytes, user + 1, |b| {
        !ends_run(b) && !matches!(b, b'/' | b'@')
    });
    if password == user + 1 || bytes.get(password) != Some(&b'@') {
        return None;
    }
    let rest = run(bytes, password + 1, |b| !ends_run(b));
    (rest > password + 1).then_some(rest)
}

/// ASCII whitespace, `\v` among it, a quote or a backslash.
fn ends_run(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c' | b'"' | b'\'' | b'\\'
    )
}

/// `eyJ…`, `.`, `eyJ…`, `.` and `…`, each run 8 or more of `[A-Za-z0-9_-]`; once
/// its header is read, a start fails as every later start inside it does.
fn jwt(bytes: &[u8], at: usize) -> Result<usize, usize> {
    if !holds(bytes, at, b"eyJ") {
        return Err(at + 1);
    }
    let header = run(bytes, at + 3, token_byte);
    let signed = || {
        if header - (at + 3) < 8 {
            return None;
        }
        let payload = led(bytes, dot(bytes, header)?, b"eyJ", token_byte, 8).ok()?;
        led(bytes, dot(bytes, payload)?, b"", token_byte, 8).ok()
    };
    signed().ok_or(header)
}

/// Past the `.` at `at`.
fn dot(bytes: &[u8], at: usize) -> Option<usize> {
    (bytes.get(at) == Some(&b'.')).then_some(at + 1)
}

/// `sk-ant-`, then 20 or more of `[A-Za-z0-9_-]`.
fn anthropic_key(bytes: &[u8], at: usize) -> Result<usize, usize> {
    led(bytes, at, b"sk-ant-", token_byte, 20)
}

/// `sk-`, an optional `proj-`, `svcacct-` or `admin-`, then 20 or more of
/// `[A-Za-z0-9_-]`: each prefix is of the class, so the run from `sk-` is the match.
fn openai_key(bytes: &[u8], at: usize) -> Result<usize, usize> {
    led(bytes, at, b"sk-", token_byte, 20)
}

/// `sk_` or `rk_`, `live_` or `test_`, then 16 or more of `[A-Za-z0-9]`; or `whsec_` and as many.
fn stripe_key(bytes: &[u8], at: usize) -> Result<usize, usize> {
    let heads: [&[u8]; 5] = [
        b"sk_live_",
        b"sk_test_",
        b"rk_live_",
        b"rk_test_",
        b"whsec_",
    ];
    match heads.into_iter().find(|head| holds(bytes, at, head)) {
        Some(head) => led(bytes, at, head, alnum, 16),
        None => Err(at + 1),
    }
}

/// `gh`, one of `pousr` and `_`, then 36 or more of `[A-Za-z0-9]`; or
/// `github_pat_`, then 22 or more of `[A-Za-z0-9_]`.
fn github_token(bytes: &[u8], at: usize) -> Result<usize, usize> {
    let heads: [&[u8]; 5] = [b"ghp_", b"gho_", b"ghu_", b"ghs_", b"ghr_"];
    match heads.into_iter().find(|head| holds(bytes, at, head)) {
        Some(head) => led(bytes, at, head, alnum, 36),
        None => led(bytes, at, b"github_pat_", word_byte, 22),
    }
}

/// `xox`, one of `abposr` and `-`, then 10 or more of `[A-Za-z0-9-]`.
fn slack_token(bytes: &[u8], at: usize) -> Result<usize, usize> {
    let heads: [&[u8]; 6] = [b"xoxa-", b"xoxb-", b"xoxp-", b"xoxo-", b"xoxs-", b"xoxr-"];
    match heads.into_iter().find(|head| holds(bytes, at, head)) {
        Some(head) => led(bytes, at, head, label_byte, 10),
        None => Err(at + 1),
    }
}

/// `AKIA` or `ASIA`, then exactly 16 of `[0-9A-Z]`.
fn aws_key(bytes: &[u8], at: usize) -> Result<usize, usize> {
    if !holds(bytes, at, b"AKIA") && !holds(bytes, at, b"ASIA") {
        return Err(at + 1);
    }
    exactly(bytes, at + 4, 16, upper_or_digit).ok_or(at + 1)
}

/// `AIza`, then exactly 35 of `[A-Za-z0-9_-]`. Touched after, a later start in
/// the window, past a `-`, may still match, so the search moves on a byte.
fn gcp_key(bytes: &[u8], at: usize) -> Result<usize, usize> {
    if !holds(bytes, at, b"AIza") {
        return Err(at + 1);
    }
    exactly(bytes, at + 4, 35, token_byte)
        .filter(|&end| !word(bytes, end))
        .ok_or(at + 1)
}

/// `tm_`, then 20 or more of `[A-Za-z0-9_-]`.
fn telmoni_key(bytes: &[u8], at: usize) -> Result<usize, usize> {
    led(bytes, at, b"tm_", token_byte, 20)
}

/// The words a secret is set by, the longest first, each with whether what it
/// sets is a password; matched in any ASCII case.
const KEYWORDS: [(&str, bool); 13] = [
    ("sharedaccesskey", false),
    ("client_secret", false),
    ("accountkey", false),
    ("access_key", false),
    ("access-key", false),
    ("password", true),
    ("api_key", false),
    ("api-key", false),
    ("apikey", false),
    ("passwd", true),
    ("secret", false),
    ("token", false),
    ("pwd", true),
];

/// What may stand, up to four, between a keyword and its `:` or `=`, and after them.
const SEPARATORS: &[u8] = b" \t\"'\\";

/// What ends a keyword's value: ASCII whitespace, a quote, a backslash, or what
/// closes a field or opens a marker.
const VALUE_ENDS: &[u8] = b" \t\n\r\x0b\x0c\"'\\,;&{}()<>[]";

/// Each value a keyword sets that reads as a secret, and its kind: a password's of
/// 8 characters or more, a token's of 16 or more, holding an ASCII letter and a digit.
fn keywords(bytes: &[u8]) -> Vec<(Span, &'static str)> {
    let mut found = Vec::new();
    // The run last read, which every value starting inside it ends with.
    let mut known: Option<Value> = None;
    let mut at = 0;
    while at < bytes.len() {
        let Some((start, password)) = assignment(bytes, at) else {
            at += 1;
            continue;
        };
        let value = match known {
            Some(value) if (value.start..=value.end).contains(&start) => value,
            _ => Value::read(bytes, start),
        };
        known = Some(value);
        let secret = if password {
            chars_at_least(bytes, start, value.end, 8)
        } else {
            chars_at_least(bytes, start, value.end, 16)
                && value.letter.is_some_and(|letter| letter >= start)
                && value.digit.is_some_and(|digit| digit >= start)
        };
        if secret {
            found.push((
                (start, value.end),
                if password { "password" } else { "token" },
            ));
            at = value.end;
        } else {
            at += 1;
        }
    }
    found
}

/// Where the value a keyword at `at` sets starts, and whether it is a password.
fn assignment(bytes: &[u8], at: usize) -> Option<(usize, bool)> {
    // Every keyword opens with a letter.
    if !bytes.get(at).is_some_and(u8::is_ascii_alphabetic)
        || byte_before(bytes, at).is_some_and(word_byte)
    {
        return None;
    }
    let (name, password) = KEYWORDS.into_iter().find(|(name, _)| {
        bytes
            .get(at..at + name.len())
            .is_some_and(|given| given.eq_ignore_ascii_case(name.as_bytes()))
    })?;
    let assigned = separated(bytes, at + name.len());
    matches!(bytes.get(assigned), Some(b':' | b'='))
        .then(|| (separated(bytes, assigned + 1), password))
}

/// Past the separators, up to four, at `at`.
fn separated(bytes: &[u8], at: usize) -> usize {
    run_max(bytes, at, 4, |b| SEPARATORS.contains(&b))
}

/// A run of a value's bytes: where it starts and ends, and its last ASCII letter and digit.
#[derive(Clone, Copy)]
struct Value {
    start: usize,
    end: usize,
    letter: Option<usize>,
    digit: Option<usize>,
}

impl Value {
    fn read(bytes: &[u8], start: usize) -> Self {
        let mut value = Self {
            start,
            end: start,
            letter: None,
            digit: None,
        };
        while let Some(&b) = bytes.get(value.end).filter(|b| !VALUE_ENDS.contains(b)) {
            if b.is_ascii_alphabetic() {
                value.letter = Some(value.end);
            }
            if b.is_ascii_digit() {
                value.digit = Some(value.end);
            }
            value.end += 1;
        }
        value
    }
}

/// Whether `bytes[start..end]` holds `least` code points or more, as every SDK counts a
/// length, never bytes: read no further than them.
fn chars_at_least(bytes: &[u8], start: usize, end: usize, least: usize) -> bool {
    bytes.get(start..end).is_some_and(|value| {
        value
            .iter()
            .filter(|&&b| (b & 0xC0) != 0x80)
            .take(least)
            .count()
            >= least
    })
}

// ── PII ──────────────────────────────────────────────────────────────────────

/// The next email, found by its `@`: a local part of `[A-Za-z0-9._%+-]`, reaching
/// back no further than `from`, then a domain.
fn email(bytes: &[u8], from: usize) -> Option<Span> {
    let mut at = from;
    loop {
        let sign = at + bytes.get(at..)?.iter().position(|&b| b == b'@')?;
        at = sign + 1;
        let local = (from..sign)
            .rev()
            .take_while(|&i| bytes.get(i).copied().is_some_and(local_byte))
            .last();
        if let Some(start) = local
            && let Some(end) = domain(bytes, sign + 1)
        {
            return Some((start, end));
        }
    }
}

/// Where a domain from `at` ends: labels of `[A-Za-z0-9-]` joined by dots, ended
/// at the 2 or more letters the last label after the first starts with.
fn domain(bytes: &[u8], at: usize) -> Option<usize> {
    let mut end = run(bytes, at, label_byte);
    if end == at {
        return None;
    }
    let mut labels = Vec::new();
    while bytes.get(end) == Some(&b'.') {
        let next = run(bytes, end + 1, label_byte);
        if next == end + 1 {
            break;
        }
        labels.push(end + 1);
        end = next;
    }
    labels.into_iter().rev().find_map(|label| {
        let letters = run(bytes, label, |b| b.is_ascii_alphabetic());
        (letters - label >= 2).then_some(letters)
    })
}

/// Two of `[A-Z]` and two digits, then 11 to 30 of `[A-Z0-9]`, or groups of four after a
/// space each (tried first), untouched by a word byte and passing ISO 7064's mod 97.
fn iban(bytes: &[u8], from: usize) -> Option<Span> {
    (from..bytes.len()).find_map(|start| {
        if byte_before(bytes, start).is_some_and(word_byte)
            || exactly(bytes, start, 2, |b| b.is_ascii_uppercase()).is_none()
            || !digits_at(bytes, start + 2, 2)
        {
            return None;
        }
        [grouped(bytes, start), compact(bytes, start)]
            .into_iter()
            .flatten()
            .find(|&end| !word(bytes, end) && iban_valid(bytes, start, end))
            .map(|end| (start, end))
    })
}

/// Where the groups at `start` end, a last of 1 to 3 allowed; `None` for none, or
/// past 7, which no IBAN is short enough to hold.
fn grouped(bytes: &[u8], start: usize) -> Option<usize> {
    let mut end = start + 4;
    let mut groups = 0;
    while groups < 8
        && bytes.get(end) == Some(&b' ')
        && exactly(bytes, end + 1, 4, upper_or_digit).is_some()
    {
        end += 5;
        groups += 1;
    }
    if !(1..8).contains(&groups) {
        return None;
    }
    if bytes.get(end) == Some(&b' ') {
        let last = run_max(bytes, end + 1, 3, upper_or_digit);
        if last > end + 1 {
            end = last;
        }
    }
    Some(end)
}

/// Where the compact form at `start` ends.
fn compact(bytes: &[u8], start: usize) -> Option<usize> {
    let end = run_max(bytes, start + 4, 30, upper_or_digit);
    (end >= start + 15).then_some(end)
}

/// Whether `bytes[start..end]`, spaces out, is 15 to 34 long and gives 1 mod 97
/// with its first four moved to its end, each letter read as 10 to 35.
fn iban_valid(bytes: &[u8], start: usize, end: usize) -> bool {
    let Some(given) = bytes.get(start..end) else {
        return false;
    };
    let joined: Vec<u8> = given.iter().copied().filter(|&b| b != b' ').collect();
    let Some((head, tail)) = joined.split_at_checked(4) else {
        return false;
    };
    (15..=34).contains(&joined.len())
        && tail.iter().chain(head).fold(0, |rest, &b| {
            if b.is_ascii_digit() {
                (rest * 10 + u32::from(b - b'0')) % 97
            } else {
                (rest * 100 + u32::from(b) - 55) % 97
            }
        }) == 1
}

/// 13 to 19 digits, one space or hyphen allowed between two, untouched by an
/// ASCII letter or digit, passing Luhn's check: the longest that does.
fn card(bytes: &[u8], from: usize) -> Option<Span> {
    (from..bytes.len()).find_map(|start| {
        // An issuer's first digit: a timestamp or an id that passes Luhn's check is no card.
        if !matches!(bytes.get(start), Some(b'2'..=b'6'))
            || byte_before(bytes, start).is_some_and(alnum)
        {
            return None;
        }
        // Where a match would end after each digit read.
        let mut ends = Vec::with_capacity(19);
        ends.push(start + 1);
        let mut at = start + 1;
        while ends.len() < 19 {
            if digit_at(bytes, at) {
                at += 1;
            } else if matches!(bytes.get(at), Some(b' ' | b'-')) && digit_at(bytes, at + 1) {
                at += 2;
            } else {
                break;
            }
            ends.push(at);
        }
        (13..=ends.len()).rev().find_map(|count| {
            let read = ends.get(..count)?;
            let end = *read.last()?;
            (!alnum_at(bytes, end) && luhn(bytes, read)).then_some((start, end))
        })
    })
}

/// Luhn's check over the digits just before each of `ends`.
fn luhn(bytes: &[u8], ends: &[usize]) -> bool {
    let sum: u32 = ends
        .iter()
        .rev()
        .enumerate()
        .map(|(n, &end)| {
            let digit = bytes.get(end - 1).map_or(0, |&b| u32::from(b - b'0'));
            match (n % 2 == 1, digit > 4) {
                (false, _) => digit,
                (true, false) => digit * 2,
                (true, true) => digit * 2 - 9,
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// `[0-9]{3}-[0-9]{2}-[0-9]{4}`, untouched by an ASCII letter or digit, its area
/// not 000, 666 or 9xx, its group not 00 and its serial not 0000.
fn ssn(bytes: &[u8], from: usize) -> Option<Span> {
    (from..bytes.len()).find_map(|start| {
        if byte_before(bytes, start).is_some_and(alnum) {
            return None;
        }
        let end = start + 11;
        let area = bytes.get(start..start + 3)?;
        let group = bytes.get(start + 4..start + 6)?;
        let serial = bytes.get(start + 7..end)?;
        let digits = |part: &[u8]| part.iter().all(u8::is_ascii_digit);
        (digits(area)
            && bytes.get(start + 3) == Some(&b'-')
            && digits(group)
            && bytes.get(start + 6) == Some(&b'-')
            && digits(serial)
            && !alnum_at(bytes, end)
            && area != b"000"
            && area != b"666"
            && area.first() != Some(&b'9')
            && group != b"00"
            && serial != b"0000")
            .then_some((start, end))
    })
}

/// An optional `+` and 1 to 3 digits, then 3 digits, bare or in parentheses, 3 and 4,
/// each after an optional space, dot or hyphen, untouched by an ASCII letter or digit,
/// no `+` before; bare digits are no phone, which needs a `+`, separator or parenthesis.
fn phone(bytes: &[u8], from: usize) -> Option<Span> {
    (from..bytes.len()).find_map(|start| {
        if byte_before(bytes, start).is_some_and(|b| alnum(b) || b == b'+') {
            return None;
        }
        let mut at = start;
        let country = bytes.get(at) == Some(&b'+');
        if country {
            let code = run_max(bytes, at + 1, 3, |b| b.is_ascii_digit());
            if code == at + 1 {
                return None;
            }
            at = separator(bytes, code);
        }
        if bytes.get(at) == Some(&b'(') {
            if !digits_at(bytes, at + 1, 3) || bytes.get(at + 4) != Some(&b')') {
                return None;
            }
            at += 5;
        } else if digits_at(bytes, at, 3) {
            at += 3;
        } else {
            return None;
        }
        at = separator(bytes, at);
        if !digits_at(bytes, at, 3) {
            return None;
        }
        at = separator(bytes, at + 3);
        if !digits_at(bytes, at, 4) || alnum_at(bytes, at + 4) {
            return None;
        }
        let end = at + 4;
        let marked = bytes.get(start..end)?.iter().any(|b| b" .-()".contains(b));
        (country || marked).then_some((start, end))
    })
}

/// Past an optional space, dot or hyphen at `at`.
fn separator(bytes: &[u8], at: usize) -> usize {
    at + usize::from(matches!(bytes.get(at), Some(b' ' | b'.' | b'-')))
}

/// An IPv4 address untouched by an ASCII letter, digit or dot; else an IPv6 run of
/// hex digits and colons, holding `::` or seven colons, untouched by those or a letter.
fn ip(bytes: &[u8], from: usize) -> Option<Span> {
    let hex = |b: u8| b.is_ascii_hexdigit() || b == b':';
    (from..bytes.len()).find_map(|start| {
        let first = *bytes.get(start)?;
        let before = byte_before(bytes, start);
        if first.is_ascii_digit()
            && !before.is_some_and(|b| alnum(b) || b == b'.')
            && let Some(end) = ipv4(bytes, start)
        {
            return Some((start, end));
        }
        if !hex(first) || before.is_some_and(|b| hex(b) || b.is_ascii_alphabetic()) {
            return None;
        }
        let end = run(bytes, start, hex);
        let candidate = bytes.get(start..end)?;
        let colons = candidate.iter().filter(|&&b| b == b':').count();
        (colons >= 2
            && (pairs(candidate) > 0 || colons == 7)
            && !bytes.get(end).is_some_and(u8::is_ascii_alphabetic)
            && ipv6_valid(candidate))
        .then_some((start, end))
    })
}

/// Where four parts of 1 to 3 digits, each up to 255, joined by dots, end at `start`.
fn ipv4(bytes: &[u8], start: usize) -> Option<usize> {
    let mut at = start;
    for part in 0..4 {
        if part > 0 {
            if bytes.get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
        let end = run_max(bytes, at, 3, |b| b.is_ascii_digit());
        let value = bytes
            .get(at..end)?
            .iter()
            .fold(0, |value, &b| value * 10 + u32::from(b - b'0'));
        if end == at || value > 255 {
            return None;
        }
        at = end;
    }
    (!bytes.get(at).is_some_and(|&b| alnum(b) || b == b'.')).then_some(at)
}

/// Whether a run of hex digits and colons is an IPv6 address: groups of 1 to 4 hex
/// digits, 8 of them, or up to 7 about one `::`.
fn ipv6_valid(candidate: &[u8]) -> bool {
    let groups = match pairs(candidate) {
        0 => {
            let groups = colon_parts(candidate);
            if groups.len() != 8 {
                return false;
            }
            groups
        }
        1 => {
            let Some(at) = candidate.windows(2).position(|pair| pair == b"::") else {
                return false;
            };
            let mut groups = colon_parts(candidate.get(..at).unwrap_or_default());
            groups.extend(colon_parts(candidate.get(at + 2..).unwrap_or_default()));
            if groups.len() > 7 {
                return false;
            }
            groups
        }
        _ => return false,
    };
    groups
        .iter()
        .all(|group| (1..=4).contains(&group.len()) && group.iter().all(u8::is_ascii_hexdigit))
}

/// How many `::` `candidate` holds, none overlapping: `:::` holds one.
fn pairs(candidate: &[u8]) -> usize {
    let mut count = 0;
    let mut rest = candidate;
    while let Some(at) = rest.windows(2).position(|pair| pair == b"::") {
        count += 1;
        rest = rest.get(at + 2..).unwrap_or_default();
    }
    count
}

/// The groups between `part`'s colons; none in an empty part.
fn colon_parts(part: &[u8]) -> Vec<&[u8]> {
    if part.is_empty() {
        Vec::new()
    } else {
        part.split(|&b| b == b':').collect()
    }
}

/// `/home/`, `/Users/`, or a drive's letter and `:` then `Users` between separators,
/// a `/` or one or two backslashes, as JSON doubles them; then the user, `[A-Za-z0-9._-]+`.
fn home_path(bytes: &[u8], from: usize) -> Option<Span> {
    (from..bytes.len()).find_map(|start| {
        let user = if holds(bytes, start, b"/home/") {
            start + 6
        } else if holds(bytes, start, b"/Users/") {
            start + 7
        } else if bytes.get(start).is_some_and(u8::is_ascii_alphabetic)
            && bytes.get(start + 1) == Some(&b':')
        {
            let users = past_separator(bytes, start + 2, |at| holds(bytes, at, b"Users"))?;
            past_separator(bytes, users + 5, |at| {
                bytes.get(at).copied().is_some_and(user_byte)
            })?
        } else {
            return None;
        };
        let end = run(bytes, user, user_byte);
        (end > user).then_some((start, end))
    })
}

/// Past the separator at `at` after which `then` holds: two backslashes, else one or a `/`.
fn past_separator(bytes: &[u8], at: usize, then: impl Fn(usize) -> bool) -> Option<usize> {
    if holds(bytes, at, b"\\\\") && then(at + 2) {
        return Some(at + 2);
    }
    (matches!(bytes.get(at), Some(b'\\' | b'/')) && then(at + 1)).then_some(at + 1)
}

// ── Bytes ────────────────────────────────────────────────────────────────────

/// `head`, then `least` or more of `class`, the run whole; one too short gives where
/// it ends, as every later start inside it ends there and falls short too.
fn led(
    bytes: &[u8],
    at: usize,
    head: &[u8],
    class: impl Fn(u8) -> bool,
    least: usize,
) -> Result<usize, usize> {
    if !holds(bytes, at, head) {
        return Err(at + 1);
    }
    let from = at + head.len();
    let end = run(bytes, from, class);
    if end - from >= least {
        Ok(end)
    } else {
        Err(end)
    }
}

/// Where exactly `n` of `class` from `at` end.
fn exactly(bytes: &[u8], at: usize, n: usize, class: impl Fn(u8) -> bool) -> Option<usize> {
    bytes
        .get(at..at + n)
        .filter(|window| window.iter().all(|&b| class(b)))
        .map(|_| at + n)
}

/// Where the run of `class` from `from` ends.
fn run(bytes: &[u8], from: usize, class: impl Fn(u8) -> bool) -> usize {
    run_max(bytes, from, usize::MAX, class)
}

/// Where the run of `class` from `from` ends, `most` long at most.
fn run_max(bytes: &[u8], from: usize, most: usize, class: impl Fn(u8) -> bool) -> usize {
    from + bytes.get(from..).map_or(0, |rest| {
        rest.iter().take(most).take_while(|&&b| class(b)).count()
    })
}

/// Where `needle` first stands from `at`.
fn find(bytes: &[u8], at: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(at..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|i| at + i)
}

/// Whether `literal` stands at `at`.
fn holds(bytes: &[u8], at: usize, literal: &[u8]) -> bool {
    bytes
        .get(at..)
        .is_some_and(|rest| rest.starts_with(literal))
}

/// Whether `n` digits stand at `at`.
fn digits_at(bytes: &[u8], at: usize, n: usize) -> bool {
    exactly(bytes, at, n, |b| b.is_ascii_digit()).is_some()
}

fn digit_at(bytes: &[u8], at: usize) -> bool {
    bytes.get(at).is_some_and(u8::is_ascii_digit)
}

fn alnum_at(bytes: &[u8], at: usize) -> bool {
    bytes.get(at).is_some_and(u8::is_ascii_alphanumeric)
}

fn word(bytes: &[u8], at: usize) -> bool {
    bytes.get(at).copied().is_some_and(word_byte)
}

fn byte_before(bytes: &[u8], at: usize) -> Option<u8> {
    at.checked_sub(1)
        .and_then(|before| bytes.get(before))
        .copied()
}

/// `[A-Za-z0-9]`.
fn alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// `[A-Za-z0-9_]`.
fn word_byte(b: u8) -> bool {
    alnum(b) || b == b'_'
}

/// `[A-Za-z0-9_-]`.
fn token_byte(b: u8) -> bool {
    word_byte(b) || b == b'-'
}

/// `[A-Za-z0-9-]`.
fn label_byte(b: u8) -> bool {
    alnum(b) || b == b'-'
}

/// `[0-9A-Z]`.
fn upper_or_digit(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit()
}

/// `[A-Z0-9 ]`.
fn armor_byte(b: u8) -> bool {
    upper_or_digit(b) || b == b' '
}

/// `[A-Za-z0-9+.-]`.
fn scheme_byte(b: u8) -> bool {
    alnum(b) || matches!(b, b'+' | b'.' | b'-')
}

/// `[A-Za-z0-9._%+-]`.
fn local_byte(b: u8) -> bool {
    alnum(b) || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-')
}

/// `[A-Za-z0-9._-]`.
fn user_byte(b: u8) -> bool {
    alnum(b) || matches!(b, b'.' | b'_' | b'-')
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    const SECRETS_ONLY: Switches = Switches {
        secrets: true,
        pii: false,
    };
    const BOTH: Switches = Switches {
        secrets: true,
        pii: true,
    };

    /// ⚠ Every case of `canary.json`, in each of its modes, comes out as it says.
    #[test]
    fn each_canary_case_comes_out_as_the_fixture_says() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/canary.json");
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("the fixture reads"))
                .expect("the fixture parses");
        let modes = fixture["modes"].as_object().expect("the modes");
        let cases = fixture["cases"].as_array().expect("the cases");
        assert!(modes.len() == 3 && !cases.is_empty());
        for case in cases {
            let (name, text) = (&case["name"], case["text"].as_str().expect("a text"));
            for (mode, switches) in modes {
                let on = Switches {
                    secrets: switches["scrub_secrets"].as_bool().expect("a switch"),
                    pii: switches["scrub_pii"].as_bool().expect("a switch"),
                };
                let expected = &case["expect"][mode];
                let counts: Counts =
                    serde_json::from_value(expected["scrubbed"].clone()).expect("the counts");
                assert_eq!(
                    scrub(text, on),
                    Some((
                        expected["text"].as_str().expect("a text").to_owned(),
                        counts
                    )),
                    "{name} in {mode}"
                );
            }
        }
    }

    /// Text past ASCII stays whole, counts as characters, and neither ends a run nor folds.
    #[test]
    fn text_past_ascii_is_kept_whole_and_never_read_as_ascii() {
        let scrubbed = |text: &str, on| scrub(text, on).expect("it scrubs").0;
        assert_eq!(
            scrubbed(
                "日本 AKIAIOSFODNN7EXAMPLE 😀, password=ééééééé, pwd=éééééééé",
                SECRETS_ONLY
            ),
            "日本 [secret: aws_key] 😀, password=ééééééé, pwd=[secret: password]"
        );
        assert_eq!(
            scrubbed(
                "db postgres://app:Hunter2@db\u{a0}x y and redis://u:p@h\u{b}z",
                SECRETS_ONLY
            ),
            "db [secret: connection_string] y and [secret: connection_string]\u{b}z"
        );
        assert_eq!(
            scrubbed(
                "İ password=Hunter2canary and api\u{212a}ey=abc123def456ghi789jkl",
                SECRETS_ONLY
            ),
            "İ password=[secret: password] and api\u{212a}ey=abc123def456ghi789jkl"
        );
        assert_eq!(
            scrubbed(
                "ünïcode dana.whitfield@example.com — 4242 4242 4242 4242 → /home/dana/x",
                BOTH
            ),
            "ünïcode [pii: email] — [pii: card] → [pii: home_path]/x"
        );
    }

    /// Hostile text at a full ingest export's size stays within the scanning budget.
    #[test]
    fn hostile_text_is_scrubbed_in_linear_time() {
        // The bar is 2 s; `cargo test` builds without optimizations, hence 5.
        const LIMIT: Duration = Duration::from_secs(5);
        let units = [
            ("eyJ-", ""),
            ("eyJ", ""),
            ("token=", ""),
            ("password=", ""),
            ("atm_", ""),
            ("xsk-ant-", ""),
            ("xoxb-", "_"),
            ("a", ""),
            ("a@", ""),
            ("a@b.", ""),
            ("x://", ""),
            ("a:b", " "),
            ("1", ""),
            ("1 ", ""),
            ("1.", ""),
            ("a:", ""),
            ("/home/", ""),
            ("AB12", ""),
            ("-----BEGIN PRIVATE KEY-----", ""),
            ("+1 ", ""),
            // Many keys before one far END, or one far quote.
            ("-----BEGIN PRIVATE KEY-----\"", "-----END PRIVATE KEY-----"),
            (
                "-----BEGIN PRIVATE KEY----- -----END PRIVATE KEY-----",
                "\"",
            ),
            ("AIza-", ""),
            ("\\", "\""),
        ];
        for (unit, tail) in units {
            let text = unit.repeat(1_048_576 / unit.len() + 1) + tail;
            for on in [SECRETS_ONLY, BOTH] {
                let started = Instant::now();
                assert!(scrub(&text, on).is_some(), "{unit:?}");
                let took = started.elapsed();
                assert!(took < LIMIT, "{unit:?} took {took:?}");
            }
        }
    }
}
