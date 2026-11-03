package telmoni_test

// Scores, live on a handle, late through the client and recorded, as
// sdk/fixtures/scores.json has every SDK send them.

import (
	"context"
	"encoding/json"
	"errors"
	"math"
	"net/http"
	"reflect"
	"strings"
	"testing"
	"time"

	"go.opentelemetry.io/otel/attribute"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
	"go.opentelemetry.io/otel/trace"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

type scoreSpec struct {
	Name    string `json:"name"`
	Type    string `json:"type"`
	Value   any    `json:"value"`
	Range   []any  `json:"range"`
	Version string `json:"version"`
}

type scoresFixture struct {
	Limits map[string]int `json:"limits"`
	Scores []struct {
		Case string `json:"case"`
		scoreSpec
		Kept *struct {
			Name    string   `json:"name"`
			Value   *float64 `json:"value"`
			Label   string   `json:"label"`
			Version string   `json:"version"`
		} `json:"kept"`
	} `json:"scores"`
	Targets []struct {
		Case      string            `json:"case"`
		RunID     string            `json:"run_id"`
		SpanID    string            `json:"span_id"`
		SessionID string            `json:"session_id"`
		Kept      map[string]string `json:"kept"`
	} `json:"targets"`
	Late struct {
		Scores []struct {
			Raw map[string]string `json:"target"`
			scoreSpec
		} `json:"scores"`
		Sent map[string]any `json:"sent"`
	} `json:"late"`
	Live struct {
		Scores []scoreSpec `json:"scores"`
		Events []any       `json:"events"`
	} `json:"live"`
}

func loadScores(t *testing.T) scoresFixture {
	t.Helper()
	var fixture scoresFixture
	if err := json.Unmarshal(loadFixture(t, "scores.json"), &fixture); err != nil {
		t.Fatalf("decoding scores.json: %v", err)
	}
	return fixture
}

// fixtureNumber is a fixture's number, NaN and the infinities written as text.
func fixtureNumber(t *testing.T, raw any) float64 {
	t.Helper()
	switch v := raw.(type) {
	case float64:
		return v
	case string:
		switch v {
		case "NaN":
			return math.NaN()
		case "Infinity":
			return math.Inf(1)
		case "-Infinity":
			return math.Inf(-1)
		}
	}
	t.Fatalf("not a fixture number: %v", raw)
	return 0
}

// scoreOf is a fixture's score as the Go API takes it.
func scoreOf(t *testing.T, spec scoreSpec) (telmoni.ScoreValue, telmoni.ScoreOptions) {
	t.Helper()
	opts := telmoni.ScoreOptions{Version: spec.Version}
	if spec.Range != nil {
		opts.Range = &[2]float64{fixtureNumber(t, spec.Range[0]), fixtureNumber(t, spec.Range[1])}
	}
	switch spec.Type {
	case "number":
		return telmoni.Number(fixtureNumber(t, spec.Value)), opts
	case "boolean":
		return telmoni.Bool(spec.Value.(bool)), opts
	case "category":
		return telmoni.Category(spec.Value.(string)), opts
	}
	t.Fatalf("no score type %q", spec.Type)
	return telmoni.ScoreValue{}, opts
}

// contentOff is a client keeping no content, as by default: a score's event
// is metadata, and leaves all the same.
func contentOff(t *testing.T) *harness {
	t.Helper()
	return newHarness(t, testConfig(), nil)
}

// eventsOf is the events of the span named name in every export.
func eventsOf(t *testing.T, h *harness, name string) []any {
	t.Helper()
	span, ok := byName(h.transport.spans(t))[name]
	if !ok {
		t.Fatalf("no span %q sent", name)
	}
	events, _ := span["events"].([]any)
	return events
}

// expectOneRefusal is one input warning, the score not sent.
func expectOneRefusal(t *testing.T, h *harness, why string) {
	t.Helper()
	warned := h.logs.warnings()
	if len(warned) != 1 || !strings.HasPrefix(warned[0], "telmoni: a score was not sent: ") ||
		(why != "" && warned[0] != "telmoni: a score was not sent: "+why) {
		t.Errorf("warnings = %q", warned)
	}
	if fields := h.logs.fields(); len(fields) != 1 || fields[0]["kind"] != "input" {
		t.Errorf("fields = %v", fields)
	}
}

func scoreRequests(h *harness) []request {
	var out []request
	for _, req := range h.transport.all() {
		if strings.HasSuffix(req.url, "/v1/scores") {
			out = append(out, req)
		}
	}
	return out
}

func TestEveryScoreIsKeptOrRefusedAsTheFixtureSays(t *testing.T) {
	for _, c := range loadScores(t).Scores {
		t.Run(c.Case, func(t *testing.T) {
			h := contentOff(t)
			value, opts := scoreOf(t, c.scoreSpec)
			_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
			run.Score(c.Name, value, opts)
			run.End()
			h.flush()
			events := eventsOf(t, h, "invoke_agent bot")
			if c.Kept == nil {
				if len(events) != 0 {
					t.Errorf("a refused score sent %v", events)
				}
				expectOneRefusal(t, h, "")
				return
			}
			want := map[string]any{"gen_ai.evaluation.name": c.Kept.Name}
			if c.Kept.Value != nil {
				want["gen_ai.evaluation.score.value"] = *c.Kept.Value
			}
			if c.Kept.Label != "" {
				want["gen_ai.evaluation.score.label"] = c.Kept.Label
			}
			if c.Type == "boolean" {
				want["telmoni.score.type"] = "boolean"
			}
			if opts.Range != nil {
				want["telmoni.score.range_min"], want["telmoni.score.range_max"] = opts.Range[0], opts.Range[1]
			}
			if c.Kept.Version != "" {
				want["telmoni.score.version"] = c.Kept.Version
			}
			if len(events) != 1 {
				t.Fatalf("events = %v", events)
			}
			event := events[0].(map[string]any)
			if got := attributesOf(t, event); event["name"] != "gen_ai.evaluation.result" || !reflect.DeepEqual(got, want) {
				t.Errorf("event\n got: %v\nwant: %v", got, want)
			}
			expectWarnings(t, h)
		})
	}
}

func TestEveryTargetIsKeptOrRefusedAsTheFixtureSays(t *testing.T) {
	for _, c := range loadScores(t).Targets {
		t.Run(c.Case, func(t *testing.T) {
			h := newHarness(t, testConfig(), nil)
			h.client.Score(telmoni.ScoreTarget{RunID: c.RunID, SpanID: c.SpanID, SessionID: c.SessionID}, "relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
			h.flush()
			if c.Kept == nil {
				expectRequests(t, h, 0)
				expectOneRefusal(t, h, "it names a run, a run and one of its spans by 16 hex digits, or a session of up to 200 characters")
				return
			}
			sent := h.transport.bodies(t)
			if len(sent) != 1 || len(scoreRequests(h)) != 1 {
				t.Fatalf("sent %v", sent)
			}
			item := sent[0]["scores"].([]any)[0].(map[string]any)
			got := map[string]string{}
			for _, key := range []string{"run_id", "span_id", "session_id"} {
				if id, ok := item[key]; ok {
					got[key] = id.(string)
				}
			}
			if !reflect.DeepEqual(got, c.Kept) {
				t.Errorf("ids\n got: %v\nwant: %v", got, c.Kept)
			}
			expectWarnings(t, h)
		})
	}
}

func TestLateScoresLeaveInOnePostAsTheFixtureSendsThem(t *testing.T) {
	fixture := loadScores(t)
	h := newHarness(t, testConfig(), nil)
	for _, s := range fixture.Late.Scores {
		value, opts := scoreOf(t, s.scoreSpec)
		target := telmoni.ScoreTarget{RunID: s.Raw["run_id"], SpanID: s.Raw["span_id"], SessionID: s.Raw["session_id"]}
		h.client.Score(target, s.Name, value, opts)
	}
	h.flush()
	all := h.transport.all()
	if len(all) != 1 || !strings.HasSuffix(all[0].url, "/v1/scores") {
		t.Fatalf("sent %d requests: %v", len(all), all)
	}
	header := all[0].header
	if header.Get("Content-Type") != "application/json" || header.Get("Authorization") != "Bearer "+testKey ||
		!strings.HasPrefix(header.Get("User-Agent"), "telmoni-go/") {
		t.Errorf("headers = %v", header)
	}
	var body map[string]any
	if err := json.Unmarshal(all[0].body, &body); err != nil {
		t.Fatalf("decoding the scores: %v", err)
	}
	for _, raw := range body["scores"].([]any) {
		item := raw.(map[string]any)
		if id, _ := item["id"].(string); !uuidV4.MatchString(id) {
			t.Errorf("id %v is no UUID v4", item["id"])
		}
		item["id"] = "<uuid>"
	}
	if !reflect.DeepEqual(body, fixture.Late.Sent) {
		t.Errorf("sent\n got: %s\nwant: %s", pretty(t, body), pretty(t, fixture.Late.Sent))
	}
	expectWarnings(t, h)
}

func TestLiveScoresAreTheFixturesEvents(t *testing.T) {
	fixture := loadScores(t)
	h := contentOff(t)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for _, s := range fixture.Live.Scores {
		value, opts := scoreOf(t, s)
		run.Score(s.Name, value, opts)
	}
	spanID := run.SpanID()
	run.End()
	h.flush()
	span := byName(h.transport.spans(t))["invoke_agent bot"]
	if span["spanId"] != spanID || !hex16.MatchString(spanID) {
		t.Errorf("SpanID() = %q, sent %v", spanID, span["spanId"])
	}
	start, end := parseNanos(t, "start", span["startTimeUnixNano"]), parseNanos(t, "end", span["endTimeUnixNano"])
	events, _ := span["events"].([]any)
	for _, raw := range events {
		event := raw.(map[string]any)
		if at := parseNanos(t, "event", event["timeUnixNano"]); at < start || at > end {
			t.Errorf("an event at %d, outside its span's %d to %d", at, start, end)
		}
		event["timeUnixNano"] = "<time>"
		sortByKey(event["attributes"].([]any))
	}
	if !reflect.DeepEqual(events, fixture.Live.Events) {
		t.Errorf("events\n got: %s\nwant: %s", pretty(t, events), pretty(t, fixture.Live.Events))
	}
	expectWarnings(t, h)
}

func TestScoresRefusedAreWarnedOnceAndNeverRetried(t *testing.T) {
	h := newHarness(t, testConfig(), newFakeTransport(t, scripted{status: http.StatusBadRequest, body: []byte("{}")}))
	h.client.Score(telmoni.ScoreTarget{SessionID: "conv-1"}, "relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
	h.flush()
	expectRequests(t, h, 1)
	expectSleeps(t, h)
	expectWarnings(t, h, "telmoni: scores refused with 400; they were dropped")
	expectFields(t, h, map[string]any{"kind": "refused", "status": int64(400)})
}

func TestScoresUnavailableAreSentAgain(t *testing.T) {
	h := newHarness(t, testConfig(), newFakeTransport(t, scripted{status: http.StatusServiceUnavailable}, scripted{status: http.StatusNoContent}))
	h.client.Score(telmoni.ScoreTarget{SessionID: "conv-1"}, "relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
	h.flush()
	all := h.transport.all()
	if len(all) != 2 || string(all[0].body) != string(all[1].body) {
		t.Errorf("sent %v", all)
	}
	expectSleeps(t, h, time.Second)
	expectWarnings(t, h)
}

func TestScoresFailingEveryAttemptAreWarnedOfOnce(t *testing.T) {
	down := scripted{status: http.StatusServiceUnavailable}
	h := newHarness(t, testConfig(), newFakeTransport(t, down, down, down))
	h.client.Score(telmoni.ScoreTarget{SessionID: "conv-1"}, "relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
	h.flush()
	expectRequests(t, h, 3)
	expectWarnings(t, h, "telmoni: scores failed after 3 attempts (503); they were dropped")
	expectFields(t, h, map[string]any{"kind": "failed", "status": int64(503)})
}

func TestScoresPastAHundredGoInBodiesOfAHundred(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	for range 101 {
		h.client.Score(telmoni.ScoreTarget{SessionID: "conv-1"}, "relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
	}
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	h.flush()
	var sizes []int
	var spans []map[string]any
	for _, body := range h.transport.bodies(t) {
		if scores, ok := body["scores"].([]any); ok {
			sizes = append(sizes, len(scores))
			continue
		}
		spans = append(spans, spansIn(body)...)
	}
	if !reflect.DeepEqual(sizes, []int{100, 1}) {
		t.Errorf("bodies of %v scores", sizes)
	}
	if got := names(spans); !reflect.DeepEqual(got, []string{"invoke_agent bot"}) {
		t.Errorf("spans sent: %v", got)
	}
}

func TestTheSeventeenthScoreOnASpanIsNotSent(t *testing.T) {
	h := contentOff(t)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for range 17 {
		run.Score("relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
	}
	run.End()
	h.flush()
	if events := eventsOf(t, h, "invoke_agent bot"); len(events) != 16 {
		t.Errorf("sent %d events", len(events))
	}
	expectOneRefusal(t, h, "its span holds 16 scores already")
}

func TestAHandleScoredAfterItsSpanEndedSendsNothing(t *testing.T) {
	h := contentOff(t)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, step := run.Step(ctx, "plan")
	step.End()
	step.Score("relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
	run.End()
	h.flush()
	if events := eventsOf(t, h, "plan"); len(events) != 0 {
		t.Errorf("sent %v", events)
	}
	expectOneRefusal(t, h, "its span had ended; send it with the client's score, naming the span")
}

func TestHandlesThatDoNothingScoreNothing(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	var run *telmoni.Run
	run.Score("relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
	ctx, blank := h.client.Run(context.Background(), " ", telmoni.RunOptions{})
	_, step := blank.Step(ctx, "plan")
	_, call := blank.ModelCall(ctx, "m", telmoni.ModelCallOptions{})
	_, tool := blank.ToolCall(ctx, "t", telmoni.ToolCallOptions{})
	for _, scored := range []interface {
		Score(string, telmoni.ScoreValue, telmoni.ScoreOptions)
		SpanID() string
	}{run, blank, step, call, tool, (*telmoni.Step)(nil), (*telmoni.ModelCall)(nil), (*telmoni.ToolCall)(nil)} {
		scored.Score("relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
		if id := scored.SpanID(); id != "" {
			t.Errorf("SpanID() = %q", id)
		}
	}
	h.flush()
	expectRequests(t, h, 0)
	// The blank agent's own warning alone.
	if warned := h.logs.warnings(); len(warned) != 1 || strings.Contains(warned[0], "score") {
		t.Errorf("warnings = %q", warned)
	}
}

func TestARecordedRunsScoresAreTimedAtEachSpansEnd(t *testing.T) {
	h := contentOff(t)
	start := time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC)
	record := telmoni.RunRecord{
		RunID: "5b7e1c2a-9d4f-4e8b-a6c3-1f2e3d4c5b6a",
		Agent: "bot",
		Start: start,
		End:   start.Add(3 * time.Second),
		Scores: []telmoni.RecordedScore{
			{Name: "thumbs", Value: telmoni.Bool(true)},
			{Name: " ", Value: telmoni.Number(0.5)},
		},
		Inside: []telmoni.RecordItem{telmoni.StepRecord{
			Name:   "plan",
			Start:  start.Add(time.Second),
			End:    start.Add(2 * time.Second),
			Scores: []telmoni.RecordedScore{{Name: "tone", Value: telmoni.Category("polite")}},
		}},
	}
	if err := h.client.RecordRun(context.Background(), record); err != nil {
		t.Fatalf("recording: %v", err)
	}
	h.flush()
	spans := byName(h.transport.spans(t))
	for name, want := range map[string]string{"invoke_agent bot": "thumbs", "plan": "tone"} {
		span := spans[name]
		events, _ := span["events"].([]any)
		if len(events) != 1 {
			t.Fatalf("%s: events = %v", name, events)
		}
		event := events[0].(map[string]any)
		if event["timeUnixNano"] != span["endTimeUnixNano"] || attributesOf(t, event)["gen_ai.evaluation.name"] != want {
			t.Errorf("%s: event = %v, its span ends at %v", name, event, span["endTimeUnixNano"])
		}
	}
	expectOneRefusal(t, h, "its name is 1 to 64 characters")
}

func TestTheScoreLimitsAreTheFixtures(t *testing.T) {
	if got := loadScores(t).Limits; !reflect.DeepEqual(got, telmoni.ScoreLimits) {
		t.Errorf("limits\n got: %v\nwant: %v", telmoni.ScoreLimits, got)
	}
}

func TestAScoresEventLeavesUntouchedByAMaskDroppingEverything(t *testing.T) {
	on := contentOn()
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret(testKey), CaptureContent: true, Mask: func(string, string) (string, bool) {
		return "", false
	}}, newFakeTransport(t, on, on, on))
	_, warm := h.client.Run(context.Background(), "warm", telmoni.RunOptions{})
	warm.End()
	h.flush()
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.Score("relevance", telmoni.Number(0.5), telmoni.ScoreOptions{Version: "judge-2"})
	trace.SpanFromContext(ctx).AddEvent("gen_ai.evaluation.result", trace.WithAttributes(
		attribute.String("gen_ai.evaluation.name", "tone"),
		attribute.String("gen_ai.evaluation.explanation", "polite throughout"),
	))
	run.Fail(errors.New("bad plan"))
	run.End()
	h.flush()
	events := eventsOf(t, h, "invoke_agent bot")
	if len(events) != 3 {
		t.Fatalf("events = %v", events)
	}
	score, own, exception := events[0].(map[string]any), events[1].(map[string]any), events[2].(map[string]any)
	// An evaluation event's other keys are content, which the mask answers.
	if got := attributesOf(t, own); !reflect.DeepEqual(got, map[string]any{"gen_ai.evaluation.name": "tone"}) {
		t.Errorf("an application's evaluation event = %v", own)
	}
	want := map[string]any{"gen_ai.evaluation.name": "relevance", "gen_ai.evaluation.score.value": 0.5, "telmoni.score.version": "judge-2"}
	if got := attributesOf(t, score); score["name"] != "gen_ai.evaluation.result" || !reflect.DeepEqual(got, want) {
		t.Errorf("score event = %v", score)
	}
	// The mask still answers every other event.
	if got := attributesOf(t, exception); exception["name"] != "exception" || len(got) != 0 {
		t.Errorf("exception event = %v", exception)
	}
}

func TestAScoreAloneAsksNoContentMode(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret(testKey), CaptureContent: true}, nil)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.Score("relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
	run.End()
	h.flush()
	if bodies := h.transport.bodies(t); len(bodies) != 1 || len(eventsOf(t, h, "invoke_agent bot")) != 1 {
		t.Errorf("sent %v", bodies)
	}
}

func TestAScoresEventStaysOnASpanTrimmedToFit(t *testing.T) {
	h := contentOff(t)
	span := bigSpans(batchMax())[0]
	stub := tracetest.SpanStubFromReadOnlySpan(span)
	stub.Events = []sdktrace.Event{
		{Name: "gen_ai.evaluation.result", Time: time.Unix(2, 0), Attributes: []attribute.KeyValue{attribute.String("gen_ai.evaluation.name", "relevance")}},
		{Name: "app.note", Time: time.Unix(2, 0)},
	}
	telmoni.ExportSpans(h.client, []sdktrace.ReadOnlySpan{stub.Snapshot()})
	sent := h.transport.spans(t)
	if len(sent) != 1 {
		t.Fatalf("sent %v", sent)
	}
	events, _ := sent[0]["events"].([]any)
	if len(events) != 1 || events[0].(map[string]any)["name"] != "gen_ai.evaluation.result" {
		t.Errorf("events = %v", events)
	}
	expectWarnings(t, h, "telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length")
}

// batchMax is an attribute's bytes that alone take a span past an export's 1 MiB.
func batchMax() int { return 1<<20 + 1 }

func TestAnEvaluationEventsOwnExplanationStaysHomeWithContentOff(t *testing.T) {
	h := contentOff(t)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	trace.SpanFromContext(ctx).AddEvent("gen_ai.evaluation.result", trace.WithAttributes(
		attribute.String("gen_ai.evaluation.name", "relevance"),
		attribute.Float64("gen_ai.evaluation.score.value", 0.5),
		attribute.String("gen_ai.evaluation.explanation", "it answered the question"),
	))
	run.End()
	h.flush()
	events := eventsOf(t, h, "invoke_agent bot")
	if len(events) != 1 {
		t.Fatalf("events = %v", events)
	}
	want := map[string]any{"gen_ai.evaluation.name": "relevance", "gen_ai.evaluation.score.value": 0.5}
	if got := attributesOf(t, events[0].(map[string]any)); !reflect.DeepEqual(got, want) {
		t.Errorf("event\n got: %v\nwant: %v", got, want)
	}
	if strings.Contains(h.transport.text(), "it answered the question") {
		t.Error("an explanation left with content off")
	}
}
