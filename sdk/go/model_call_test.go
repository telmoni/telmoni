package telmoni_test

import (
	"context"
	"errors"
	"math"
	"net"
	"reflect"
	"slices"
	"strings"
	"testing"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

func calls(t *testing.T, h *harness) []map[string]any {
	t.Helper()
	return withPrefix(h.transport.spans(t), "chat ")
}

// contentOn is an answer naming the project's content mode on.
func contentOn() scripted {
	return scripted{
		status:  200,
		headers: map[string]string{"Telmoni-Content-Mode": "on"},
		body:    []byte(`{"partialSuccess":null}`),
	}
}

func TestAModelCallCarriesTheConventionsNames(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{SessionID: "conv"})
	_, call := run.ModelCall(ctx, "gpt-4o-mini", telmoni.ModelCallOptions{Provider: "openai"})
	call.Answered(telmoni.Answer{
		ResponseModel: "gpt-4o-mini-2024-07-18",
		Usage:         &telmoni.Usage{InputTokens: 12, OutputTokens: 7, CacheReadInputTokens: 3},
	})
	call.End()
	run.End()
	h.flush()
	spans := calls(t, h)
	if len(spans) != 1 {
		t.Fatalf("want one model call, got %v", names(spans))
	}
	span := spans[0]
	if span["name"] != "chat gpt-4o-mini" || span["kind"] != float64(3) {
		t.Errorf("name %v kind %v, want chat gpt-4o-mini of kind 3", span["name"], span["kind"])
	}
	// Go has no None: the four counts of a given Usage travel, a zero among
	// them.
	want := map[string]any{
		"telmoni.run_id":                        run.ID(),
		"telmoni.agent":                         "bot",
		"gen_ai.conversation.id":                "conv",
		"gen_ai.operation.name":                 "chat",
		"gen_ai.request.model":                  "gpt-4o-mini",
		"gen_ai.provider.name":                  "openai",
		"gen_ai.response.model":                 "gpt-4o-mini-2024-07-18",
		"gen_ai.usage.input_tokens":             int64(12),
		"gen_ai.usage.output_tokens":            int64(7),
		"gen_ai.usage.cache_read.input_tokens":  int64(3),
		"gen_ai.usage.cache_write.input_tokens": int64(0),
	}
	if got := attributesOf(t, span); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
}

func TestAnswerWithoutUsageOrModelSetsNeither(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{})
	call.Answered(telmoni.Answer{})
	call.End()
	run.End()
	h.flush()
	want := map[string]any{
		"telmoni.run_id":        run.ID(),
		"telmoni.agent":         "bot",
		"gen_ai.operation.name": "chat",
		"gen_ai.request.model":  "m",
	}
	if got := attributesOf(t, calls(t, h)[0]); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
}

// A token count below zero is left off its call, with one warning per count.
func TestATokenCountBelowZeroIsLeftOffItsCallWithOneWarning(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for range 2 {
		_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{})
		call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: -1, OutputTokens: 7, CacheReadInputTokens: math.MinInt}})
		call.End()
	}
	run.End()
	h.flush()
	spans := calls(t, h)
	if len(spans) != 2 {
		t.Fatalf("want two model calls, got %v", sortedNames(h.transport.spans(t)))
	}
	want := map[string]any{
		"telmoni.run_id":                        run.ID(),
		"telmoni.agent":                         "bot",
		"gen_ai.operation.name":                 "chat",
		"gen_ai.request.model":                  "m",
		"gen_ai.usage.output_tokens":            int64(7),
		"gen_ai.usage.cache_write.input_tokens": int64(0),
	}
	for _, span := range spans {
		if got := attributesOf(t, span); !reflect.DeepEqual(got, want) {
			t.Errorf("attributes\n got: %v\nwant: %v", got, want)
		}
	}
	wantWarnings := []string{
		"telmoni: gen_ai.usage.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
		"telmoni: gen_ai.usage.cache_read.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
	}
	if warnings := h.logs.warnings(); !reflect.DeepEqual(warnings, wantWarnings) {
		t.Errorf("warnings = %q\nwant one for each count: %q", warnings, wantWarnings)
	}
}

// A blank operation is chat, as one left out is: sent blank, the span would
// be named " m" and ingest would read it as a step.
func TestABlankOperationIsChat(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for _, blank := range []string{"", "  "} {
		_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{Operation: blank})
		call.End()
	}
	run.End()
	h.flush()
	spans := calls(t, h)
	if len(spans) != 2 {
		t.Fatalf("want two chat calls, got %v", sortedNames(h.transport.spans(t)))
	}
	for _, span := range spans {
		if span["name"] != "chat m" {
			t.Errorf("name = %q, want chat m", span["name"])
		}
		if got := attributesOf(t, span)["gen_ai.operation.name"]; got != "chat" {
			t.Errorf("gen_ai.operation.name = %v, want chat", got)
		}
	}
}

func TestContentIsTakenOffBeforeItLeaves(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{
		InputMessages:      []map[string]string{{"role": "user", "content": "my secret prompt"}},
		SystemInstructions: "be brief",
	})
	call.Answered(telmoni.Answer{OutputMessages: []map[string]string{{"role": "assistant", "content": "the answer"}}})
	call.End()
	run.End()
	h.flush()
	attributes := attributesOf(t, calls(t, h)[0])
	for _, key := range []string{"gen_ai.input.messages", "gen_ai.output.messages", "gen_ai.system_instructions"} {
		if _, has := attributes[key]; has {
			t.Errorf("%s left the process", key)
		}
	}
	for _, secret := range []string{"my secret prompt", "the answer", "be brief"} {
		if strings.Contains(h.transport.text(), secret) {
			t.Errorf("%q left the process", secret)
		}
	}
}

func TestContentLeavesOnlyWhenAskedAndTheProjectAllows(t *testing.T) {
	on := contentOn()
	// An empty run's export is answered off, the next two runs' on; each run
	// flushed before the next opens, the mode each call is encoded under is the
	// same on every machine.
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true}, newFakeTransport(t, kept(t), on, on))
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	h.flush()
	if mode := h.client.ContentMode(); mode != "off" {
		t.Fatalf("the content mode is %q after the empty run, want off", mode)
	}
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{InputMessages: "first"})
	call.End()
	run.End()
	h.flush()
	if mode := h.client.ContentMode(); mode != "on" {
		t.Fatalf("the content mode is %q after the first call's run, want on", mode)
	}
	ctx, run = h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, call = run.ModelCall(ctx, "m", telmoni.ModelCallOptions{
		InputMessages:      "second",
		SystemInstructions: []string{"be brief"},
	})
	call.Answered(telmoni.Answer{OutputMessages: map[string]any{"role": "assistant", "content": "<b>hi</b>"}})
	call.End()
	run.End()
	h.flush()
	spans := calls(t, h)
	if len(spans) != 2 {
		t.Fatalf("want two model calls, got %v", names(spans))
	}
	if _, has := attributesOf(t, spans[0])["gen_ai.input.messages"]; has {
		t.Error("content left while the project's mode was off")
	}
	second := attributesOf(t, spans[1])
	if second["gen_ai.input.messages"] != "second" {
		t.Errorf("gen_ai.input.messages = %v, want the string as given", second["gen_ai.input.messages"])
	}
	if second["gen_ai.system_instructions"] != `["be brief"]` {
		t.Errorf("gen_ai.system_instructions = %v, want JSON", second["gen_ai.system_instructions"])
	}
	if second["gen_ai.output.messages"] != `{"content":"<b>hi</b>","role":"assistant"}` {
		t.Errorf("gen_ai.output.messages = %v, want JSON with its brackets as written", second["gen_ai.output.messages"])
	}
}

func TestContentStaysHomeWithoutCaptureContentEvenWhenAllowed(t *testing.T) {
	on := contentOn()
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, on, on))
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	h.flush()
	if mode := h.client.ContentMode(); mode != "on" {
		t.Fatalf("the content mode is %q, want on", mode)
	}
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{InputMessages: "never"})
	call.End()
	run.End()
	h.flush()
	if strings.Contains(h.transport.text(), "never") {
		t.Error("content left without CaptureContent")
	}
}

func TestAFailedCallRecordsTheErrorType(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{Operation: "generate_content"})
	call.Fail(&net.OpError{Op: "dial", Net: "tcp", Err: errors.New("provider down")})
	call.End()
	run.End()
	h.flush()
	span := byName(h.transport.spans(t))["generate_content m"]
	if span == nil {
		t.Fatalf("spans sent: %v", sortedNames(h.transport.spans(t)))
	}
	if !reflect.DeepEqual(span["status"], map[string]any{"code": float64(2)}) {
		t.Errorf("status = %v, want {code: 2}", span["status"])
	}
	if got := attributesOf(t, span)["error.type"]; got != "net.OpError" {
		t.Errorf("error.type = %v", got)
	}
	if strings.Contains(h.transport.text(), "provider down") {
		t.Error("the error's message was sent")
	}
}

// Ingest refuses a call over an empty model, or text past its cap.
func TestAModelCallsTextPastIngestsCapsKeepsItsCall(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	m := func(n int) string { return strings.Repeat("m", n) }
	for range 2 {
		_, call := run.ModelCall(ctx, "", telmoni.ModelCallOptions{})
		call.End()
		_, call = run.ModelCall(ctx, m(129), telmoni.ModelCallOptions{Provider: strings.Repeat("p", 129), Operation: "generate_content"})
		call.Answered(telmoni.Answer{ResponseModel: strings.Repeat("r", 129)})
		call.End()
		_, call = run.ModelCall(ctx, m(128), telmoni.ModelCallOptions{Operation: strings.Repeat("o", 20)})
		call.End()
	}
	// At their caps: the longest operation and a model at its cap fit.
	smiles := strings.Repeat("😀", 128)
	_, call := run.ModelCall(ctx, smiles, telmoni.ModelCallOptions{Operation: "generate_content"})
	call.Answered(telmoni.Answer{ResponseModel: smiles})
	call.End()
	run.End()
	h.flush()
	sent := map[string]map[string]any{}
	for _, span := range h.transport.spans(t) {
		if name := span["name"].(string); name != "invoke_agent bot" {
			sent[name] = attributesOf(t, span)
		}
	}
	want := []string{"chat", "generate_content " + m(128), "generate_content " + smiles, (strings.Repeat("o", 20) + " " + m(128))[:145]}
	got := make([]string, 0, len(sent))
	for name := range sent {
		got = append(got, name)
	}
	slices.Sort(got)
	slices.Sort(want)
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("calls sent: %q", got)
	}
	if _, has := sent["chat"]["gen_ai.request.model"]; has {
		t.Error("a call with no model carries an empty one")
	}
	cut := sent["generate_content "+m(128)]
	for key, value := range map[string]string{
		"gen_ai.request.model":  m(128),
		"gen_ai.provider.name":  strings.Repeat("p", 128),
		"gen_ai.response.model": strings.Repeat("r", 128),
	} {
		if cut[key] != value {
			t.Errorf("%s is not cut to its 128 characters", key)
		}
	}
	if sent["generate_content "+smiles]["gen_ai.response.model"] != smiles {
		t.Error("a response model at its cap is not sent whole")
	}
	warnings := h.logs.warnings()
	slices.Sort(warnings)
	wantWarnings := []string{
		"telmoni: a model call needs its model; one without is sent named by its operation alone",
		"telmoni: a model call's name must be up to 145 characters; it is sent cut to its first 145",
		"telmoni: a model must be up to 128 characters; it is sent cut to its first 128",
		"telmoni: a provider must be up to 128 characters; it is sent cut to its first 128",
		"telmoni: a response model must be up to 128 characters; it is sent cut to its first 128",
	}
	if !reflect.DeepEqual(warnings, wantWarnings) {
		t.Errorf("warnings = %q, want %q", warnings, wantWarnings)
	}
}
