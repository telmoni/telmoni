package telmoni_test

// The SDK beside an application's own OpenTelemetry: its spans stay in their
// traces at their own sampling rate, a run inside a trace it does not sample
// is still sent, and its OTEL_* variables change nothing of ours.

import (
	"context"
	"reflect"
	"sort"
	"sync"
	"sync/atomic"
	"testing"

	"go.opentelemetry.io/otel"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
	"go.opentelemetry.io/otel/trace"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

// everyOther samples every second root span: a rate of one half,
// deterministic.
type everyOther struct {
	calls atomic.Int64
}

func (s *everyOther) ShouldSample(sdktrace.SamplingParameters) sdktrace.SamplingResult {
	if s.calls.Add(1)%2 == 0 {
		return sdktrace.SamplingResult{Decision: sdktrace.RecordAndSample}
	}
	return sdktrace.SamplingResult{Decision: sdktrace.Drop}
}

func (*everyOther) Description() string { return "EveryOther" }

// appProvider is the application's own OpenTelemetry: a provider exporting to
// memory, ParentBased as an application's default sampler is, so its root
// decision is root's and a span inside a run follows the run's flag.
func appProvider(t *testing.T, root sdktrace.Sampler) (trace.Tracer, *tracetest.InMemoryExporter) {
	t.Helper()
	exporter := tracetest.NewInMemoryExporter()
	provider := sdktrace.NewTracerProvider(
		sdktrace.WithSampler(sdktrace.ParentBased(root)),
		sdktrace.WithSyncer(exporter),
	)
	t.Cleanup(func() { _ = provider.Shutdown(context.Background()) })
	return provider.Tracer("the-application"), exporter
}

func ourRuns(t *testing.T, h *harness) []map[string]any {
	t.Helper()
	return withPrefix(h.transport.spans(t), "invoke_agent ")
}

func TestARunJoinsTheCurrentTraceAndTheApplicationKeepsItsSpan(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	app, exporter := appProvider(t, sdktrace.AlwaysSample())
	ctx, request := app.Start(context.Background(), "request")
	runCtx, run := h.client.Run(ctx, "bot", telmoni.RunOptions{})
	_, query := app.Start(runCtx, "db.query")
	query.End()
	run.End()
	request.End()
	h.flush()
	runs := ourRuns(t, h)
	if len(runs) != 1 {
		t.Fatalf("want one run, got %d", len(runs))
	}
	requestContext := request.SpanContext()
	if runs[0]["traceId"] != requestContext.TraceID().String() {
		t.Error("the run did not join the application's trace")
	}
	if runs[0]["parentSpanId"] != requestContext.SpanID().String() {
		t.Error("the application's span is not the run's parent")
	}
	appSpans := exporter.GetSpans()
	appNames := make([]string, 0, len(appSpans))
	var querySpan tracetest.SpanStub
	for _, span := range appSpans {
		appNames = append(appNames, span.Name)
		if span.Name == "db.query" {
			querySpan = span
		}
	}
	sort.Strings(appNames)
	if !reflect.DeepEqual(appNames, []string{"db.query", "request"}) {
		t.Fatalf("the application exported %v", appNames)
	}
	if querySpan.Parent.SpanID().String() != runs[0]["spanId"] {
		t.Error("the application's span inside the run is not nested under it")
	}
	if querySpan.SpanContext.TraceID().String() != runs[0]["traceId"] {
		t.Error("the application's span inside the run left the trace")
	}
	for _, span := range h.transport.spans(t) {
		if name := span["name"]; name == "request" || name == "db.query" {
			t.Errorf("the application's span %v was sent to Telmoni", name)
		}
	}
}

// openedByInstrumentation opens a span as otelhttp does when given no
// provider: on the provider of the span ctx carries.
func openedByInstrumentation(ctx context.Context) {
	_, span := trace.SpanFromContext(ctx).TracerProvider().Tracer("otelhttp").Start(ctx, "HTTP POST")
	span.End()
}

// Inside a run, a step or a call, the current span's provider is the one the
// run displaced, the application's, never the SDK's.
func TestInstrumentationInsideARunOpensSpansOnTheApplicationsProvider(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	app, exporter := appProvider(t, sdktrace.AlwaysSample())
	ctx, request := app.Start(context.Background(), "request")
	runCtx, run := h.client.Run(ctx, "bot", telmoni.RunOptions{})
	openedByInstrumentation(runCtx)
	callCtx, call := run.ModelCall(runCtx, "gpt-4o-mini", telmoni.ModelCallOptions{Provider: "openai"})
	openedByInstrumentation(callCtx)
	call.End()
	run.End()
	request.End()
	h.flush()
	appNames := make([]string, 0, 3)
	for _, span := range exporter.GetSpans() {
		appNames = append(appNames, span.Name)
	}
	sort.Strings(appNames)
	if !reflect.DeepEqual(appNames, []string{"HTTP POST", "HTTP POST", "request"}) {
		t.Errorf("the application exported %v", appNames)
	}
	for _, name := range names(h.transport.spans(t)) {
		if name == "HTTP POST" {
			t.Error("the application's span was sent to Telmoni")
		}
	}
}

// A run opened with no span of the application's current displaces the
// global provider.
func TestInstrumentationInARunOpenedOutsideAnySpanUsesTheGlobalProvider(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	exporter := tracetest.NewInMemoryExporter()
	provider := sdktrace.NewTracerProvider(sdktrace.WithSyncer(exporter))
	t.Cleanup(func() { _ = provider.Shutdown(context.Background()) })
	previous := otel.GetTracerProvider()
	otel.SetTracerProvider(provider)
	t.Cleanup(func() { otel.SetTracerProvider(previous) })
	runCtx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	openedByInstrumentation(runCtx)
	run.End()
	h.flush()
	if got := len(exporter.GetSpans()); got != 1 {
		t.Errorf("the global provider exported %d spans, want the one", got)
	}
	if got := names(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"invoke_agent bot"}) {
		t.Errorf("Telmoni was sent %v, want the run alone", got)
	}
}

func TestTheApplicationKeepsItsOwnSamplingRateInsideRuns(t *testing.T) {
	every := &everyOther{}
	h := newHarness(t, telmoni.Config{APIKey: testKey, Sampler: every}, nil)
	app, exporter := appProvider(t, every)
	for range 100 {
		runCtx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
		_, inside := app.Start(runCtx, "inside")
		inside.End()
		run.End()
	}
	h.flush()
	if got := len(ourRuns(t, h)); got != 100 {
		t.Errorf("%d runs reached Telmoni, want every one of 100", got)
	}
	if got := len(exporter.GetSpans()); got != 50 {
		t.Errorf("the application kept %d of its spans inside runs, want half", got)
	}
}

func TestARunInsideATraceTheApplicationDoesNotSampleIsStillSent(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	app, exporter := appProvider(t, sdktrace.NeverSample())
	ctx, request := app.Start(context.Background(), "request")
	if request.SpanContext().IsSampled() {
		t.Fatal("the application sampled a span its sampler refuses")
	}
	runCtx, run := h.client.Run(ctx, "bot", telmoni.RunOptions{})
	_, step := run.Step(runCtx, "s")
	step.End()
	run.End()
	request.End()
	h.flush()
	runs := ourRuns(t, h)
	if len(runs) != 1 {
		t.Fatalf("want one run, got %d", len(runs))
	}
	if runs[0]["traceId"] != request.SpanContext().TraceID().String() {
		t.Error("the run did not join the application's trace")
	}
	if _, has := byName(h.transport.spans(t))["s"]; !has {
		t.Error("the step inside the unsampled trace was not sent")
	}
	if got := len(exporter.GetSpans()); got != 0 {
		t.Errorf("the application exported %d spans from a trace it does not sample", got)
	}
}

func TestAConfiguredSamplerDecidesARootRun(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: "k", Sampler: sdktrace.NeverSample()}, nil)
	app, _ := appProvider(t, sdktrace.AlwaysSample())
	runCtx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	if trace.SpanContextFromContext(runCtx).IsSampled() {
		t.Error("the run took a sampled flag its sampler did not give")
	}
	_, inside := app.Start(runCtx, "inside")
	if inside.SpanContext().IsSampled() {
		t.Error("the application's span inside the run did not follow the run's flag")
	}
	inside.End()
	run.End()
	h.flush()
	if got := len(ourRuns(t, h)); got != 1 {
		t.Errorf("%d runs sent, want 1 regardless of the flag", got)
	}
}

func TestOTELVariablesChangeNothingOfOurs(t *testing.T) {
	for key, value := range map[string]string{
		"OTEL_SDK_DISABLED":               "true",
		"OTEL_TRACES_SAMPLER":             "always_off",
		"OTEL_RESOURCE_ATTRIBUTES":        "leaked=yes",
		"OTEL_SERVICE_NAME":               "leaked",
		"OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT": "1",
		"OTEL_BSP_MAX_QUEUE_SIZE":         "1",
	} {
		t.Setenv(key, value)
	}
	h := newHarness(t, telmoni.Config{APIKey: "k"}, nil)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{
		UserID:    "c",
		SessionID: "v",
		Tags:      map[string]string{"a": "b"},
	})
	run.End()
	h.flush()
	runs := ourRuns(t, h)
	if len(runs) != 1 {
		t.Fatalf("want one run, got %d", len(runs))
	}
	attributes := attributesOf(t, runs[0])
	if attributes["telmoni.tag.a"] != "b" || attributes["user.id"] != "c" {
		t.Errorf("the run lost attributes: %v", attributes)
	}
	if len(attributes) != 8 {
		t.Errorf("the run carries %d attributes, want all 8", len(attributes))
	}
	resource := h.transport.bodies(t)[0]["resourceSpans"].([]any)[0].(map[string]any)["resource"].(map[string]any)
	keys := map[string]bool{}
	for _, raw := range resource["attributes"].([]any) {
		keys[raw.(map[string]any)["key"].(string)] = true
	}
	want := map[string]bool{
		"telemetry.distro.name":    true,
		"telemetry.distro.version": true,
		"telemetry.sdk.language":   true,
		"telemetry.sdk.name":       true,
		"telemetry.sdk.version":    true,
	}
	if !reflect.DeepEqual(keys, want) {
		t.Errorf("the resource holds %v", keys)
	}
}

func TestTheRunIsFoundFromTheContextInAnotherGoroutine(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		_, step := telmoni.RunFromContext(ctx).Step(ctx, "threaded")
		step.End()
	}()
	wg.Wait()
	run.End()
	h.flush()
	spans := byName(h.transport.spans(t))
	step := spans["threaded"]
	if step == nil {
		t.Fatal("the step from the goroutine was not sent")
	}
	if got := attributesOf(t, step)["telmoni.run_id"]; got != run.ID() {
		t.Errorf("the step carries run %v, want %s", got, run.ID())
	}
	if step["parentSpanId"] != spans["invoke_agent bot"]["spanId"] {
		t.Error("the step is not the run's child")
	}
}
