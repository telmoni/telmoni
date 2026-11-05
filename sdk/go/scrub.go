package telmoni

// The scrubbers: secrets, and personal data where asked, replaced in the
// content that leaves, rule by rule, as sdk/fixtures/canary.json holds every
// SDK to. Each rule reads a text in time linear in its length, whatever it
// holds: searching on a byte past a failed match, as the reference does,
// would read a long run again from every start inside it.

import (
	"regexp"
	"strings"
	"unicode/utf8"
)

// A matcher is a rule readied for one text: its first match from pos, start
// -1 for none, with the kind where the rule names more than one. pos grows
// from call to call, so a matcher may keep what it has read.
type matcher func(pos int) (start, end int, kind string)

// A finder is a matcher that keeps nothing, handed the text each time.
type finder func(text string, pos int) (start, end int, kind string)

type rule struct {
	kind string
	in   func(text string) matcher
}

// The rules in the order they run, each over the text the ones before it left.
var (
	secretRules = []rule{
		{"private_key", privateKeys},
		{"connection_string", each(findConnection)},
		{"jwt", each(findJWT)},
		{"anthropic_key", each(keyMatch("sk-ant-", nil, isKeyChar, 20))},
		// Its optional proj-, svcacct- or admin- is of its run's own characters.
		{"openai_key", each(keyMatch("sk-", nil, isKeyChar, 20))},
		{"stripe_key", earliest(
			keyMatch("sk_", []string{"live_", "test_"}, isAlnum, 16),
			keyMatch("rk_", []string{"live_", "test_"}, isAlnum, 16),
			keyMatch("whsec_", nil, isAlnum, 16),
		)},
		{"github_token", earliest(
			keyMatch("gh", []string{"p_", "o_", "u_", "s_", "r_"}, isAlnum, 36),
			keyMatch("github_pat_", nil, isWord, 22),
		)},
		{"slack_token", each(keyMatch("xox", []string{"a-", "b-", "p-", "o-", "s-", "r-"}, isSlackChar, 10))},
		{"aws_key", each(fixedMatch(`(?:AKIA|ASIA)[0-9A-Z]{16}`))},
		{"gcp_key", each(fixedMatch(`AIza[0-9A-Za-z_\-]{35}`))},
		{"telmoni_key", each(keyMatch("tm_", nil, isKeyChar, 20))},
		{"token", keywordValues},
	}
	piiRules = []rule{
		{"email", each(findEmail)},
		{"iban", each(findIBAN)},
		{"card", each(findCard)},
		{"ssn", each(findSSN)},
		{"phone", each(findPhone)},
		{"ip", each(findIP)},
		{"home_path", each(findHome)},
	}
)

// scrub is text with each secret replaced by a marker naming its kind,
// [secret: aws_key] say, with pii each piece of personal data too, and what
// was replaced, counted by family:kind.
func scrub(text string, secrets, pii bool) (string, map[string]int) {
	counts := map[string]int{}
	if secrets {
		for _, r := range secretRules {
			text = r.apply(text, "secret", counts)
		}
	}
	if pii {
		for _, r := range piiRules {
			text = r.apply(text, "pii", counts)
		}
	}
	return text, counts
}

// scrubber is scrub, which a test makes panic.
var scrubber = scrub

// apply replaces each of the rule's matches in text, left to right, none
// overlapping.
func (r rule) apply(text, family string, counts map[string]int) string {
	next := r.in(text)
	var out strings.Builder
	last := 0
	for start, end, kind := next(0); start >= 0; start, end, kind = next(last) {
		if kind == "" {
			kind = r.kind
		}
		out.WriteString(text[last:start])
		out.WriteString("[" + family + ": " + kind + "]")
		counts[family+":"+kind]++
		last = end
	}
	if last == 0 {
		return text
	}
	out.WriteString(text[last:])
	return out.String()
}

// each readies a rule that keeps nothing between its matches.
func each(find finder) func(string) matcher {
	return func(text string) matcher {
		return func(pos int) (int, int, string) { return find(text, pos) }
	}
}

// earliest finds the first match any of finds gives, none of which can start
// where another does; each one's answer is kept until the search passes it.
func earliest(finds ...finder) func(string) matcher {
	return func(text string) matcher {
		starts, ends := make([]int, len(finds)), make([]int, len(finds))
		for k, find := range finds {
			starts[k], ends[k], _ = find(text, 0)
		}
		return func(pos int) (int, int, string) {
			first := -1
			for k, find := range finds {
				if starts[k] >= 0 && starts[k] < pos {
					starts[k], ends[k], _ = find(text, pos)
				}
				if starts[k] >= 0 && (first < 0 || starts[k] < starts[first]) {
					first = k
				}
			}
			if first < 0 {
				return -1, -1, ""
			}
			return starts[first], ends[first], ""
		}
	}
}

// keyMatch finds a key: anchor, then one of then where given, then a run of at
// least least of class, with no ASCII letter, digit or _ beside the whole. One
// that fails fails from every later start up to its run's end, each having a
// word character before it or ending its run at the same place, so the search
// goes on from there.
func keyMatch(anchor string, then []string, class func(byte) bool, least int) finder {
	return func(text string, pos int) (int, int, string) {
		for from := pos; from < len(text); {
			n := strings.Index(text[from:], anchor)
			if n < 0 {
				break
			}
			start := from + n
			from = start + 1
			if start > 0 && isWord(text[start-1]) {
				continue
			}
			run := afterOneOf(text, start+len(anchor), then)
			if run < 0 {
				continue
			}
			end := runEnd(text, run, class)
			if end-run >= least && (end == len(text) || !isWord(text[end])) {
				return start, end, ""
			}
			from = max(from, end)
		}
		return -1, -1, ""
	}
}

// afterOneOf is j past whichever of then text holds at j, j itself for none
// given, or -1.
func afterOneOf(text string, j int, then []string) int {
	if len(then) == 0 {
		return j
	}
	for _, t := range then {
		if strings.HasPrefix(text[j:], t) {
			return j + len(t)
		}
	}
	return -1
}

// runEnd is where the run of class from j ends.
func runEnd(text string, j int, class func(byte) bool) int {
	for j < len(text) && class(text[j]) {
		j++
	}
	return j
}

// fixedMatch finds pattern's leftmost match with no ASCII letter, digit or _
// beside it, searching on a byte past one that has: the pattern's length is
// fixed, so a match passed over costs no more than it.
func fixedMatch(pattern string) finder {
	re := regexp.MustCompile(pattern)
	return func(text string, pos int) (int, int, string) {
		for pos < len(text) {
			m := re.FindStringIndex(text[pos:])
			if m == nil {
				break
			}
			start, end := pos+m[0], pos+m[1]
			if bounded(text, start, end) {
				return start, end, ""
			}
			pos = start + 1
		}
		return -1, -1, ""
	}
}

// bounded is whether text[start:end] has no ASCII letter, digit or _ beside it.
func bounded(text string, start, end int) bool {
	return (start == 0 || !isWord(text[start-1])) && (end == len(text) || !isWord(text[end]))
}

var (
	pemBegin = regexp.MustCompile(`-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----`)
	pemEnd   = regexp.MustCompile(`-----END [A-Z0-9 ]*PRIVATE KEY-----`)
)

// privateKeys finds each key in text from its BEGIN line to its END line, a
// quote no backslash escapes or the end of the text, whichever comes first: a
// key never runs past a JSON string's end, so a message's JSON stays whole.
// The END and the quote last found are kept, as a key before either finds it
// first too, so the rest is never read twice.
func privateKeys(text string) matcher {
	endRead, endStart, endEnd := false, -1, -1
	quoteRead, quote := false, -1
	return func(pos int) (int, int, string) {
		begin := pemBegin.FindStringIndex(text[pos:])
		if begin == nil {
			return -1, -1, ""
		}
		start, from := pos+begin[0], pos+begin[1]
		if !endRead || (endStart >= 0 && from > endStart) {
			endRead, endStart, endEnd = true, -1, -1
			if end := pemEnd.FindStringIndex(text[from:]); end != nil {
				endStart, endEnd = from+end[0], from+end[1]
			}
		}
		if !quoteRead || (quote >= 0 && from > quote) {
			quoteRead, quote = true, unescapedQuote(text, from)
		}
		switch {
		case endStart >= 0 && (quote < 0 || endEnd <= quote):
			return start, endEnd, ""
		case quote >= 0:
			return start, quote, ""
		}
		return start, len(text), ""
	}
}

// unescapedQuote is the first quote from j with an even run of backslashes,
// none among them, before it, or -1.
func unescapedQuote(text string, j int) int {
	for {
		n := strings.IndexByte(text[j:], '"')
		if n < 0 {
			return -1
		}
		quote := j + n
		slashes := 0
		for quote-slashes > 0 && text[quote-slashes-1] == '\\' {
			slashes++
		}
		if slashes%2 == 0 {
			return quote
		}
		j = quote + 1
	}
}

// findConnection finds scheme://user:password@rest from each :// in turn,
// whose scheme, a letter then 1 to 20 of [A-Za-z0-9+.-], starts as far left
// as it can.
func findConnection(text string, pos int) (int, int, string) {
	for from := pos; from < len(text); {
		n := strings.Index(text[from:], "://")
		if n < 0 {
			break
		}
		colon := from + n
		from = colon + 1
		start := colon
		for start > max(pos, colon-21) && isSchemeChar(text[start-1]) {
			start--
		}
		for start < colon-1 && !isLetter(text[start]) {
			start++
		}
		if start >= colon-1 {
			continue
		}
		j := partEnd(text, colon+3, "/@:")
		if j == colon+3 || j == len(text) || text[j] != ':' {
			continue
		}
		k := partEnd(text, j+1, "/@")
		if k == j+1 || k == len(text) || text[k] != '@' {
			continue
		}
		if end := partEnd(text, k+1, ""); end > k+1 {
			return start, end, ""
		}
	}
	return -1, -1, ""
}

// partEnd is where a part of a connection string from j ends: at ASCII
// whitespace, a quote, a backslash or one of stops.
func partEnd(text string, j int, stops string) int {
	for j < len(text) && strings.IndexByte(" \t\n\r\f\v\"'\\", text[j]) < 0 && strings.IndexByte(stops, text[j]) < 0 {
		j++
	}
	return j
}

// findJWT finds eyJ and 8 or more of [A-Za-z0-9_-], a dot, the same again, a
// dot and 8 or more, with no word character before it. One that fails fails
// from every later start in its first run, which ends where its own does.
func findJWT(text string, pos int) (int, int, string) {
	for from := pos; from < len(text); {
		n := strings.Index(text[from:], "eyJ")
		if n < 0 {
			break
		}
		start := from + n
		from = start + 1
		if start > 0 && isWord(text[start-1]) {
			continue
		}
		first := runEnd(text, start+3, isKeyChar)
		if first-start-3 >= 8 && byteAt(text, first) == '.' && strings.HasPrefix(text[first+1:], "eyJ") {
			second := runEnd(text, first+4, isKeyChar)
			if second-first-4 >= 8 && byteAt(text, second) == '.' {
				if end := runEnd(text, second+1, isKeyChar); end-second-1 >= 8 {
					return start, end, ""
				}
			}
		}
		from = max(from, first)
	}
	return -1, -1, ""
}

// keywords name a secret beside them, longest first, each tried in turn.
var keywords = []string{
	"sharedaccesskey", "client_secret", "accountkey", "access_key", "access-key", "password",
	"api_key", "api-key", "apikey", "passwd", "secret", "token", "pwd",
}

// keywordStarts are the bytes a keyword starts with, in either case.
var keywordStarts = func() (starts [256]bool) {
	for _, word := range keywords {
		starts[word[0]], starts[word[0]-'a'+'A'] = true, true
	}
	return starts
}()

// valueStops end a keyword's value.
const valueStops = " \t\n\r\f\v\"'\\,;&{}()<>[]"

// keywordValues finds the values keywords name in text (password=…,
// "api_key": "…"): a password's from 8 characters, any other's from 16 holding
// a letter and a digit, so a word or a count beside one is left alone. A value
// starting inside the one last read ends where it did, so none is read twice.
func keywordValues(text string) matcher {
	valueFrom, valueTo, lastLetter, lastDigit := -1, -1, -1, -1
	return func(pos int) (int, int, string) {
		for i := pos; i < len(text); i++ {
			if i > 0 && isWord(text[i-1]) {
				continue
			}
			word := keywordAt(text, i)
			if word == "" {
				continue
			}
			j := skipSeparators(text, i+len(word))
			if j == len(text) || (text[j] != ':' && text[j] != '=') {
				continue
			}
			start := skipSeparators(text, j+1)
			if start < valueFrom || start > valueTo {
				valueFrom, valueTo, lastLetter, lastDigit = start, start, -1, -1
				for ; valueTo < len(text) && strings.IndexByte(valueStops, text[valueTo]) < 0; valueTo++ {
					if isLetter(text[valueTo]) {
						lastLetter = valueTo
					} else if isDigit(text[valueTo]) {
						lastDigit = valueTo
					}
				}
			}
			value := text[start:valueTo]
			if word == "password" || word == "passwd" || word == "pwd" {
				if atLeast(value, 8) {
					return start, valueTo, "password"
				}
			} else if atLeast(value, 16) && lastLetter >= start && lastDigit >= start {
				return start, valueTo, "token"
			}
		}
		return -1, -1, ""
	}
}

// keywordAt is the keyword text holds at i, in any case of ASCII, or "".
func keywordAt(text string, i int) string {
	if !keywordStarts[text[i]] {
		return ""
	}
	for _, word := range keywords {
		if len(text)-i >= len(word) && asciiFold(text[i:i+len(word)], word) {
			return word
		}
	}
	return ""
}

// asciiFold is whether s is lower, a lowercase ASCII word, with any of its
// letters in capitals.
func asciiFold(s, lower string) bool {
	for k := 0; k < len(s); k++ {
		c := s[k]
		if 'A' <= c && c <= 'Z' {
			c += 'a' - 'A'
		}
		if c != lower[k] {
			return false
		}
	}
	return true
}

// skipSeparators is j past up to four spaces, tabs, quotes or backslashes,
// which a keyword's JSON or its escaping puts about its colon.
func skipSeparators(text string, j int) int {
	for n := 0; n < 4 && j < len(text) && strings.IndexByte(" \t\"'\\", text[j]) >= 0; n++ {
		j++
	}
	return j
}

// atLeast is whether s holds n characters or more: past 4n bytes it must, so
// only a short one is counted.
func atLeast(s string, n int) bool {
	return len(s) >= 4*n || utf8.RuneCountInString(s) >= n
}

var emailDomain = regexp.MustCompile(`^[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}`)

// findEmail finds local@domain.tld from each @ in turn, the local part
// reaching as far left as its characters run.
func findEmail(text string, pos int) (int, int, string) {
	for from := pos; from < len(text); {
		n := strings.IndexByte(text[from:], '@')
		if n < 0 {
			break
		}
		at := from + n
		from = at + 1
		start := at
		for start > pos && (isAlnum(text[start-1]) || strings.IndexByte("._%+-", text[start-1]) >= 0) {
			start--
		}
		if start == at {
			continue
		}
		// Only the domain's characters, which the pattern cannot pass.
		run := at + 1
		for run < len(text) && (isAlnum(text[run]) || text[run] == '-' || text[run] == '.') {
			run++
		}
		if m := emailDomain.FindStringIndex(text[at+1 : run]); m != nil {
			return start, at + 1 + m[1], ""
		}
	}
	return -1, -1, ""
}

// findIBAN finds two capitals and two digits, then groups of four after a
// space or 11 to 30 run together, that pass the IBAN's mod-97 check.
func findIBAN(text string, pos int) (int, int, string) {
	for i := pos; i+4 <= len(text); i++ {
		if !isUpper(text[i]) || !isUpper(text[i+1]) || !isDigit(text[i+2]) || !isDigit(text[i+3]) {
			continue
		}
		for _, end := range [2]int{ibanGroupsEnd(text, i+4), ibanRunEnd(text, i+4)} {
			if end > 0 && bounded(text, i, end) && ibanValid(text[i:end]) {
				return i, end, ""
			}
		}
	}
	return -1, -1, ""
}

// ibanGroupsEnd is where " XXXX" once or more, then maybe " X" to " XXX", end
// from j, or -1: past seven groups none is short enough to be an IBAN, so the
// rest is never read.
func ibanGroupsEnd(text string, j int) int {
	groups := 0
	for byteAt(text, j) == ' ' && ibanRun(text, j+1, 4) == 4 {
		j += 5
		groups++
		if groups > 7 {
			return -1
		}
	}
	if groups == 0 {
		return -1
	}
	if n := ibanRun(text, j+1, 3); byteAt(text, j) == ' ' && n > 0 {
		j += 1 + n
	}
	return j
}

// ibanRunEnd is where 11 to 30 capitals and digits from j end, or -1.
func ibanRunEnd(text string, j int) int {
	if n := ibanRun(text, j, 30); n >= 11 {
		return j + n
	}
	return -1
}

// ibanRun is how many capitals and digits run from j, up to most.
func ibanRun(text string, j, most int) int {
	n := 0
	for n < most && j+n < len(text) && (isUpper(text[j+n]) || isDigit(text[j+n])) {
		n++
	}
	return n
}

// ibanValid is the IBAN's check: 15 to 34 characters but its spaces, the
// first four moved to the end and each letter read as 10 to 35, mod 97 is 1.
func ibanValid(candidate string) bool {
	s := strings.ReplaceAll(candidate, " ", "")
	if len(s) < 15 || len(s) > 34 {
		return false
	}
	rest := 0
	for _, c := range []byte(s[4:] + s[:4]) {
		if isDigit(c) {
			rest = (rest*10 + int(c-'0')) % 97
		} else {
			rest = (rest*100 + int(c-'A') + 10) % 97
		}
	}
	return rest == 1
}

// findCard finds 13 to 19 digits, one space or hyphen allowed between two,
// that pass Luhn's check: from where they start, the most that do. The first
// is an issuer's, 2 to 6, so a timestamp or an id passing the check is none.
func findCard(text string, pos int) (int, int, string) {
	for i := pos; i < len(text); i++ {
		if c := text[i]; c < '2' || c > '6' || (i > 0 && isAlnum(text[i-1])) {
			continue
		}
		// The digits, and where in text each ends.
		var number [19]byte
		var ends [19]int
		number[0], ends[0] = text[i], i+1
		n, j := 1, i+1
		for n < 19 {
			if j < len(text) && isDigit(text[j]) {
				number[n] = text[j]
				j++
			} else if j+1 < len(text) && (text[j] == ' ' || text[j] == '-') && isDigit(text[j+1]) {
				number[n] = text[j+1]
				j += 2
			} else {
				break
			}
			ends[n] = j
			n++
		}
		for k := n; k >= 13; k-- {
			end := ends[k-1]
			if (end == len(text) || !isAlnum(text[end])) && luhn(number[:k]) {
				return i, end, ""
			}
		}
	}
	return -1, -1, ""
}

func luhn(number []byte) bool {
	sum := 0
	for k := range number {
		d := int(number[len(number)-1-k] - '0')
		if k%2 == 1 {
			d *= 2
			if d > 9 {
				d -= 9
			}
		}
		sum += d
	}
	return sum%10 == 0
}

// findSSN finds NNN-NN-NNNN apart from letters and digits, in no range never
// issued: 000, 666 or 9xx first, 00 between, 0000 last.
func findSSN(text string, pos int) (int, int, string) {
	for i := pos; i+11 <= len(text); i++ {
		c := text[i : i+11]
		if c[3] != '-' || c[6] != '-' || !only(c[:3], digits) || !only(c[4:6], digits) || !only(c[7:], digits) {
			continue
		}
		if (i > 0 && isAlnum(text[i-1])) || (i+11 < len(text) && isAlnum(text[i+11])) {
			continue
		}
		if c[:3] == "000" || c[:3] == "666" || c[0] == '9' || c[4:6] == "00" || c[7:] == "0000" {
			continue
		}
		return i, i + 11, ""
	}
	return -1, -1, ""
}

// findPhone finds an optional + and 1 to 3 digits, then three digits or three
// in brackets, three and four, each after an optional space, dot or hyphen;
// with neither a + nor one of those, ten digits are no phone number.
func findPhone(text string, pos int) (int, int, string) {
	for i := pos; i < len(text); i++ {
		if c := text[i]; (c != '+' && c != '(' && !isDigit(c)) || (i > 0 && (isAlnum(text[i-1]) || text[i-1] == '+')) {
			continue
		}
		j, country := i, false
		if text[j] == '+' {
			k := j + 1
			for k < len(text) && k-(j+1) < 3 && isDigit(text[k]) {
				k++
			}
			if k == j+1 {
				continue
			}
			j, country = phoneSeparator(text, k), true
		}
		switch {
		case byteAt(text, j) == '(':
			if !digitsAt(text, j+1, 3) || byteAt(text, j+4) != ')' {
				continue
			}
			j += 5
		case digitsAt(text, j, 3):
			j += 3
		default:
			continue
		}
		j = phoneSeparator(text, j)
		if !digitsAt(text, j, 3) {
			continue
		}
		j = phoneSeparator(text, j+3)
		if !digitsAt(text, j, 4) {
			continue
		}
		j += 4
		if isAlnum(byteAt(text, j)) {
			continue
		}
		if country || strings.ContainsAny(text[i:j], " .-()") {
			return i, j, ""
		}
	}
	return -1, -1, ""
}

func phoneSeparator(text string, j int) int {
	if c := byteAt(text, j); c == ' ' || c == '.' || c == '-' {
		return j + 1
	}
	return j
}

func digitsAt(text string, j, n int) bool {
	return j+n <= len(text) && only(text[j:j+n], digits)
}

// findIP finds an IPv4 address of four parts from 0 to 255, or an IPv6 one
// with two or more colons, all eight groups or a ::, apart from what would
// carry it on.
func findIP(text string, pos int) (int, int, string) {
	for i := pos; i < len(text); i++ {
		c, before := text[i], byteAt(text, i-1)
		if isDigit(c) && !isAlnum(before) && before != '.' {
			if end := ipv4End(text, i); end > 0 {
				return i, end, ""
			}
		}
		if (isHexDigitAnyCase(c) || c == ':') && !isHexDigitAnyCase(before) && before != ':' && !isLetter(before) {
			j := i
			for j < len(text) && (isHexDigitAnyCase(text[j]) || text[j] == ':') {
				j++
			}
			candidate := text[i:j]
			colons := strings.Count(candidate, ":")
			if colons >= 2 && (colons == 7 || strings.Contains(candidate, "::")) && !isLetter(byteAt(text, j)) && ipv6Valid(candidate) {
				return i, j, ""
			}
		}
	}
	return -1, -1, ""
}

// ipv4End is where an IPv4 address from i ends, or -1.
func ipv4End(text string, i int) int {
	j := i
	for part := range 4 {
		value, k := 0, j
		for k < len(text) && k-j < 3 && isDigit(text[k]) {
			value = value*10 + int(text[k]-'0')
			k++
		}
		if k == j || value > 255 {
			return -1
		}
		j = k
		if part < 3 {
			if byteAt(text, j) != '.' {
				return -1
			}
			j++
		}
	}
	if after := byteAt(text, j); isAlnum(after) || after == '.' {
		return -1
	}
	return j
}

// ipv6Valid is whether s, hex digits and colons, is eight groups of one to four
// digits, or at most seven about one ::.
func ipv6Valid(s string) bool {
	var groups []string
	switch strings.Count(s, "::") {
	case 0:
		if groups = strings.Split(s, ":"); len(groups) != 8 {
			return false
		}
	case 1:
		head, tail, _ := strings.Cut(s, "::")
		if head != "" {
			groups = strings.Split(head, ":")
		}
		if tail != "" {
			groups = append(groups, strings.Split(tail, ":")...)
		}
		if len(groups) > 7 {
			return false
		}
	default:
		return false
	}
	for _, group := range groups {
		if len(group) < 1 || len(group) > 4 {
			return false
		}
	}
	return true
}

// findHome finds /home/ or /Users/ and the user after it, or a drive's
// \Users\, a backslash maybe doubled as a message's JSON writes it, or a slash.
func findHome(text string, pos int) (int, int, string) {
	for from := pos; from < len(text); {
		n := strings.IndexAny(text[from:], "/:")
		if n < 0 {
			break
		}
		k := from + n
		from = k + 1
		start, j := k, -1
		switch {
		case strings.HasPrefix(text[k:], "/home/"):
			j = k + len("/home/")
		case strings.HasPrefix(text[k:], "/Users/"):
			j = k + len("/Users/")
		case text[k] == ':' && k > pos && isLetter(text[k-1]):
			start = k - 1
			if j = homeSeparatorEnd(text, k+1); j > 0 && strings.HasPrefix(text[j:], "Users") {
				j = homeSeparatorEnd(text, j+len("Users"))
			} else {
				j = -1
			}
		}
		if j < 0 {
			continue
		}
		end := j
		for end < len(text) && (isAlnum(text[end]) || strings.IndexByte("._-", text[end]) >= 0) {
			end++
		}
		if end > j {
			return start, end, ""
		}
	}
	return -1, -1, ""
}

// homeSeparatorEnd is j past one or two backslashes or a slash, or -1.
func homeSeparatorEnd(text string, j int) int {
	switch {
	case strings.HasPrefix(text[j:], `\\`):
		return j + 2
	case strings.HasPrefix(text[j:], `\`), strings.HasPrefix(text[j:], "/"):
		return j + 1
	}
	return -1
}

// byteAt is text[i], or 0 outside it.
func byteAt(text string, i int) byte {
	if i < 0 || i >= len(text) {
		return 0
	}
	return text[i]
}

func isDigit(c byte) bool      { return '0' <= c && c <= '9' }
func isUpper(c byte) bool      { return 'A' <= c && c <= 'Z' }
func isLetter(c byte) bool     { return isUpper(c) || ('a' <= c && c <= 'z') }
func isAlnum(c byte) bool      { return isDigit(c) || isLetter(c) }
func isWord(c byte) bool       { return isAlnum(c) || c == '_' }
func isKeyChar(c byte) bool    { return isWord(c) || c == '-' }
func isSlackChar(c byte) bool  { return isAlnum(c) || c == '-' }
func isSchemeChar(c byte) bool { return isAlnum(c) || c == '+' || c == '.' || c == '-' }
