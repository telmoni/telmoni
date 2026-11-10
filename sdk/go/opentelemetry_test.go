package telmoni_test

// The SDK beside an application's own OpenTelemetry: each trace is its own,
// linked to the application's span rather than inside its trace, the
// application's spans inside a trace keep their own sampling rate, and its
// OTEL_* variables change nothing of ours.

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
// decision is root's and a span inside a trace follows the trace's flag.
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

func ourTraces(t *testing.T, h *harness) []map[string]any {
	t.Helper()
	return withPrefix(h.transport.spans(t), "invoke_agent ")
}

// linksOf is the links a sent span carries, each as its traceId and spanId.
func linksOf(span map[string]any) [][2]string {
	var out [][2]string
	links, _ := span["links"].([]any)
	for _, raw := range links {
		link := raw.(map[string]any)
		out = append(out, [2]string{link["traceId"].(string), link["spanId"].(string)})
	}
	return out
}

func TestATraceIsItsOwnLinkedToTheApplicationsSpan(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	app, exporter := appProvider(t, sdktrace.AlwaysSample())
	ctx, request := app.Start(context.Background(), "request")
	traceCtx, tr := h.client.Trace(ctx, "bot", telmoni.TraceOptions{})
	_, query := app.Start(traceCtx, "db.query")
	query.End()
	tr.End()
	request.End()
	h.flush()
	traces := ourTraces(t, h)
	if len(traces) != 1 {
		t.Fatalf("want one trace, got %d", len(traces))
	}
	requestContext := request.SpanContext()
	if traces[0]["traceId"] == requestContext.TraceID().String() || traces[0]["traceId"] != tr.TraceID() {
		t.Errorf("the trace is %v, want one of its own, TraceID() %s, not the application's %s", traces[0]["traceId"], tr.TraceID(), requestContext.TraceID())
	}
	if _, has := traces[0]["parentSpanId"]; has {
		t.Error("the agent's span has a parent, want a root")
	}
	want := [][2]string{{requestContext.TraceID().String(), requestContext.SpanID().String()}}
	if got := linksOf(traces[0]); !reflect.DeepEqual(got, want) {
		t.Errorf("the agent's span links %v, want the application's span %v", got, want)
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
	if querySpan.Parent.SpanID().String() != traces[0]["spanId"] {
		t.Error("the application's span inside the trace is not nested under the agent's")
	}
	if querySpan.SpanContext.TraceID().String() != traces[0]["traceId"] {
		t.Error("the application's span inside the trace is not in it")
	}
	for _, span := range h.transport.spans(t) {
		if name := span["name"]; name == "request" || name == "db.query" {
			t.Errorf("the application's span %v was sent to Telmoni", name)
		}
	}
	for _, span := range h.transport.spans(t) {
		if span["name"] != "invoke_agent bot" && len(linksOf(span)) != 0 {
			t.Errorf("%v is linked, want the agent's span alone", span["name"])
		}
	}
}

// A trace opened with no span current links to nothing: the field is left
// off, as the other SDKs leave it.
func TestATraceOpenedOutsideAnySpanLinksToNothing(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	_, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	tr.End()
	h.flush()
	traces := ourTraces(t, h)
	if len(traces) != 1 {
		t.Fatalf("want one trace, got %d", len(traces))
	}
	if _, has := traces[0]["links"]; has {
		t.Errorf("the agent's span carries links %v, want none", traces[0]["links"])
	}
}

// openedByInstrumentation opens a span as otelhttp does when given no
// provider: on the provider of the span ctx carries.
func openedByInstrumentation(ctx context.Context) {
	_, span := trace.SpanFromContext(ctx).TracerProvider().Tracer("otelhttp").Start(ctx, "HTTP POST")
	span.End()
}

// Inside a trace, a step or a call, the current span's provider is the one
// the trace displaced, the application's, never the SDK's.
func TestInstrumentationInsideATraceOpensSpansOnTheApplicationsProvider(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	app, exporter := appProvider(t, sdktrace.AlwaysSample())
	ctx, request := app.Start(context.Background(), "request")
	traceCtx, tr := h.client.Trace(ctx, "bot", telmoni.TraceOptions{})
	openedByInstrumentation(traceCtx)
	callCtx, call := tr.ModelCall(traceCtx, "gpt-4o-mini", telmoni.ModelCallOptions{Provider: "openai"})
	openedByInstrumentation(callCtx)
	call.End()
	tr.End()
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

// A trace opened with no span of the application's current displaces the
// global provider.
func TestInstrumentationInATraceOpenedOutsideAnySpanUsesTheGlobalProvider(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	exporter := tracetest.NewInMemoryExporter()
	provider := sdktrace.NewTracerProvider(sdktrace.WithSyncer(exporter))
	t.Cleanup(func() { _ = provider.Shutdown(context.Background()) })
	previous := otel.GetTracerProvider()
	otel.SetTracerProvider(provider)
	t.Cleanup(func() { otel.SetTracerProvider(previous) })
	traceCtx, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	openedByInstrumentation(traceCtx)
	tr.End()
	h.flush()
	if got := len(exporter.GetSpans()); got != 1 {
		t.Errorf("the global provider exported %d spans, want the one", got)
	}
	if got := names(h.transport.spans(t)); !reflect.DeepEqual(got, []string{"invoke_agent bot"}) {
		t.Errorf("Telmoni was sent %v, want the agent's span alone", got)
	}
}

func TestTheApplicationKeepsItsOwnSamplingRateInsideTraces(t *testing.T) {
	every := &everyOther{}
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret(testKey), Sampler: every}, nil)
	app, exporter := appProvider(t, every)
	for range 100 {
		traceCtx, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
		_, inside := app.Start(traceCtx, "inside")
		inside.End()
		tr.End()
	}
	h.flush()
	if got := len(ourTraces(t, h)); got != 100 {
		t.Errorf("%d traces reached Telmoni, want every one of 100", got)
	}
	if got := len(exporter.GetSpans()); got != 50 {
		t.Errorf("the application kept %d of its spans inside traces, want half", got)
	}
}

// A trace opened inside a span the application does not sample is still
// sent, sampled by the configured sampler as its own trace, and linked to
// that span all the same.
func TestATraceInsideASpanTheApplicationDoesNotSampleIsStillSentAndLinked(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	app, exporter := appProvider(t, sdktrace.NeverSample())
	ctx, request := app.Start(context.Background(), "request")
	if request.SpanContext().IsSampled() {
		t.Fatal("the application sampled a span its sampler refuses")
	}
	traceCtx, tr := h.client.Trace(ctx, "bot", telmoni.TraceOptions{})
	if !trace.SpanContextFromContext(traceCtx).IsSampled() {
		t.Error("the trace took the application's flag, want its own sampler's")
	}
	_, step := tr.Step(traceCtx, "s")
	step.End()
	tr.End()
	request.End()
	h.flush()
	traces := ourTraces(t, h)
	if len(traces) != 1 {
		t.Fatalf("want one trace, got %d", len(traces))
	}
	requestContext := request.SpanContext()
	want := [][2]string{{requestContext.TraceID().String(), requestContext.SpanID().String()}}
	if got := linksOf(traces[0]); !reflect.DeepEqual(got, want) {
		t.Errorf("the agent's span links %v, want the application's unsampled span %v", got, want)
	}
	if _, has := byName(h.transport.spans(t))["s"]; !has {
		t.Error("the step was not sent")
	}
	if got := len(exporter.GetSpans()); got != 0 {
		t.Errorf("the application exported %d spans from a trace it does not sample", got)
	}
}

func TestAConfiguredSamplerDecidesEachTrace(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), Sampler: sdktrace.NeverSample()}, nil)
	app, _ := appProvider(t, sdktrace.AlwaysSample())
	ctx, request := app.Start(context.Background(), "request")
	traceCtx, tr := h.client.Trace(ctx, "bot", telmoni.TraceOptions{})
	if trace.SpanContextFromContext(traceCtx).IsSampled() {
		t.Error("the trace took a sampled flag its sampler did not give")
	}
	_, inside := app.Start(traceCtx, "inside")
	if inside.SpanContext().IsSampled() {
		t.Error("the application's span inside the trace did not follow the trace's flag")
	}
	inside.End()
	tr.End()
	request.End()
	h.flush()
	if got := len(ourTraces(t, h)); got != 1 {
		t.Errorf("%d traces sent, want 1 regardless of the flag", got)
	}
}

func TestOTELVariablesChangeNothingOfOurs(t *testing.T) {
	for key, value := range map[string]string{
		"OTEL_SDK_DISABLED":               "true",
		"OTEL_TRACES_SAMPLER":             "always_off",
		"OTEL_RESOURCE_ATTRIBUTES":        "leaked=yes",
		"OTEL_SERVICE_NAME":               "leaked",
		"OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT": "1",
		"OTEL_SPAN_LINK_COUNT_LIMIT":      "0",
		"OTEL_BSP_MAX_QUEUE_SIZE":         "1",
	} {
		t.Setenv(key, value)
	}
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k")}, nil)
	app, _ := appProvider(t, sdktrace.AlwaysSample())
	ctx, request := app.Start(context.Background(), "request")
	_, tr := h.client.Trace(ctx, "bot", telmoni.TraceOptions{
		UserID:    "c",
		SessionID: "v",
		Tags:      map[string]string{"a": "b"},
	})
	tr.End()
	request.End()
	h.flush()
	traces := ourTraces(t, h)
	if len(traces) != 1 {
		t.Fatalf("want one trace, got %d", len(traces))
	}
	attributes := attributesOf(t, traces[0])
	if attributes["telmoni.tag.a"] != "b" || attributes["user.id"] != "c" {
		t.Errorf("the trace lost attributes: %v", attributes)
	}
	if len(attributes) != 6 {
		t.Errorf("the agent's span carries %d attributes, want all 6", len(attributes))
	}
	if got := len(linksOf(traces[0])); got != 1 {
		t.Errorf("the agent's span carries %d links, want its one to the application's span", got)
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

func TestTheTraceIsFoundFromTheContextInAnotherGoroutine(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, tr := h.client.Trace(context.Background(), "bot", telmoni.TraceOptions{})
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		_, step := telmoni.TraceFromContext(ctx).Step(ctx, "threaded")
		step.End()
	}()
	wg.Wait()
	tr.End()
	h.flush()
	spans := byName(h.transport.spans(t))
	step := spans["threaded"]
	if step == nil {
		t.Fatal("the step from the goroutine was not sent")
	}
	if step["traceId"] != tr.TraceID() {
		t.Errorf("the step is in trace %v, want %s", step["traceId"], tr.TraceID())
	}
	if step["parentSpanId"] != spans["invoke_agent bot"]["spanId"] {
		t.Error("the step is not the agent's child")
	}
}
