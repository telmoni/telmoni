package telmoni_test

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"maps"
	"net"
	"reflect"
	"slices"
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

// traceSpan is the one agent's span sent: the trace's own.
func traceSpan(t *testing.T, h *harness) map[string]any {
	t.Helper()
	agents := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(agents) != 1 {
		t.Fatalf("want one agent span, got %v", names(agents))
	}
	return agents[0]
}

func TestATraceIsASpanWithTheContractAttributes(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, tr := h.client.Trace(context.Background(), "support-bot", telmoni.TraceOptions{
		UserID:    "user_1",
		SessionID: "conv_1",
		Tags:      map[string]string{"env": "test", "team": "a"},
	})
	if !hex32.MatchString(tr.TraceID()) {
		t.Errorf("trace id %q is not 32 lowercase hex digits", tr.TraceID())
	}
	if tr.Agent() != "support-bot" {
		t.Errorf("Agent() = %q", tr.Agent())
	}
	if telmoni.TraceFromContext(ctx) != tr {
		t.Error("the trace is not found from the context it returned")
	}
	spanID := trace.SpanContextFromContext(ctx).SpanID().String()
	tr.End()
	if telmoni.TraceFromContext(context.Background()) != nil {
		t.Error("a trace was found in a context without one")
	}
	h.flush()
	span := traceSpan(t, h)
	if span["kind"] != float64(1) {
		t.Errorf("kind = %v, want 1", span["kind"])
	}
	if span["spanId"] != spanID {
		t.Errorf("the context carries span %s, the agent's is %v", spanID, span["spanId"])
	}
	if span["traceId"] != tr.TraceID() {
		t.Errorf("the agent's span is in trace %v, TraceID() says %s", span["traceId"], tr.TraceID())
	}
	if _, has := span["parentSpanId"]; has {
		t.Error("a trace opened with no span in its context has a parent")
	}
	if _, has := span["status"]; has {
		t.Error("a trace that went well has a status")
	}
	want := map[string]any{
		"gen_ai.operation.name":  "invoke_agent",
		"gen_ai.agent.name":      "support-bot",
		"telmoni.agent":          "support-bot",
		"user.id":                "user_1",
		"gen_ai.conversation.id": "conv_1",
		"telmoni.tag.env":        "test",
		"telmoni.tag.team":       "a",
	}
	if got := attributesOf(t, span); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
}

func TestAnErrorFailsTheTraceWithItsTypeAlone(t *testing.T) {
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
			_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
			tr.Fail(tc.err)
			tr.End()
			h.flush()
			span := traceSpan(t, h)
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

func TestFailWithAndCancelMarkTheTrace(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, failed := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	failed.FailWith("RateLimit")
	failed.End()
	_, cancelled := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	cancelled.Cancel()
	cancelled.End()
	h.flush()
	for _, span := range withPrefix(h.transport.spans(t), "invoke_agent ") {
		attributes := attributesOf(t, span)
		switch span["traceId"] {
		case failed.TraceID():
			if attributes["error.type"] != "RateLimit" {
				t.Errorf("error.type = %v, want RateLimit", attributes["error.type"])
			}
			if !reflect.DeepEqual(span["status"], map[string]any{"code": float64(2)}) {
				t.Errorf("status = %v, want {code: 2}", span["status"])
			}
		case cancelled.TraceID():
			if attributes["telmoni.cancelled"] != true {
				t.Errorf("telmoni.cancelled = %v, want true", attributes["telmoni.cancelled"])
			}
			if _, has := span["status"]; has {
				t.Error("a cancelled trace has a status")
			}
			if _, has := attributes["error.type"]; has {
				t.Error("a cancelled trace has an error.type")
			}
		default:
			t.Errorf("an agent span of an unknown trace: %v", span["traceId"])
		}
	}
}

// Ingest refuses a span over an empty error.type: here, the agent's.
func TestFailWithAnEmptyTypeFailsTheTraceWithNone(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	tr.FailWith("")
	tr.End()
	h.flush()
	spans := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(spans) != 1 {
		t.Fatalf("%d agent spans sent, want 1", len(spans))
	}
	if _, has := attributesOf(t, spans[0])["error.type"]; has {
		t.Error("the agent's span has an empty error.type")
	}
	if !reflect.DeepEqual(spans[0]["status"], map[string]any{"code": float64(2)}) {
		t.Errorf("status = %v, want {code: 2}", spans[0]["status"])
	}
}

// generic's type runs past ingest's 128 characters, as a generic's instance
// can: %T spells out its type argument.
type generic[T any] struct{}

func (generic[T]) Error() string { return "failed" }

// On every span of the trace, any of these past its cap cost ingest the trace
// whole.
func TestATracesTextPastIngestsCapsIsCutOrLeftOffWarnedOnce(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	smiles := func(n int) string { return strings.Repeat("😀", n) }
	err := generic[struct {
		TheFirstFieldOfALongTypeName, TheSecondFieldOfALongTypeName int
		TheThirdFieldOfALongTypeName, TheFourthFieldOfALongTypeName int
	}]{}
	errorType := fmt.Sprintf("%T", err)
	if len(errorType) <= 128 {
		t.Fatalf("%q is within the cap", errorType)
	}
	for range 2 {
		_, tr := h.client.Trace(context.Background(), smiles(129), telmoni.TraceOptions{
			UserID:    strings.Repeat("u", 129),
			SessionID: strings.Repeat("s", 201),
		})
		if got := tr.Agent(); got != smiles(128) {
			t.Errorf("Agent() = %q, want it cut to 128 characters", got)
		}
		tr.Fail(err)
		tr.End()
	}
	h.flush()
	for _, span := range h.transport.spans(t) {
		attributes := attributesOf(t, span)
		if attributes["telmoni.agent"] != smiles(128) {
			t.Errorf("%v carries telmoni.agent uncut", span["name"])
		}
		for _, key := range []string{"user.id", "gen_ai.conversation.id"} {
			if _, has := attributes[key]; has {
				t.Errorf("%v carries %s past its cap", span["name"], key)
			}
		}
	}
	agents := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(agents) != 2 {
		t.Fatalf("want two agent spans, got %v", names(agents))
	}
	for _, span := range agents {
		attributes := attributesOf(t, span)
		if span["name"] != "invoke_agent "+smiles(128) || attributes["gen_ai.agent.name"] != smiles(128) {
			t.Errorf("the agent's name and gen_ai.agent.name are not cut")
		}
		if attributes["error.type"] != errorType[:128] {
			t.Errorf("error.type = %v, want its first 128 characters", attributes["error.type"])
		}
	}
	warnings := h.logs.warnings()
	slices.Sort(warnings)
	want := []string{
		"telmoni: a session id must be up to 200 characters; the trace is sent without it",
		"telmoni: a span's error type must be up to 128 characters; it is sent cut to its first 128",
		"telmoni: a trace's agent must be up to 128 characters; it is sent cut to its first 128",
		"telmoni: a user id must be up to 128 characters; the trace is sent without it",
	}
	if !reflect.DeepEqual(warnings, want) {
		t.Errorf("warnings = %q, want %q", warnings, want)
	}
}

// Counted in characters, as ingest counts them, never in UTF-8's bytes.
func TestATracesTextAtIngestsCapsIsSentWhole(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	smiles := func(n int) string { return strings.Repeat("😀", n) }
	_, tr := h.client.Trace(context.Background(), smiles(128), telmoni.TraceOptions{UserID: smiles(128), SessionID: smiles(200)})
	tr.FailWith(smiles(128))
	tr.End()
	h.flush()
	attributes := attributesOf(t, traceSpan(t, h))
	for key, want := range map[string]string{
		"telmoni.agent":          smiles(128),
		"user.id":                smiles(128),
		"gen_ai.conversation.id": smiles(200),
		"error.type":             smiles(128),
	} {
		if attributes[key] != want {
			t.Errorf("%s is not as given", key)
		}
	}
	if warnings := h.logs.warnings(); len(warnings) != 0 {
		t.Errorf("warnings = %q, want none", warnings)
	}
}

// A deadline that ran out is a failure, not a cancellation, as the other SDKs
// fail a timeout with its type.
func TestADeadlineThatRanOutIsAFailure(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	tr.Fail(context.DeadlineExceeded)
	tr.End()
	h.flush()
	span := traceSpan(t, h)
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
			_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
			tr.Fail(err)
			tr.End()
			h.flush()
			span := traceSpan(t, h)
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

func TestAFunctionOpensATracePerCall(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	var seen []string
	handle := func(ctx context.Context, question string) string {
		ctx, tr := h.client.Trace(ctx, "bot", telmoni.TraceOptions{})
		defer tr.End()
		seen = append(seen, telmoni.TraceFromContext(ctx).TraceID())
		return strings.ToUpper(question)
	}
	if got := handle(context.Background(), "a"); got != "A" {
		t.Errorf("handle(a) = %q", got)
	}
	if got := handle(context.Background(), "b"); got != "B" {
		t.Errorf("handle(b) = %q", got)
	}
	h.flush()
	if agents := withPrefix(h.transport.spans(t), "invoke_agent bot"); len(agents) != 2 {
		t.Errorf("want two traces, got %d", len(agents))
	}
	if len(seen) != 2 || seen[0] == seen[1] {
		t.Errorf("want two distinct trace ids, got %v", seen)
	}
}

func TestTracesAndStepsCrossGoroutines(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	var threaded string
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		ctx, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
		defer tr.End()
		threaded = tr.TraceID()
		var inner sync.WaitGroup
		inner.Add(1)
		go func() {
			defer inner.Done()
			_, step := telmoni.TraceFromContext(ctx).Step(ctx, "think")
			step.End()
		}()
		inner.Wait()
	}()
	_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	tr.End()
	wg.Wait()
	h.flush()
	spans := h.transport.spans(t)
	if agents := withPrefix(spans, "invoke_agent bot"); len(agents) != 2 {
		t.Errorf("want two traces, got %d", len(agents))
	}
	step := byName(spans)["think"]
	if step == nil {
		t.Fatal("the step from the goroutine was not sent")
	}
	if got := step["traceId"]; got != threaded {
		t.Errorf("the step is in trace %v, want %s", got, threaded)
	}
}

// A nil handle of the caller's own, a deferred End on a call never opened
// say, does nothing, whichever method it is given.
func TestANilHandleDoesNothing(t *testing.T) {
	var (
		tr   *telmoni.Trace
		step *telmoni.Step
		call *telmoni.ModelCall
		tool *telmoni.ToolCall
	)
	for _, nothing := range []interface {
		End()
		Fail(error)
		FailWith(string)
		Cancel()
	}{tr, step, call, tool} {
		nothing.End()
		nothing.Fail(errors.New("x"))
		nothing.FailWith("x")
		nothing.Cancel()
	}
}

func TestNothingIsSentOutsideATrace(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx := context.Background()
	none := telmoni.TraceFromContext(ctx)
	if none != nil {
		t.Fatal("a trace was found outside one")
	}
	if none.TraceID() != "" || none.Agent() != "" {
		t.Error("a nil trace has an id or an agent")
	}
	stepCtx, step := none.Step(ctx, "alone")
	step.Fail(errors.New("x"))
	step.End()
	if stepCtx != ctx {
		t.Error("a step outside a trace changed the context")
	}
	_, call := none.ModelCall(ctx, "m", telmoni.ModelCallOptions{})
	call.Answered(telmoni.Answer{ResponseModel: "m2"})
	call.End()
	_, tool := none.ToolCall(ctx, "t", telmoni.ToolCallOptions{})
	tool.Returned(map[string]int{"a": 1})
	tool.Cancel()
	tool.End()
	// The nil trace's own marks and End do nothing either, rather than panic.
	none.Fail(errors.New("x"))
	none.FailWith("x")
	none.Cancel()
	none.End()
	h.flush()
	if sent := len(h.transport.all()) + len(h.transport.allOpenings()); sent != 0 {
		t.Errorf("sent %d exports outside a trace", sent)
	}
}

func TestABlankAgentOpensNoTraceAndNeverPanics(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, tr := h.client.Trace(context.Background(), " ", telmoni.TraceOptions{})
	// Every handle still works, and does nothing.
	_, step := tr.Step(ctx, "plan")
	step.FailWith("x")
	step.End()
	_, call := tr.ModelCall(ctx, "m", telmoni.ModelCallOptions{})
	call.Answered(telmoni.Answer{ResponseModel: "m2"})
	call.End()
	tr.Fail(errors.New("x"))
	tr.End()
	h.flush()
	if sent := len(h.transport.all()) + len(h.transport.allOpenings()); sent != 0 {
		t.Errorf("a trace that did not open sent %d exports", sent)
	}
	if tr.TraceID() != "" || telmoni.TraceFromContext(ctx) != nil {
		t.Errorf("a trace that did not open has an id %q, or is in its context", tr.TraceID())
	}
	warnings := h.logs.warnings()
	if len(warnings) != 1 || !strings.Contains(warnings[0], "agent's name") {
		t.Errorf("warnings = %q, want one naming the agent", warnings)
	}
}

// A trace sends its first 32 tags by key with one warning, the marks it ends
// with kept.
func TestATraceSendsItsFirst32TagsByKeyWithOneWarning(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, all := h.client.Trace(context.Background(), "thirty-two", telmoni.TraceOptions{Tags: numberedTags(32)})
	all.End()
	if warnings := h.logs.warnings(); len(warnings) != 0 {
		t.Errorf("warnings = %q, want none for 32 tags", warnings)
	}
	_, failed := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{Tags: numberedTags(130)})
	failed.FailWith("RateLimit")
	failed.End()
	_, stopped := h.client.Trace(context.Background(), "stopped", telmoni.TraceOptions{Tags: numberedTags(130)})
	stopped.Cancel()
	stopped.End()
	h.flush()
	agents := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(agents) != 3 {
		t.Fatalf("want three traces, got %v", names(agents))
	}
	want := slices.Sorted(maps.Keys(numberedTags(32)))
	for _, span := range agents {
		attributes := attributesOf(t, span)
		if got := tagsOf(attributes); !reflect.DeepEqual(got, want) {
			t.Errorf("%v sent the tags %v\nwant the first 32 by key: %v", span["name"], got, want)
		}
		if span["name"] == "invoke_agent bot" && attributes["error.type"] != "RateLimit" {
			t.Errorf("error.type = %v, want the mark the trace ended with", attributes["error.type"])
		}
		if span["name"] == "invoke_agent stopped" && attributes["telmoni.cancelled"] != true {
			t.Errorf("telmoni.cancelled = %v, want the mark the trace ended with", attributes["telmoni.cancelled"])
		}
	}
	wantWarnings := []string{"telmoni: a trace carries up to 32 tags, the most ingest keeps; the rest are not sent"}
	if warnings := h.logs.warnings(); !reflect.DeepEqual(warnings, wantWarnings) {
		t.Errorf("warnings = %q, want %q", warnings, wantWarnings)
	}
}

// A tag ingest drops is left off with one warning, taking none of the 32.
func TestATagIngestDropsIsLeftOffWithOneWarningTakingNoneOfThe32(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	// At the caps in characters, two bytes each: kept.
	edge, value := strings.Repeat("é", 64), strings.Repeat("é", 256)
	tags := numberedTags(31)
	tags[edge] = value
	maps.Copy(tags, droppedTags())
	for range 2 {
		_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{Tags: tags})
		tr.End()
	}
	_, many := h.client.Trace(context.Background(), "many", telmoni.TraceOptions{Tags: numberedTags(40)})
	many.End()
	h.flush()
	agents := withPrefix(h.transport.spans(t), "invoke_agent bot")
	if len(agents) != 2 {
		t.Fatalf("want two traces, got %v", names(agents))
	}
	want := append(slices.Sorted(maps.Keys(numberedTags(31))), edge)
	for _, span := range agents {
		attributes := attributesOf(t, span)
		if got := tagsOf(attributes); !reflect.DeepEqual(got, want) {
			t.Errorf("tags sent: %q\nwant the 32 ingest keeps: %q", got, want)
		}
		if attributes["telmoni.tag."+edge] != value {
			t.Error("the tag at the caps is not sent as given")
		}
	}
	wantWarnings := []string{
		"telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent",
		"telmoni: a trace carries up to 32 tags, the most ingest keeps; the rest are not sent",
	}
	if warnings := h.logs.warnings(); !reflect.DeepEqual(warnings, wantWarnings) {
		t.Errorf("warnings = %q\nwant one for the tags dropped, then one past the 32: %q", warnings, wantWarnings)
	}
}

// A logger that panics on a warning never reaches the agent's call.
func TestALoggerThatPanicsOnAWarningNeverReachesTheAgentsCall(t *testing.T) {
	h := newHarness(t, testConfig(), nil, telmoni.WithLogger(slog.New(panicking{})))
	defer func() {
		if r := recover(); r != nil {
			t.Fatalf("the logger's panic reached the agent's call: %v", r)
		}
	}()
	// Two warnings on this call: a tag dropped, and the tags past the 32.
	tags := numberedTags(40)
	tags[""] = "v"
	ctx, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{Tags: tags})
	_, step := tr.Step(ctx, "s")
	step.End()
	tr.End()
	h.flush()
	want := []string{"invoke_agent bot", "s"}
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, want) {
		t.Errorf("spans sent: %v, want %v", got, want)
	}
}

func TestATracesOwnMessagesLeaveOnlyWhenAskedAndTheProjectAllows(t *testing.T) {
	on := contentOn()
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true}, newFakeTransport(t, on, on))
	_, warm := h.client.Trace(context.Background(), "warm", telmoni.TraceOptions{})
	warm.End()
	h.flush()
	_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{InputMessages: []map[string]string{{"role": "user", "content": "hi"}}})
	tr.Answered(telmoni.TraceReply{OutputMessages: []map[string]string{{"role": "assistant", "content": "hello"}}})
	tr.End()
	h.flush()
	sent := attributesOf(t, byName(h.transport.spans(t))["invoke_agent bot"])
	if sent["gen_ai.input.messages"] != `[{"content":"hi","role":"user"}]` {
		t.Errorf("gen_ai.input.messages = %v", sent["gen_ai.input.messages"])
	}
	if sent["gen_ai.output.messages"] != `[{"content":"hello","role":"assistant"}]` {
		t.Errorf("gen_ai.output.messages = %v", sent["gen_ai.output.messages"])
	}
}

func TestATracesOwnMessagesStayHomeWithoutCaptureContentEvenWhenAllowed(t *testing.T) {
	on := contentOn()
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, on, on))
	_, warm := h.client.Trace(context.Background(), "warm", telmoni.TraceOptions{})
	warm.End()
	h.flush()
	if mode := h.client.ContentMode(); mode != "on" {
		t.Fatalf("the content mode is %q, want on", mode)
	}
	_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{InputMessages: "never asked"})
	tr.Answered(telmoni.TraceReply{OutputMessages: "never answered"})
	tr.End()
	h.flush()
	if text := h.transport.text(); strings.Contains(text, "never asked") || strings.Contains(text, "never answered") {
		t.Error("a trace's content left without CaptureContent")
	}
}

func TestAFailureLeavesWithItsMessageUnderBothLocks(t *testing.T) {
	on := contentOn()
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true}, newFakeTransport(t, on, on))
	_, warm := h.client.Trace(context.Background(), "warm", telmoni.TraceOptions{})
	warm.End()
	h.flush()
	ctx, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	_, step := tr.Step(ctx, "plan")
	step.Fail(&planningFailed{detail: "bad plan"})
	step.End()
	tr.FailWith("RateLimit")
	tr.End()
	h.flush()
	spans := byName(h.transport.spans(t))
	plan := spans["plan"]
	if !reflect.DeepEqual(plan["status"], map[string]any{"code": 2.0, "message": "bad plan"}) {
		t.Errorf("status = %v", plan["status"])
	}
	events := plan["events"].([]any)
	if len(events) != 1 {
		t.Fatalf("events = %v", events)
	}
	event := events[0].(map[string]any)
	sent := attributesOf(t, event)
	want := map[string]any{"exception.type": "telmoni_test.planningFailed", "exception.message": "bad plan"}
	if event["name"] != "exception" || !reflect.DeepEqual(sent, want) {
		t.Errorf("event = %v", event)
	}
	// A type alone carries no message to record.
	if agent := spans["invoke_agent bot"]; !reflect.DeepEqual(agent["status"], map[string]any{"code": 2.0}) || agent["events"] != nil {
		t.Errorf("a type alone sent %v", agent)
	}
}

func TestAFailuresMessageStaysHomeWithoutCaptureContent(t *testing.T) {
	on := contentOn()
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, on, on))
	_, warm := h.client.Trace(context.Background(), "warm", telmoni.TraceOptions{})
	warm.End()
	h.flush()
	_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	tr.Fail(&planningFailed{detail: "bad plan"})
	tr.End()
	h.flush()
	agent := byName(h.transport.spans(t))["invoke_agent bot"]
	if !reflect.DeepEqual(agent["status"], map[string]any{"code": 2.0}) || agent["events"] != nil {
		t.Errorf("sent %v", agent)
	}
	if strings.Contains(h.transport.text(), "bad plan") {
		t.Error("a failure's message left without CaptureContent")
	}
}

func TestAnApplicationsEventLeavesOnlyUnderBothLocks(t *testing.T) {
	for _, capture := range []bool{true, false} {
		on := contentOn()
		h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: capture}, newFakeTransport(t, on, on))
		_, warm := h.client.Trace(context.Background(), "warm", telmoni.TraceOptions{})
		warm.End()
		h.flush()
		ctx, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
		trace.SpanFromContext(ctx).AddEvent("cache miss")
		tr.End()
		h.flush()
		_, sent := byName(h.transport.spans(t))["invoke_agent bot"]["events"]
		if sent != capture || strings.Contains(h.transport.text(), "cache miss") != capture {
			t.Errorf("with CaptureContent %v, the event was sent: %v", capture, sent)
		}
	}
}

func TestATracesAnswerAfterItEndedIsNotSentWithOneWarning(t *testing.T) {
	on := contentOn()
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true}, newFakeTransport(t, on, on))
	_, warm := h.client.Trace(context.Background(), "warm", telmoni.TraceOptions{})
	warm.End()
	h.flush()
	_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	tr.End()
	for range 2 {
		tr.Answered(telmoni.TraceReply{OutputMessages: "too late"})
	}
	h.flush()
	if strings.Contains(h.transport.text(), "too late") {
		t.Error("an answer given after the trace ended was sent")
	}
	want := []string{"telmoni: a trace must be answered before it ends; an answer given after is not sent"}
	if got := h.logs.warnings(); !slices.Equal(got, want) {
		t.Errorf("warned %q", got)
	}
}
