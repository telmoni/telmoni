package telmoni

// The hooks the tests reach for: the random source the sample rate is drawn
// against, the queue's numbers and its hold for the processor's own tests,
// and the W3C grammar's helpers for the propagation's.
var (
	WithRandom         = withRandom
	WithQueue          = withQueue
	Hold               = func(c *Client) (release func()) { return c.processor.hold() }
	EncodeBaggageValue = encodeBaggageValue
	DecodeBaggageValue = decodeBaggageValue
	ParseTraceparent   = parseTraceparent
)
