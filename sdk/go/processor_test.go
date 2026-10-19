package telmoni_test

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"testing/synctest"
	"time"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

func TestTheQueueIsBoundedAndDropsAreCountedOnce(t *testing.T) {
	// A queue of two, and a worker that will not wake until flushed.
	h := newHarness(t, testConfig(), nil, telmoni.WithQueue(2, 100, time.Minute))
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	// The start wakes the worker and goes alone; waiting for it makes what
	// follows the same on every run: two steps queued, the rest dropped.
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	for n := range 5 {
		_, step := run.Step(ctx, fmt.Sprintf("s%d", n))
		step.End()
	}
	run.End()
	h.flush()
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"s0", "s1", "telmoni.run.start"}) {
		t.Errorf("spans sent: %v", got)
	}
	warnings := h.logs.warnings()
	if len(warnings) != 1 {
		t.Fatalf("want one warning, got %q", warnings)
	}
	if !strings.Contains(warnings[0], "queue was full") || !strings.Contains(warnings[0], "4 spans") {
		t.Errorf("warning = %q, want the count of the three steps and the run dropped", warnings[0])
	}
}

func TestShutdownDrainsWhatIsQueued(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	// The start goes alone once the worker has it; an empty run ended at once
	// would otherwise travel with it, in one export.
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	run.End()
	if err := h.client.Shutdown(context.Background()); err != nil {
		t.Fatalf("shutting down: %v", err)
	}
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"invoke_agent bot", "telmoni.run.start"}) {
		t.Errorf("spans sent: %v", got)
	}
	if err := h.client.Shutdown(context.Background()); err != nil {
		t.Errorf("a second shutdown: %v", err)
	}
	// Nothing opened after a shutdown is sent, and nothing breaks.
	_, late := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	late.End()
	if err := h.client.Flush(context.Background()); err != nil {
		t.Errorf("a flush after shutdown: %v", err)
	}
	if got := len(h.transport.all()); got != 2 {
		t.Errorf("%d exports after a shutdown, want the two before it", got)
	}
}

func TestAShutdownWhoseContextIsDoneStillStopsTheWorker(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	// As a deferred Shutdown under signal.NotifyContext finds it after Ctrl-C.
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := h.client.Shutdown(ctx); err != nil && !errors.Is(err, context.Canceled) {
		t.Fatalf("shutting down: %v", err)
	}
	_, late := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	late.End()
	if err := h.client.Flush(context.Background()); err != nil {
		t.Errorf("a flush after shutdown: %v", err)
	}
	if got := len(h.transport.all()); got != 0 {
		t.Errorf("%d exports after a shutdown, want none", got)
	}
}

func TestABatchFillsAndGoesWithoutAFlush(t *testing.T) {
	h := newHarness(t, testConfig(), nil, telmoni.WithQueue(2048, 3, time.Minute))
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	for n := range 3 {
		_, step := run.Step(ctx, fmt.Sprintf("s%d", n))
		step.End()
	}
	if !h.transport.waitFor(2, waitTimeout) {
		t.Fatal("three steps did not fill a batch of three")
	}
	second := spansIn(h.transport.bodies(t)[1])
	if got := sortedNames(second); !reflect.DeepEqual(got, []string{"s0", "s1", "s2"}) {
		t.Errorf("the second export holds %v", got)
	}
	run.End()
}

func TestFlushHonoursItsContext(t *testing.T) {
	// A transport that never answers until released holds the worker in an
	// export; a flush with a deadline gives up, a later one goes through.
	release := make(chan struct{})
	transport := newFakeTransport(t)
	h := newHarness(t, testConfig(), transport, telmoni.WithTransport(func(req *http.Request) (*http.Response, error) {
		<-release
		return transport.send(req)
	}))
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	// The provider joins its processors' errors, so the deadline is wrapped.
	if err := h.client.Flush(ctx); !errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("Flush = %v, want context.DeadlineExceeded", err)
	}
	close(release)
	h.flush()
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"invoke_agent bot", "telmoni.run.start"}) {
		t.Errorf("spans sent: %v", got)
	}
}

// A flush whose context has no deadline still returns while an export hangs:
// after 30 seconds, as the other SDKs wait at most. The bubble's clock makes
// those seconds pass at once.
func TestAFlushWithoutADeadlineWaitsThirtySeconds(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		release := make(chan struct{})
		transport := newFakeTransport(t)
		h := newHarness(t, testConfig(), transport, telmoni.WithTransport(func(req *http.Request) (*http.Response, error) {
			<-release
			return transport.send(req)
		}))
		_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
		run.End()
		start := time.Now()
		if err := h.client.Flush(context.Background()); !errors.Is(err, context.DeadlineExceeded) {
			t.Errorf("Flush = %v, want context.DeadlineExceeded", err)
		}
		if waited := time.Since(start); waited != 30*time.Second {
			t.Errorf("Flush waited %v, want 30s", waited)
		}
		close(release)
	})
}

// panicking is a slog handler that panics on every record, as a broken
// handler of the application's would.
type panicking struct{}

func (panicking) Enabled(context.Context, slog.Level) bool  { return true }
func (panicking) Handle(context.Context, slog.Record) error { panic("the handler broke") }
func (panicking) WithAttrs([]slog.Attr) slog.Handler        { return panicking{} }
func (panicking) WithGroup(string) slog.Handler             { return panicking{} }

// A hook of the application's that panics, here the logger on the queue's
// warning, kills no worker: the warning is lost, the batch beside it and the
// next one still go, and a flush and a shutdown still return.
func TestAHookThatPanicsDoesNotStopTheExporter(t *testing.T) {
	// The start's export is held once it is recorded; the steps behind it
	// overflow a queue of one, so the next batch carries a drop count and
	// the logger panics on its warning.
	release := make(chan struct{})
	transport := newFakeTransport(t)
	h := newHarness(t, testConfig(), transport,
		telmoni.WithTransport(func(req *http.Request) (*http.Response, error) {
			resp, err := transport.send(req)
			<-release
			return resp, err
		}),
		telmoni.WithLogger(slog.New(panicking{})),
		telmoni.WithQueue(1, 100, time.Minute),
	)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	if !transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	for n := range 3 {
		_, step := run.Step(ctx, fmt.Sprintf("s%d", n))
		step.End()
	}
	run.End()
	close(release)
	h.flush()
	// The worker is alive: a run after the panic is sent. Its start goes
	// alone first, so the queue of one has room for the run itself.
	_, again := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	if !transport.waitFor(3, waitTimeout) {
		t.Fatal("the run after the panic did not start")
	}
	again.End()
	h.flush()
	want := []string{"invoke_agent bot", "s0", "telmoni.run.start", "telmoni.run.start"}
	if got := sortedNames(transport.spans(t)); !reflect.DeepEqual(got, want) {
		t.Errorf("spans sent: %v, want %v", got, want)
	}
}

// A transport that panics loses the batch it was given, and the logger says
// so; the worker goes on, and the next batch is sent.
func TestATransportThatPanicsLosesOnlyItsBatch(t *testing.T) {
	transport := newFakeTransport(t)
	panicked := make(chan struct{})
	var sends atomic.Int32
	h := newHarness(t, testConfig(), transport, telmoni.WithTransport(func(req *http.Request) (*http.Response, error) {
		if sends.Add(1) == 1 {
			close(panicked)
			panic("the transport broke")
		}
		return transport.send(req)
	}))
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	// Waiting for the panic keeps the start alone in the batch it costs.
	select {
	case <-panicked:
	case <-time.After(waitTimeout):
		t.Fatal("the run's start was not sent at once")
	}
	run.End()
	h.flush()
	if got := names(transport.spans(t)); !reflect.DeepEqual(got, []string{"invoke_agent bot"}) {
		t.Errorf("spans sent: %v, want the run's own, its start lost with the panic", got)
	}
	warnings := h.logs.warnings()
	if len(warnings) != 1 || !strings.Contains(warnings[0], "a hook panicked during an export; the batch was dropped") {
		t.Errorf("warnings = %q, want one saying the batch was dropped", warnings)
	}
}
