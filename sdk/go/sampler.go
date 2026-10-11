package telmoni

import (
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
)

// recordingSampler records every span and decides nothing of its own.
//
// A run takes the sampled flag of the trace it joins. Where it starts a trace,
// it takes the decision the configured root sampler gives it, so an
// application sampling at a tenth keeps a tenth of its own spans inside runs
// as outside them. Nothing is ever dropped here: the SDK's processor exports
// recorded spans whether the flag is set or not, and applies the run's sample
// rate itself.
type recordingSampler struct {
	root sdktrace.Sampler
}

func (s recordingSampler) ShouldSample(p sdktrace.SamplingParameters) sdktrace.SamplingResult {
	// The span's own attributes come from its start options; returned here as
	// well they would count twice toward the attribute limit.
	parent := trace.SpanContextFromContext(p.ParentContext)
	if parent.IsValid() {
		return sdktrace.SamplingResult{
			Decision:   recording(parent.IsSampled()),
			Tracestate: parent.TraceState(),
		}
	}
	decision := s.root.ShouldSample(p)
	return sdktrace.SamplingResult{
		Decision:   recording(decision.Decision == sdktrace.RecordAndSample),
		Tracestate: decision.Tracestate,
	}
}

func (recordingSampler) Description() string {
	return "TelmoniRecordingSampler"
}

func recording(sampled bool) sdktrace.SamplingDecision {
	if sampled {
		return sdktrace.RecordAndSample
	}
	return sdktrace.RecordOnly
}
