package telmoni

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"time"

	sdktrace "go.opentelemetry.io/otel/sdk/trace"
)

// queueLimits size the processor: OpenTelemetry's own batch processor's
// numbers by default.
type queueLimits struct {
	maxQueue int
	maxBatch int
	delay    time.Duration
}

var defaultQueueLimits = queueLimits{maxQueue: 2048, maxBatch: 512, delay: 5 * time.Second}

// defaultWait bounds a Flush or a Shutdown whose context has no deadline, as
// the other SDKs bound theirs: a batch mid-retry is given this long, and the
// worker is then left to finish on its own.
const defaultWait = 30 * time.Second

// processor is the SDK's own batch processor: OpenTelemetry's batching and
// bounded queue, without the check that skips a span whose trace is not
// sampled.
//
// A run inside a trace the application does not sample is still sent; the
// run's own sample rate is applied here, by run, never through the trace's
// flag. The queue is bounded, so a slow Telmoni never slows an agent: past its
// size, spans are dropped and counted. telmoni.run.start wakes the worker at
// once.
type processor struct {
	exporter *exporter
	logger   *slog.Logger
	limits   queueLimits

	mu         sync.Mutex
	queue      []sdktrace.ReadOnlySpan
	sampledOut map[string]struct{}
	dropped    int
	urgent     bool
	flushing   int
	exporting  bool
	stopped    bool
	// drained is closed for each Flush waiting, once the queue is empty.
	drained []chan struct{}

	// wake holds at most one pending signal for the worker; done closes
	// when the worker has drained and left.
	wake chan struct{}
	done chan struct{}
}

func newProcessor(exp *exporter, logger *slog.Logger, limits queueLimits) *processor {
	p := &processor{
		exporter:   exp,
		logger:     logger,
		limits:     limits,
		sampledOut: map[string]struct{}{},
		wake:       make(chan struct{}, 1),
		done:       make(chan struct{}),
	}
	go p.work()
	return p
}

// sampleOut drops every span of this run, the run's own among them.
func (p *processor) sampleOut(runID string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.sampledOut[runID] = struct{}{}
}

func (p *processor) OnStart(context.Context, sdktrace.ReadWriteSpan) {}

func (p *processor) OnEnd(span sdktrace.ReadOnlySpan) {
	var runID, operation string
	for _, kv := range span.Attributes() {
		switch string(kv.Key) {
		case attrRunID:
			runID = kv.Value.AsString()
		case attrOperation:
			operation = kv.Value.AsString()
		}
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.stopped {
		return
	}
	if _, out := p.sampledOut[runID]; out && runID != "" {
		// The run's own span is the last of its run to end, so its id leaves
		// the set with it.
		if runOperations[operation] {
			delete(p.sampledOut, runID)
		}
		return
	}
	if len(p.queue) >= p.limits.maxQueue {
		p.dropped++
		return
	}
	p.queue = append(p.queue, span)
	if span.Name() == runStart {
		p.urgent = true
	}
	if p.urgent || len(p.queue) >= p.limits.maxBatch {
		p.signal()
	}
}

func (p *processor) ForceFlush(ctx context.Context) error {
	p.mu.Lock()
	if p.stopped || (len(p.queue) == 0 && !p.exporting) {
		p.mu.Unlock()
		return nil
	}
	drained := make(chan struct{})
	p.drained = append(p.drained, drained)
	p.flushing++
	p.signal()
	p.mu.Unlock()
	defer func() {
		p.mu.Lock()
		p.flushing--
		p.mu.Unlock()
	}()
	ctx, cancel := withDefaultWait(ctx)
	defer cancel()
	select {
	case <-drained:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (p *processor) Shutdown(ctx context.Context) error {
	p.mu.Lock()
	if p.stopped {
		p.mu.Unlock()
		return nil
	}
	p.stopped = true
	p.signal()
	p.mu.Unlock()
	ctx, cancel := withDefaultWait(ctx)
	defer cancel()
	select {
	case <-p.done:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

// withDefaultWait is ctx, given defaultWait when it has no deadline of its
// own: the documented `defer client.Shutdown(context.Background())` would
// otherwise hold a process exit, and a Flush with that context its caller,
// for as long as a batch's retries take.
func withDefaultWait(ctx context.Context) (context.Context, context.CancelFunc) {
	if _, has := ctx.Deadline(); has {
		return ctx, func() {}
	}
	return context.WithTimeout(ctx, defaultWait)
}

// signal wakes the worker; a signal already pending is enough. Called under
// the lock, so the state it announces is in place when the worker reads it.
func (p *processor) signal() {
	select {
	case p.wake <- struct{}{}:
	default:
	}
}

func (p *processor) work() {
	defer close(p.done)
	timer := time.NewTimer(p.limits.delay)
	defer timer.Stop()
	for {
		p.mu.Lock()
		if p.due() {
			// This pass handles the state a pending signal announced; left,
			// it would wake the next pass for nothing.
			select {
			case <-p.wake:
			default:
			}
		} else {
			p.mu.Unlock()
			timer.Reset(p.limits.delay)
			select {
			case <-p.wake:
			case <-timer.C:
			}
			p.mu.Lock()
		}
		if p.stopped && len(p.queue) == 0 {
			p.release()
			p.mu.Unlock()
			return
		}
		n := min(len(p.queue), p.limits.maxBatch)
		batch := make([]sdktrace.ReadOnlySpan, n)
		copy(batch, p.queue)
		rest := copy(p.queue, p.queue[n:])
		clear(p.queue[rest:])
		p.queue = p.queue[:rest]
		dropped := p.dropped
		p.dropped = 0
		p.urgent = false
		p.exporting = n > 0
		p.mu.Unlock()
		if dropped > 0 {
			p.warn(fmt.Sprintf("telmoni: the export queue was full; %d spans were dropped", dropped))
		}
		if n > 0 {
			p.export(batch)
		}
		p.mu.Lock()
		p.exporting = false
		if len(p.queue) == 0 {
			p.release()
		}
		p.mu.Unlock()
	}
}

// export sends one batch. A hook of the application's (its transport, sleep
// or logger) may panic, and a panic on the worker would take the process
// down: recovered here, so the worker outlives it and the next batch still
// goes. The batch in hand is lost, and said so.
func (p *processor) export(batch []sdktrace.ReadOnlySpan) {
	defer func() {
		if recover() != nil {
			p.warn("telmoni: a hook panicked during an export; the batch was dropped")
		}
	}()
	p.exporter.export(batch)
}

// warn logs text through the application's logger, whose handler may panic
// too: recovered on its own, so a warning that panics costs no batch. Nothing
// says so, since the logger is where warnings go.
func (p *processor) warn(text string) {
	defer func() { _ = recover() }()
	p.logger.Warn(text)
}

// due is whether the worker should export without waiting; under the lock.
func (p *processor) due() bool {
	return p.stopped || p.flushing > 0 || p.urgent || len(p.queue) >= p.limits.maxBatch
}

// release lets every Flush waiting go; under the lock.
func (p *processor) release() {
	for _, drained := range p.drained {
		close(drained)
	}
	p.drained = nil
}
