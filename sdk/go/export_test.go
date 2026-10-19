package telmoni

import (
	"context"

	"go.opentelemetry.io/otel/trace"
)

// The hooks the tests reach for: the random source the sample rate is drawn
// against, the queue's numbers and a record's hold for the processor's own
// tests, and the W3C grammar's helpers for the propagation's.
var (
	WithRandom         = withRandom
	WithQueue          = withQueue
	HoldOne            = holdOne
	EncodeBaggageValue = encodeBaggageValue
	DecodeBaggageValue = decodeBaggageValue
	ParseTraceparent   = parseTraceparent
)

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
