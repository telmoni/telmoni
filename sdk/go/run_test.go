package telmoni_test

import (
	"context"
	"errors"
	"fmt"
	"net"
	"reflect"
	"strings"
	"sync"
	"testing"

	"go.opentelemetry.io/otel/trace"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

type planningFailed struct {
	detail string
}

func (e *planningFailed) Error() string { return e.detail }

func runSpan(t *testing.T, h *harness) map[string]any {
	t.Helper()
	runs := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(runs) != 1 {
		t.Fatalf("want one run span, got %v", names(runs))
	}
	return runs[0]
}

func TestARunIsASpanWithTheContractAttributes(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "support-bot", telmoni.RunOptions{
		UserID:         "user_1",
		ConversationID: "conv_1",
		Tags:           map[string]string{"env": "test", "team": "a"},
		Bounds:         &telmoni.Bounds{StallAfterS: 30, MaxSteps: 5, MaxCostUSD: 0.5},
	})
	if !uuidV4.MatchString(run.ID()) {
		t.Errorf("run id %q is not a lowercase version 4 UUID", run.ID())
	}
	if run.Agent() != "support-bot" {
		t.Errorf("Agent() = %q", run.Agent())
	}
	if telmoni.RunFromContext(ctx) != run {
		t.Error("the run is not found from the context it returned")
	}
	spanID := trace.SpanContextFromContext(ctx).SpanID().String()
	run.End()
	if telmoni.RunFromContext(context.Background()) != nil {
		t.Error("a run was found in a context without one")
	}
	h.flush()
	span := runSpan(t, h)
	if span["kind"] != float64(1) {
		t.Errorf("kind = %v, want 1", span["kind"])
	}
	if span["spanId"] != spanID {
		t.Errorf("the context carries span %s, the run's is %v", spanID, span["spanId"])
	}
	if _, has := span["parentSpanId"]; has {
		t.Error("a run opened with no span in its context has a parent")
	}
	if _, has := span["status"]; has {
		t.Error("a run that went well has a status")
	}
	want := map[string]any{
		"gen_ai.operation.name":       "invoke_agent",
		"gen_ai.agent.name":           "support-bot",
		"telmoni.run_id":              run.ID(),
		"telmoni.agent":               "support-bot",
		"user.id":                     "user_1",
		"gen_ai.conversation.id":      "conv_1",
		"telmoni.bound.stall_after_s": int64(30),
		"telmoni.bound.max_steps":     int64(5),
		"telmoni.bound.max_cost_usd":  0.5,
		"telmoni.sample_rate":         1.0,
		"telmoni.tag.env":             "test",
		"telmoni.tag.team":            "a",
	}
	if got := attributesOf(t, span); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
}

func TestTheStartIsSentAtOnceWithTheBounds(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{Bounds: &telmoni.Bounds{MaxSteps: 3}})
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	spans := h.transport.spans(t)
	if got := names(spans); !reflect.DeepEqual(got, []string{"telmoni.run.start"}) {
		t.Fatalf("sent at the start: %v", got)
	}
	start := spans[0]
	if start["startTimeUnixNano"] != start["endTimeUnixNano"] {
		t.Error("telmoni.run.start has a duration")
	}
	want := map[string]any{
		"telmoni.run_id":          run.ID(),
		"telmoni.agent":           "bot",
		"telmoni.bound.max_steps": int64(3),
		"telmoni.sample_rate":     1.0,
	}
	if got := attributesOf(t, start); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
	run.End()
	h.flush()
	if runSpan(t, h)["spanId"] != h.transport.spans(t)[0]["parentSpanId"] {
		t.Error("the start is not the run's child")
	}
}

func TestAnErrorFailsTheRunWithItsTypeAlone(t *testing.T) {
	for _, tc := range []struct {
		err  error
		want string
	}{
		{errors.New("the message, which is content and never sent"), "errors.errorString"},
		{&net.OpError{Op: "dial", Net: "tcp", Err: errors.New("the message, which is content and never sent")}, "net.OpError"},
		{&planningFailed{detail: "the message, which is content and never sent"}, "telmoni_test.planningFailed"},
	} {
		t.Run(tc.want, func(t *testing.T) {
			h := newHarness(t, testConfig(), nil)
			_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
			run.Fail(tc.err)
			run.End()
			h.flush()
			span := runSpan(t, h)
			if !reflect.DeepEqual(span["status"], map[string]any{"code": float64(2)}) {
				t.Errorf("status = %v, want {code: 2}", span["status"])
			}
			if got := attributesOf(t, span)["error.type"]; got != tc.want {
				t.Errorf("error.type = %v, want %s", got, tc.want)
			}
			if strings.Contains(h.transport.text(), "never sent") {
				t.Error("the error's message was sent")
			}
		})
	}
}

func TestFailWithAndCancelMarkTheRun(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, failed := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	failed.FailWith("RateLimit")
	failed.End()
	_, cancelled := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	cancelled.Cancel()
	cancelled.End()
	h.flush()
	for _, span := range withPrefix(h.transport.spans(t), "invoke_agent ") {
		attributes := attributesOf(t, span)
		switch attributes["telmoni.run_id"] {
		case failed.ID():
			if attributes["error.type"] != "RateLimit" {
				t.Errorf("error.type = %v, want RateLimit", attributes["error.type"])
			}
			if !reflect.DeepEqual(span["status"], map[string]any{"code": float64(2)}) {
				t.Errorf("status = %v, want {code: 2}", span["status"])
			}
		case cancelled.ID():
			if attributes["telmoni.cancelled"] != true {
				t.Errorf("telmoni.cancelled = %v, want true", attributes["telmoni.cancelled"])
			}
			if _, has := span["status"]; has {
				t.Error("a cancelled run has a status")
			}
			if _, has := attributes["error.type"]; has {
				t.Error("a cancelled run has an error.type")
			}
		default:
			t.Errorf("a run span of an unknown run: %v", attributes["telmoni.run_id"])
		}
	}
}

// A deadline that ran out is a failure, not a cancellation: a stalled call is
// what a monitor is for, and the other SDKs fail a timeout with its type.
func TestADeadlineThatRanOutIsAFailure(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.Fail(context.DeadlineExceeded)
	run.End()
	h.flush()
	span := runSpan(t, h)
	attributes := attributesOf(t, span)
	if _, has := attributes["telmoni.cancelled"]; has {
		t.Error("a deadline that ran out was recorded as a cancellation")
	}
	if got := attributes["error.type"]; got != "context.deadlineExceededError" {
		t.Errorf("error.type = %v, want context.deadlineExceededError", got)
	}
	if !reflect.DeepEqual(span["status"], map[string]any{"code": float64(2)}) {
		t.Errorf("status = %v, want {code: 2}", span["status"])
	}
}

func TestAContextErrorIsACancellation(t *testing.T) {
	for name, err := range map[string]error{
		"Canceled": context.Canceled,
		"wrapped":  fmt.Errorf("fetching the page: %w", context.Canceled),
	} {
		t.Run(name, func(t *testing.T) {
			h := newHarness(t, testConfig(), nil)
			_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
			run.Fail(err)
			run.End()
			h.flush()
			span := runSpan(t, h)
			attributes := attributesOf(t, span)
			if attributes["telmoni.cancelled"] != true {
				t.Errorf("telmoni.cancelled = %v, want true", attributes["telmoni.cancelled"])
			}
			if _, has := attributes["error.type"]; has {
				t.Error("a cancellation has an error.type")
			}
			if _, has := span["status"]; has {
				t.Error("a cancellation has a status")
			}
		})
	}
}

func TestAFunctionOpensARunPerCall(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	var seen []string
	handle := func(ctx context.Context, question string) string {
		ctx, run := h.client.Run(ctx, "bot", telmoni.RunOptions{})
		defer run.End()
		seen = append(seen, telmoni.RunFromContext(ctx).ID())
		return strings.ToUpper(question)
	}
	if got := handle(context.Background(), "a"); got != "A" {
		t.Errorf("handle(a) = %q", got)
	}
	if got := handle(context.Background(), "b"); got != "B" {
		t.Errorf("handle(b) = %q", got)
	}
	h.flush()
	if runs := withPrefix(h.transport.spans(t), "invoke_agent bot"); len(runs) != 2 {
		t.Errorf("want two runs, got %d", len(runs))
	}
	if len(seen) != 2 || seen[0] == seen[1] {
		t.Errorf("want two distinct run ids, got %v", seen)
	}
}

func TestRunsAndStepsCrossGoroutines(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	var threaded string
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
		defer run.End()
		threaded = run.ID()
		var inner sync.WaitGroup
		inner.Add(1)
		go func() {
			defer inner.Done()
			_, step := telmoni.RunFromContext(ctx).Step(ctx, "think")
			step.End()
		}()
		inner.Wait()
	}()
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	wg.Wait()
	h.flush()
	spans := h.transport.spans(t)
	if runs := withPrefix(spans, "invoke_agent bot"); len(runs) != 2 {
		t.Errorf("want two runs, got %d", len(runs))
	}
	step := byName(spans)["think"]
	if step == nil {
		t.Fatal("the step from the goroutine was not sent")
	}
	if got := attributesOf(t, step)["telmoni.run_id"]; got != threaded {
		t.Errorf("the step carries run %v, want %s", got, threaded)
	}
}

func TestNothingIsSentOutsideARun(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx := context.Background()
	none := telmoni.RunFromContext(ctx)
	if none != nil {
		t.Fatal("a run was found outside one")
	}
	if none.ID() != "" || none.Agent() != "" {
		t.Error("a nil run has an id or an agent")
	}
	stepCtx, step := none.Step(ctx, "alone")
	step.Fail(errors.New("x"))
	step.End()
	if stepCtx != ctx {
		t.Error("a step outside a run changed the context")
	}
	_, call := none.ModelCall(ctx, "m", telmoni.ModelCallOptions{})
	call.Answered(telmoni.Answer{ResponseModel: "m2"})
	call.End()
	_, tool := none.ToolCall(ctx, "t", telmoni.ToolCallOptions{})
	tool.Returned(map[string]int{"a": 1})
	tool.Cancel()
	tool.End()
	h.flush()
	if sent := len(h.transport.all()); sent != 0 {
		t.Errorf("sent %d exports outside a run", sent)
	}
}

func TestTheSampleRateDropsWholeRunsInTheExporter(t *testing.T) {
	random := 0.9
	h := newHarness(t, telmoni.Config{APIKey: "k", SampleRate: 0.25}, nil,
		telmoni.WithRandom(func() float64 { return random }))
	ctx, out := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, step := out.Step(ctx, "s")
	step.End()
	out.End()
	random = 0.1
	_, kept := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	kept.End()
	_, keptAtHalf := h.client.Run(context.Background(), "bot", telmoni.RunOptions{SampleRate: 0.5})
	keptAtHalf.End()
	h.flush()
	spans := h.transport.spans(t)
	rates := map[string]any{}
	for _, span := range spans {
		attributes := attributesOf(t, span)
		rates[attributes["telmoni.run_id"].(string)] = attributes["telmoni.sample_rate"]
	}
	if want := map[string]any{kept.ID(): 0.25, keptAtHalf.ID(): 0.5}; !reflect.DeepEqual(rates, want) {
		t.Errorf("runs and rates sent\n got: %v\nwant: %v", rates, want)
	}
	want := []string{"invoke_agent bot", "invoke_agent bot", "telmoni.run.start", "telmoni.run.start"}
	if got := sortedNames(spans); !reflect.DeepEqual(got, want) {
		t.Errorf("spans sent: %v", got)
	}
}

func TestABlankAgentOpensNoRunAndNeverPanics(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), " ", telmoni.RunOptions{})
	// Every handle still works, and does nothing.
	_, step := run.Step(ctx, "plan")
	step.FailWith("x")
	step.End()
	_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{})
	call.Answered(telmoni.Answer{ResponseModel: "m2"})
	call.End()
	run.Fail(errors.New("x"))
	run.End()
	h.flush()
	if sent := len(h.transport.all()); sent != 0 {
		t.Errorf("a run that did not open sent %d exports", sent)
	}
	if run.ID() != "" || telmoni.RunFromContext(ctx) != nil {
		t.Errorf("a run that did not open has an id %q, or is in its context", run.ID())
	}
	warnings := h.logs.warnings()
	if len(warnings) != 1 || !strings.Contains(warnings[0], "agent's name") {
		t.Errorf("warnings = %q, want one naming the agent", warnings)
	}
}

func TestABadRunRateFallsBackToTheClientsWithOneWarning(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	for _, rate := range []float64{1.5, -0.5} {
		_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{SampleRate: rate})
		run.End()
	}
	h.flush()
	runs := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(runs) != 2 {
		t.Fatalf("want two runs sent at the client's rate, got %v", names(runs))
	}
	for _, span := range runs {
		if got := attributesOf(t, span)["telmoni.sample_rate"]; got != 1.0 {
			t.Errorf("telmoni.sample_rate = %v, want the client's 1", got)
		}
	}
	warnings := h.logs.warnings()
	if len(warnings) != 1 || !strings.Contains(warnings[0], "SampleRate") {
		t.Errorf("warnings = %q, want one about the rate", warnings)
	}
}
