package telmoni

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"time"

	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
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
// the other SDKs bound theirs: a batch mid-retry is given this long, and a
// Shutdown then gives up what is left.
const defaultWait = 30 * time.Second

// processor is the SDK's own batch processor: OpenTelemetry's batching and
// bounded queue, without the check that skips a span whose trace is not
// sampled.
//
// A run inside a trace the application does not sample is still sent; the
// run's own sample rate is applied here, by run, never through the trace's
// flag. The queue is bounded, so a slow Telmoni never slows an agent: past its
// size, spans are dropped and counted. A recorded run goes at once, in exports
// of its own.
type processor struct {
	exporter *exporter
	logger   *slog.Logger
	limits   queueLimits

	// recording lets one record at a time be built.
	recording sync.Mutex

	mu    sync.Mutex
	queue []sdktrace.ReadOnlySpan
	// The queue's first span is the taken-th ever queued, counted in 64 bits
	// whatever the target's int; a batch stops at each fence, a record's
	// first span and one past its last.
	taken  int64
	fences []int64
	// The record being built, its trace's spans as they end, and its room.
	stage     *stage
	reserved  int
	dropped   int
	flushing  int
	exporting bool
	stopped   bool
	// drained is closed for each Flush waiting, once the queue is empty.
	drained []chan struct{}

	// wake holds at most one pending signal for the worker; done closes
	// when the worker has drained and left.
	wake chan struct{}
	done chan struct{}
}

func newProcessor(exp *exporter, logger *slog.Logger, limits queueLimits) *processor {
	p := &processor{
		exporter: exp,
		logger:   logger,
		limits:   limits,
		wake:     make(chan struct{}, 1),
		done:     make(chan struct{}),
	}
	go p.work()
	return p
}

// stage is a record being built: its trace, and its spans as they end.
type stage struct {
	traceID trace.TraceID
	spans   []sdktrace.ReadOnlySpan
}

// hold stages the spans of the record in trace id as they end, and release
// queues them whole, to travel in exports of their own: so the same record
// sent again is the same bytes, whatever else ends meanwhile. One record at a
// time, its room kept for it; refused, holding nothing, once the processor is
// shut down or when the queue has none for its need spans. A release told the
// record was not built queues nothing.
func (p *processor) hold(id trace.TraceID, need int) (release func(built bool), err error) {
	p.recording.Lock()
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.stopped {
		p.recording.Unlock()
		return func(bool) {}, ErrShutdown
	}
	if len(p.queue)+need > p.limits.maxQueue {
		p.recording.Unlock()
		return func(bool) {}, ErrQueueFull
	}
	p.stage = &stage{traceID: id}
	p.reserved = need
	return func(built bool) {
		defer p.recording.Unlock()
		p.mu.Lock()
		defer p.mu.Unlock()
		staged := p.stage.spans
		p.stage, p.reserved = nil, 0
		if !built || p.stopped || len(staged) == 0 {
			return
		}
		first := p.taken + int64(len(p.queue))
		if first > p.taken {
			p.fences = append(p.fences, first)
		}
		p.fences = append(p.fences, first+int64(len(staged)))
		p.queue = append(p.queue, staged...)
		p.signal()
	}, nil
}

func (p *processor) OnStart(context.Context, sdktrace.ReadWriteSpan) {}

func (p *processor) OnEnd(span sdktrace.ReadOnlySpan) {
	for _, kv := range span.Attributes() {
		if string(kv.Key) == attrSampledOut {
			return
		}
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.stopped {
		return
	}
	if p.stage != nil && span.SpanContext().TraceID() == p.stage.traceID {
		p.stage.spans = append(p.stage.spans, span)
		return
	}
	if len(p.queue)+p.reserved >= p.limits.maxQueue {
		p.dropped++
		return
	}
	p.queue = append(p.queue, span)
	if len(p.queue) >= p.limits.maxBatch {
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

// Shutdown drains, then takes nothing more. Past its wait what is left is
// given up, and with a ctx already done before the worker wakes: no batch or
// attempt follows, as in the other SDKs.
func (p *processor) Shutdown(ctx context.Context) error {
	if ctx.Err() != nil {
		p.exporter.giveUp()
	}
	if !p.stop() {
		return nil
	}
	ctx, cancel := withDefaultWait(ctx)
	defer cancel()
	select {
	case <-p.done:
		return nil
	case <-ctx.Done():
		p.exporter.giveUp()
		return ctx.Err()
	}
}

// stop tells the worker to send what is queued and leave, without waiting, as
// a cleanup must; false when it was told already.
func (p *processor) stop() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.stopped {
		return false
	}
	p.stopped = true
	p.signal()
	return true
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
		if p.stopped && (len(p.queue) == 0 || p.exporter.gaveUp()) {
			p.release()
			p.mu.Unlock()
			return
		}
		n := min(len(p.queue), p.limits.maxBatch)
		if len(p.fences) > 0 {
			n = min(n, int(p.fences[0]-p.taken))
		}
		batch := make([]sdktrace.ReadOnlySpan, n)
		copy(batch, p.queue)
		rest := copy(p.queue, p.queue[n:])
		clear(p.queue[rest:])
		p.queue = p.queue[:rest]
		p.taken += int64(n)
		for len(p.fences) > 0 && p.fences[0] <= p.taken {
			p.fences = p.fences[1:]
		}
		dropped := p.dropped
		p.dropped = 0
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

// due is whether the worker should export without waiting; under the lock. A
// record queued goes at once.
func (p *processor) due() bool {
	return p.stopped || p.flushing > 0 || len(p.fences) > 0 || len(p.queue) >= p.limits.maxBatch
}

// release lets every Flush waiting go; under the lock.
func (p *processor) release() {
	for _, drained := range p.drained {
		close(drained)
	}
	p.drained = nil
}
