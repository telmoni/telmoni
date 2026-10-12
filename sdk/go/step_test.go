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
	if got := sortedNames(spans); !reflect.DeepEqual(got, []string{"invoke_agent bot", "plan", "telmoni.run.start"}) {
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
