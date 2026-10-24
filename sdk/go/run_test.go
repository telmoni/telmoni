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
		UserID:    "user_1",
		SessionID: "conv_1",
		Tags:      map[string]string{"env": "test", "team": "a"},
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
		"gen_ai.operation.name":  "invoke_agent",
		"gen_ai.agent.name":      "support-bot",
		"telmoni.run_id":         run.ID(),
		"telmoni.agent":          "support-bot",
		"user.id":                "user_1",
		"gen_ai.conversation.id": "conv_1",
		"telmoni.sample_rate":    1.0,
		"telmoni.tag.env":        "test",
		"telmoni.tag.team":       "a",
	}
	if got := attributesOf(t, span); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
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

// Ingest refuses a span over an empty error.type: here, the run.
func TestFailWithAnEmptyTypeFailsTheRunWithNone(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.FailWith("")
	run.End()
	h.flush()
	spans := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(spans) != 1 {
		t.Fatalf("%d runs sent, want 1", len(spans))
	}
	if _, has := attributesOf(t, spans[0])["error.type"]; has {
		t.Error("the run has an empty error.type")
	}
	if !reflect.DeepEqual(spans[0]["status"], map[string]any{"code": float64(2)}) {
		t.Errorf("status = %v, want {code: 2}", spans[0]["status"])
	}
}

// generic's type runs past ingest's 128 characters, as a generic's instance
// can: %T spells out its type argument.
type generic[T any] struct{}

func (generic[T]) Error() string { return "failed" }

// On every span of the run, any of these past its cap cost ingest the run
// whole.
func TestARunsTextPastIngestsCapsIsCutOrLeftOffWarnedOnce(t *testing.T) {
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
		_, run := h.client.Run(context.Background(), smiles(129), telmoni.RunOptions{
			UserID:    strings.Repeat("u", 129),
			SessionID: strings.Repeat("s", 201),
		})
		if got := run.Agent(); got != smiles(128) {
			t.Errorf("Agent() = %q, want it cut to 128 characters", got)
		}
		run.Fail(err)
		run.End()
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
	runs := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(runs) != 2 {
		t.Fatalf("want two runs, got %v", names(runs))
	}
	for _, span := range runs {
		attributes := attributesOf(t, span)
		if span["name"] != "invoke_agent "+smiles(128) || attributes["gen_ai.agent.name"] != smiles(128) {
			t.Errorf("the run's name and gen_ai.agent.name are not cut")
		}
		if attributes["error.type"] != errorType[:128] {
			t.Errorf("error.type = %v, want its first 128 characters", attributes["error.type"])
		}
	}
	warnings := h.logs.warnings()
	slices.Sort(warnings)
	want := []string{
		"telmoni: a run's agent must be up to 128 characters; it is sent cut to its first 128",
		"telmoni: a session id must be up to 200 characters; the run is sent without it",
		"telmoni: a span's error type must be up to 128 characters; it is sent cut to its first 128",
		"telmoni: a user id must be up to 128 characters; the run is sent without it",
	}
	if !reflect.DeepEqual(warnings, want) {
		t.Errorf("warnings = %q, want %q", warnings, want)
	}
}

// Counted in characters, as ingest counts them, never in UTF-8's bytes.
func TestARunsTextAtIngestsCapsIsSentWhole(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	smiles := func(n int) string { return strings.Repeat("😀", n) }
	_, run := h.client.Run(context.Background(), smiles(128), telmoni.RunOptions{UserID: smiles(128), SessionID: smiles(200)})
	run.FailWith(smiles(128))
	run.End()
	h.flush()
	attributes := attributesOf(t, runSpan(t, h))
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
	// The nil run's own marks and End do nothing either, rather than panic.
	none.Fail(errors.New("x"))
	none.FailWith("x")
	none.Cancel()
	none.End()
	h.flush()
	if sent := len(h.transport.all()); sent != 0 {
		t.Errorf("sent %d exports outside a run", sent)
	}
}

func TestTheSampleRateDropsWholeRunsInTheProcessor(t *testing.T) {
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
	want := []string{"invoke_agent bot", "invoke_agent bot"}
	if got := sortedNames(spans); !reflect.DeepEqual(got, want) {
		t.Errorf("spans sent: %v", got)
	}
}

func TestASpanEndingAfterItsSampledOutRunIsDroppedToo(t *testing.T) {
	random := 0.9
	h := newHarness(t, telmoni.Config{APIKey: "k", SampleRate: 0.25}, nil,
		telmoni.WithRandom(func() float64 { return random }))
	ctx, dropped := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, late := dropped.Step(ctx, "late")
	dropped.End()
	late.End()
	random = 0.1
	_, kept := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	kept.End()
	h.flush()
	sent := map[any][]string{}
	for _, span := range h.transport.spans(t) {
		id := attributesOf(t, span)["telmoni.run_id"]
		sent[id] = append(sent[id], span["name"].(string))
	}
	if len(sent) != 1 || len(sent[kept.ID()]) == 0 {
		t.Errorf("spans sent by run: %v; want the kept run's alone", sent)
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

// A run sends its first 32 tags by key with one warning, the marks it ends
// with kept.
func TestARunSendsItsFirst32TagsByKeyWithOneWarning(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, all := h.client.Run(context.Background(), "thirty-two", telmoni.RunOptions{Tags: numberedTags(32)})
	all.End()
	if warnings := h.logs.warnings(); len(warnings) != 0 {
		t.Errorf("warnings = %q, want none for 32 tags", warnings)
	}
	_, failed := h.client.Run(context.Background(), "bot", telmoni.RunOptions{Tags: numberedTags(130)})
	failed.FailWith("RateLimit")
	failed.End()
	_, stopped := h.client.Run(context.Background(), "stopped", telmoni.RunOptions{Tags: numberedTags(130)})
	stopped.Cancel()
	stopped.End()
	h.flush()
	runs := withPrefix(h.transport.spans(t), "invoke_agent ")
	if len(runs) != 3 {
		t.Fatalf("want three runs, got %v", names(runs))
	}
	want := slices.Sorted(maps.Keys(numberedTags(32)))
	for _, span := range runs {
		attributes := attributesOf(t, span)
		if got := tagsOf(attributes); !reflect.DeepEqual(got, want) {
			t.Errorf("%v sent the tags %v\nwant the first 32 by key: %v", span["name"], got, want)
		}
		if span["name"] == "invoke_agent bot" && attributes["error.type"] != "RateLimit" {
			t.Errorf("error.type = %v, want the mark the run ended with", attributes["error.type"])
		}
		if span["name"] == "invoke_agent stopped" && attributes["telmoni.cancelled"] != true {
			t.Errorf("telmoni.cancelled = %v, want the mark the run ended with", attributes["telmoni.cancelled"])
		}
	}
	wantWarnings := []string{"telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent"}
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
		_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{Tags: tags})
		run.End()
	}
	_, many := h.client.Run(context.Background(), "many", telmoni.RunOptions{Tags: numberedTags(40)})
	many.End()
	h.flush()
	runs := withPrefix(h.transport.spans(t), "invoke_agent bot")
	if len(runs) != 2 {
		t.Fatalf("want two runs, got %v", names(runs))
	}
	want := append(slices.Sorted(maps.Keys(numberedTags(31))), edge)
	for _, span := range runs {
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
		"telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent",
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
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{Tags: tags})
	_, step := run.Step(ctx, "s")
	step.End()
	run.End()
	h.flush()
	want := []string{"invoke_agent bot", "s"}
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, want) {
		t.Errorf("spans sent: %v, want %v", got, want)
	}
}
