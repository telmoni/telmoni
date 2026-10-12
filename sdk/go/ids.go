package telmoni

import (
	"context"
	"crypto/sha1"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"math/rand/v2"
	"slices"
	"strconv"

	"go.opentelemetry.io/otel/trace"
)

// ids makes the client's trace and span ids: at random, as OpenTelemetry's own
// generator makes them, except for the spans of a recorded run, whose ids are
// derived from the run's id, so that a record sent twice is the same bytes and
// ingest's claim on the body answers it once.
type ids struct{}

// givenKey carries the ids a span started with that context takes. In the
// context rather than on the generator: a span another goroutine opens while
// a record is built must never take them.
type givenKey struct{}

type givenIDs struct {
	trace trace.TraceID
	span  trace.SpanID
}

// withIDs is ctx with the ids a span started with it takes.
func withIDs(ctx context.Context, traceID trace.TraceID, spanID trace.SpanID) context.Context {
	return context.WithValue(ctx, givenKey{}, givenIDs{trace: traceID, span: spanID})
}

func (ids) NewIDs(ctx context.Context) (trace.TraceID, trace.SpanID) {
	if given, ok := ctx.Value(givenKey{}).(givenIDs); ok {
		return given.trace, given.span
	}
	var traceID trace.TraceID
	for !traceID.IsValid() {
		binary.NativeEndian.PutUint64(traceID[:8], rand.Uint64())
		binary.NativeEndian.PutUint64(traceID[8:], rand.Uint64())
	}
	return traceID, randomSpanID()
}

func (ids) NewSpanID(ctx context.Context, _ trace.TraceID) trace.SpanID {
	if given, ok := ctx.Value(givenKey{}).(givenIDs); ok {
		return given.span
	}
	return randomSpanID()
}

func randomSpanID() trace.SpanID {
	var spanID trace.SpanID
	for !spanID.IsValid() {
		binary.NativeEndian.PutUint64(spanID[:], rand.Uint64())
	}
	return spanID
}

// recordedSpanID is the id of a recorded run's span at place: the first eight
// bytes of the version 5 UUID (RFC 9562) named by the place, in decimal, under
// the run's id. Its version's bits are among those eight, so it is never zero.
func recordedSpanID(runID [16]byte, place int) trace.SpanID {
	u := uuid5(runID, strconv.Itoa(place))
	return trace.SpanID(u[:8])
}

// uuid5 is the version 5 UUID named by name under namespace. SHA-1 is the
// version's own hash: the UUID names a span, and keeps no secret.
func uuid5(namespace [16]byte, name string) [16]byte {
	sum := sha1.Sum(slices.Concat(namespace[:], []byte(name)))
	var u [16]byte
	copy(u[:], sum[:])
	u[6] = u[6]&0x0f | 0x50
	u[8] = u[8]&0x3f | 0x80
	return u
}

// parseUUID reads a UUID in its 8-4-4-4-12 form, in either case and of any
// version.
func parseUUID(s string) ([16]byte, bool) {
	var u [16]byte
	if len(s) != 36 || s[8] != '-' || s[13] != '-' || s[18] != '-' || s[23] != '-' {
		return u, false
	}
	digits := s[:8] + s[9:13] + s[14:18] + s[19:23] + s[24:]
	if _, err := hex.Decode(u[:], []byte(digits)); err != nil {
		return u, false
	}
	return u, true
}

// formatUUID is u in its lowercase 8-4-4-4-12 form.
func formatUUID(u [16]byte) string {
	return fmt.Sprintf("%x-%x-%x-%x-%x", u[:4], u[4:6], u[6:8], u[8:10], u[10:])
}
