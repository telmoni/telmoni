package telmoni

import (
	"context"
	"testing"

	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
)

// The largest file put, which media.json names too.
const MediaMaxBytes = mediaMaxBytes

// The names the exporter holds back as content, which content.json names too.
var Content = content

// The score limits, which scores.json names too.
var ScoreLimits = map[string]int{
	"name_max":     scoreNameMax,
	"category_max": scoreCategoryMax,
	"version_max":  scoreVersionMax,
	"session_max":  sessionIDMax,
	"per_span_max": scoresPerSpanMax,
	"sent_max":     scoresSentMax,
}

// The hooks the tests reach for: a record's hold and its spans' ids, the W3C
// grammar's helpers and a time as sent.
var (
	HoldOne            = holdOne
	RecordedSpanID     = recordedSpanID
	EncodeBaggageValue = encodeBaggageValue
	DecodeBaggageValue = decodeBaggageValue
	ParseTraceparent   = parseTraceparent
	Nanos              = nanos
	DefaultClient      = defaultClient
)

// ExportSpans sends spans as the processor would a batch.
func ExportSpans(c *Client, spans []sdktrace.ReadOnlySpan) { c.exporter.export(spans) }

// Scrub is the scrubbers over one text, which canary.json holds every SDK to.
var Scrub = scrub

// FailScrubbing makes the scrubbers panic on every field until the test ends:
// called before the client is built, its cleanup runs after the client's.
func FailScrubbing(t testing.TB) {
	scrubber = func(string, bool, bool) (string, map[string]int) { panic("a scrubber failed") }
	t.Cleanup(func() { scrubber = scrub })
}

// holdOne ends one span in the trace of runID under a record's hold, then
// releases the hold, told the record was built only when built. False when
// the hold is refused.
func holdOne(c *Client, runID [16]byte, built bool) bool {
	release, err := c.processor.hold(trace.TraceID(runID), 1)
	if err != nil {
		return false
	}
	_, span := c.tracer.Start(withIDs(context.Background(), trace.TraceID(runID), recordedSpanID(runID, 0)), "staged")
	span.End()
	release(built)
	return true
}
