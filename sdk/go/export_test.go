package telmoni

import (
	"context"

	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
)

// The hooks the tests reach for: the sample rate's random source, the queue's
// numbers, a record's hold, the W3C grammar's helpers and a time as sent.
var (
	WithRandom         = withRandom
	WithQueue          = withQueue
	HoldOne            = holdOne
	EncodeBaggageValue = encodeBaggageValue
	DecodeBaggageValue = decodeBaggageValue
	ParseTraceparent   = parseTraceparent
	Nanos              = nanos
	DefaultClient      = defaultClient
)

// ExportSpans sends spans as the processor would a batch.
func ExportSpans(c *Client, spans []sdktrace.ReadOnlySpan) { c.exporter.export(spans) }

// holdOne ends one span in the trace of runID under a record's hold, then
// releases the hold, told the record was built only when built. False when
// the hold is refused.
func holdOne(c *Client, runID [16]byte, built bool) bool {
	release, ok := c.processor.hold(trace.TraceID(runID), 1)
	if !ok {
		return false
	}
	_, span := c.tracer.Start(withIDs(context.Background(), trace.TraceID(runID), recordedSpanID(runID, 0)), "staged")
	span.End()
	release(built)
	return true
}
