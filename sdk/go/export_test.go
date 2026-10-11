package telmoni

// The hooks the tests reach for: the random source the sample rate is drawn
// against, and the queue's numbers for the processor's own tests.
var (
	WithRandom = withRandom
	WithQueue  = withQueue
)
