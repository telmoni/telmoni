/**
 * The scrubbers over content, secrets' and PII's: each rule in order, every
 * match replaced by `[secret: <kind>]` or `[pii: <kind>]`, as every SDK
 * replaces it (`sdk/fixtures/canary.json`). Each rule reads a text in time
 * linear in its length, whatever it holds: content can be anyone's.
 */

/** A match's start and end, and its kind where the rule finds more than one. */
type Found = readonly [start: number, end: number, kind?: string];
/** A rule's first match at or after `from` in the text it reads. */
type Find = (from: number) => Found | undefined;
/** A rule's kind, and how it reads a text, keeping what it learns there from one find to the next. */
type Rule = readonly [kind: string, reading: (text: string) => Find];

// Every class is ASCII, read a UTF-16 unit at a time: no unit of another character falls in one.
function digitAt(text: string, i: number): boolean {
  const c = text.charCodeAt(i);
  return c >= 0x30 && c <= 0x39;
}

function upperAt(text: string, i: number): boolean {
  const c = text.charCodeAt(i);
  return c >= 0x41 && c <= 0x5a;
}

function letterAt(text: string, i: number): boolean {
  const c = text.charCodeAt(i);
  return (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a);
}

function alnumAt(text: string, i: number): boolean {
  return digitAt(text, i) || letterAt(text, i);
}

function wordAt(text: string, i: number): boolean {
  return alnumAt(text, i) || text.charAt(i) === '_';
}

/** `[A-Za-z0-9_-]`, a key's and a JWT's characters. */
function keyAt(text: string, i: number): boolean {
  return wordAt(text, i) || text.charAt(i) === '-';
}

function hexAt(text: string, i: number): boolean {
  const c = text.charCodeAt(i);
  return digitAt(text, i) || (c >= 0x41 && c <= 0x46) || (c >= 0x61 && c <= 0x66);
}

function digitsAt(text: string, i: number, count: number): boolean {
  for (let n = 0; n < count; n++) if (!digitAt(text, i + n)) return false;
  return true;
}

/** No ASCII letter, digit or `_` either side. */
function bounded(text: string, start: number, end: number): boolean {
  return !wordAt(text, start - 1) && !wordAt(text, end);
}

/** A rule that keeps nothing from one find to the next. */
function each(find: (text: string, from: number) => Found | undefined): (text: string) => Find {
  return (text) => (from) => find(text, from);
}

/** The leftmost match of a `g` pattern; one failing the boundary, which only a fixed length asks, is skipped a character on. */
function matching(pattern: RegExp, boundary: boolean): (text: string) => Find {
  return (text) => (from) => {
    for (let at = from; at <= text.length; ) {
      pattern.lastIndex = at;
      const match = pattern.exec(text);
      if (match === null) return undefined;
      const end = match.index + match[0].length;
      if (!boundary || bounded(text, match.index, end)) return [match.index, end];
      at = match.index + 1;
    }
    return undefined;
  };
}

/**
 * A prefix, then a run of a class at least `least` long, taken whole, with the boundary each side. The boundary
 * before is asked first; and as every start inside a run ends where the run does, a run that fails fails them all,
 * so the search goes on past it.
 */
function keyed(
  prefix: RegExp,
  runOf: (prefix: string) => readonly [inRun: (text: string, i: number) => boolean, least: number],
): (text: string) => Find {
  return (text) => (from) => {
    for (prefix.lastIndex = from; ; ) {
      const found = prefix.exec(text);
      if (found === null) return undefined;
      const start = found.index;
      if (wordAt(text, start - 1)) {
        prefix.lastIndex = start + 1;
        continue;
      }
      const [inRun, least] = runOf(found[0]);
      const run = start + found[0].length;
      let end = run;
      while (inRun(text, end)) end++;
      if (end - run >= least && !wordAt(text, end)) return [start, end];
      prefix.lastIndex = end;
    }
  };
}

// ── Secrets ──────────────────────────────────────────────────────────────────

const BEGIN = /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----/g;
const END = /-----END [A-Z0-9 ]*PRIVATE KEY-----/g;

/** The first `"` from `i` no backslash escapes, an even run of them before it, none among them; -1 where there is none. */
function unescapedQuote(text: string, i: number): number {
  for (let at = text.indexOf('"', i); at >= 0; at = text.indexOf('"', at + 1)) {
    let slashes = 0;
    while (text.charAt(at - 1 - slashes) === '\\') slashes++;
    if (slashes % 2 === 0) return at;
  }
  return -1;
}

/**
 * From a BEGIN to its END, or to the first quote no backslash escapes, not taken, where that comes first, or to the
 * text's end: a key never runs past a JSON string's end. Each search is kept until a BEGIN lies past what it found.
 */
function privateKeys(text: string): Find {
  let end: RegExpExecArray | null | undefined;
  let quote: number | undefined;
  return (from) => {
    BEGIN.lastIndex = from;
    const begin = BEGIN.exec(text);
    if (begin === null) return undefined;
    const after = begin.index + begin[0].length;
    if (end === undefined || (end !== null && end.index < after)) {
      END.lastIndex = after;
      end = END.exec(text);
    }
    if (quote === undefined || (quote >= 0 && quote < after)) quote = unescapedQuote(text, after);
    const stop = quote < 0 ? text.length : quote;
    if (end !== null && end.index + end[0].length <= stop) return [begin.index, end.index + end[0].length];
    return [begin.index, stop];
  };
}

function schemeAt(text: string, i: number): boolean {
  const c = text.charAt(i);
  return alnumAt(text, i) || c === '+' || c === '.' || c === '-';
}

// What follows `://`: a user, `:`, a password, `@` and the rest. ASCII's six blanks, not `\s`, which in JavaScript
// takes Unicode's spaces too.
const CREDENTIALS = /[^ \t\n\v\f\r"'\\\/@:]+:[^ \t\n\v\f\r"'\\\/@]+@[^ \t\n\v\f\r"'\\]+/y;

/**
 * A scheme of 2 to 21 of `[A-Za-z0-9+.-]`, a letter first, `://` and its credentials. Found from each `://`, as a search
 * from every letter is slow: what follows it never depends on where the scheme starts, so its leftmost start is the match's.
 */
function findConnection(text: string, from: number): Found | undefined {
  for (let at = text.indexOf('://', from); at >= 0; at = text.indexOf('://', at + 1)) {
    let start = at;
    while (start > from && start > at - 21 && schemeAt(text, start - 1)) start--;
    while (start < at - 1 && !letterAt(text, start)) start++;
    if (start > at - 2) continue;
    CREDENTIALS.lastIndex = at + 3;
    const credentials = CREDENTIALS.exec(text);
    if (credentials !== null) return [start, at + 3 + credentials[0].length];
  }
  return undefined;
}

function keyRunEnd(text: string, i: number): number {
  while (keyAt(text, i)) i++;
  return i;
}

/** After a JWT's first segment: `.eyJ` and 8 or more, `.` and 8 or more; where it ends, or undefined. */
function jwtTail(text: string, at: number): number | undefined {
  if (text.charAt(at) !== '.' || !text.startsWith('eyJ', at + 1)) return undefined;
  const second = keyRunEnd(text, at + 4);
  if (second - (at + 4) < 8 || text.charAt(second) !== '.') return undefined;
  const third = keyRunEnd(text, second + 1);
  return third - (second + 1) >= 8 ? third : undefined;
}

/**
 * `eyJ` and 8 or more, then the tail, bounded. Each segment is a whole run of its class, and nothing after the last is
 * one, so the boundary after always holds; a first segment that fails fails every later start in its run.
 */
function findJwt(text: string, from: number): Found | undefined {
  for (let start = text.indexOf('eyJ', from); start >= 0; ) {
    if (wordAt(text, start - 1)) {
      start = text.indexOf('eyJ', start + 1);
      continue;
    }
    const first = keyRunEnd(text, start + 3);
    const end = first - (start + 3) >= 8 ? jwtTail(text, first) : undefined;
    if (end !== undefined) return [start, end];
    start = text.indexOf('eyJ', first);
  }
  return undefined;
}

const KEYWORDS = [
  'sharedaccesskey',
  'client_secret',
  'accountkey',
  'access_key',
  'access-key',
  'password',
  'api_key',
  'api-key',
  'apikey',
  'passwd',
  'secret',
  'token',
  'pwd',
];
// Longest first, in any ASCII case: without `u`, the `i` flag folds nothing past ASCII into it, as the Kelvin sign to k.
const KEYWORD = new RegExp(KEYWORDS.join('|'), 'gi');
const PASSWORDS: ReadonlySet<string> = new Set(['password', 'passwd', 'pwd']);
const SEPARATORS: ReadonlySet<string> = new Set(' \t"\'\\');
const VALUE_STOPS: ReadonlySet<string> = new Set(' \t\n\v\f\r"\'\\,;&{}()<>[]');

/** Past up to four quotes, blanks or backslashes. */
function separated(text: string, i: number): number {
  for (let n = 0; n < 4 && SEPARATORS.has(text.charAt(i)); n++) i++;
  return i;
}

/** Whether `text` holds `count` characters from `start` to `end`, an astral one counted once. */
function holds(text: string, start: number, end: number, count: number): boolean {
  let held = 0;
  for (let i = start; i < end && held < count; i += text.codePointAt(i)! > 0xffff ? 2 : 1) held++;
  return held >= count;
}

/**
 * The value after a keyword and its `:` or `=`: a password of 8 or more, a token of 16 or more with a letter and a
 * digit. Values in one run without a stop end together, so each run is read once, its last letter and digit kept.
 */
function keywords(text: string): Find {
  let runFrom = -1;
  let runEnd = -1;
  let lastLetter = -1;
  let lastDigit = -1;
  return (from) => {
    for (KEYWORD.lastIndex = from; ; ) {
      const match = KEYWORD.exec(text);
      if (match === null) return undefined;
      const i = match.index;
      KEYWORD.lastIndex = i + 1;
      if (wordAt(text, i - 1)) continue;
      const keyword = match[0].toLowerCase();
      let start = separated(text, i + keyword.length);
      if (text.charAt(start) !== ':' && text.charAt(start) !== '=') continue;
      start = separated(text, start + 1);
      if (start < runFrom || start > runEnd) {
        runFrom = start;
        lastLetter = lastDigit = -1;
        for (runEnd = start; runEnd < text.length && !VALUE_STOPS.has(text.charAt(runEnd)); runEnd++) {
          if (letterAt(text, runEnd)) lastLetter = runEnd;
          else if (digitAt(text, runEnd)) lastDigit = runEnd;
        }
      }
      const kind = PASSWORDS.has(keyword) ? 'password' : 'token';
      const held =
        kind === 'password'
          ? holds(text, start, runEnd, 8)
          : holds(text, start, runEnd, 16) && lastLetter >= start && lastDigit >= start;
      if (held) return [start, runEnd, kind];
    }
  };
}

const SECRETS: readonly Rule[] = [
  ['private_key', privateKeys],
  ['connection_string', each(findConnection)],
  ['jwt', each(findJwt)],
  ['anthropic_key', keyed(/sk-ant-/g, () => [keyAt, 20])],
  ['openai_key', keyed(/sk-/g, () => [keyAt, 20])],
  ['stripe_key', keyed(/(?:sk|rk)_(?:live|test)_|whsec_/g, () => [alnumAt, 16])],
  ['github_token', keyed(/gh[pousr]_|github_pat_/g, (prefix) => (prefix.length === 4 ? [alnumAt, 36] : [wordAt, 22]))],
  ['slack_token', keyed(/xox[abposr]-/g, () => [(text, i) => alnumAt(text, i) || text.charAt(i) === '-', 10])],
  ['aws_key', matching(/(?:AKIA|ASIA)[0-9A-Z]{16}/g, true)],
  ['gcp_key', matching(/AIza[0-9A-Za-z_-]{35}/g, true)],
  ['telmoni_key', keyed(/tm_/g, () => [keyAt, 20])],
  ['token', keywords],
];

// ── PII ──────────────────────────────────────────────────────────────────────

const LOCAL_MARKS: ReadonlySet<string> = new Set('._%+-');
const DOMAIN = /[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}/y;

/** Found from each `@`, its local part as far left as its class goes from where the search began. */
function findEmail(text: string, from: number): Found | undefined {
  for (let at = text.indexOf('@', from); at >= 0; at = text.indexOf('@', at + 1)) {
    let start = at;
    while (start > from && (alnumAt(text, start - 1) || LOCAL_MARKS.has(text.charAt(start - 1)))) start--;
    if (start === at) continue;
    DOMAIN.lastIndex = at + 1;
    const domain = DOMAIN.exec(text);
    if (domain !== null) return [start, at + 1 + domain[0].length];
  }
  return undefined;
}

function ibanCharsAt(text: string, i: number, count: number): boolean {
  for (let n = 0; n < count; n++) if (!upperAt(text, i + n) && !digitAt(text, i + n)) return false;
  return true;
}

/** Where the grouped form at `i` ends, groups as many as there are; undefined past 34 characters, never valid. */
function ibanGrouped(text: string, i: number): number | undefined {
  let end = i + 4;
  for (let held = 4; text.charAt(end) === ' ' && ibanCharsAt(text, end + 1, 4); held += 4) {
    if (held + 4 > 34) return undefined;
    end += 5;
  }
  if (end === i + 4) return undefined;
  let tail = end + 1;
  if (text.charAt(end) === ' ') while (tail - (end + 1) < 3 && ibanCharsAt(text, tail, 1)) tail++;
  return tail > end + 1 ? tail : end;
}

/** Where the compact form at `i` ends: 11 to 30 more, as many as there are. */
function ibanCompact(text: string, i: number): number | undefined {
  let end = i + 4;
  while (end - (i + 4) < 30 && ibanCharsAt(text, end, 1)) end++;
  return end - (i + 4) >= 11 ? end : undefined;
}

/** ISO 13616's check: the first four moved to the end, each letter as 10 to 35, the whole mod 97 is 1. */
function ibanValid(candidate: string): boolean {
  const compact = candidate.replaceAll(' ', '');
  if (compact.length < 15 || compact.length > 34) return false;
  let rest = 0;
  for (const c of compact.slice(4) + compact.slice(0, 4)) {
    const value = c >= 'A' ? c.charCodeAt(0) - 55 : c.charCodeAt(0) - 48;
    rest = (rest * (value > 9 ? 100 : 10) + value) % 97;
  }
  return rest === 1;
}

// The grouped form first; each as long as it goes, never shorter.
const IBAN_FORMS = [ibanGrouped, ibanCompact];

function findIban(text: string, from: number): Found | undefined {
  for (let i = from; i < text.length; i++) {
    if (!upperAt(text, i) || !upperAt(text, i + 1) || !digitAt(text, i + 2) || !digitAt(text, i + 3)) continue;
    for (const form of IBAN_FORMS) {
      const end = form(text, i);
      if (end !== undefined && bounded(text, i, end) && ibanValid(text.slice(i, end))) return [i, end];
    }
  }
  return undefined;
}

/**
 * 13 to 19 digits, one blank or hyphen allowed between two, the longest that passes Luhn's check. Which digits the
 * check doubles turns on the count's parity alone, so two running sums check every count in one pass.
 */
function findCard(text: string, from: number): Found | undefined {
  for (let i = from; i < text.length; i++) {
    // The first digit an issuer's, 2 to 6: a timestamp or an id that passes Luhn's check is no card.
    const first = text.charCodeAt(i);
    if (first < 0x32 || first > 0x36 || alnumAt(text, i - 1)) continue;
    // Luhn's sum doubling the digits at even indexes, an even count's; and at odd ones, an odd count's.
    let evens = 0;
    let odds = 0;
    let longest: number | undefined;
    for (let j = i, count = 1; ; count++) {
      const digit = text.charCodeAt(j) - 0x30;
      const twice = digit > 4 ? 2 * digit - 9 : 2 * digit;
      evens += count % 2 === 1 ? twice : digit;
      odds += count % 2 === 1 ? digit : twice;
      j += 1;
      if (count > 12 && (count % 2 === 0 ? evens : odds) % 10 === 0 && !alnumAt(text, j)) longest = j;
      if (count === 19) break;
      if (digitAt(text, j)) continue;
      if ((text.charAt(j) !== ' ' && text.charAt(j) !== '-') || !digitAt(text, j + 1)) break;
      j += 1;
    }
    if (longest !== undefined) return [i, longest];
  }
  return undefined;
}

const SSN = /[0-9]{3}-[0-9]{2}-[0-9]{4}/g;

/** `ddd-dd-dddd`, but for the numbers never issued. */
function findSsn(text: string, from: number): Found | undefined {
  for (let i = from; ; i++) {
    SSN.lastIndex = i;
    const match = SSN.exec(text);
    if (match === null) return undefined;
    i = match.index;
    if (alnumAt(text, i - 1) || alnumAt(text, i + 11)) continue;
    const area = text.slice(i, i + 3);
    if (area === '000' || area === '666' || area.startsWith('9')) continue;
    if (text.slice(i + 4, i + 6) === '00' || text.slice(i + 7, i + 11) === '0000') continue;
    return [i, i + 11];
  }
}

const PHONE_SEPARATORS: ReadonlySet<string> = new Set(' .-');
const PHONE_MARKS = /[ .\-()]/;

/** An optional `+` and country code, then 3, 3 and 4 digits, the area's maybe in brackets; a bare run of ten is none. */
function findPhone(text: string, from: number): Found | undefined {
  for (let i = from; i < text.length; i++) {
    const c = text.charAt(i);
    if ((c !== '+' && c !== '(' && !digitAt(text, i)) || alnumAt(text, i - 1) || text.charAt(i - 1) === '+') continue;
    let j = i;
    let country = false;
    if (c === '+') {
      let k = j + 1;
      while (k - (j + 1) < 3 && digitAt(text, k)) k++;
      if (k === j + 1) continue;
      j = k;
      country = true;
      if (PHONE_SEPARATORS.has(text.charAt(j))) j++;
    }
    if (text.charAt(j) === '(') {
      if (!digitsAt(text, j + 1, 3) || text.charAt(j + 4) !== ')') continue;
      j += 5;
    } else if (digitsAt(text, j, 3)) {
      j += 3;
    } else {
      continue;
    }
    if (PHONE_SEPARATORS.has(text.charAt(j))) j++;
    if (!digitsAt(text, j, 3)) continue;
    j += 3;
    if (PHONE_SEPARATORS.has(text.charAt(j))) j++;
    if (!digitsAt(text, j, 4)) continue;
    j += 4;
    if (alnumAt(text, j)) continue;
    if (country || PHONE_MARKS.test(text.slice(i, j))) return [i, j];
  }
  return undefined;
}

/** The end of the IPv4 address at `i`: four octets to 255, none longer than three digits. */
function ipv4At(text: string, i: number): number | undefined {
  let j = i;
  for (let part = 0; part < 4; part++) {
    let k = j;
    let octet = 0;
    for (; k - j < 3 && digitAt(text, k); k++) octet = octet * 10 + text.charCodeAt(k) - 0x30;
    if (k === j || octet > 255) return undefined;
    j = k;
    if (part < 3) {
      if (text.charAt(j) !== '.') return undefined;
      j++;
    }
  }
  return alnumAt(text, j) || text.charAt(j) === '.' ? undefined : j;
}

/** Groups of one to four hex digits: eight, or up to seven around one `::`. */
function ipv6Valid(candidate: string): boolean {
  const halves = candidate.split('::');
  if (halves.length > 2) return false;
  const groups = halves.length === 2 ? halves.flatMap((half) => (half ? half.split(':') : [])) : candidate.split(':');
  if (halves.length === 2 ? groups.length > 7 : groups.length !== 8) return false;
  return groups.every((group) => /^[0-9A-Fa-f]{1,4}$/.test(group));
}

/** IPv4, or IPv6 as a whole run of hex digits and colons, each run read from its start alone. */
function findIp(text: string, from: number): Found | undefined {
  for (let i = from; i < text.length; i++) {
    // Either form opens on a hex digit or a colon, a decimal one among them.
    if (!hexAt(text, i) && text.charAt(i) !== ':') continue;
    if (digitAt(text, i) && !alnumAt(text, i - 1) && text.charAt(i - 1) !== '.') {
      const end = ipv4At(text, i);
      if (end !== undefined) return [i, end];
    }
    if (hexAt(text, i - 1) || text.charAt(i - 1) === ':' || letterAt(text, i - 1)) continue;
    // Colons counted as the run is read: most runs, a word or a number, hold none.
    let end = i;
    let colons = 0;
    for (; hexAt(text, end) || text.charAt(end) === ':'; end++) if (text.charAt(end) === ':') colons++;
    if (colons < 2 || letterAt(text, end)) continue;
    const candidate = text.slice(i, end);
    if ((candidate.includes('::') || colons === 7) && ipv6Valid(candidate)) return [i, end];
  }
  return undefined;
}

// A Windows path's backslashes may be doubled, as a message's JSON text escapes them.
const HOME = /(?:\/home\/|\/Users\/|[A-Za-z]:(?:\\{1,2}|\/)Users(?:\\{1,2}|\/))[A-Za-z0-9._-]+/g;

const PII: readonly Rule[] = [
  ['email', each(findEmail)],
  ['iban', each(findIban)],
  ['card', each(findCard)],
  ['ssn', each(findSsn)],
  ['phone', each(findPhone)],
  ['ip', each(findIp)],
  ['home_path', matching(HOME, false)],
];

/** Every match of the rule, left to right and none overlapping, replaced by its marker and counted. */
function replaced(text: string, family: string, [kind, reading]: Rule, counts: Map<string, number>): string {
  const find = reading(text);
  let out = '';
  let at = 0;
  for (let found = find(at); found !== undefined; found = find(at)) {
    const [start, end, named = kind] = found;
    out += `${text.slice(at, start)}[${family}: ${named}]`;
    counts.set(`${family}:${named}`, (counts.get(`${family}:${named}`) ?? 0) + 1);
    at = end;
  }
  return out + text.slice(at);
}

/**
 * `text` with the rules switched on applied, secrets' first, each to the text
 * the rules before it left; and how many each replaced, by `<family>:<kind>`.
 */
export function scrub(text: string, secrets: boolean, pii: boolean): [string, Map<string, number>] {
  const counts = new Map<string, number>();
  if (secrets) for (const rule of SECRETS) text = replaced(text, 'secret', rule, counts);
  if (pii) for (const rule of PII) text = replaced(text, 'pii', rule, counts);
  return [text, counts];
}

/** Counts as `telmoni.scrubbed` carries them: a JSON object, its keys sorted, no spaces. */
export function scrubbedOf(counts: ReadonlyMap<string, number>): string {
  return JSON.stringify(Object.fromEntries([...counts].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))));
}

/** The exporter's way to the scrubbers, which a test replaces to make one fail; the package's entry exports neither. */
export const scrubber = { scrub };
