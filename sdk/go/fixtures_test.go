package telmoni_test

// The golden fixtures in sdk/fixtures: the export every SDK must send for the
// same run, and what each must do with the server's answers.

import (
	"context"
	"encoding/json"
	"errors"
	"reflect"
	"regexp"
	"sort"
	"strconv"
	"testing"
	"time"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

var (
	uuidV4 = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$`)
	hex32  = regexp.MustCompile(`^[0-9a-f]{32}$`)
	hex16  = regexp.MustCompile(`^[0-9a-f]{16}$`)
)

var versioned = map[string]bool{"telemetry.sdk.version": true, "telemetry.distro.version": true}

type runFixture struct {
	Scenario struct {
		Run struct {
			Agent     string            `json:"agent"`
			UserID    string            `json:"user_id"`
			SessionID string            `json:"session_id"`
			Tags      map[string]string `json:"tags"`
			Bounds    struct {
				StallAfterS int     `json:"stall_after_s"`
				MaxSteps    int     `json:"max_steps"`
				MaxCostUSD  float64 `json:"max_cost_usd"`
			} `json:"bounds"`
			SampleRate float64 `json:"sample_rate"`
			Inside     []struct {
				Step   string `json:"step"`
				Inside []struct {
					ModelCall *modelCallSpec `json:"model_call"`
				} `json:"inside"`
				ToolCall *struct {
					Name      string `json:"name"`
					FailsWith string `json:"fails_with"`
				} `json:"tool_call"`
			} `json:"inside"`
		} `json:"run"`
		CaptureContent bool   `json:"capture_content"`
		Environment    string `json:"environment"`
		Release        string `json:"release"`
	} `json:"scenario"`
	Export map[string]any `json:"export"`
}

type modelCallSpec struct {
	Provider      string `json:"provider"`
	Model         string `json:"model"`
	ResponseModel string `json:"response_model"`
	Usage         struct {
		InputTokens           int `json:"input_tokens"`
		OutputTokens          int `json:"output_tokens"`
		CacheReadInputTokens  int `json:"cache_read_input_tokens"`
		CacheWriteInputTokens int `json:"cache_write_input_tokens"`
	} `json:"usage"`
	InputMessages  any `json:"input_messages"`
	OutputMessages any `json:"output_messages"`
}

func loadRun(t *testing.T) runFixture {
	t.Helper()
	var fixture runFixture
	if err := json.Unmarshal(loadFixture(t, "run.json"), &fixture); err != nil {
		t.Fatalf("decoding run.json: %v", err)
	}
	return fixture
}

func deepCopy(t *testing.T, value map[string]any) map[string]any {
	t.Helper()
	raw, err := json.Marshal(value)
	if err != nil {
		t.Fatalf("copying: %v", err)
	}
	var out map[string]any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("copying: %v", err)
	}
	return out
}

func pretty(t *testing.T, value any) string {
	t.Helper()
	raw, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		t.Fatalf("printing: %v", err)
	}
	return string(raw)
}

func sortByKey(attributes []any) {
	sort.Slice(attributes, func(i, j int) bool {
		return attributes[i].(map[string]any)["key"].(string) < attributes[j].(map[string]any)["key"].(string)
	})
}

func parseNanos(t *testing.T, name string, raw any) int64 {
	t.Helper()
	s, ok := raw.(string)
	if !ok {
		t.Fatalf("%s: a time is not a decimal string: %v", name, raw)
	}
	n, err := strconv.ParseInt(s, 10, 64)
	if err != nil {
		t.Fatalf("%s: a time is not a decimal string: %q", name, s)
	}
	return n
}

// normalize is the fixture's normalization, as sdk/fixtures/README.md sets
// it out.
func normalize(t *testing.T, export map[string]any) map[string]any {
	t.Helper()
	export = deepCopy(t, export)
	resourceSpans := export["resourceSpans"].([]any)
	if len(resourceSpans) != 1 {
		t.Fatalf("want one resourceSpans, got %d", len(resourceSpans))
	}
	first := resourceSpans[0].(map[string]any)
	scopeSpans := first["scopeSpans"].([]any)
	if len(scopeSpans) != 1 {
		t.Fatalf("want one scopeSpans, got %d", len(scopeSpans))
	}
	scoped := scopeSpans[0].(map[string]any)
	spans := scoped["spans"].([]any)
	sort.Slice(spans, func(i, j int) bool {
		return spans[i].(map[string]any)["name"].(string) < spans[j].(map[string]any)["name"].(string)
	})
	traceIDs := map[string]bool{}
	spanNames := map[string]string{}
	for _, raw := range spans {
		span := raw.(map[string]any)
		traceIDs[span["traceId"].(string)] = true
		spanID := span["spanId"].(string)
		if !hex16.MatchString(spanID) {
			t.Errorf("%s: spanId %q is not 16 lowercase hex characters", span["name"], spanID)
		}
		spanNames[spanID] = span["name"].(string)
	}
	if len(traceIDs) != 1 {
		t.Errorf("want one traceId across the export, got %d", len(traceIDs))
	}
	for id := range traceIDs {
		if !hex32.MatchString(id) {
			t.Errorf("traceId %q is not 32 lowercase hex characters", id)
		}
	}
	runIDs := map[string]bool{}
	for _, raw := range spans {
		span := raw.(map[string]any)
		name := span["name"].(string)
		span["traceId"] = "<trace>"
		span["spanId"] = "<span:" + spanNames[span["spanId"].(string)] + ">"
		if parent, has := span["parentSpanId"]; has {
			parentName, known := spanNames[parent.(string)]
			if !known {
				t.Errorf("%s: parentSpanId %v is not in the export", name, parent)
			}
			span["parentSpanId"] = "<span:" + parentName + ">"
		}
		start := parseNanos(t, name, span["startTimeUnixNano"])
		end := parseNanos(t, name, span["endTimeUnixNano"])
		if end < start {
			t.Errorf("%s ends before it starts", name)
		}
		if name == "telmoni.run.start" && end != start {
			t.Errorf("telmoni.run.start lasts %d ns; it has no duration", end-start)
		}
		span["startTimeUnixNano"], span["endTimeUnixNano"] = "<time>", "<time>"
		attributes := span["attributes"].([]any)
		sortByKey(attributes)
		for _, raw := range attributes {
			attribute := raw.(map[string]any)
			if attribute["key"] == "telmoni.run_id" {
				value := attribute["value"].(map[string]any)
				runIDs[value["stringValue"].(string)] = true
				value["stringValue"] = "<uuid>"
			}
		}
		span["attributes"] = attributes
	}
	if len(runIDs) != 1 {
		t.Errorf("want one run id on every span, got %d", len(runIDs))
	}
	for id := range runIDs {
		if !uuidV4.MatchString(id) {
			t.Errorf("run id %q is not a lowercase version 4 UUID", id)
		}
	}
	scoped["spans"] = spans
	scoped["scope"].(map[string]any)["version"] = "<version>"
	resource := first["resource"].(map[string]any)
	resourceAttributes := resource["attributes"].([]any)
	sortByKey(resourceAttributes)
	for _, raw := range resourceAttributes {
		attribute := raw.(map[string]any)
		key := attribute["key"].(string)
		value := attribute["value"].(map[string]any)
		switch {
		case key == "telemetry.sdk.language":
			value["stringValue"] = "<language>"
		case versioned[key]:
			value["stringValue"] = "<version>"
		}
	}
	resource["attributes"] = resourceAttributes
	return export
}

// perform runs the fixture's scenario through the SDK, as every suite does.
func perform(t *testing.T, h *harness, fixture runFixture) {
	t.Helper()
	spec := fixture.Scenario.Run
	ctx, run := h.client.Run(context.Background(), spec.Agent, telmoni.RunOptions{
		UserID:    spec.UserID,
		SessionID: spec.SessionID,
		Tags:      spec.Tags,
		Bounds: &telmoni.Bounds{
			StallAfterS: spec.Bounds.StallAfterS,
			MaxSteps:    spec.Bounds.MaxSteps,
			MaxCostUSD:  spec.Bounds.MaxCostUSD,
		},
		SampleRate: spec.SampleRate,
	})
	// The run's start is sent at once, before anything else ends.
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	if got := names(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"telmoni.run.start"}) {
		t.Fatalf("sent before the run went on: %v", got)
	}
	stepSpec := spec.Inside[0]
	stepCtx, step := run.Step(ctx, stepSpec.Step)
	callSpec := stepSpec.Inside[0].ModelCall
	_, call := run.ModelCall(stepCtx, callSpec.Model, telmoni.ModelCallOptions{
		Provider:      callSpec.Provider,
		InputMessages: callSpec.InputMessages,
	})
	call.Answered(telmoni.Answer{
		ResponseModel: callSpec.ResponseModel,
		Usage: &telmoni.Usage{
			InputTokens:           callSpec.Usage.InputTokens,
			OutputTokens:          callSpec.Usage.OutputTokens,
			CacheReadInputTokens:  callSpec.Usage.CacheReadInputTokens,
			CacheWriteInputTokens: callSpec.Usage.CacheWriteInputTokens,
		},
		OutputMessages: callSpec.OutputMessages,
	})
	call.End()
	step.End()
	toolSpec := spec.Inside[1].ToolCall
	_, tool := run.ToolCall(ctx, toolSpec.Name, telmoni.ToolCallOptions{})
	tool.FailWith(toolSpec.FailsWith)
	tool.End()
	run.End()
	h.flush()
}

func TestTheRunMatchesTheGoldenExport(t *testing.T) {
	fixture := loadRun(t)
	h := newHarness(t, telmoni.Config{
		APIKey:         testKey,
		CaptureContent: fixture.Scenario.CaptureContent,
		Environment:    fixture.Scenario.Environment,
		Release:        fixture.Scenario.Release,
	}, nil)
	perform(t, h, fixture)
	bodies := h.transport.bodies(t)
	if len(bodies) < 2 {
		t.Fatalf("the run's start travels in an export of its own; got %d exports", len(bodies))
	}
	gathered := deepCopy(t, bodies[0])
	var spans []any
	for _, body := range bodies {
		for _, span := range spansIn(body) {
			spans = append(spans, span)
		}
	}
	gathered["resourceSpans"].([]any)[0].(map[string]any)["scopeSpans"].([]any)[0].(map[string]any)["spans"] = spans
	got := normalize(t, gathered)
	if !reflect.DeepEqual(got, fixture.Export) {
		t.Errorf("the export differs from run.json\n got: %s\nwant: %s", pretty(t, got), pretty(t, fixture.Export))
	}
	for _, req := range h.transport.all() {
		if req.url != "https://api.telmoni.com/otlp/v1/traces" {
			t.Errorf("sent to %s", req.url)
		}
		if got := req.header.Get("Authorization"); got != "Bearer "+testKey {
			t.Errorf("Authorization = %q", got)
		}
		if got := req.header.Get("Content-Type"); got != "application/json" {
			t.Errorf("Content-Type = %q", got)
		}
		if got := req.header.Get("User-Agent"); got != "telmoni-go/"+telmoni.Version {
			t.Errorf("User-Agent = %q", got)
		}
	}
}

// aRun is one empty run, its start exported before its end so the two travel
// apart, as they do in any run that lasts.
func aRun(t *testing.T, h *harness) {
	t.Helper()
	before := len(h.transport.all())
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	if !h.transport.waitFor(before+1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	run.End()
	h.flush()
}

func expectWarnings(t *testing.T, h *harness, want ...string) {
	t.Helper()
	if got := h.logs.warnings(); !reflect.DeepEqual(got, want) {
		t.Errorf("warnings\n got: %q\nwant: %q", got, want)
	}
}

func expectSleeps(t *testing.T, h *harness, want ...time.Duration) {
	t.Helper()
	got := h.sleeps.all()
	if len(got) == 0 && len(want) == 0 {
		return
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("sleeps\n got: %v\nwant: %v", got, want)
	}
}

func expectRequests(t *testing.T, h *harness, want int) {
	t.Helper()
	if got := len(h.transport.all()); got != want {
		t.Errorf("sent %d exports, want %d", got, want)
	}
}

func TestKept(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: "k"}, newFakeTransport(t, answerFrom(t, "answers/kept.json")))
	if mode := h.client.ContentMode(); mode != "" {
		t.Errorf("the content mode is %q before any answer", mode)
	}
	aRun(t, h)
	h.shutdown()
	expectWarnings(t, h)
	if mode := h.client.ContentMode(); mode != "off" {
		t.Errorf("the content mode is %q after kept.json, want off", mode)
	}
	expectSleeps(t, h)
}

func TestDroppedIsWarnedOnce(t *testing.T) {
	dropped := answerFrom(t, "answers/dropped.json")
	h := newHarness(t, telmoni.Config{APIKey: "k"}, newFakeTransport(t, dropped, dropped, dropped, dropped))
	aRun(t, h)
	aRun(t, h)
	h.shutdown()
	expectWarnings(t, h, "telmoni: the server dropped gen_ai.input.messages (content; the project's content mode is off); "+
		"the project's content mode decides whether content is kept: "+
		"https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door")
}

func TestUnlistedNamesTheTagThatWouldKeepIt(t *testing.T) {
	unlisted := answerFrom(t, "answers/unlisted.json")
	h := newHarness(t, telmoni.Config{APIKey: "k"}, newFakeTransport(t, unlisted, unlisted))
	aRun(t, h)
	h.shutdown()
	expectWarnings(t, h, "telmoni: the server dropped ai.prompt (unlisted; text the field inventory does not name is never kept); "+
		"send it as the tag telmoni.tag.ai.prompt to keep it")
}

func TestRefusedIsWarnedOnceAndNeverRetried(t *testing.T) {
	refused := answerFrom(t, "answers/refused.json")
	h := newHarness(t, telmoni.Config{APIKey: "k"}, newFakeTransport(t, refused, refused))
	aRun(t, h)
	h.shutdown()
	expectWarnings(t, h, "telmoni: the server refused a span: no telmoni.run_id and telmoni.agent; a span outside a Telmoni run is refused")
	expectRequests(t, h, 2)
	expectSleeps(t, h)
}

func TestRateLimitedWaitsRetryAfterAndSendsTheSameBytes(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: "k"},
		newFakeTransport(t, answerFrom(t, "answers/rate-limited.json"), kept(t), kept(t)))
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, 2*time.Second)
	requests := h.transport.all()
	expectRequests(t, h, 3)
	if len(requests) >= 2 && !reflect.DeepEqual(requests[0].body, requests[1].body) {
		t.Error("the retry did not send the same bytes")
	}
	expectWarnings(t, h)
}

func TestUnavailableIsRetriedThreeTimesThenDropped(t *testing.T) {
	unavailable := answerFrom(t, "answers/unavailable.json")
	h := newHarness(t, telmoni.Config{APIKey: "k"}, newFakeTransport(t, unavailable, unavailable, unavailable, kept(t)))
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, time.Second, time.Second)
	expectWarnings(t, h, "telmoni: export failed after 3 attempts (503 /errors/telemetry/store-unavailable); the batch was dropped")
	expectRequests(t, h, 4)
}

func TestForbiddenIsNotRetriedAndTheNextBatchIsStillSent(t *testing.T) {
	forbidden := answerFrom(t, "answers/forbidden.json")
	h := newHarness(t, telmoni.Config{APIKey: "k"}, newFakeTransport(t, forbidden, forbidden, kept(t)))
	aRun(t, h)
	aRun(t, h)
	h.shutdown()
	expectWarnings(t, h, "telmoni: export refused with 403 /errors/authz/forbidden; the batch was dropped")
	expectRequests(t, h, 4)
	expectSleeps(t, h)
}

func TestANetworkFailureIsRetriedWithBackoff(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: "k"},
		newFakeTransport(t, errors.New("refused"), errors.New("refused"), kept(t), kept(t)))
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, time.Second, 2*time.Second)
	expectWarnings(t, h)
	expectRequests(t, h, 4)
}

func TestANetworkFailureThreeTimesNamesTheErrorsType(t *testing.T) {
	down := errors.New("refused")
	h := newHarness(t, telmoni.Config{APIKey: "k"}, newFakeTransport(t, down, down, down, kept(t)))
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, time.Second, 2*time.Second)
	expectWarnings(t, h, "telmoni: export failed after 3 attempts (errors.errorString); the batch was dropped")
	expectRequests(t, h, 4)
}
