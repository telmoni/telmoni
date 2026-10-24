package telmoni_test

import (
	"context"
	"reflect"
	"strings"
	"testing"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

func TestAStepNestsUnderItsRunAndCarriesItsIDs(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{UserID: "c", SessionID: "v"})
	planCtx, plan := run.Step(ctx, "plan")
	_, inner := run.Step(planCtx, "plan.inner")
	inner.End()
	plan.End()
	run.End()
	h.flush()
	spans := byName(h.transport.spans(t))
	planSpan, innerSpan, agent := spans["plan"], spans["plan.inner"], spans["invoke_agent bot"]
	if planSpan == nil || innerSpan == nil || agent == nil {
		t.Fatalf("spans sent: %v", sortedNames(h.transport.spans(t)))
	}
	if planSpan["parentSpanId"] != agent["spanId"] {
		t.Error("the step is not the run's child")
	}
	if innerSpan["parentSpanId"] != planSpan["spanId"] {
		t.Error("the inner step is not the step's child")
	}
	if planSpan["traceId"] != agent["traceId"] || innerSpan["traceId"] != agent["traceId"] {
		t.Error("the steps left the run's trace")
	}
	want := map[string]any{
		"telmoni.run_id":         run.ID(),
		"telmoni.agent":          "bot",
		"user.id":                "c",
		"gen_ai.conversation.id": "v",
	}
	if got := attributesOf(t, planSpan); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
	if planSpan["kind"] != float64(1) {
		t.Errorf("kind = %v, want 1", planSpan["kind"])
	}
	if _, has := planSpan["status"]; has {
		t.Error("a step that went well has a status")
	}
}

func TestAStepFromAContextWithoutASpanGoesUnderTheRun(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	stepCtx, step := run.Step(context.Background(), "background")
	if telmoni.RunFromContext(stepCtx) != run {
		t.Error("the step's context does not carry its run")
	}
	step.End()
	run.End()
	h.flush()
	spans := byName(h.transport.spans(t))
	if spans["background"]["parentSpanId"] != spans["invoke_agent bot"]["spanId"] {
		t.Error("a step opened with no span in its context is not the run's child")
	}
	if spans["background"]["traceId"] != spans["invoke_agent bot"]["traceId"] {
		t.Error("the step started a trace of its own")
	}
}

func TestAFailingStepRecordsTheErrorTypeAndTheRunGoesOn(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, step := run.Step(ctx, "plan")
	step.Fail(&planningFailed{detail: "secret details"})
	step.End()
	run.End()
	h.flush()
	spans := byName(h.transport.spans(t))
	if !reflect.DeepEqual(spans["plan"]["status"], map[string]any{"code": float64(2)}) {
		t.Errorf("status = %v, want {code: 2}", spans["plan"]["status"])
	}
	if got := attributesOf(t, spans["plan"])["error.type"]; got != "telmoni_test.planningFailed" {
		t.Errorf("error.type = %v", got)
	}
	if _, has := spans["invoke_agent bot"]["status"]; has {
		t.Error("the run itself did not go on")
	}
	if strings.Contains(h.transport.text(), "secret details") {
		t.Error("the error's message was sent")
	}
}

func TestAFunctionWrappedInAStep(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	plan := func(ctx context.Context, n int) int {
		_, step := telmoni.RunFromContext(ctx).Step(ctx, "plan")
		defer step.End()
		return n + 1
	}
	if got := plan(ctx, 1); got != 2 {
		t.Errorf("plan(1) = %d", got)
	}
	run.End()
	h.flush()
	if _, has := byName(h.transport.spans(t))["plan"]; !has {
		t.Error("the step was not sent")
	}
}

func TestASecondEndAndALateMarkDoNothing(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, step := run.Step(ctx, "plan")
	step.End()
	step.End()
	step.FailWith("Late")
	step.Cancel()
	run.End()
	run.End()
	h.flush()
	spans := h.transport.spans(t)
	if got := sortedNames(spans); !reflect.DeepEqual(got, []string{"invoke_agent bot", "plan"}) {
		t.Errorf("spans sent: %v", got)
	}
	attributes := attributesOf(t, byName(spans)["plan"])
	if _, has := attributes["error.type"]; has {
		t.Error("a mark after End was recorded")
	}
	if _, has := attributes["telmoni.cancelled"]; has {
		t.Error("a cancel after End was recorded")
	}
}

// Ingest refuses a span named nothing, or past 145 characters, each invalid
// byte one of them, which JSON sends as U+FFFD.
func TestAStepNamedNothingOrPastIngestsCapKeepsItsSpan(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	n := func(count int) string { return strings.Repeat("n", count) }
	for _, name := range []string{"", "", n(146), n(146), n(145), strings.Repeat("\xff", 146)} {
		_, step := run.Step(ctx, name)
		step.End()
	}
	run.End()
	h.flush()
	want := []string{"invoke_agent bot", n(145), n(145), n(145), "step", "step", strings.Repeat("�", 145)}
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, want) {
		t.Errorf("spans sent: %q", got)
	}
	warnings := []string{
		"telmoni: a step needs its name; one without is sent named step",
		"telmoni: a step's name must be up to 145 characters; it is sent cut to its first 145",
	}
	if got := h.logs.warnings(); !reflect.DeepEqual(got, warnings) {
		t.Errorf("warnings = %q, want %q", got, warnings)
	}
}
