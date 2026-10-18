package telmoni

// The hooks the tests reach for: the random source the sample rate is drawn
// against, the queue's numbers and its hold for the processor's own tests,
// and the W3C grammar's helpers for the propagation's.
var (
	WithRandom         = withRandom
	WithQueue          = withQueue
	Hold               = func(c *Client) (release func()) { release, _ = c.processor.hold(0); return release }
	EncodeBaggageValue = encodeBaggageValue
	DecodeBaggageValue = decodeBaggageValue
	ParseTraceparent   = parseTraceparent
)
