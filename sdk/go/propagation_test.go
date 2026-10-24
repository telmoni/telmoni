package telmoni_test

// A run across services: Inject writes the W3C traceparent and our two
// baggage members as sdk/fixtures/headers.json spells them, Extract reads
// them back as a run continued here, and nothing of a continued run but its
// steps and calls is sent.

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"reflect"
	"strings"
	"testing"

	"go.opentelemetry.io/otel/trace"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

type headersFixture struct {
	Inject struct {
		Agent       string `json:"agent"`
		Traceparent string `json:"traceparent"`
		Baggage     string `json:"baggage"`
		Beside      struct {
			Given   string `json:"given"`
			Written string `json:"written"`
		} `json:"beside"`
	} `json:"inject"`
	Extract []struct {
		Name    string            `json:"name"`
		Headers map[string]string `json:"headers"`
		Run     *struct {
			RunID        string `json:"run_id"`
			Agent        string `json:"agent"`
			TraceID      string `json:"trace_id"`
			ParentSpanID string `json:"parent_span_id"`
			Sampled      bool   `json:"sampled"`
		} `json:"run"`
	} `json:"extract"`
}

func loadHeaders(t *testing.T) headersFixture {
	t.Helper()
	var fixture headersFixture
	if err := json.Unmarshal(loadFixture(t, "headers.json"), &fixture); err != nil {
		t.Fatalf("decoding headers.json: %v", err)
	}
	return fixture
}

func headerOf(given map[string]string) http.Header {
	header := http.Header{}
	for key, value := range given {
		header.Set(key, value)
	}
	return header
}

func TestInjectWritesTheHeadersTheFixtureSpellsBesideTheCarriersOwn(t *testing.T) {
	fixture := loadHeaders(t)
	h := newHarness(t, testConfig(), nil)
	header := http.Header{}
	beside := http.Header{}
	beside.Set("Baggage", fixture.Inject.Beside.Given)
	ctx, run := h.client.Run(context.Background(), fixture.Inject.Agent, telmoni.RunOptions{})
	stepCtx, step := run.Step(ctx, "plan")
	h.client.Inject(stepCtx, header)
	h.client.Inject(stepCtx, beside)
	step.End()
	run.End()
	h.flush()
	plan := byName(h.transport.spans(t))["plan"]
	fill := func(template string) string {
		return strings.NewReplacer(
			"<trace>", plan["traceId"].(string),
			"<span>", plan["spanId"].(string),
			"<uuid>", run.ID(),
		).Replace(template)
	}
	if got := header.Get("traceparent"); got != fill(fixture.Inject.Traceparent) {
		t.Errorf("traceparent = %q, want %q", got, fill(fixture.Inject.Traceparent))
	}
	if got := header.Get("baggage"); got != fill(fixture.Inject.Baggage) {
		t.Errorf("baggage = %q, want %q", got, fill(fixture.Inject.Baggage))
	}
	if len(header) != 2 {
		t.Errorf("wrote %d headers, want 2: %v", len(header), header)
	}
	if got := beside.Get("baggage"); got != fill(fixture.Inject.Beside.Written) {
		t.Errorf("the carrier's members kept and ours replaced: baggage = %q, want %q", got, fill(fixture.Inject.Beside.Written))
	}
	if got := beside.Values("baggage"); len(got) != 1 {
		t.Errorf("baggage set once, got %v", got)
	}
}

func TestInjectWritesNothingOutsideARunOrForARunSampledOut(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: "k", SampleRate: 0.5}, nil,
		telmoni.WithRandom(func() float64 { return 0.9 }))
	outside := http.Header{}
	outside.Set("baggage", "tenant=acme")
	h.client.Inject(context.Background(), outside)
	if got := outside.Get("baggage"); got != "tenant=acme" || outside.Get("traceparent") != "" {
		t.Errorf("outside a run something was written: %v", outside)
	}
	h.client.Inject(context.Background(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	header := http.Header{}
	h.client.Inject(ctx, header)
	run.End()
	if len(header) != 0 {
		t.Errorf("a run sampled out wrote headers: %v", header)
	}
	h.flush()
	if spans := h.transport.spans(t); len(spans) != 0 {
		t.Errorf("a run sampled out sent %v", names(spans))
	}
}

func TestExtractReadsTheRunTheFixtureNamesAndNothingFromHeadersNamingNone(t *testing.T) {
	fixture := loadHeaders(t)
	h := newHarness(t, testConfig(), nil)
	for _, given := range fixture.Extract {
		ctx := h.client.Extract(context.Background(), headerOf(given.Headers))
		run := telmoni.RunFromContext(ctx)
		if given.Run == nil {
			if run != nil {
				t.Errorf("%s: a run was read: %s %s", given.Name, run.ID(), run.Agent())
			}
			continue
		}
		if run == nil {
			t.Errorf("%s: no run was read", given.Name)
			continue
		}
		if run.ID() != given.Run.RunID || run.Agent() != given.Run.Agent {
			t.Errorf("%s: read %q %q, want %q %q", given.Name, run.ID(), run.Agent(), given.Run.RunID, given.Run.Agent)
		}
		span := trace.SpanContextFromContext(ctx)
		if span.TraceID().String() != given.Run.TraceID || span.SpanID().String() != given.Run.ParentSpanID {
			t.Errorf("%s: under %s/%s, want %s/%s", given.Name, span.TraceID(), span.SpanID(), given.Run.TraceID, given.Run.ParentSpanID)
		}
		if span.IsSampled() != given.Run.Sampled || !span.IsRemote() {
			t.Errorf("%s: sampled %t remote %t", given.Name, span.IsSampled(), span.IsRemote())
		}
	}
}

func TestAContinuedRunsStepsAndCallsCarryTheCallersRunUnderItsSpan(t *testing.T) {
	caller := newHarness(t, testConfig(), nil)
	header := http.Header{}
	ctx, run := caller.client.Run(context.Background(), "orchestrator", telmoni.RunOptions{})
	stepCtx, step := run.Step(ctx, "hand over")
	callerSpan := trace.SpanContextFromContext(stepCtx).SpanID().String()
	caller.client.Inject(stepCtx, header)
	step.End()
	run.End()
	caller.flush()
	callerTrace := byName(caller.transport.spans(t))["hand over"]["traceId"].(string)

	worker := newHarness(t, testConfig(), nil)
	workerCtx := worker.client.Extract(context.Background(), header)
	continued := telmoni.RunFromContext(workerCtx)
	if continued == nil {
		t.Fatal("the worker read no run")
	}
	workCtx, work := continued.Step(workerCtx, "work")
	if telmoni.RunFromContext(workCtx) != continued {
		t.Error("the run is not found from the step's context")
	}
	_, call := continued.ModelCall(workCtx, "gpt-x", telmoni.ModelCallOptions{Provider: "openai"})
	call.End()
	work.End()
	_, tool := continued.ToolCall(workerCtx, "search", telmoni.ToolCallOptions{})
	tool.End()
	continued.Fail(errors.New("ignored"))
	continued.End()
	worker.flush()

	spans := worker.transport.spans(t)
	if got := sortedNames(spans); !reflect.DeepEqual(got, []string{"chat gpt-x", "execute_tool search", "work"}) {
		t.Fatalf("sent %v: nothing of the run itself, whose invoke_agent the caller sends", got)
	}
	for _, span := range spans {
		if span["traceId"] != callerTrace {
			t.Errorf("%s is in trace %v, want the caller's %s", span["name"], span["traceId"], callerTrace)
		}
		attributes := attributesOf(t, span)
		if attributes["telmoni.run_id"] != run.ID() || attributes["telmoni.agent"] != "orchestrator" {
			t.Errorf("%s carries %v %v, want the caller's run", span["name"], attributes["telmoni.run_id"], attributes["telmoni.agent"])
		}
	}
	named := byName(spans)
	if named["work"]["parentSpanId"] != callerSpan {
		t.Errorf("work sits under %v, want the caller's %s", named["work"]["parentSpanId"], callerSpan)
	}
	if named["execute_tool search"]["parentSpanId"] != callerSpan {
		t.Errorf("the tool call sits under %v, want the caller's %s", named["execute_tool search"]["parentSpanId"], callerSpan)
	}
	if named["chat gpt-x"]["parentSpanId"] != named["work"]["spanId"] {
		t.Errorf("the model call sits under %v, want the step", named["chat gpt-x"]["parentSpanId"])
	}
}

// Uncut, ingest would refuse every span this side sends of the run.
func TestAContinuedRunsSpansCarryItsAgentCutToItsCap(t *testing.T) {
	fixture := loadHeaders(t)
	h := newHarness(t, testConfig(), nil)
	for _, given := range fixture.Extract {
		if !strings.HasPrefix(given.Name, "an agent past") {
			continue
		}
		ctx := h.client.Extract(context.Background(), headerOf(given.Headers))
		_, work := telmoni.RunFromContext(ctx).Step(ctx, "work")
		work.End()
		h.flush()
		span := byName(h.transport.spans(t))["work"]
		if got := attributesOf(t, span)["telmoni.agent"]; got != given.Run.Agent {
			t.Errorf("the continued run's step carries telmoni.agent %q, want it cut", got)
		}
		return
	}
	t.Fatal("headers.json holds no agent past its cap")
}

func TestAContinuedRunHandsItselfOnAgainUnderTheSpanCurrentHere(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	first := http.Header{}
	first.Set("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
	first.Set("baggage", "telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot")
	ctx := h.client.Extract(context.Background(), first)
	continued := telmoni.RunFromContext(ctx)
	relayCtx, relay := continued.Step(ctx, "relay")
	onwards := http.Header{}
	h.client.Inject(relayCtx, onwards)
	relay.End()
	h.flush()
	relaySpan := byName(h.transport.spans(t))["relay"]["spanId"].(string)
	if got := onwards.Get("traceparent"); got != "00-4bf92f3577b34da6a3ce929d0e0e4736-"+relaySpan+"-01" {
		t.Errorf("traceparent = %q", got)
	}
	if got := onwards.Get("baggage"); got != "telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot" {
		t.Errorf("baggage = %q", got)
	}
}

func TestABaggageValueIsEncodedAsTheGrammarAsksAndReadBack(t *testing.T) {
	value := "support bot, \"the\" one; é\\%"
	if got := telmoni.EncodeBaggageValue(value); got != "support%20bot%2C%20%22the%22%20one%3B%20%C3%A9%5C%25" {
		t.Errorf("encoded %q", got)
	}
	if got := telmoni.DecodeBaggageValue(telmoni.EncodeBaggageValue(value)); got != value {
		t.Errorf("read back %q", got)
	}
	for given, want := range map[string]string{"a%2": "a%2", "a%zz": "a%zz", "%c3%A9": "é"} {
		if got := telmoni.DecodeBaggageValue(given); got != want {
			t.Errorf("decode %q = %q, want %q", given, got, want)
		}
	}
	plain := "plain-bot_1.0~!#$&'()*+/:<=>?@[]^`{|}"
	if got := telmoni.EncodeBaggageValue(plain); got != plain {
		t.Errorf("encoded the plain %q as %q", plain, got)
	}
}

// Bytes that are no UTF-8 are one U+FFFD a maximal subpart, as the other three read them.
func TestABaggageValueThatIsNoUTF8IsReadAsTheOtherSDKsReadIt(t *testing.T) {
	for given, want := range map[string]string{
		"bot%E2%82%FF":                          "bot\uFFFD\uFFFD",
		"%E2%82":                                "\uFFFD",
		"%FF%FE":                                "\uFFFD\uFFFD",
		"%C0%AF":                                "\uFFFD\uFFFD",
		"%C3(":                                  "\uFFFD(",
		"%E0%80%AF":                             "\uFFFD\uFFFD\uFFFD",
		"%E0%9F%BF":                             "\uFFFD\uFFFD\uFFFD",
		"%E0%A0":                                "\uFFFD",
		"%E2%82%C0":                             "\uFFFD\uFFFD",
		"%E2%82%7F":                             "\uFFFD\x7f",
		"%ED%A0%80":                             "\uFFFD\uFFFD\uFFFD",
		"%ED%9F%BF":                             "\uD7FF",
		"%F0%8F%BF%BF":                          "\uFFFD\uFFFD\uFFFD\uFFFD",
		"%F0%9F%98":                             "\uFFFD",
		"%F0%9F%98%80%9F":                       "\U0001F600\uFFFD",
		"%F4%8F%BF%BF":                          "\U0010FFFF",
		"%F4%90%80%80":                          "\uFFFD\uFFFD\uFFFD\uFFFD",
		"%F5%80":                                "\uFFFD\uFFFD",
		"%EF%BF%BD%C3":                          "\uFFFD\uFFFD",
		"a%E2%82%FFb%ED%A0%80c%F0%9F%98":        "a\uFFFD\uFFFDb\uFFFD\uFFFD\uFFFDc\uFFFD",
		"%C0%AF%E0%80%AF%F4%90%80%80%F8%88%80x": "\uFFFD\uFFFD\uFFFD\uFFFD\uFFFD\uFFFD\uFFFD\uFFFD\uFFFD\uFFFD\uFFFD\uFFFDx",
	} {
		if got := telmoni.DecodeBaggageValue(given); got != want {
			t.Errorf("decode %q = %+q, want %+q", given, got, want)
		}
	}
}

func TestATraceparentIsParsedAsTheGrammarAsks(t *testing.T) {
	span, ok := telmoni.ParseTraceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
	if !ok || span.TraceID().String() != "4bf92f3577b34da6a3ce929d0e0e4736" || span.SpanID().String() != "00f067aa0ba902b7" || !span.IsSampled() || !span.IsRemote() {
		t.Errorf("parsed %v %t", span, ok)
	}
	if flagged, ok := telmoni.ParseTraceparent(" 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-03 "); !ok || !flagged.IsSampled() {
		t.Errorf("the sampled bit among others: %v %t", flagged, ok)
	}
	if future, ok := telmoni.ParseTraceparent("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00-more"); !ok || future.IsSampled() {
		t.Errorf("a future version with more: %v %t", future, ok)
	}
	for _, refused := range []string{
		"",
		"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
		"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
		"ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
		"00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
		"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b-01",
		"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-1",
		"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0g",
	} {
		if _, ok := telmoni.ParseTraceparent(refused); ok {
			t.Errorf("%q was taken", refused)
		}
	}
}
