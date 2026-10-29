package telmoni_test

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"reflect"
	"runtime"
	"strings"
	"sync/atomic"
	"testing"
	"testing/synctest"
	"time"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

func TestTheQueueIsBoundedAndDropsAreCountedOnce(t *testing.T) {
	// A queue of two, and a worker that will not wake until flushed.
	h := newHarness(t, testConfig(), nil, telmoni.WithBatching(2, 100, time.Minute))
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for n := range 5 {
		_, step := run.Step(ctx, fmt.Sprintf("s%d", n))
		step.End()
	}
	run.End()
	h.flush()
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"s0", "s1"}) {
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
	run.End()
	if err := h.client.Shutdown(context.Background()); err != nil {
		t.Fatalf("shutting down: %v", err)
	}
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"invoke_agent bot"}) {
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
	if got := len(h.transport.all()); got != 1 {
		t.Errorf("%d exports after a shutdown, want the one before it", got)
	}
}

func TestAShutdownWhoseContextIsDoneStillStopsTheWorker(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, queued := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	queued.End()
	// As a deferred Shutdown under signal.NotifyContext finds it after Ctrl-C,
	// what is queued given up with it.
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

// A shutdown drains until its deadline, then sends nothing more however long the process lives.
func TestAShutdownGivesUpAtItsDeadline(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		// Every answer asks for 20 s, waited through the SDK's own wait; a
		// batch of one leaves the run's own span queued behind its step.
		slow := scripted{status: http.StatusTooManyRequests, headers: map[string]string{"Retry-After": "20"}, body: []byte("{}")}
		transport := newFakeTransport(t, slow, slow, slow, slow, slow, slow)
		client, err := telmoni.New(testConfig(), telmoni.WithTransport(transport.send), telmoni.WithBatching(2048, 1, time.Minute))
		if err != nil {
			t.Fatalf("building the client: %v", err)
		}
		ctx, run := client.Run(context.Background(), "bot", telmoni.RunOptions{})
		_, step := run.Step(ctx, "s")
		step.End()
		run.End()
		if err := client.Shutdown(context.Background()); !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("Shutdown = %v, want context.DeadlineExceeded", err)
		}
		// Before the deadline the drain went on: the step's retry, 20 s in.
		if got := len(transport.all()); got != 2 {
			t.Errorf("%d attempts before the deadline, want 2", got)
		}
		time.Sleep(10 * time.Minute)
		if got := len(transport.all()); got != 2 {
			t.Errorf("%d attempts once given up, want still 2", got)
		}
	})
}

// A client dropped without Shutdown sends what it queued at once and lets its worker go.
func TestAClientDroppedWithoutShutdownSendsWhatItQueued(t *testing.T) {
	transport := newFakeTransport(t)
	func() {
		client, err := telmoni.New(testConfig(), telmoni.WithTransport(transport.send), telmoni.WithBatching(2048, 512, time.Hour))
		if err != nil {
			t.Fatalf("building the client: %v", err)
		}
		_, run := client.Run(context.Background(), "bot", telmoni.RunOptions{})
		run.End()
	}()
	// The cleanup runs once a collection finds nothing reaching the client.
	for deadline := time.Now().Add(waitTimeout); time.Now().Before(deadline); {
		runtime.GC()
		if transport.waitFor(1, 10*time.Millisecond) {
			return
		}
	}
	t.Fatal("a client dropped without Shutdown held its queue for the hour's delay")
}

func TestABatchFillsAndGoesWithoutAFlush(t *testing.T) {
	h := newHarness(t, testConfig(), nil, telmoni.WithBatching(2048, 3, time.Minute))
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for n := range 3 {
		_, step := run.Step(ctx, fmt.Sprintf("s%d", n))
		step.End()
	}
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("three steps did not fill a batch of three")
	}
	first := spansIn(h.transport.bodies(t)[0])
	if got := sortedNames(first); !reflect.DeepEqual(got, []string{"s0", "s1", "s2"}) {
		t.Errorf("the first export holds %v", got)
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
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"invoke_agent bot"}) {
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
	// The steps overflow a queue of one before the worker wakes, so the batch
	// the flush takes carries a drop count and the logger panics on its warning.
	transport := newFakeTransport(t)
	h := newHarness(t, testConfig(), transport,
		telmoni.WithLogger(slog.New(panicking{})),
		telmoni.WithBatching(1, 100, time.Minute),
	)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for n := range 3 {
		_, step := run.Step(ctx, fmt.Sprintf("s%d", n))
		step.End()
	}
	run.End()
	h.flush()
	// The worker is alive: a run after the panic is sent.
	_, again := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	again.End()
	h.flush()
	want := []string{"invoke_agent bot", "s0"}
	if got := sortedNames(transport.spans(t)); !reflect.DeepEqual(got, want) {
		t.Errorf("spans sent: %v, want %v", got, want)
	}
}

// A transport that panics loses the batch it was given, and the logger says
// so; the worker goes on, and the next batch is sent.
func TestATransportThatPanicsLosesOnlyItsBatch(t *testing.T) {
	transport := newFakeTransport(t)
	var sends atomic.Int32
	h := newHarness(t, testConfig(), transport, telmoni.WithTransport(func(req *http.Request) (*http.Response, error) {
		if sends.Add(1) == 1 {
			panic("the transport broke")
		}
		return transport.send(req)
	}))
	_, lost := h.client.Run(context.Background(), "lost", telmoni.RunOptions{})
	lost.End()
	h.flush()
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	h.flush()
	if got := names(transport.spans(t)); !reflect.DeepEqual(got, []string{"invoke_agent bot"}) {
		t.Errorf("spans sent: %v, want the second run's own, the first's lost with the panic", got)
	}
	warnings := h.logs.warnings()
	if len(warnings) != 1 || !strings.Contains(warnings[0], "a hook panicked during an export; the batch was dropped") {
		t.Errorf("warnings = %q, want one saying the batch was dropped", warnings)
	}
}
