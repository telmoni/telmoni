package telmoni

// A run across services: the W3C traceparent of the span current inside a
// run, and the run's id and agent as the W3C baggage members telmoni.run_id
// and telmoni.agent, written into an outgoing request's headers and read
// back from an incoming one's. The SDK's own helpers, never a propagator set
// globally: an application's propagators are its own.

import (
	"context"
	"net/http"
	"strconv"
	"strings"
	"unicode/utf8"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/trace"
)

// Inject writes into header what the next service continues the run from:
// the W3C traceparent of the span ctx carries, inside a run, and the run's
// id and agent as the W3C baggage members telmoni.run_id and telmoni.agent,
// beside whatever members the header holds. Outside a run, or for a run
// sampled out, it writes nothing.
func (c *Client) Inject(ctx context.Context, header http.Header) {
	run := RunFromContext(ctx)
	if run == nil || run.span == nil || run.sampledOut || header == nil {
		return
	}
	span := trace.SpanContextFromContext(ctx)
	if !span.IsValid() {
		span = run.span.SpanContext()
	}
	if !span.IsValid() {
		return
	}
	writeHeaders(header, span, run.id, run.agent)
}

// Extract returns ctx carrying the run header names, continued here: a step
// or a call opened with it takes the caller's span as its parent and carries
// the caller's run, so the trace shows the work where it was asked for, and
// RunFromContext finds the run. Nothing ends a continued run here; the
// caller's own span does. For headers that name no run, ctx is returned as
// given.
func (c *Client) Extract(ctx context.Context, header http.Header) context.Context {
	if ctx == nil {
		ctx = context.Background()
	}
	if !c.built() {
		return ctx
	}
	read, ok := readHeaders(header)
	if !ok {
		return ctx
	}
	ctx = trace.ContextWithRemoteSpanContext(ctx, read.spanContext)
	agent := c.cut(read.agent, agentMax, "run's agent")
	// The caller's span, non-recording here: steps and calls nest under it,
	// and nothing of it is sent or ended from this side.
	run := &Run{
		handle: handle{span: trace.SpanFromContext(ctx), client: c},
		id:     read.runID,
		agent:  agent,
		inherited: []attribute.KeyValue{
			attribute.String(attrRunID, read.runID),
			attribute.String(attrAgent, agent),
		},
	}
	return contextWithRun(ctx, run)
}

// continued is what headers name: the span to continue under, and the run.
type continued struct {
	spanContext trace.SpanContext
	runID       string
	agent       string
}

// writeHeaders is span and the run as the next service reads them, beside
// the members header already holds.
func writeHeaders(header http.Header, span trace.SpanContext, runID, agent string) {
	sampled := "00"
	if span.IsSampled() {
		sampled = "01"
	}
	header.Set(headerTraceparent, "00-"+span.TraceID().String()+"-"+span.SpanID().String()+"-"+sampled)
	var kept []string
	for _, member := range strings.Split(strings.Join(header.Values(headerBaggage), ","), ",") {
		member = strings.TrimSpace(member)
		if member == "" || isOurs(member) {
			continue
		}
		kept = append(kept, member)
	}
	kept = append(kept, attrRunID+"="+runID, attrAgent+"="+encodeBaggageValue(agent))
	header.Set(headerBaggage, strings.Join(kept, ","))
}

// readHeaders is the run header names, or false for headers that name none.
func readHeaders(header http.Header) (continued, bool) {
	traceparent := header.Get(headerTraceparent)
	if traceparent == "" {
		return continued{}, false
	}
	span, ok := parseTraceparent(traceparent)
	if !ok {
		return continued{}, false
	}
	members := baggageMembers(strings.Join(header.Values(headerBaggage), ","))
	runID, agent := members[attrRunID], members[attrAgent]
	if _, ok := parseUUID(runID); !ok || agent == "" {
		return continued{}, false
	}
	return continued{spanContext: span, runID: strings.ToLower(runID), agent: agent}, true
}

// parseTraceparent reads a traceparent as the W3C spells it: a version that
// is not ff, a trace id and a span id that are not zeros, and the flags; a
// version other than 00 may be followed by more, which is ignored.
func parseTraceparent(header string) (trace.SpanContext, bool) {
	parts := strings.Split(strings.TrimSpace(header), "-")
	if len(parts) < 4 {
		return trace.SpanContext{}, false
	}
	version, traceID, spanID, flags := parts[0], parts[1], parts[2], parts[3]
	if !isHex(version, 2) || version == "ff" {
		return trace.SpanContext{}, false
	}
	if version == "00" && len(parts) != 4 {
		return trace.SpanContext{}, false
	}
	tid, err := trace.TraceIDFromHex(traceID)
	if err != nil {
		return trace.SpanContext{}, false
	}
	sid, err := trace.SpanIDFromHex(spanID)
	if err != nil {
		return trace.SpanContext{}, false
	}
	if !isHex(flags, 2) {
		return trace.SpanContext{}, false
	}
	f, _ := strconv.ParseUint(flags, 16, 8)
	return trace.NewSpanContext(trace.SpanContextConfig{
		TraceID:    tid,
		SpanID:     sid,
		TraceFlags: trace.TraceFlags(f) & trace.FlagsSampled,
		Remote:     true,
	}), true
}

// baggageMembers is the baggage's members by key, each value
// percent-decoded, its properties left off.
func baggageMembers(header string) map[string]string {
	members := map[string]string{}
	for _, member := range strings.Split(header, ",") {
		key, value, found := strings.Cut(member, "=")
		if !found {
			continue
		}
		key = strings.TrimSpace(key)
		value, _, _ = strings.Cut(value, ";")
		if key != "" {
			members[key] = decodeBaggageValue(strings.TrimSpace(value))
		}
	}
	return members
}

func isOurs(member string) bool {
	key, _, _ := strings.Cut(member, "=")
	key = strings.TrimSpace(key)
	return key == attrRunID || key == attrAgent
}

// isHex is whether s is n lowercase hex digits.
func isHex(s string, n int) bool {
	if len(s) != n {
		return false
	}
	for i := 0; i < len(s); i++ {
		if !isHexDigit(s[i]) {
			return false
		}
	}
	return true
}

func isHexDigit(b byte) bool {
	return ('0' <= b && b <= '9') || ('a' <= b && b <= 'f')
}

// encodeBaggageValue is a baggage value as the W3C writes it: the printable
// ASCII its grammar allows as it is, and every other byte of the UTF-8
// percent-encoded, so a space, a comma or a name in another script travels
// whole.
func encodeBaggageValue(value string) string {
	const hexDigits = "0123456789ABCDEF"
	var out strings.Builder
	for i := 0; i < len(value); i++ {
		b := value[i]
		if allowedInBaggage(b) {
			out.WriteByte(b)
			continue
		}
		out.WriteByte('%')
		out.WriteByte(hexDigits[b>>4])
		out.WriteByte(hexDigits[b&0x0f])
	}
	return out.String()
}

// decodeBaggageValue is a baggage value read back: each %XX its byte, a %
// without two hex digits kept as it is.
func decodeBaggageValue(value string) string {
	var out []byte
	for i := 0; i < len(value); i++ {
		if value[i] == '%' && i+2 < len(value) && isHexDigitAnyCase(value[i+1]) && isHexDigitAnyCase(value[i+2]) {
			b, _ := strconv.ParseUint(value[i+1:i+3], 16, 8)
			out = append(out, byte(b))
			i += 2
			continue
		}
		out = append(out, value[i])
	}
	return toValidUTF8(out)
}

// toValidUTF8 is b with one U+FFFD for each maximal subpart of no UTF-8, as the other
// SDKs' decoders read it, where strings.ToValidUTF8 gives a whole run one.
func toValidUTF8(b []byte) string {
	var out strings.Builder
	for len(b) > 0 {
		r, size := utf8.DecodeRune(b)
		if r == utf8.RuneError && size == 1 {
			out.WriteRune(utf8.RuneError)
			b = b[maximalSubpart(b):]
			continue
		}
		out.Write(b[:size])
		b = b[size:]
	}
	return out.String()
}

// maximalSubpart is how many bytes of b, which opens no UTF-8 sequence, one
// U+FFFD stands for: its lead byte and what a sequence could still follow it with.
func maximalSubpart(b []byte) int {
	lo, hi, length := byte(0x80), byte(0xBF), 0
	switch lead := b[0]; {
	case 0xC2 <= lead && lead <= 0xDF:
		length = 2
	case lead == 0xE0:
		lo, length = 0xA0, 3
	case lead == 0xED:
		hi, length = 0x9F, 3
	case 0xE1 <= lead && lead <= 0xEF:
		length = 3
	case lead == 0xF0:
		lo, length = 0x90, 4
	case lead == 0xF4:
		hi, length = 0x8F, 4
	case 0xF1 <= lead && lead <= 0xF3:
		length = 4
	default:
		return 1
	}
	if len(b) < 2 || b[1] < lo || b[1] > hi {
		return 1
	}
	n := 2
	for n < length && n < len(b) && 0x80 <= b[n] && b[n] <= 0xBF {
		n++
	}
	return n
}

func isHexDigitAnyCase(b byte) bool {
	return isHexDigit(b) || ('A' <= b && b <= 'F')
}

// allowedInBaggage is the W3C's baggage-octet: printable ASCII but space, a
// double quote, a comma, a semicolon and a backslash; and but %, which the
// grammar allows and a reader would decode.
func allowedInBaggage(b byte) bool {
	switch {
	case b == 0x21:
		return true
	case 0x23 <= b && b <= 0x2b:
		return b != 0x25
	case 0x2d <= b && b <= 0x3a, 0x3c <= b && b <= 0x5b, 0x5d <= b && b <= 0x7e:
		return true
	}
	return false
}
