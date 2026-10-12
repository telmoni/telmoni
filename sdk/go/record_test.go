package telmoni_test

// A run recorded after it ended: RecordRun, and the export every SDK must send
// for sdk/fixtures/recorded.json.

import (
	"bytes"
	"context"
	"encoding/json"
	"reflect"
	"strings"
	"testing"
	"time"

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
	// The run's own span took its id back out of those sampled out, so the
	// same record, drawn in, is sent.
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
