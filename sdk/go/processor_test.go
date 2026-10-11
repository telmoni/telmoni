package telmoni_test

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"reflect"
	"strings"
	"testing"
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
