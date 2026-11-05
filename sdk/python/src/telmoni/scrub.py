"""The scrubbers over content: each secret, and each piece of personal data where
asked, replaced by a marker naming its kind.

Every SDK holds the same rules, in the same order, to ``sdk/fixtures/canary.json``.
A rule runs over the whole text as the rules before it left it, left to right, and
finds what a backtracking regex engine would, every class ASCII's, in time linear in
the text whatever it holds: content can be a page an agent fetched, written to stall
it. Each rule searches from what its matches start with, so ``re`` skips the rest in
C; a lookbehind follows a pattern's first character, as one leading it would make
``re`` try every position.
"""

from __future__ import annotations

import re
import string
from collections.abc import Callable, Iterator
from itertools import accumulate, pairwise

Match = tuple[int, int, str]
"""A match's start, its end and its kind."""
Rule = Callable[[str], Iterator[Match]]

_WORD = frozenset(string.ascii_letters + string.digits + "_")
_ALNUM = frozenset(string.ascii_letters + string.digits)
_LETTERS = frozenset(string.ascii_letters)
_DIGITS = frozenset(string.digits)


def scrub(text: str, secrets: bool, pii: bool) -> tuple[str, dict[str, int]]:
    """``text`` with each match of the rules switched on replaced by
    ``[secret: <kind>]`` or ``[pii: <kind>]``, and how many of each it replaced,
    keyed ``<family>:<kind>`` in sorted order."""
    counts: dict[str, int] = {}
    for family, rules, on in (("secret", _SECRETS, secrets), ("pii", _PII, pii)):
        if on:
            for rule in rules:
                text = _apply(text, family, rule, counts)
    return text, dict(sorted(counts.items()))


def _apply(text: str, family: str, rule: Rule, counts: dict[str, int]) -> str:
    parts: list[str] = []
    pos = 0
    for start, end, kind in rule(text):
        parts += (text[pos:start], f"[{family}: {kind}]")
        key = f"{family}:{kind}"
        counts[key] = counts.get(key, 0) + 1
        pos = end
    if not parts:
        return text
    parts.append(text[pos:])
    return "".join(parts)


def _regex(compiled: re.Pattern[str], kind: str) -> Rule:
    def rule(text: str) -> Iterator[Match]:
        pos = 0
        while (found := compiled.search(text, pos)) is not None:
            yield found.start(), found.end(), kind
            pos = found.end()

    return rule


# ── Secrets ──────────────────────────────────────────────────────────────────

_BEGIN = re.compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----")
_END = re.compile(r"-----END [A-Z0-9 ]*PRIVATE KEY-----")


def _private_keys(text: str) -> Iterator[Match]:
    """From a BEGIN to its END, or to a ``"`` no backslash escapes, or to the
    text's end, whichever comes first: a key never runs past the end of a JSON
    string, cut off or with its END in a later one, so a message stays whole."""
    pos = 0
    # Each first END and quote after where it was sought, kept while each BEGIN
    # ends before it: sought afresh, a text holding neither is read once a BEGIN.
    end, end_sought = None, -1
    quote, quote_sought = -1, -1
    while (begin := _BEGIN.search(text, pos)) is not None:
        after = begin.end()
        if end_sought < 0 or (end is not None and end.start() < after):
            end, end_sought = _END.search(text, after), after
        if quote_sought < 0 or 0 <= quote < after:
            quote, quote_sought = _closing_quote(text, after), after
        if end is not None and (quote == -1 or end.end() <= quote):
            pos = end.end()
        else:
            pos = len(text) if quote == -1 else quote
        yield begin.start(), pos, "private_key"


def _closing_quote(text: str, pos: int) -> int:
    """The first ``"`` from ``pos`` after an even run of backslashes, none among
    them, or -1."""
    while (quote := text.find('"', pos)) != -1:
        first = quote
        while first > 0 and text[first - 1] == "\\":
            first -= 1
        if (quote - first) % 2 == 0:
            return quote
        pos = quote + 1
    return -1


_SCHEME = frozenset(string.ascii_letters + string.digits + "+.-")
_CREDENTIALS = re.compile(
    r"[^ \t\n\r\f\v\"'\\/@:]+:[^ \t\n\r\f\v\"'\\/@]+@[^ \t\n\r\f\v\"'\\]+"
)
"""What follows a connection string's ``://``: its user, password and the rest."""


def _connection_strings(text: str) -> Iterator[Match]:
    """``scheme://user:password@rest``, read from each ``://``: the scheme a
    letter, then 1 to 20 of ``[A-Za-z0-9+.-]``, starting as far left as it can."""
    pos = 0
    at = text.find("://")
    while at != -1:
        floor = max(pos, at - 21)
        first = at
        while first > floor and text[first - 1] in _SCHEME:
            first -= 1
        start = next((p for p in range(first, at - 1) if text[p] in _LETTERS), None)
        rest = None if start is None else _CREDENTIALS.match(text, at + 3)
        if start is not None and rest is not None:
            yield start, rest.end(), "connection_string"
            pos = rest.end()
            at = text.find("://", pos)
        else:
            at = text.find("://", at + 1)


def _key(starts: str, shape: str, kind: str, run: str | None = None) -> Rule:
    """A key's ``shape``, from where ``starts`` finds its fixed prefix, bounded: no
    ASCII letter, digit or ``_`` on either side. One touching a word is passed
    over, never cut to a shorter match, and the search resumes one character after
    its start, or, where the shape opens with a ``run`` of one class, past that
    run: every start inside it ends where this one did, and fails alike."""
    first, whole = re.compile(starts), re.compile(shape)
    across = re.compile(f"{run}*" if run else "")

    def rule(text: str) -> Iterator[Match]:
        found = first.search(text)
        while found is not None:
            at = found.start()
            # The side before first, so a run a word leads into is never read.
            if at and text[at - 1] in _WORD:
                found = first.search(text, at + 1)
                continue
            key = whole.match(text, at)
            if key is not None and (
                key.end() == len(text) or text[key.end()] not in _WORD
            ):
                yield at, key.end(), kind
                found = first.search(text, key.end())
            else:
                past = across.match(text, at)
                found = first.search(text, max(at + 1, past.end() if past else 0))

    return rule


_KEYWORD = re.compile(
    r"client_secret|sharedaccesskey|accountkey|access_key|access-key|api_key"
    r"|api-key|apikey|password|passwd|secret|token|pwd"
)
_PASSWORDS = frozenset({"password", "passwd", "pwd"})
_ASCII_LOWER = str.maketrans(string.ascii_uppercase, string.ascii_lowercase)
_SEPARATORS = frozenset(" \t\"'\\")
_VALUE_STOP = re.compile(r"[ \t\n\r\f\v\"'\\,;&{}()<>\[\]]")
_LETTER = re.compile(r"[A-Za-z]")
_DIGIT = re.compile(r"[0-9]")


class _Next:
    """The next match of a pattern at or after a position, remembered, so many
    keywords inside one long value read it once."""

    __slots__ = ("_at", "_pattern", "_since", "_text")

    def __init__(self, pattern: re.Pattern[str], text: str) -> None:
        self._pattern = pattern
        self._text = text
        self._since = self._at = -1

    def at(self, pos: int) -> int:
        """Its start, or the text's end where there is none."""
        if not self._since <= pos <= self._at:
            found = self._pattern.search(self._text, pos)
            self._since = pos
            self._at = len(self._text) if found is None else found.start()
        return self._at


def _past_separators(text: str, pos: int) -> int:
    end = min(pos + 4, len(text))
    while pos < end and text[pos] in _SEPARATORS:
        pos += 1
    return pos


def _keywords(text: str) -> Iterator[Match]:
    """The value a keyword is given, the keyword kept: a password's whatever it
    holds, any other's only from 16 characters holding a letter and a digit."""
    # ASCII's letters folded alone: Unicode's lowering would move positions.
    lowered = text.translate(_ASCII_LOWER)
    stop, letter, digit = (_Next(p, text) for p in (_VALUE_STOP, _LETTER, _DIGIT))
    found = _KEYWORD.search(lowered)
    while found is not None:
        at = found.start()
        given = _past_separators(text, found.end())
        if (
            (at == 0 or text[at - 1] not in _WORD)
            and given < len(text)
            and text[given] in ":="
        ):
            start = _past_separators(text, given + 1)
            end = stop.at(start)
            password = found.group() in _PASSWORDS
            if end - start >= 8 and (
                password
                or (
                    end - start >= 16
                    and letter.at(start) < end
                    and digit.at(start) < end
                )
            ):
                yield start, end, "password" if password else "token"
                found = _KEYWORD.search(lowered, end)
                continue
        found = _KEYWORD.search(lowered, at + 1)


_TOKEN_RUN = r"[A-Za-z0-9_\-]"
_SECRETS: tuple[Rule, ...] = (
    _private_keys,
    _connection_strings,
    _key(
        "eyJ",
        r"eyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
        "jwt",
        _TOKEN_RUN,
    ),
    _key("sk-ant-", r"sk-ant-[A-Za-z0-9_\-]{20,}", "anthropic_key", _TOKEN_RUN),
    _key(
        "sk-",
        r"sk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{20,}",
        "openai_key",
        _TOKEN_RUN,
    ),
    # No start inside these runs can match, as each start holds the ``_`` the run
    # lacks or follows a word in it: a failed start reads its own run alone.
    _key(
        "sk_|rk_|whsec_",
        r"(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}|whsec_[A-Za-z0-9]{16,}",
        "stripe_key",
    ),
    _key(
        "gh[pousr]_|github_pat_",
        r"gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,}",
        "github_token",
    ),
    _key(
        "xox[abposr]-",
        r"xox[abposr]-[A-Za-z0-9\-]{10,}",
        "slack_token",
        r"[A-Za-z0-9\-]",
    ),
    # Fixed lengths: a start costs the same however long its run.
    _key("AKIA|ASIA", r"(?:AKIA|ASIA)[0-9A-Z]{16}", "aws_key"),
    _key("AIza", r"AIza[0-9A-Za-z_\-]{35}", "gcp_key"),
    _key("tm_", r"tm_[A-Za-z0-9_\-]{20,}", "telmoni_key", _TOKEN_RUN),
    _keywords,
)

# ── PII ──────────────────────────────────────────────────────────────────────

_LOCAL = frozenset(string.ascii_letters + string.digits + "._%+-")
_DOMAIN = re.compile(r"[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}")


def _emails(text: str) -> Iterator[Match]:
    """``local@domain``, read from each ``@``: the local part as far left as its
    characters run, the domain as a backtracking engine takes it."""
    pos = 0
    at = text.find("@")
    while at != -1:
        start = at
        while start > pos and text[start - 1] in _LOCAL:
            start -= 1
        domain = _DOMAIN.match(text, at + 1) if start < at else None
        if domain is not None:
            yield start, domain.end(), "email"
            pos = domain.end()
            at = text.find("@", pos)
        else:
            at = text.find("@", at + 1)


_IBAN_START = re.compile(r"[A-Z](?<![A-Za-z0-9_][A-Z])[A-Z][0-9]{2}")
_IBAN_FORMS = (
    re.compile(r"[A-Z]{2}[0-9]{2}(?: [A-Z0-9]{4})+(?: [A-Z0-9]{1,3})?"),
    re.compile(r"[A-Z]{2}[0-9]{2}[A-Z0-9]{11,30}"),
)
_IBAN_REACH = 44
"""How far an IBAN is read: the longest that can be valid, grouped, is 43
characters, and one more shows a group that makes it too long."""


def _ibans(text: str) -> Iterator[Match]:
    found = _IBAN_START.search(text)
    while found is not None:
        at = found.start()
        for form in _IBAN_FORMS:
            iban = form.match(text, at, at + _IBAN_REACH)
            if (
                iban is not None
                and (iban.end() == len(text) or text[iban.end()] not in _WORD)
                and _iban_valid(iban.group())
            ):
                yield at, iban.end(), "iban"
                found = _IBAN_START.search(text, iban.end())
                break
        else:
            found = _IBAN_START.search(text, at + 1)


def _iban_valid(candidate: str) -> bool:
    """ISO 13616's check: 15 to 34 characters whose mod 97 is 1."""
    compact = candidate.replace(" ", "")
    if not 15 <= len(compact) <= 34:
        return False
    moved = compact[4:] + compact[:4]
    digits = "".join(str(ord(c) - 55) if c in _LETTERS else c for c in moved)
    return int(digits) % 97 == 1


_DIGIT_RUN = re.compile(r"[0-9](?<![0-9]{2})(?<![0-9][ \-][0-9])(?:[ \-]?[0-9]){12,}")
"""A whole run of digits, one space or hyphen allowed between two, long enough to
hold a card."""
_ISSUERS = frozenset("23456")
_ONCE = bytes.maketrans(b"0123456789", bytes(range(10)))
_TWICE = bytes.maketrans(b"0123456789", bytes((0, 2, 4, 6, 8, 1, 3, 5, 7, 9)))
"""Each digit as Luhn's check doubles it, the doubled number's digits added."""


def _cards(text: str) -> Iterator[Match]:
    """From each issuer's digit, 2 to 6, after no ASCII letter or digit, the most
    of its next 13 to 19 digits that pass Luhn's check, with no letter or digit
    after them: a timestamp or an id starting 0, 1 or 7 to 9 is no card.

    Each run of digits is read once: Luhn's sum of any digits in it is then a
    difference of two running sums, so a start costs the same however many
    starts the run holds."""
    pos = 0
    while (run := _DIGIT_RUN.search(text, pos)) is not None:
        base, pos = run.start(), run.end()
        places = [base + i for i, c in enumerate(run.group()) if c in _DIGITS]
        # Whether no ASCII letter or digit follows each digit.
        free = [after - place == 2 for place, after in pairwise(places)]
        free.append(pos == len(text) or text[pos] not in _ALNUM)
        starts = [0] if base == 0 or text[base - 1] not in _ALNUM else []
        starts += [at + 1 for at in range(len(places) - 13) if free[at]]
        starts = [first for first in starts if text[places[first]] in _ISSUERS]
        if not starts:
            continue
        raw = run.group().encode()
        once, twice = raw.translate(_ONCE, b" -"), raw.translate(_TWICE, b" -")
        # Read from a last digit at an even place, the even places count once.
        even, odd = bytearray(once), bytearray(twice)
        even[1::2], odd[1::2] = twice[1::2], once[1::2]
        tens = (
            [total % 10 for total in accumulate(even, initial=0)],
            [total % 10 for total in accumulate(odd, initial=0)],
        )
        resume = 0
        for first in starts:
            if first < resume:
                continue
            for last in range(min(first + 18, len(places) - 1), first + 11, -1):
                sums = tens[last % 2]
                if free[last] and sums[last + 1] == sums[first]:
                    yield places[first], places[last] + 1, "card"
                    resume = last + 1
                    break


_SSN = re.compile(
    r"[0-8](?<![A-Za-z0-9][0-8])[0-9]{2}(?<!000)(?<!666)-(?!00)[0-9]{2}-(?!0000)"
    r"[0-9]{4}(?![A-Za-z0-9])"
)
# Each choice is the only one its characters allow, so backtracking finds no phone
# a scan would not: a country code takes up to three digits, as a scan does.
_PHONE = re.compile(
    r"[+(0-9](?<![A-Za-z0-9+][+(0-9])"
    r"(?:(?<=\+)(?P<country>[0-9]{3}|[0-9]{2}(?![0-9])|[0-9](?![0-9]))[ .\-]?"
    r"(?:\([0-9]{3}\)|[0-9]{3})|(?<=\()[0-9]{3}\)|(?<=[0-9])[0-9]{2})"
    r"[ .\-]?[0-9]{3}[ .\-]?[0-9]{4}(?![A-Za-z0-9])"
)


def _phones(text: str) -> Iterator[Match]:
    """A country code, a separator or parentheses: ten bare digits are no phone."""
    found = _PHONE.search(text)
    while found is not None:
        if found.group("country") is not None or any(
            c in " .-()" for c in found.group()
        ):
            yield found.start(), found.end(), "phone"
            found = _PHONE.search(text, found.end())
        else:
            found = _PHONE.search(text, found.start() + 1)


_IPV4 = re.compile(
    r"[0-9](?<![A-Za-z0-9.][0-9])[0-9]{0,2}(?:\.[0-9]{1,3}){3}(?![A-Za-z0-9.])"
)
_HEX_COLON = frozenset(string.hexdigits + ":")
_BEFORE_IPV6 = frozenset(string.ascii_letters + string.digits + ":")
_COLONS = re.compile(r":[0-9A-Fa-f]*:[0-9A-Fa-f:]*")
"""An IPv6 address's first two colons, which every one holds, to its run's end."""


def _ips(text: str) -> Iterator[Match]:
    """An IPv4 address, or an IPv6 one holding ``::`` or all eight groups; where
    both start at one place, the IPv4."""
    four, six = _IPV4.search(text), _ipv6_run(text, 0)
    while four is not None or six is not None:
        at = min(
            len(text) if four is None else four.start(),
            len(text) if six is None else six[0],
        )
        end = None
        if (
            four is not None
            and four.start() == at
            and all(int(part) <= 255 for part in four.group().split("."))
        ):
            end = four.end()
        if end is None and six is not None and six[0] == at and _ipv6(text, *six):
            end = six[1]
        if end is not None:
            yield at, end, "ip"
        pos = at + 1 if end is None else end
        if four is not None and four.start() < pos:
            four = _IPV4.search(text, pos)
        if six is not None and six[0] < pos:
            six = _ipv6_run(text, pos)


def _ipv6_run(text: str, pos: int) -> tuple[int, int] | None:
    """The next whole run of hex digits and colons from ``pos`` holding two
    colons, with no ASCII letter, digit or colon right before it."""
    while (colons := _COLONS.search(text, pos)) is not None:
        start = colons.start()
        while start > pos and text[start - 1] in _HEX_COLON:
            start -= 1
        if start == 0 or text[start - 1] not in _BEFORE_IPV6:
            return start, colons.end()
        pos = colons.end()
    return None


def _ipv6(text: str, start: int, end: int) -> bool:
    """Groups of 1 to 4 hex digits, at most one ``::``, and no letter after."""
    run = text[start:end]
    if end < len(text) and text[end] in _LETTERS:
        return False
    if run.count("::") > 1:
        return False
    if "::" in run:
        head, tail = run.split("::")
        groups = (head.split(":") if head else []) + (tail.split(":") if tail else [])
        if len(groups) > 7:
            return False
    else:
        groups = run.split(":")
        if len(groups) != 8:
            return False
    return all(1 <= len(group) <= 4 for group in groups)


# A Windows path's backslashes may be doubled, as a message's JSON text escapes them.
_HOME = re.compile(
    r"(?:/home/|/Users/|[A-Za-z]:(?:\\{1,2}|/)Users(?:\\{1,2}|/))[A-Za-z0-9._\-]+"
)
_HOME_MARK = re.compile(r"/home/|/Users/|:(?:\\{1,2}|/)Users")
"""Where a home folder's path shows itself: past a Windows drive's letter."""


def _homes(text: str) -> Iterator[Match]:
    pos = 0
    while (found := _HOME_MARK.search(text, pos)) is not None:
        at = found.start() - (text[found.start()] == ":")
        home = _HOME.match(text, at) if at >= pos else None
        if home is not None:
            yield at, home.end(), "home_path"
            pos = home.end()
        else:
            pos = found.start() + 1


_PII: tuple[Rule, ...] = (
    _emails,
    _ibans,
    _cards,
    _regex(_SSN, "ssn"),
    _phones,
    _ips,
    _homes,
)
