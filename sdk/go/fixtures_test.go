package telmoni_test

// The golden fixtures in sdk/fixtures: the export every SDK must send for the
// same run, and what each must do with the server's answers.

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net"
	"net/url"
	"path/filepath"
	"reflect"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/trace"

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
			Agent      string            `json:"agent"`
			UserID     string            `json:"user_id"`
			SessionID  string            `json:"session_id"`
			Tags       map[string]string `json:"tags"`
			SampleRate float64           `json:"sample_rate"`
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
	Request       struct {
		Temperature      *float64 `json:"temperature"`
		TopP             *float64 `json:"top_p"`
		TopK             *float64 `json:"top_k"`
		MaxTokens        *int64   `json:"max_tokens"`
		FrequencyPenalty *float64 `json:"frequency_penalty"`
		PresencePenalty  *float64 `json:"presence_penalty"`
		Seed             *int64   `json:"seed"`
	} `json:"request"`
	TimeToFirstToken float64 `json:"time_to_first_token"`
	Usage            struct {
		InputTokens           int      `json:"input_tokens"`
		OutputTokens          int      `json:"output_tokens"`
		CacheReadInputTokens  int      `json:"cache_read_input_tokens"`
		CacheWriteInputTokens int      `json:"cache_write_input_tokens"`
		Cost                  *float64 `json:"cost"`
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
		UserID:     spec.UserID,
		SessionID:  spec.SessionID,
		Tags:       spec.Tags,
		SampleRate: spec.SampleRate,
	})
	stepSpec := spec.Inside[0]
	stepCtx, step := run.Step(ctx, stepSpec.Step)
	callSpec := stepSpec.Inside[0].ModelCall
	_, call := run.ModelCall(stepCtx, callSpec.Model, telmoni.ModelCallOptions{
		Provider:         callSpec.Provider,
		InputMessages:    callSpec.InputMessages,
		Temperature:      callSpec.Request.Temperature,
		TopP:             callSpec.Request.TopP,
		TopK:             callSpec.Request.TopK,
		FrequencyPenalty: callSpec.Request.FrequencyPenalty,
		PresencePenalty:  callSpec.Request.PresencePenalty,
		MaxTokens:        callSpec.Request.MaxTokens,
		Seed:             callSpec.Request.Seed,
	})
	call.Answered(telmoni.Answer{
		ResponseModel:    callSpec.ResponseModel,
		TimeToFirstToken: time.Duration(callSpec.TimeToFirstToken * float64(time.Second)),
		Usage: &telmoni.Usage{
			InputTokens:           callSpec.Usage.InputTokens,
			OutputTokens:          callSpec.Usage.OutputTokens,
			CacheReadInputTokens:  callSpec.Usage.CacheReadInputTokens,
			CacheWriteInputTokens: callSpec.Usage.CacheWriteInputTokens,
			Cost:                  callSpec.Usage.Cost,
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
		APIKey:         telmoni.NewSecret(testKey),
		CaptureContent: fixture.Scenario.CaptureContent,
		Environment:    fixture.Scenario.Environment,
		Release:        fixture.Scenario.Release,
	}, nil)
	perform(t, h, fixture)
	bodies := h.transport.bodies(t)
	if len(bodies) == 0 {
		t.Fatal("the run was not sent")
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
		if got := req.header.Get("Telmoni-Sent-At"); got == "" || strings.Trim(got, "0123456789") != "" {
			t.Errorf("Telmoni-Sent-At = %q", got)
		}
	}
}

// In nanoseconds, a retry its own: ingest moves a run by it.
type failureFixture struct {
	Scenario struct {
		Agent string `json:"agent"`
		Event struct {
			Name       string         `json:"name"`
			Attributes map[string]any `json:"attributes"`
		} `json:"event"`
		Message string `json:"message"`
	} `json:"scenario"`
	Cases []struct {
		CaptureContent bool   `json:"capture_content"`
		ContentMode    string `json:"content_mode"`
		Sends          string `json:"sends"`
	} `json:"cases"`
	Kept     map[string]any `json:"kept"`
	Withheld map[string]any `json:"withheld"`
}

// RateLimited is failure.json's error, its type named as Go names one.
type RateLimited struct{ message string }

func (e *RateLimited) Error() string { return e.message }

// normalizeFailure is the run's span as failure.json's README normalizes it.
func normalizeFailure(t *testing.T, span map[string]any) map[string]any {
	t.Helper()
	errorType := attributesOf(t, span)["error.type"]
	span = deepCopy(t, span)
	sent := map[string]any{"status": span["status"]}
	events, ok := span["events"].([]any)
	if !ok {
		return sent
	}
	start := parseNanos(t, "start", span["startTimeUnixNano"])
	end := parseNanos(t, "end", span["endTimeUnixNano"])
	for _, raw := range events {
		event := raw.(map[string]any)
		if at := parseNanos(t, "event", event["timeUnixNano"]); at < start || at > end {
			t.Errorf("an event at %d, outside its span's %d to %d", at, start, end)
		}
		event["timeUnixNano"] = "<time>"
		attributes := event["attributes"].([]any)
		sortByKey(attributes)
		for _, a := range attributes {
			attribute := a.(map[string]any)
			value := attribute["value"].(map[string]any)
			switch attribute["key"] {
			case "exception.stacktrace":
				t.Error("Go sends no stack trace")
			case "exception.type":
				if value["stringValue"] != errorType {
					t.Errorf("exception.type = %v, error.type = %v", value["stringValue"], errorType)
				}
				value["stringValue"] = "<error.type>"
			}
		}
	}
	sent["events"] = events
	return sent
}

func TestAFailureLeavesInFullOnlyUnderBothLocks(t *testing.T) {
	var fixture failureFixture
	if err := json.Unmarshal(loadFixture(t, "failure.json"), &fixture); err != nil {
		t.Fatalf("decoding failure.json: %v", err)
	}
	scenario := fixture.Scenario
	for _, each := range fixture.Cases {
		mode := scripted{status: 200, headers: map[string]string{"Telmoni-Content-Mode": each.ContentMode}, body: []byte("{}")}
		h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: each.CaptureContent}, newFakeTransport(t, mode))
		_, warm := h.client.Run(context.Background(), scenario.Agent, telmoni.RunOptions{})
		warm.End()
		h.flush()
		if got := h.client.ContentMode(); got != each.ContentMode {
			t.Fatalf("the content mode is %q, want %q", got, each.ContentMode)
		}
		ctx, run := h.client.Run(context.Background(), scenario.Agent, telmoni.RunOptions{})
		var attributes []attribute.KeyValue
		for key, value := range scenario.Event.Attributes {
			switch v := value.(type) {
			case float64:
				attributes = append(attributes, attribute.Int64(key, int64(v)))
			case string:
				attributes = append(attributes, attribute.String(key, v))
			}
		}
		trace.SpanFromContext(ctx).AddEvent(scenario.Event.Name, trace.WithAttributes(attributes...))
		run.Fail(&RateLimited{scenario.Message})
		run.End()
		h.flush()
		spans := h.transport.spans(t)
		want := fixture.Kept
		if each.Sends == "withheld" {
			want = fixture.Withheld
		}
		if got := normalizeFailure(t, spans[len(spans)-1]); !reflect.DeepEqual(got, want) {
			t.Errorf("%+v: sent\n%s\nwant\n%s", each, pretty(t, got), pretty(t, want))
		}
		if text := h.transport.text(); each.Sends == "withheld" && (strings.Contains(text, scenario.Message) || strings.Contains(text, scenario.Event.Name)) {
			t.Errorf("%+v: the failure's content left", each)
		}
	}
}

type mediaFixture struct {
	Scenario struct {
		Agent              string `json:"agent"`
		Provider           string `json:"provider"`
		Model              string `json:"model"`
		SystemInstructions any    `json:"system_instructions"`
		InputMessages      any    `json:"input_messages"`
		OutputMessages     any    `json:"output_messages"`
	} `json:"scenario"`
	Cases []struct {
		CaptureContent bool   `json:"capture_content"`
		ContentMode    string `json:"content_mode"`
		Sends          bool   `json:"sends"`
	} `json:"cases"`
	Uploads []map[string]any `json:"uploads"`
	Sent    map[string]any   `json:"sent"`
}

type askFixture struct {
	Body          map[string]any `json:"body"`
	Agent         string         `json:"agent"`
	Model         string         `json:"model"`
	InputMessages any            `json:"input_messages"`
	Cases         []struct {
		CaptureContent bool  `json:"capture_content"`
		Content        bool  `json:"content"`
		Answers        []any `json:"answers"`
		Asked          bool  `json:"asked"`
		Sent           bool  `json:"sent"`
		Requests       int   `json:"requests"`
	} `json:"cases"`
}

func TestTheModeIsAskedBeforeTheFirstContentItWouldSend(t *testing.T) {
	var fixture askFixture
	if err := json.Unmarshal(loadFixture(t, "ask.json"), &fixture); err != nil {
		t.Fatalf("decoding ask.json: %v", err)
	}
	for _, each := range fixture.Cases {
		var answers []any
		for _, given := range each.Answers {
			switch given := given.(type) {
			case string:
				answers = append(answers, scripted{status: 200, headers: map[string]string{"Telmoni-Content-Mode": given}, body: []byte("{}")})
			case float64:
				answers = append(answers, scripted{status: int(given)})
			}
		}
		h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: each.CaptureContent}, newFakeTransport(t, answers...))
		for range 2 {
			ctx, run := h.client.Run(context.Background(), fixture.Agent, telmoni.RunOptions{})
			var opts telmoni.ModelCallOptions
			if each.Content {
				opts.InputMessages = fixture.InputMessages
			}
			_, call := run.ModelCall(ctx, fixture.Model, opts)
			call.End()
			run.End()
			h.flush()
		}
		bodies := h.transport.bodies(t)
		if len(bodies) != each.Requests {
			t.Fatalf("%+v: %d requests, want %d", each, len(bodies), each.Requests)
		}
		var asks, exports []map[string]any
		for _, body := range bodies {
			if len(spansIn(body)) == 0 {
				asks = append(asks, body)
			} else {
				exports = append(exports, body)
			}
		}
		if each.Asked {
			if len(asks) != 1 || !reflect.DeepEqual(bodies[0], fixture.Body) {
				t.Errorf("%+v: asks %v, first body %v; want the ask first and alone", each, asks, bodies[0])
			}
			if got := h.transport.all()[0].header.Get("Authorization"); got != "Bearer k" {
				t.Errorf("%+v: the ask's Authorization = %q", each, got)
			}
		} else if len(asks) != 0 {
			t.Errorf("%+v: asked %d times, want none", each, len(asks))
		}
		calls := withPrefix(spansIn(exports[0]), "chat ")
		if len(calls) != 1 {
			t.Fatalf("%+v: %d model calls in the first export, want 1", each, len(calls))
		}
		if _, sent := attributesOf(t, calls[0])["gen_ai.input.messages"]; sent != each.Sent {
			t.Errorf("%+v: the first run's content sent = %v, want %v", each, sent, each.Sent)
		}
	}
}

func TestImagesAreLiftedOutOfMessagesOnlyUnderBothLocks(t *testing.T) {
	var fixture mediaFixture
	if err := json.Unmarshal(loadFixture(t, "media.json"), &fixture); err != nil {
		t.Fatalf("decoding media.json: %v", err)
	}
	scenario := fixture.Scenario
	for _, each := range fixture.Cases {
		mode := scripted{status: 200, headers: map[string]string{"Telmoni-Content-Mode": each.ContentMode}, body: []byte("{}")}
		h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: each.CaptureContent}, newFakeTransport(t, mode, mode, mode))
		aRun(t, h)
		// Twice, in two exports: each file is put once.
		for range 2 {
			ctx, run := h.client.Run(context.Background(), scenario.Agent, telmoni.RunOptions{})
			_, call := run.ModelCall(ctx, scenario.Model, telmoni.ModelCallOptions{
				Provider:           scenario.Provider,
				InputMessages:      scenario.InputMessages,
				SystemInstructions: scenario.SystemInstructions,
			})
			call.Answered(telmoni.Answer{OutputMessages: scenario.OutputMessages})
			call.End()
			run.End()
			h.flush()
		}
		calls := withPrefix(h.transport.spans(t), "chat ")
		if len(calls) != 2 {
			t.Fatalf("%+v: %d model calls sent, want 2", each, len(calls))
		}
		uploads := h.transport.allUploads()
		if !each.Sends {
			if len(uploads) != 0 {
				t.Errorf("%+v: %d files put, want none", each, len(uploads))
			}
			for _, call := range calls {
				for key := range fixture.Sent {
					if _, has := attributesOf(t, call)[key]; has {
						t.Errorf("%+v: %s left the process", each, key)
					}
				}
			}
			continue
		}
		sort.Slice(uploads, func(i, j int) bool { return uploads[i].url < uploads[j].url })
		got := make([]map[string]any, 0, len(uploads))
		for _, upload := range uploads {
			if upload.header.Get("Authorization") != "Bearer k" {
				t.Errorf("%+v: a file put without the key", each)
			}
			got = append(got, map[string]any{
				"path":         strings.TrimPrefix(upload.url, "https://api.telmoni.com"),
				"content_type": upload.header.Get("Content-Type"),
				"body":         base64.StdEncoding.EncodeToString(upload.body),
			})
		}
		if !reflect.DeepEqual(got, fixture.Uploads) {
			t.Errorf("%+v: put\n%s\nwant\n%s", each, pretty(t, got), pretty(t, fixture.Uploads))
		}
		for _, call := range calls {
			sent := map[string]any{}
			for key, value := range attributesOf(t, call) {
				if _, ok := fixture.Sent[key]; ok {
					var decoded any
					if err := json.Unmarshal([]byte(value.(string)), &decoded); err != nil {
						t.Fatalf("%s is no JSON: %v", key, err)
					}
					sent[key] = decoded
				}
			}
			if !reflect.DeepEqual(sent, fixture.Sent) {
				t.Errorf("%+v: sent\n%s\nwant\n%s", each, pretty(t, sent), pretty(t, fixture.Sent))
			}
		}
	}
}

func TestEachAttemptCarriesThisClockAsItLeaves(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")},
		newFakeTransport(t, answerFrom(t, "answers/unavailable.json")),
		telmoni.WithSleep(func(time.Duration) { time.Sleep(5 * time.Millisecond) }))
	before := time.Now().UnixNano()
	aRun(t, h)
	h.shutdown()
	after := time.Now().UnixNano()
	requests := h.transport.all()
	var sent []int64
	for _, req := range requests {
		at, err := strconv.ParseInt(req.header.Get("Telmoni-Sent-At"), 10, 64)
		if err != nil || at < before || at > after {
			t.Errorf("Telmoni-Sent-At %q, want nanoseconds within %d to %d", req.header.Get("Telmoni-Sent-At"), before, after)
		}
		sent = append(sent, at)
	}
	if len(requests) < 2 || !bytes.Equal(requests[0].body, requests[1].body) {
		t.Fatal("the export was not sent again")
	}
	if sent[1] <= sent[0] {
		t.Errorf("the retry left at %d, the first attempt at %d", sent[1], sent[0])
	}
}

// aRun is one empty run, sent in an export of its own.
func aRun(t *testing.T, h *harness) {
	t.Helper()
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	h.flush()
}

func expectWarnings(t *testing.T, h *harness, want ...string) {
	t.Helper()
	if got := h.logs.warnings(); !reflect.DeepEqual(got, want) {
		t.Errorf("warnings\n got: %q\nwant: %q", got, want)
	}
}

// expectAnswer is the warning an answers/*.json fixture names, word for word,
// with exactly the fields it names beside it.
func expectAnswer(t *testing.T, h *harness, name string) {
	t.Helper()
	expectWarnings(t, h, warningOf(t, name)...)
	expectFields(t, h, fieldsOf(t, name)...)
}

func expectFields(t *testing.T, h *harness, want ...map[string]any) {
	t.Helper()
	if got := h.logs.fields(); !reflect.DeepEqual(got, want) {
		t.Errorf("fields\n got: %v\nwant: %v", got, want)
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
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, answerFrom(t, "answers/kept.json")))
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
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, dropped, dropped))
	aRun(t, h)
	aRun(t, h)
	h.shutdown()
	expectAnswer(t, h, "answers/dropped.json")
}

func TestUnlistedNamesTheTagThatWouldKeepIt(t *testing.T) {
	unlisted := answerFrom(t, "answers/unlisted.json")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, unlisted))
	aRun(t, h)
	h.shutdown()
	expectAnswer(t, h, "answers/unlisted.json")
}

func TestATagPastTheCardinalityCapIsWarnedWithWhatKeepsIt(t *testing.T) {
	cardinality := answerFrom(t, "answers/cardinality.json")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, cardinality))
	aRun(t, h)
	h.shutdown()
	expectAnswer(t, h, "answers/cardinality.json")
}

func TestRefusedIsWarnedOnceAndNeverRetried(t *testing.T) {
	refused := answerFrom(t, "answers/refused.json")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, refused, refused))
	aRun(t, h)
	aRun(t, h)
	h.shutdown()
	expectAnswer(t, h, "answers/refused.json")
	expectRequests(t, h, 2)
	expectSleeps(t, h)
}

func TestRateLimitedWaitsRetryAfterAndSendsTheSameBytes(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")},
		newFakeTransport(t, answerFrom(t, "answers/rate-limited.json"), kept(t)))
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, 2*time.Second)
	requests := h.transport.all()
	expectRequests(t, h, 2)
	if len(requests) >= 2 && !reflect.DeepEqual(requests[0].body, requests[1].body) {
		t.Error("the retry did not send the same bytes")
	}
	expectWarnings(t, h)
}

func TestARetryAfterPastTheCapWaitsTheCap(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")},
		newFakeTransport(t, answerFrom(t, "answers/rate-limited-long.json"), kept(t)))
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, time.Minute)
	requests := h.transport.all()
	expectRequests(t, h, 2)
	if len(requests) >= 2 && !bytes.Equal(requests[0].body, requests[1].body) {
		t.Error("the retry did not send the same bytes")
	}
	expectWarnings(t, h)
}

// Retry-After is read as text: ASCII digits, a sign allowed, below 0 none and
// past 60 the cap however long, where its seconds as a Duration would overflow.
func TestRetryAfterIsReadAsText(t *testing.T) {
	limited := func(retryAfter string) scripted {
		return scripted{status: 429, headers: map[string]string{"Retry-After": retryAfter}, body: []byte("{}")}
	}
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t,
		limited("99999999999"), limited("1_0"), kept(t),
		limited("-5"), limited(" +7\t"), kept(t),
		limited(strings.Repeat("9", 400)), kept(t)))
	for range 3 {
		aRun(t, h)
	}
	h.shutdown()
	expectSleeps(t, h, time.Minute, 2*time.Second, 0, 7*time.Second, time.Minute)
	expectRequests(t, h, 8)
	expectWarnings(t, h)
}

func TestUnavailableIsRetriedThreeTimesThenDropped(t *testing.T) {
	unavailable := answerFrom(t, "answers/unavailable.json")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, unavailable, unavailable, unavailable, kept(t)))
	aRun(t, h)
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, time.Second, time.Second)
	expectAnswer(t, h, "answers/unavailable.json")
	expectRequests(t, h, 4)
}

func TestForbiddenIsNotRetriedAndTheNextBatchIsStillSent(t *testing.T) {
	forbidden := answerFrom(t, "answers/forbidden.json")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, forbidden, forbidden))
	aRun(t, h)
	aRun(t, h)
	h.shutdown()
	expectAnswer(t, h, "answers/forbidden.json")
	expectRequests(t, h, 2)
	expectSleeps(t, h)
}

// Each answer met on every attempt logs its fixture's line word for word, and its fields, as the plugins read it.
func TestEachAnswerIsWarnedWordForWord(t *testing.T) {
	paths, err := filepath.Glob(filepath.Join(fixturesDir, "answers", "*.json"))
	if err != nil || len(paths) == 0 {
		t.Fatalf("listing the answers: %v", err)
	}
	for _, path := range paths {
		name := "answers/" + filepath.Base(path)
		t.Run(filepath.Base(path), func(t *testing.T) {
			answer := answerFrom(t, name)
			h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, answer, answer, answer))
			aRun(t, h)
			h.shutdown()
			expectAnswer(t, h, name)
		})
	}
}

func TestANetworkFailureIsRetriedWithBackoff(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")},
		newFakeTransport(t, errors.New("refused"), errors.New("refused"), kept(t)))
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, time.Second, 2*time.Second)
	expectWarnings(t, h)
	expectRequests(t, h, 3)
}

func TestANetworkFailureThreeTimesNamesTheErrorsType(t *testing.T) {
	down := errors.New("refused")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, down, down, down, kept(t)))
	aRun(t, h)
	aRun(t, h)
	h.shutdown()
	expectSleeps(t, h, time.Second, 2*time.Second)
	expectWarnings(t, h, "telmoni: export failed after 3 attempts (errors.errorString); the batch was dropped")
	// No answer came, so neither a status nor a problem type.
	expectFields(t, h, map[string]any{"kind": "failed"})
	expectRequests(t, h, 4)
}

// A transport failure last names no status, though an answer came before it.
func TestAFailureEndingInTransportErrorCarriesNoStatus(t *testing.T) {
	down := errors.New("refused")
	unavailable := answerFrom(t, "answers/unavailable.json")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, newFakeTransport(t, unavailable, unavailable, down))
	aRun(t, h)
	h.shutdown()
	expectWarnings(t, h, "telmoni: export failed after 3 attempts (errors.errorString); the batch was dropped")
	expectFields(t, h, map[string]any{"kind": "failed"})
}

// net/http wraps every failure in a *url.Error: each is named by its cause,
// a timeout apart, and so warned of once each.
func TestATransportFailureIsNamedByItsCause(t *testing.T) {
	refused := &url.Error{Op: "Post", URL: "https://api.telmoni.com", Err: &net.OpError{Op: "dial", Net: "tcp", Err: errors.New("connection refused")}}
	timedOut := &url.Error{Op: "Post", URL: "https://api.telmoni.com", Err: context.DeadlineExceeded}
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")},
		newFakeTransport(t, refused, refused, refused, timedOut, timedOut, timedOut))
	aRun(t, h)
	aRun(t, h)
	h.shutdown()
	expectWarnings(t, h,
		"telmoni: export failed after 3 attempts (net.OpError); the batch was dropped",
		"telmoni: export failed after 3 attempts (timeout); the batch was dropped",
	)
}
