package telmoni_test

// A run recorded after it ended: RecordRun, and the export every SDK must send
// for sdk/fixtures/recorded.json.

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"maps"
	"reflect"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

type recordedFixture struct {
	Scenario struct {
		Record         recordSpec `json:"record"`
		CaptureContent bool       `json:"capture_content"`
	} `json:"scenario"`
	Export map[string]any `json:"export"`
}

type recordSpec struct {
	RunID     string            `json:"run_id"`
	Agent     string            `json:"agent"`
	UserID    string            `json:"user_id"`
	SessionID string            `json:"session_id"`
	Tags      map[string]string `json:"tags"`
	Start     time.Time         `json:"start"`
	End       time.Time         `json:"end"`
	ErrorType string            `json:"error_type"`
	Cancelled bool              `json:"cancelled"`
	Inside    []struct {
		Step *struct {
			Name      string           `json:"name"`
			Start     time.Time        `json:"start"`
			End       time.Time        `json:"end"`
			Calls     []recordCallSpec `json:"calls"`
			ErrorType string           `json:"error_type"`
			Cancelled bool             `json:"cancelled"`
		} `json:"step"`
		recordCallSpec
	} `json:"inside"`
}

type recordCallSpec struct {
	ModelCall *struct {
		Model     string    `json:"model"`
		Provider  string    `json:"provider"`
		Operation string    `json:"operation"`
		Start     time.Time `json:"start"`
		End       time.Time `json:"end"`
		Usage     *struct {
			InputTokens           int `json:"input_tokens"`
			OutputTokens          int `json:"output_tokens"`
			CacheReadInputTokens  int `json:"cache_read_input_tokens"`
			CacheWriteInputTokens int `json:"cache_write_input_tokens"`
		} `json:"usage"`
		ErrorType string `json:"error_type"`
		Cancelled bool   `json:"cancelled"`
	} `json:"model_call"`
	ToolCall *struct {
		Name      string    `json:"name"`
		Start     time.Time `json:"start"`
		End       time.Time `json:"end"`
		ErrorType string    `json:"error_type"`
		Cancelled bool      `json:"cancelled"`
	} `json:"tool_call"`
}

func loadRecorded(t *testing.T) recordedFixture {
	t.Helper()
	var fixture recordedFixture
	if err := json.Unmarshal(loadFixture(t, "recorded.json"), &fixture); err != nil {
		t.Fatalf("decoding recorded.json: %v", err)
	}
	return fixture
}

func callFrom(spec recordCallSpec) telmoni.CallRecord {
	if call := spec.ModelCall; call != nil {
		record := telmoni.ModelCallRecord{
			Model:     call.Model,
			Provider:  call.Provider,
			Operation: call.Operation,
			Start:     call.Start,
			End:       call.End,
			ErrorType: call.ErrorType,
			Cancelled: call.Cancelled,
		}
		if usage := call.Usage; usage != nil {
			record.Usage = &telmoni.Usage{
				InputTokens:           usage.InputTokens,
				OutputTokens:          usage.OutputTokens,
				CacheReadInputTokens:  usage.CacheReadInputTokens,
				CacheWriteInputTokens: usage.CacheWriteInputTokens,
			}
		}
		return record
	}
	tool := spec.ToolCall
	return telmoni.ToolCallRecord{
		Name:      tool.Name,
		Start:     tool.Start,
		End:       tool.End,
		ErrorType: tool.ErrorType,
		Cancelled: tool.Cancelled,
	}
}

func recordFrom(spec recordSpec) telmoni.RunRecord {
	inside := make([]telmoni.RecordItem, 0, len(spec.Inside))
	for _, item := range spec.Inside {
		step := item.Step
		if step == nil {
			inside = append(inside, callFrom(item.recordCallSpec))
			continue
		}
		calls := make([]telmoni.CallRecord, 0, len(step.Calls))
		for _, call := range step.Calls {
			calls = append(calls, callFrom(call))
		}
		inside = append(inside, telmoni.StepRecord{
			Name:      step.Name,
			Start:     step.Start,
			End:       step.End,
			Calls:     calls,
			ErrorType: step.ErrorType,
			Cancelled: step.Cancelled,
		})
	}
	return telmoni.RunRecord{
		RunID:     spec.RunID,
		Agent:     spec.Agent,
		UserID:    spec.UserID,
		SessionID: spec.SessionID,
		Tags:      spec.Tags,
		Start:     spec.Start,
		End:       spec.End,
		Inside:    inside,
		ErrorType: spec.ErrorType,
		Cancelled: spec.Cancelled,
	}
}

func theRecord(t *testing.T) telmoni.RunRecord {
	t.Helper()
	return recordFrom(loadRecorded(t).Scenario.Record)
}

// normalizeRecorded is recorded.json's normalization, as sdk/fixtures/README.md
// sets it out: every attributes array sorted by key, the language and the
// versions replaced, and nothing else touched.
func normalizeRecorded(t *testing.T, body map[string]any) map[string]any {
	t.Helper()
	body = deepCopy(t, body)
	resourceSpans := body["resourceSpans"].([]any)
	if len(resourceSpans) != 1 {
		t.Fatalf("want one resourceSpans, got %d", len(resourceSpans))
	}
	first := resourceSpans[0].(map[string]any)
	scopeSpans := first["scopeSpans"].([]any)
	if len(scopeSpans) != 1 {
		t.Fatalf("want one scopeSpans, got %d", len(scopeSpans))
	}
	scoped := scopeSpans[0].(map[string]any)
	scoped["scope"].(map[string]any)["version"] = "<version>"
	for _, span := range scoped["spans"].([]any) {
		sortByKey(span.(map[string]any)["attributes"].([]any))
	}
	resourceAttributes := first["resource"].(map[string]any)["attributes"].([]any)
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
	return body
}

func recordRun(t *testing.T, h *harness, record telmoni.RunRecord) {
	t.Helper()
	if err := h.client.RecordRun(context.Background(), record); err != nil {
		t.Fatalf("RecordRun: %v", err)
	}
	h.flush()
}

// sameBody fails unless each harness sent one export, and the same bytes.
func sameBody(t *testing.T, got, want *harness) {
	t.Helper()
	gotRequests, wantRequests := got.transport.all(), want.transport.all()
	if len(gotRequests) != 1 || len(wantRequests) != 1 {
		t.Fatalf("sent %d and %d exports, want one each", len(gotRequests), len(wantRequests))
	}
	if !bytes.Equal(gotRequests[0].body, wantRequests[0].body) {
		t.Errorf("sent other bytes\n got: %s\nwant: %s", gotRequests[0].body, wantRequests[0].body)
	}
}

func TestTheRecordMatchesTheGoldenExport(t *testing.T) {
	fixture := loadRecorded(t)
	h := newHarness(t, telmoni.Config{APIKey: testKey, CaptureContent: fixture.Scenario.CaptureContent}, nil)
	recordRun(t, h, recordFrom(fixture.Scenario.Record))
	bodies := h.transport.bodies(t)
	if len(bodies) != 1 {
		t.Fatalf("a record travels in one export; got %d", len(bodies))
	}
	if got := normalizeRecorded(t, bodies[0]); !reflect.DeepEqual(got, fixture.Export) {
		t.Errorf("the export differs from recorded.json\n got: %s\nwant: %s", pretty(t, got), pretty(t, fixture.Export))
	}
	for _, req := range h.transport.all() {
		if req.url != "https://api.telmoni.com/otlp/v1/traces" {
			t.Errorf("sent to %s", req.url)
		}
		if got := req.header.Get("Authorization"); got != "Bearer "+testKey {
			t.Errorf("Authorization = %q", got)
		}
		if got := req.header.Get("User-Agent"); got != "telmoni-go/"+telmoni.Version {
			t.Errorf("User-Agent = %q", got)
		}
	}
}

func TestTheSameRecordSentAgainIsTheSameBytes(t *testing.T) {
	record := theRecord(t)
	// Tags enough that a map ranged in its own order would show it.
	record.Tags = map[string]string{"skills": "review", "env": "prod", "team": "a", "tier": "pro", "region": "eu", "shift": "night"}
	var first *harness
	// Again and again, a client of its own each time, since a worker that
	// took a batch mid-record would split it only now and then.
	for range 20 {
		h := newHarness(t, testConfig(), nil)
		recordRun(t, h, record)
		h.shutdown()
		if first == nil {
			first = h
		}
		sameBody(t, h, first)
	}
}

func TestARecordInsideALiveRunTakesATraceOfItsOwn(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	record := theRecord(t)
	ctx, live := h.client.Run(context.Background(), "live", telmoni.RunOptions{})
	if err := h.client.RecordRun(ctx, record); err != nil {
		t.Fatalf("RecordRun: %v", err)
	}
	if telmoni.RunFromContext(ctx) != live {
		t.Error("the live run is no longer found from its context")
	}
	// A step opened after the record is the live run's, ids and all.
	_, step := live.Step(ctx, "after")
	step.End()
	live.End()
	h.flush()
	var recorded []map[string]any
	for _, span := range h.transport.spans(t) {
		if attributesOf(t, span)["telmoni.run_id"] == record.RunID {
			recorded = append(recorded, span)
		}
	}
	if len(recorded) != 9 {
		t.Fatalf("sent %d spans of the record, want its 9", len(recorded))
	}
	want := strings.ReplaceAll(record.RunID, "-", "")
	for _, span := range recorded {
		if span["traceId"] != want {
			t.Errorf("%s is in trace %v, want the run's id %s", span["name"], span["traceId"], want)
		}
	}
	if _, has := byName(recorded)["invoke_agent claude-code"]["parentSpanId"]; has {
		t.Error("the recorded run has a parent")
	}
	liveSpan := trace.SpanContextFromContext(ctx)
	after := byName(h.transport.spans(t))["after"]
	if after["traceId"] != liveSpan.TraceID().String() || after["parentSpanId"] != liveSpan.SpanID().String() {
		t.Error("the step opened after the record is not the live run's child")
	}
}

func TestAFailedRecordIsMarkedOnItsRunsSpan(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	record := theRecord(t)
	record.ErrorType, record.Cancelled, record.Inside = "RateLimitError", false, nil
	recordRun(t, h, record)
	run := byName(h.transport.spans(t))["invoke_agent claude-code"]
	if run == nil {
		t.Fatalf("spans sent: %v", sortedNames(h.transport.spans(t)))
	}
	if !reflect.DeepEqual(run["status"], map[string]any{"code": float64(2)}) {
		t.Errorf("status = %v, want {code: 2}", run["status"])
	}
	attributes := attributesOf(t, run)
	if attributes["error.type"] != "RateLimitError" {
		t.Errorf("error.type = %v, want RateLimitError", attributes["error.type"])
	}
	if _, has := attributes["telmoni.cancelled"]; has {
		t.Error("a run that failed is marked cancelled")
	}
}

func TestTheSampleRateDropsARecordWhole(t *testing.T) {
	random := 0.9
	h := newHarness(t, telmoni.Config{APIKey: "k", SampleRate: 0.25}, nil,
		telmoni.WithRandom(func() float64 { return random }))
	record := theRecord(t)
	recordRun(t, h, record)
	if sent := len(h.transport.all()); sent != 0 {
		t.Fatalf("a record sampled out sent %d exports", sent)
	}
	// A record sampled out builds no span, so the same record, drawn in, is
	// sent.
	random = 0.1
	recordRun(t, h, record)
	spans := h.transport.spans(t)
	if len(spans) != 9 {
		t.Fatalf("sent %d spans, want the record's 9", len(spans))
	}
	if got := attributesOf(t, byName(spans)["invoke_agent claude-code"])["telmoni.sample_rate"]; got != 0.25 {
		t.Errorf("telmoni.sample_rate = %v, want the client's 0.25", got)
	}
}

func TestARecordRefusedReturnsAnErrorAndSendsNothing(t *testing.T) {
	const outOfRange = "telmoni: a recorded time must fall after the Unix epoch and less than 2^64 nanoseconds after it"
	start := time.Date(2026, 10, 12, 9, 0, 0, 0, time.UTC)
	backwards := telmoni.StepRecord{
		Name:  "s",
		Start: start,
		End:   start.Add(2 * time.Second),
		Calls: []telmoni.CallRecord{telmoni.ToolCallRecord{Name: "t", Start: start.Add(time.Second), End: start}},
	}
	for _, tc := range []struct {
		name   string
		change func(*telmoni.RunRecord)
		want   string
	}{
		{"a blank agent", func(r *telmoni.RunRecord) { r.Agent = " " }, "agent's name"},
		{"a run id that is no UUID", func(r *telmoni.RunRecord) { r.RunID = "run-1" }, "must be a UUID"},
		{"a run id that is not hex", func(r *telmoni.RunRecord) { r.RunID = "3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5g" }, "must be a UUID"},
		{"the nil UUID", func(r *telmoni.RunRecord) { r.RunID = "00000000-0000-0000-0000-000000000000" }, "nil UUID"},
		{"a zero start", func(r *telmoni.RunRecord) { r.Start = time.Time{} }, "cannot be zero"},
		{"a call with no end", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.ModelCallRecord{Model: "m", Start: start})
		}, "cannot be zero"},
		{"a run that ends before it starts", func(r *telmoni.RunRecord) { r.End = r.Start.Add(-time.Second) }, "end before it starts"},
		{"a call that ends before it starts, last in the record", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, backwards)
		}, "end before it starts"},
		{"a start at the Unix epoch", func(r *telmoni.RunRecord) { r.Start = time.Unix(0, 0) }, outOfRange},
		{"a step that starts before 1970", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.StepRecord{Name: "s", Start: time.Date(1969, 12, 31, 23, 59, 59, 0, time.UTC), End: start})
		}, outOfRange},
		{"a call that ends 2^64 nanoseconds past the epoch", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.ToolCallRecord{Name: "t", Start: start, End: time.Date(2554, 7, 21, 23, 34, 33, 709551616, time.UTC)})
		}, outOfRange},
		{"a run that ends in 3000", func(r *telmoni.RunRecord) { r.End = time.Date(3000, 1, 1, 0, 0, 0, 0, time.UTC) }, outOfRange},
		{"a step with no name", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.StepRecord{Start: start, End: start})
		}, "cannot be empty"},
		{"a model call with no model, in a step", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.StepRecord{Name: "s", Start: start, End: start, Calls: []telmoni.CallRecord{
				telmoni.ModelCallRecord{Start: start, End: start},
			}})
		}, "cannot be empty"},
		{"a model call with no model", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.ModelCallRecord{Start: start, End: start})
		}, "cannot be empty"},
		{"an agent of 129 characters", func(r *telmoni.RunRecord) { r.Agent = strings.Repeat("a", 129) },
			"telmoni: a recorded run's Agent must be up to 128 characters"},
		{"a user of 129 characters", func(r *telmoni.RunRecord) { r.UserID = strings.Repeat("u", 129) },
			"telmoni: a recorded run's UserID must be up to 128 characters"},
		{"a session of 201 characters", func(r *telmoni.RunRecord) { r.SessionID = strings.Repeat("s", 201) },
			"telmoni: a recorded run's SessionID must be up to 200 characters"},
		{"a run's error type of 129 characters", func(r *telmoni.RunRecord) { r.ErrorType = strings.Repeat("e", 129) },
			"telmoni: a recorded run's ErrorType must be up to 128 characters"},
		{"a step's name of 146 characters", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.StepRecord{Name: strings.Repeat("s", 146), Start: start, End: start})
		}, "telmoni: a recorded step's Name must be up to 145 characters"},
		{"a step's error type of 129 characters", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.StepRecord{Name: "s", ErrorType: strings.Repeat("e", 129), Start: start, End: start})
		}, "telmoni: a recorded step's ErrorType must be up to 128 characters"},
		{"a model of 129 characters", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.ModelCallRecord{Model: strings.Repeat("m", 129), Start: start, End: start})
		}, "telmoni: a recorded model call's Model must be up to 128 characters"},
		{"a provider of 129 characters", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.ModelCallRecord{Model: "m", Provider: strings.Repeat("p", 129), Start: start, End: start})
		}, "telmoni: a recorded model call's Provider must be up to 128 characters"},
		{"a model call's error type of 129 characters", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.ModelCallRecord{Model: "m", ErrorType: strings.Repeat("e", 129), Start: start, End: start})
		}, "telmoni: a recorded model call's ErrorType must be up to 128 characters"},
		{"an operation and a model of 145 characters together", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.ModelCallRecord{Operation: strings.Repeat("o", 17), Model: strings.Repeat("m", 128), Start: start, End: start})
		}, "telmoni: a recorded model call's operation and model must be up to 144 characters together"},
		{"an operation and a model of 145 characters together, in a step", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.StepRecord{Name: "s", Start: start, End: start, Calls: []telmoni.CallRecord{
				telmoni.ModelCallRecord{Operation: strings.Repeat("é", 20), Model: strings.Repeat("é", 125), Start: start, End: start},
			}})
		}, "telmoni: a recorded model call's operation and model must be up to 144 characters together"},
		{"a tool's name of 146 characters", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.ToolCallRecord{Name: strings.Repeat("t", 146), Start: start, End: start})
		}, "telmoni: a recorded tool call's Name must be up to 145 characters"},
		{"a tool call's error type of 129 characters, in a step", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.StepRecord{Name: "s", Start: start, End: start, Calls: []telmoni.CallRecord{
				telmoni.ToolCallRecord{Name: "t", ErrorType: strings.Repeat("e", 129), Start: start, End: start},
			}})
		}, "telmoni: a recorded tool call's ErrorType must be up to 128 characters"},
		{"a nil item", func(r *telmoni.RunRecord) { r.Inside = append(r.Inside, nil) }, "a recorded item must be"},
		{"a nil pointer to a call", func(r *telmoni.RunRecord) {
			r.Inside = append(r.Inside, telmoni.StepRecord{
				Name:  "s",
				Start: start,
				End:   start,
				Calls: []telmoni.CallRecord{(*telmoni.ToolCallRecord)(nil)},
			})
		}, "a recorded item must be"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			h := newHarness(t, testConfig(), nil)
			record := theRecord(t)
			tc.change(&record)
			err := h.client.RecordRun(context.Background(), record)
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Errorf("RecordRun = %v, want an error naming %q", err, tc.want)
			}
			h.flush()
			if sent := len(h.transport.all()); sent != 0 {
				t.Errorf("a record refused sent %d exports", sent)
			}
		})
	}
}

// Ingest's first and last times, names of spaces, and a tool's empty name,
// which ingest reads as none, are sent as given.
func TestARecordAtTheEdgesOfWhatIngestReadsIsSentAsGiven(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	first, last := time.Unix(0, 1), time.Date(2554, 7, 21, 23, 34, 33, 709551615, time.UTC)
	record := theRecord(t)
	record.Start, record.End = first, last
	record.Inside = []telmoni.RecordItem{telmoni.StepRecord{Name: " ", Start: first, End: last, Calls: []telmoni.CallRecord{
		telmoni.ToolCallRecord{Name: " ", Start: first, End: last},
		telmoni.ToolCallRecord{Start: first, End: last},
		telmoni.ModelCallRecord{Model: " ", Start: first, End: last},
	}}}
	recordRun(t, h, record)
	spans := h.transport.spans(t)
	want := []string{" ", "chat  ", "execute_tool ", "execute_tool  ", "invoke_agent claude-code", "telmoni.run.start"}
	if got := sortedNames(spans); !reflect.DeepEqual(got, want) {
		t.Fatalf("spans sent: %q, want %q", got, want)
	}
	for _, span := range spans {
		end := "18446744073709551615"
		if span["name"] == "telmoni.run.start" {
			end = "1"
		}
		if span["startTimeUnixNano"] != "1" || span["endTimeUnixNano"] != end {
			t.Errorf("%q runs from %v to %v, want from 1 to %s", span["name"], span["startTimeUnixNano"], span["endTimeUnixNano"], end)
		}
	}
}

// Text at ingest's caps, counted in characters, is sent as given.
func TestARecordAtIngestsCapsIsSentAsGiven(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	// Two bytes a character: counted in bytes, each would be past its cap.
	wide := func(n int) string { return strings.Repeat("é", n) }
	record := theRecord(t)
	start := record.Start
	record.Agent, record.UserID, record.SessionID, record.ErrorType = wide(128), wide(128), wide(200), wide(128)
	record.Inside = []telmoni.RecordItem{
		telmoni.StepRecord{Name: wide(145), ErrorType: wide(128), Start: start, End: start, Calls: []telmoni.CallRecord{
			telmoni.ToolCallRecord{Name: wide(145), ErrorType: wide(128), Start: start, End: start},
		}},
		telmoni.ModelCallRecord{Model: wide(128), Provider: wide(128), ErrorType: wide(128), Start: start, End: start},
		telmoni.ModelCallRecord{Operation: "generate_content", Model: wide(128), Start: start, End: start},
	}
	recordRun(t, h, record)
	spans := h.transport.spans(t)
	if len(spans) != 6 {
		t.Fatalf("sent %d spans, want the record's 6", len(spans))
	}
	for name, want := range map[string]map[string]any{
		"invoke_agent " + wide(128):     {"telmoni.agent": wide(128), "user.id": wide(128), "gen_ai.conversation.id": wide(200), "error.type": wide(128)},
		wide(145):                       {"error.type": wide(128)},
		"execute_tool " + wide(145):     {"gen_ai.tool.name": wide(145), "error.type": wide(128)},
		"chat " + wide(128):             {"gen_ai.request.model": wide(128), "gen_ai.provider.name": wide(128), "error.type": wide(128)},
		"generate_content " + wide(128): {"gen_ai.request.model": wide(128)},
	} {
		span := byName(spans)[name]
		if span == nil {
			t.Fatalf("a span of %d characters was not sent", len([]rune(name)))
		}
		attributes := attributesOf(t, span)
		for key, value := range want {
			if attributes[key] != value {
				t.Errorf("%s of a span of %d characters is not as given", key, len([]rune(name)))
			}
		}
	}
	if warnings := h.logs.warnings(); len(warnings) != 0 {
		t.Errorf("warnings = %q, want none", warnings)
	}
}

// A run id is read in either case and sent in lowercase: in capitals it is
// the same run, and the same bytes.
func TestARunIDInCapitalsIsTheSameRun(t *testing.T) {
	record := theRecord(t)
	lower := newHarness(t, testConfig(), nil)
	recordRun(t, lower, record)
	record.RunID = strings.ToUpper(record.RunID)
	upper := newHarness(t, testConfig(), nil)
	recordRun(t, upper, record)
	sameBody(t, upper, lower)
}

func TestItemsGivenByPointerAreReadThroughThem(t *testing.T) {
	record := theRecord(t)
	byValue := newHarness(t, testConfig(), nil)
	recordRun(t, byValue, record)
	pointer := func(call telmoni.CallRecord) telmoni.CallRecord {
		switch call := call.(type) {
		case telmoni.ModelCallRecord:
			return &call
		case telmoni.ToolCallRecord:
			return &call
		}
		return call
	}
	inside := make([]telmoni.RecordItem, 0, len(record.Inside))
	for _, item := range record.Inside {
		switch item := item.(type) {
		case telmoni.StepRecord:
			calls := make([]telmoni.CallRecord, 0, len(item.Calls))
			for _, call := range item.Calls {
				calls = append(calls, pointer(call))
			}
			item.Calls = calls
			inside = append(inside, &item)
		case telmoni.CallRecord:
			inside = append(inside, pointer(item))
		}
	}
	record.Inside = inside
	byPointer := newHarness(t, testConfig(), nil)
	recordRun(t, byPointer, record)
	sameBody(t, byPointer, byValue)
}

// A record as large as a batch, its run, its start and 510 calls, travels in
// one export, its start first and its run last.
func TestARecordAsLargeAsABatchTravelsInOneExport(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	record := theRecord(t)
	record.Inside = nil
	for n := range 510 {
		at := record.Start.Add(time.Duration(n) * time.Millisecond)
		record.Inside = append(record.Inside, telmoni.ToolCallRecord{Name: "t", Start: at, End: at})
	}
	recordRun(t, h, record)
	bodies := h.transport.bodies(t)
	if len(bodies) != 1 {
		t.Fatalf("sent %d exports, want a record of a batch's size in one", len(bodies))
	}
	spans := names(spansIn(bodies[0]))
	if len(spans) != 512 {
		t.Fatalf("the export holds %d spans, want 512", len(spans))
	}
	if spans[0] != "telmoni.run.start" || spans[511] != "invoke_agent claude-code" {
		t.Errorf("the export runs from %s to %s, want from the start to the run", spans[0], spans[511])
	}
}

// A record the queue has no room for is refused whole, before anything is
// queued, since its tail — the run's own span first — would otherwise be
// dropped; drained, the queue takes it whole.
func TestARecordTheQueueHasNoRoomForIsRefusedWhole(t *testing.T) {
	// Room for five spans, and a worker that will not wake until flushed.
	h := newHarness(t, testConfig(), nil, telmoni.WithQueue(5, 100, time.Minute))
	record := theRecord(t)
	record.Inside = nil
	for range 2 {
		record.Inside = append(record.Inside, telmoni.ToolCallRecord{Name: "t", Start: record.Start, End: record.Start})
	}
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	// The start goes alone; two steps then hold two of the five slots.
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	for n := range 2 {
		_, step := run.Step(ctx, fmt.Sprintf("s%d", n))
		step.End()
	}
	if err := h.client.RecordRun(context.Background(), record); !errors.Is(err, telmoni.ErrQueueFull) {
		t.Fatalf("RecordRun with room for three of the record's four spans = %v, want ErrQueueFull", err)
	}
	run.End()
	h.flush()
	if err := h.client.RecordRun(context.Background(), record); err != nil {
		t.Fatalf("RecordRun with the queue drained = %v", err)
	}
	h.flush()
	got := names(h.transport.spans(t))
	calls := 0
	for _, name := range got {
		if name == "execute_tool t" {
			calls++
		}
	}
	if calls != 2 || len(got) != 8 {
		t.Errorf("spans sent: %v, want the live run's four and the record's four", got)
	}
	if warnings := h.logs.warnings(); len(warnings) != 0 {
		t.Errorf("warnings: %q, want none, since nothing was dropped", warnings)
	}
}

// tracesIn is the set of traces an export's spans are in.
func tracesIn(t *testing.T, body []byte) map[string]bool {
	t.Helper()
	var decoded map[string]any
	if err := json.Unmarshal(body, &decoded); err != nil {
		t.Fatalf("decoding an export: %v", err)
	}
	out := map[string]bool{}
	for _, span := range spansIn(decoded) {
		out[span["traceId"].(string)] = true
	}
	return out
}

// A span queued before a record or ending after it, and a record beside it,
// go in other exports, so the record is the same bytes as when sent alone.
func TestARecordTravelsInExportsOfItsOwn(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	record := theRecord(t)
	beside := record
	beside.RunID = "0191a3b4-2222-4aaa-8bbb-000000000002"
	ctx, live := h.client.Run(context.Background(), "live", telmoni.RunOptions{})
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	_, step := live.Step(ctx, "live step")
	step.End()
	for _, r := range []telmoni.RunRecord{record, beside} {
		if err := h.client.RecordRun(context.Background(), r); err != nil {
			t.Fatalf("RecordRun: %v", err)
		}
	}
	live.End()
	h.flush()
	sent := len(h.transport.all())
	recordRun(t, h, record)
	requests := h.transport.all()
	var first, other [][]byte
	for i, request := range requests {
		traces := tracesIn(t, request.body)
		if len(traces) != 1 {
			t.Errorf("export %d is in traces %v, want one", i, traces)
		}
		if i < sent && traces[strings.ReplaceAll(record.RunID, "-", "")] {
			first = append(first, request.body)
		}
		if traces[strings.ReplaceAll(beside.RunID, "-", "")] {
			other = append(other, request.body)
		}
	}
	if len(requests) != sent+1 || len(first) != 1 || !bytes.Equal(first[0], requests[sent].body) {
		t.Errorf("the record sent beside the live run is not the bytes it is sent again as")
	}
	if len(other) != 1 {
		t.Errorf("the record beside it went in %d exports, want one of its own", len(other))
	}
}

// A record larger than a batch is cut into batches counted from its own
// start, wherever it lands in the queue.
func TestARecordLargerThanABatchIsCutFromItsOwnStart(t *testing.T) {
	h := newHarness(t, testConfig(), nil, telmoni.WithQueue(2048, 3, time.Minute))
	record := theRecord(t)
	record.Inside = nil
	for range 3 {
		record.Inside = append(record.Inside, telmoni.ToolCallRecord{Name: "t", Start: record.Start, End: record.Start})
	}
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	for n := range 2 {
		_, step := run.Step(ctx, fmt.Sprintf("s%d", n))
		step.End()
	}
	if err := h.client.RecordRun(context.Background(), record); err != nil {
		t.Fatalf("RecordRun: %v", err)
	}
	run.End()
	h.flush()
	var got [][]string
	for _, body := range h.transport.bodies(t) {
		got = append(got, names(spansIn(body)))
	}
	want := [][]string{
		{"telmoni.run.start"},
		{"s0", "s1"},
		{"telmoni.run.start", "execute_tool t", "execute_tool t"},
		{"execute_tool t", "invoke_agent claude-code"},
		{"invoke_agent bot"},
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("exports: %v\nwant: %v", got, want)
	}
}

// Nothing a caller passes can fail a build part-way, so the guard is held
// here, below RecordRun, to queue only a record built whole.
func TestARecordNotBuiltQueuesNothing(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	runID := [16]byte{1}
	if !telmoni.HoldOne(h.client, runID, false) {
		t.Fatal("the hold was refused")
	}
	h.flush()
	if sent := len(h.transport.all()); sent != 0 {
		t.Fatalf("%d exports of a record not built, want none", sent)
	}
	if !telmoni.HoldOne(h.client, runID, true) {
		t.Fatal("the hold was refused")
	}
	h.flush()
	if sent := len(h.transport.all()); sent != 1 {
		t.Errorf("%d exports of a record built, want its one", sent)
	}
}

// hookedSampler calls its hook once, as the next span with no parent starts:
// a record's own span, mid-build.
type hookedSampler struct {
	hook func()
}

func (s *hookedSampler) ShouldSample(sdktrace.SamplingParameters) sdktrace.SamplingResult {
	if hook := s.hook; hook != nil {
		s.hook = nil
		hook()
	}
	return sdktrace.SamplingResult{Decision: sdktrace.RecordAndSample}
}

func (*hookedSampler) Description() string { return "hooked" }

// While a record is built its room in the queue is kept: a live step ending
// meanwhile is dropped past it, so the queue never holds more than its size.
func TestARecordsRoomIsKeptWhileItIsBuilt(t *testing.T) {
	sampler := &hookedSampler{}
	cfg := testConfig()
	cfg.Sampler = sampler
	// Room for five spans, and a worker that will not wake until flushed.
	h := newHarness(t, cfg, nil, telmoni.WithQueue(5, 100, time.Minute))
	record := theRecord(t)
	record.Inside = nil
	for range 2 {
		record.Inside = append(record.Inside, telmoni.ToolCallRecord{Name: "t", Start: record.Start, End: record.Start})
	}
	ctx, live := h.client.Run(context.Background(), "live", telmoni.RunOptions{})
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	// Room for one step beside the record's four spans.
	sampler.hook = func() {
		for n := range 2 {
			_, step := live.Step(ctx, fmt.Sprintf("s%d", n))
			step.End()
		}
	}
	if err := h.client.RecordRun(context.Background(), record); err != nil {
		t.Fatalf("RecordRun: %v", err)
	}
	h.flush()
	live.End()
	h.flush()
	want := []string{"execute_tool t", "execute_tool t", "invoke_agent claude-code", "invoke_agent live", "s0", "telmoni.run.start", "telmoni.run.start"}
	if got := sortedNames(h.transport.spans(t)); !reflect.DeepEqual(got, want) {
		t.Errorf("spans sent: %v\nwant: %v", got, want)
	}
	if warnings := h.logs.warnings(); len(warnings) != 1 || !strings.Contains(warnings[0], "1 spans were dropped") {
		t.Errorf("warnings = %q, want the one live step past the record's room", warnings)
	}
}

// A record queued behind a live step goes in the export after the step's, as
// a live run's start would, with no flush.
func TestARecordGoesAtOnce(t *testing.T) {
	h := newHarness(t, testConfig(), nil, telmoni.WithQueue(2048, 512, time.Minute))
	ctx, live := h.client.Run(context.Background(), "live", telmoni.RunOptions{})
	if !h.transport.waitFor(1, waitTimeout) {
		t.Fatal("the run's start was not sent at once")
	}
	_, step := live.Step(ctx, "live step")
	step.End()
	if err := h.client.RecordRun(context.Background(), theRecord(t)); err != nil {
		t.Fatalf("RecordRun: %v", err)
	}
	if !h.transport.waitFor(3, waitTimeout) {
		t.Error("the record waited for the delay")
	}
	live.End()
}

// A live step ending on another goroutine while a record is built is in no
// export of the record's. The queue is large, so the goroutine never fills it.
func TestASpanEndingWhileARecordIsBuiltTravelsApart(t *testing.T) {
	for range 20 {
		h := newHarness(t, testConfig(), nil, telmoni.WithQueue(1<<16, 512, 5*time.Second))
		ctx, live := h.client.Run(context.Background(), "live", telmoni.RunOptions{})
		var stop atomic.Bool
		done := make(chan struct{})
		go func() {
			defer close(done)
			for !stop.Load() {
				_, step := live.Step(ctx, "live step")
				step.End()
			}
		}()
		err := h.client.RecordRun(context.Background(), theRecord(t))
		stop.Store(true)
		<-done
		if err != nil {
			t.Fatalf("RecordRun: %v", err)
		}
		live.End()
		h.flush()
		for i, request := range h.transport.all() {
			if traces := tracesIn(t, request.body); len(traces) != 1 {
				t.Fatalf("export %d is in traces %v, want one", i, traces)
			}
		}
	}
}

// recordingLogger keeps each warning, recording a run of its own as it logs
// the first, as a handler sending the application's logs to Telmoni might.
type recordingLogger struct {
	logSink
	client *telmoni.Client
	once   sync.Once
	err    error
}

func (l *recordingLogger) Handle(ctx context.Context, r slog.Record) error {
	l.once.Do(func() {
		at := time.Date(2026, 10, 18, 9, 0, 0, 0, time.UTC)
		l.err = l.client.RecordRun(ctx, telmoni.RunRecord{RunID: "0191a3b4-3333-4aaa-8bbb-000000000003", Agent: "logged", Start: at, End: at})
	})
	return l.logSink.Handle(ctx, r)
}

func (l *recordingLogger) WithAttrs([]slog.Attr) slog.Handler { return l }
func (l *recordingLogger) WithGroup(string) slog.Handler      { return l }

// recordWarnedOf records record through a client whose logger records a run
// as it warns, failing the test should a warning wait on the record's hold.
func recordWarnedOf(t *testing.T, record telmoni.RunRecord) (*harness, *recordingLogger) {
	t.Helper()
	logs := &recordingLogger{}
	h := newHarness(t, testConfig(), nil, telmoni.WithLogger(slog.New(logs)))
	logs.client = h.client
	done := make(chan error, 1)
	go func() { done <- h.client.RecordRun(context.Background(), record) }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("RecordRun: %v", err)
		}
	case <-time.After(waitTimeout):
		t.Fatal("RecordRun waits on its own hold: a warning was logged under it")
	}
	h.flush()
	if spans := h.transport.spans(t); logs.err != nil || byName(spans)["invoke_agent logged"] == nil {
		t.Fatalf("the logger's own record (%v) is not among the spans sent: %v", logs.err, sortedNames(spans))
	}
	return h, logs
}

// A record sends its first 32 tags by key, warned of before its hold.
func TestARecordSendsItsFirst32TagsByKeyWarnedOfBeforeItsHold(t *testing.T) {
	record := theRecord(t)
	record.Tags = numberedTags(130)
	h, logs := recordWarnedOf(t, record)
	run := byName(h.transport.spans(t))["invoke_agent claude-code"]
	if run == nil {
		t.Fatalf("spans sent: %v", sortedNames(h.transport.spans(t)))
	}
	attributes := attributesOf(t, run)
	if got, want := tagsOf(attributes), slices.Sorted(maps.Keys(numberedTags(32))); !reflect.DeepEqual(got, want) {
		t.Errorf("tags sent: %v\nwant the first 32 by key: %v", got, want)
	}
	if attributes["telmoni.cancelled"] != true {
		t.Errorf("telmoni.cancelled = %v, want the mark the run ended with", attributes["telmoni.cancelled"])
	}
	want := []string{"telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent"}
	if warnings := logs.warnings(); !reflect.DeepEqual(warnings, want) {
		t.Errorf("warnings = %q, want %q", warnings, want)
	}
}

// A tag ingest drops takes none of a record's 32, warned of before its hold.
func TestATagIngestDropsTakesNoneOfARecords32WarnedOfBeforeItsHold(t *testing.T) {
	record := theRecord(t)
	record.Tags = numberedTags(32)
	maps.Copy(record.Tags, droppedTags())
	h, logs := recordWarnedOf(t, record)
	run := byName(h.transport.spans(t))["invoke_agent claude-code"]
	if run == nil {
		t.Fatalf("spans sent: %v", sortedNames(h.transport.spans(t)))
	}
	if got, want := tagsOf(attributesOf(t, run)), slices.Sorted(maps.Keys(numberedTags(32))); !reflect.DeepEqual(got, want) {
		t.Errorf("tags sent: %q\nwant all 32 ingest keeps: %q", got, want)
	}
	want := []string{"telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent"}
	if warnings := logs.warnings(); !reflect.DeepEqual(warnings, want) {
		t.Errorf("warnings = %q, want %q", warnings, want)
	}
}

// A record's count below zero is left off its call, warned of before the hold.
func TestARecordedTokenCountBelowZeroIsLeftOffWarnedOfBeforeItsHold(t *testing.T) {
	record := theRecord(t)
	start := record.Start
	record.Inside = []telmoni.RecordItem{
		telmoni.ModelCallRecord{Model: "alone", Start: start, End: start, Usage: &telmoni.Usage{InputTokens: 5, CacheWriteInputTokens: -2}},
		telmoni.StepRecord{Name: "s", Start: start, End: start, Calls: []telmoni.CallRecord{
			&telmoni.ModelCallRecord{Model: "in a step", Start: start, End: start, Usage: &telmoni.Usage{OutputTokens: -1}},
		}},
	}
	h, logs := recordWarnedOf(t, record)
	spans := byName(h.transport.spans(t))
	for name, want := range map[string]map[string]any{
		"chat alone": {
			"gen_ai.usage.input_tokens":            int64(5),
			"gen_ai.usage.output_tokens":           int64(0),
			"gen_ai.usage.cache_read.input_tokens": int64(0),
		},
		"chat in a step": {
			"gen_ai.usage.input_tokens":             int64(0),
			"gen_ai.usage.cache_read.input_tokens":  int64(0),
			"gen_ai.usage.cache_write.input_tokens": int64(0),
		},
	} {
		if spans[name] == nil {
			t.Fatalf("spans sent: %v", sortedNames(h.transport.spans(t)))
		}
		if got := attributesUnder(attributesOf(t, spans[name]), "gen_ai.usage."); !reflect.DeepEqual(got, want) {
			t.Errorf("%s carries the counts %v, want %v", name, got, want)
		}
	}
	want := []string{
		"telmoni: gen_ai.usage.cache_write.input_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
		"telmoni: gen_ai.usage.output_tokens must be a whole number from 0 within 64 bits; the call is sent without it",
	}
	if warnings := logs.warnings(); !reflect.DeepEqual(warnings, want) {
		t.Errorf("warnings = %q\nwant one for each count: %q", warnings, want)
	}
}
