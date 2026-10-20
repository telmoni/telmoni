package telmoni_test

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"maps"
	"math"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"regexp"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"go.opentelemetry.io/otel/attribute"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/sdk/trace/tracetest"
	"go.opentelemetry.io/otel/trace"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

func TestTheDefaultEndpointIsTheMachineHost(t *testing.T) {
	if telmoni.DefaultEndpoint != "https://api.telmoni.com" {
		t.Errorf("DefaultEndpoint = %q", telmoni.DefaultEndpoint)
	}
	if telmoni.DefaultConfig().Endpoint != telmoni.DefaultEndpoint {
		t.Errorf("DefaultConfig().Endpoint = %q", telmoni.DefaultConfig().Endpoint)
	}
	cfg := mustNew(t, telmoni.Config{}).Config()
	if cfg.Endpoint != telmoni.DefaultEndpoint {
		t.Errorf("New(Config{}).Config().Endpoint = %q", cfg.Endpoint)
	}
	if cfg.SampleRate != 1 {
		t.Errorf("the default SampleRate = %g, want 1", cfg.SampleRate)
	}
}

// A span's time outside 64 bits of nanoseconds goes as 0, so ingest refuses
// that span rather than the whole export.
func TestASpanTimeOutsideWhatIngestReadsGoesAs0(t *testing.T) {
	last := time.Unix(1<<64/1_000_000_000, 1<<64%1_000_000_000-1)
	for _, c := range []struct {
		at   time.Time
		want string
	}{
		{time.Unix(0, 1), "1"},
		{time.Date(2300, 1, 1, 0, 0, 0, 0, time.UTC), "10413792000000000000"},
		{last, "18446744073709551615"},
		{last.Add(time.Nanosecond), "0"},
		{time.Unix(0, 0), "0"},
		{time.Unix(-1, 0), "0"},
		{time.Time{}, "0"},
	} {
		if got := telmoni.Nanos(c.at); got != c.want {
			t.Errorf("a span's time of %v is sent as %s, want %s", c.at, got, c.want)
		}
	}
}

func TestATrailingSlashIsStripped(t *testing.T) {
	for _, given := range []string{
		"https://api.telmoni.local/",
		"https://api.telmoni.local///",
		"  https://api.telmoni.local/ ",
	} {
		if got := mustNew(t, telmoni.Config{Endpoint: given}).Config().Endpoint; got != "https://api.telmoni.local" {
			t.Errorf("Endpoint %q became %q", given, got)
		}
	}
}

func TestABlankEndpointOrganizationKeyEnvironmentOrReleaseReadsAsUnset(t *testing.T) {
	cfg := mustNew(t, telmoni.Config{Endpoint: "  ", OrganizationID: " ", APIKey: "\t", Environment: " ", Release: ""}).Config()
	if cfg.Endpoint != telmoni.DefaultEndpoint {
		t.Errorf("Endpoint = %q", cfg.Endpoint)
	}
	if cfg.OrganizationID != "" {
		t.Errorf("OrganizationID = %q", cfg.OrganizationID)
	}
	if cfg.APIKey != "" {
		t.Errorf("APIKey = %q", cfg.APIKey)
	}
	if cfg.Environment != "" || cfg.Release != "" {
		t.Errorf("Environment = %q, Release = %q", cfg.Environment, cfg.Release)
	}
	deployed := mustNew(t, telmoni.Config{Environment: " staging ", Release: "2026.10.18"}).Config()
	if deployed.Environment != "staging" || deployed.Release != "2026.10.18" {
		t.Errorf("Environment = %q, Release = %q", deployed.Environment, deployed.Release)
	}
}

func TestWithoutAKeyNoAuthorizationHeaderIsSent(t *testing.T) {
	h := newHarness(t, telmoni.Config{}, nil)
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	h.flush()
	requests := h.transport.all()
	if len(requests) == 0 {
		t.Fatal("nothing was sent")
	}
	for _, req := range requests {
		if _, set := req.header["Authorization"]; set {
			t.Errorf("Authorization was sent without a key: %q", req.header.Get("Authorization"))
		}
	}
}

func TestTheKeyIsNeverPrintedOrSerialized(t *testing.T) {
	cfg := telmoni.Config{OrganizationID: "org_1", APIKey: "telmoni_secret_key"}
	client := mustNew(t, cfg)
	marshalled, err := json.Marshal(cfg)
	if err != nil {
		t.Fatalf("marshalling the configuration: %v", err)
	}
	for _, printed := range []string{
		fmt.Sprint(cfg), fmt.Sprintf("%+v", cfg), fmt.Sprintf("%#v", cfg),
		fmt.Sprint(client), fmt.Sprintf("%+v", client), fmt.Sprintf("%#v", client),
		fmt.Sprint(client.Config()), string(marshalled),
	} {
		if strings.Contains(printed, "telmoni_secret_key") {
			t.Errorf("printed the key: %s", printed)
		}
		if !strings.Contains(printed, "org_1") {
			t.Errorf("lost the organization: %s", printed)
		}
	}
	if client.Config().APIKey != "telmoni_secret_key" {
		t.Errorf("the key did not stay readable: %q", client.Config().APIKey)
	}
}

func TestFromEnvReadsTheFiveVariablesAndNoOther(t *testing.T) {
	t.Setenv("TELMONI_ENDPOINT", "")
	t.Setenv("TELMONI_API_KEY", "")
	t.Setenv("TELMONI_ORG", "org_2")
	t.Setenv("TELMONI_ENVIRONMENT", "staging")
	t.Setenv("TELMONI_RELEASE", "")
	t.Setenv("OTEL_EXPORTER_OTLP_ENDPOINT", "https://elsewhere.example")
	t.Setenv("OTEL_RESOURCE_ATTRIBUTES", "deployment.environment.name=leaked,service.version=leaked")
	cfg := telmoni.ConfigFromEnv()
	if cfg.Endpoint != telmoni.DefaultEndpoint {
		t.Errorf("Endpoint = %q", cfg.Endpoint)
	}
	if cfg.APIKey != "" {
		t.Errorf("a blank TELMONI_API_KEY read as %q", cfg.APIKey)
	}
	if cfg.OrganizationID != "org_2" {
		t.Errorf("OrganizationID = %q", cfg.OrganizationID)
	}
	if cfg.Environment != "staging" || cfg.Release != "" {
		t.Errorf("Environment = %q, Release = %q", cfg.Environment, cfg.Release)
	}

	t.Setenv("TELMONI_ENDPOINT", " https://api.telmoni.local/ ")
	t.Setenv("TELMONI_API_KEY", " telmoni_k ")
	t.Setenv("TELMONI_ENVIRONMENT", "")
	t.Setenv("TELMONI_RELEASE", " 2026.10.18 ")
	cfg = telmoni.ConfigFromEnv()
	if cfg.Endpoint != "https://api.telmoni.local" {
		t.Errorf("Endpoint = %q", cfg.Endpoint)
	}
	if cfg.APIKey != "telmoni_k" {
		t.Errorf("APIKey = %q", cfg.APIKey)
	}
	if cfg.Environment != "" || cfg.Release != "2026.10.18" {
		t.Errorf("Environment = %q, Release = %q", cfg.Environment, cfg.Release)
	}
}

func TestSampleRateIsAboveZeroAndUpToOne(t *testing.T) {
	for given, want := range map[float64]float64{1: 1, 0.25: 0.25, 0: 1} {
		if got := mustNew(t, telmoni.Config{SampleRate: given}).Config().SampleRate; got != want {
			t.Errorf("SampleRate %g became %g, want %g", given, got, want)
		}
	}
	for _, bad := range []float64{-0.5, 1.5, math.NaN()} {
		if _, err := telmoni.New(telmoni.Config{SampleRate: bad}); err == nil {
			t.Errorf("New accepted SampleRate %g", bad)
		}
	}
}

func TestTheVersionIsTheTagsSemver(t *testing.T) {
	if !regexp.MustCompile(`^\d+\.\d+\.\d+$`).MatchString(telmoni.Version) {
		t.Errorf("Version %q is not a semver for the sdk/go/v tag to match", telmoni.Version)
	}
}

func TestTheClientBuildsFromEnvWithTheRealTransport(t *testing.T) {
	t.Setenv("TELMONI_ENDPOINT", "")
	client, err := telmoni.NewFromEnv()
	if err != nil {
		t.Fatalf("NewFromEnv: %v", err)
	}
	defer func() { _ = client.Shutdown(context.Background()) }()
	if client.Config().Endpoint != telmoni.DefaultEndpoint {
		t.Errorf("Endpoint = %q", client.Config().Endpoint)
	}
	if mode := client.ContentMode(); mode != "" {
		t.Errorf("the content mode is %q before any answer", mode)
	}
}

func TestAnAnswerIsReadUpToItsFirstMebibyte(t *testing.T) {
	// Appended by the worker alone, and read once the shutdown has stopped it.
	var answers []*countedBody
	transport := func(*http.Request) (*http.Response, error) {
		answer := &countedBody{left: 2 << 20}
		answers = append(answers, answer)
		return &http.Response{StatusCode: http.StatusOK, Header: http.Header{}, Body: answer}, nil
	}
	h := newHarness(t, testConfig(), nil, telmoni.WithTransport(transport))
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	h.shutdown()
	if len(answers) == 0 {
		t.Fatal("nothing was sent")
	}
	for _, answer := range answers {
		if answer.read != 1<<20 {
			t.Errorf("%d bytes of an answer read, want its first mebibyte", answer.read)
		}
	}
}

// A refusal whose body breaks is sent once, named by the problem that came before the break.
func TestARefusalWhoseBodyBreaksIsSentOnce(t *testing.T) {
	const problem = `{"message":"/errors/batch-too-large: the batch is too large"}`
	var mu sync.Mutex
	var bodies []string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		mu.Lock()
		bodies = append(bodies, string(body))
		mu.Unlock()
		conn, sent, err := w.(http.Hijacker).Hijack()
		if err != nil {
			return
		}
		defer conn.Close()
		fmt.Fprintf(sent, "HTTP/1.1 413 Answer\r\nContent-Length: 300\r\n\r\n%s", problem)
		_ = sent.Flush()
	}))
	defer server.Close()
	sleeps, logs := &sleepLog{}, &logSink{}
	client, err := telmoni.New(telmoni.Config{Endpoint: server.URL, APIKey: testKey},
		telmoni.WithSleep(sleeps.sleep), telmoni.WithLogger(slog.New(logs)))
	if err != nil {
		t.Fatalf("building the client: %v", err)
	}
	_, run := client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	ctx, cancel := context.WithTimeout(context.Background(), waitTimeout)
	defer cancel()
	if err := client.Shutdown(ctx); err != nil {
		t.Errorf("shutting down: %v", err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(bodies) == 0 {
		t.Fatal("nothing was sent")
	}
	seen := map[string]bool{}
	for _, body := range bodies {
		if seen[body] {
			t.Error("an export was sent again")
		}
		seen[body] = true
	}
	if got := sleeps.all(); len(got) != 0 {
		t.Errorf("waited %v before sending again", got)
	}
	want := []string{"telmoni: export refused with 413 /errors/batch-too-large; the batch was dropped"}
	if got := logs.warnings(); !slices.Equal(got, want) {
		t.Errorf("warnings %q, want %q", got, want)
	}
}

// countedBody is an answer's body of left spaces, counting what is read of it.
type countedBody struct {
	left, read int
}

func (b *countedBody) Read(p []byte) (int, error) {
	if b.left == 0 {
		return 0, io.EOF
	}
	n := min(len(p), b.left)
	copy(p, strings.Repeat(" ", n))
	b.left -= n
	b.read += n
	return n, nil
}

func (b *countedBody) Close() error { return nil }

// Every endpoint, configured or from the environment, is read as sdk/fixtures/endpoints.json says.
func TestEveryEndpointIsReadAsTheFixtureReadsIt(t *testing.T) {
	var fixture struct {
		Taken []struct {
			Given    string `json:"given"`
			Endpoint string `json:"endpoint"`
		} `json:"taken"`
		Refused []struct {
			Given string `json:"given"`
			As    string `json:"as"`
		} `json:"refused"`
	}
	if err := json.Unmarshal(loadFixture(t, "endpoints.json"), &fixture); err != nil {
		t.Fatalf("decoding endpoints.json: %v", err)
	}
	for _, taken := range fixture.Taken {
		for _, given := range []string{taken.Given, taken.Endpoint} {
			if got := mustNew(t, telmoni.Config{Endpoint: given}).Config().Endpoint; got != taken.Endpoint {
				t.Errorf("Endpoint %q became %q, want %q", given, got, taken.Endpoint)
			}
		}
		// From the environment, read twice on its way to New.
		t.Setenv("TELMONI_ENDPOINT", taken.Given)
		client, err := telmoni.NewFromEnv()
		if err != nil {
			t.Errorf("NewFromEnv() with %q: %v", taken.Given, err)
			continue
		}
		if got := client.Config().Endpoint; got != taken.Endpoint {
			t.Errorf("NewFromEnv() read %q as %q, want %q", taken.Given, got, taken.Endpoint)
		}
		_ = client.Shutdown(context.Background())
	}
	for _, refused := range fixture.Refused {
		why := "http:// or https://"
		if refused.As == "cleartext" {
			why = "unencrypted HTTP"
		}
		if _, err := telmoni.New(telmoni.Config{Endpoint: refused.Given}); err == nil || !strings.Contains(err.Error(), why) {
			t.Errorf("New(%q) = %v, want a refusal naming %q", refused.Given, err, why)
		}
		t.Setenv("TELMONI_ENDPOINT", refused.Given)
		if _, err := telmoni.NewFromEnv(); err == nil || !strings.Contains(err.Error(), why) {
			t.Errorf("NewFromEnv() with %q = %v, want a refusal naming %q", refused.Given, err, why)
		}
	}
}

func TestAClientNotFromNewSendsNothingAndNeverPanics(t *testing.T) {
	ctx := context.Background()
	header := http.Header{}
	header.Set("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
	header.Set("baggage", "telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot")
	for name, client := range map[string]*telmoni.Client{"zero": {}, "nil": nil} {
		runCtx, run := client.Run(ctx, "bot", telmoni.RunOptions{})
		stepCtx, step := run.Step(runCtx, "plan")
		_, call := run.ModelCall(stepCtx, "claude", telmoni.ModelCallOptions{})
		call.Answered(telmoni.Answer{})
		step.End()
		run.End()
		client.Inject(runCtx, http.Header{})
		if telmoni.RunFromContext(client.Extract(ctx, header)) != nil {
			t.Errorf("%s: Extract continued a run", name)
		}
		if err := client.RecordRun(ctx, telmoni.RunRecord{Agent: "bot"}); err == nil {
			t.Errorf("%s: RecordRun took a record", name)
		}
		if client.Flush(ctx) != nil || client.Shutdown(ctx) != nil || client.ContentMode() != "" || client.Config() != (telmoni.Config{}) {
			t.Errorf("%s: a client not from New answered as one that is", name)
		}
	}
}

func TestFlushAndShutdownTakeANilContextAsRunDoes(t *testing.T) {
	client := mustNew(t, telmoni.Config{})
	if err := client.Flush(nil); err != nil {
		t.Errorf("Flush(nil) = %v", err)
	}
	if err := client.Shutdown(nil); err != nil {
		t.Errorf("Shutdown(nil) = %v", err)
	}
}

// This machine is reached through no proxy: one would take plain HTTP's key off
// it in the clear. Another host keeps net/http's own, the environment's.
func TestThisMachineIsReachedThroughNoProxy(t *testing.T) {
	for _, endpoint := range []string{
		"http://localhost:7100", "http://LOCALHOST:7100", "http://api.localhost:7100",
		"http://127.0.0.2:7100", "http://[::1]:7100", "http://[::ffff:127.0.0.1]:7100", "https://localhost",
	} {
		direct, ok := telmoni.DefaultClient(endpoint).Transport.(*http.Transport)
		if !ok || direct.Proxy != nil {
			t.Errorf("%s may go through a proxy", endpoint)
		}
	}
	if transport := telmoni.DefaultClient("https://api.telmoni.com").Transport; transport != nil {
		t.Errorf("another host is sent through %T, not net/http's default", transport)
	}
}

// A proxy the environment names, which net/http's own client takes for a name
// under localhost, or one the application dials through, never stands between an
// export and this machine.
func TestAProxyTheEnvironmentNamesIsPassedForThisMachine(t *testing.T) {
	var mu sync.Mutex
	var taken, keys []string
	answer := func(seen *[]string, what func(*http.Request) string) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			_, _ = io.Copy(io.Discard, r.Body)
			mu.Lock()
			*seen = append(*seen, what(r))
			mu.Unlock()
			_, _ = w.Write([]byte("{}"))
		})
	}
	proxy := httptest.NewServer(answer(&taken, func(r *http.Request) string { return r.URL.String() }))
	defer proxy.Close()
	server := httptest.NewServer(answer(&keys, func(r *http.Request) string { return r.Header.Get("Authorization") }))
	defer server.Close()
	port := server.URL[strings.LastIndex(server.URL, ":")+1:]
	// A process of its own, which reads the environment afresh.
	helper := exec.Command(os.Args[0], "-test.run=^TestProxyEnvironmentHelper$")
	helper.Env = append(os.Environ(), "TELMONI_PROXY_HELPER="+port, "TELMONI_PROXY_DIAL="+proxy.Listener.Addr().String(),
		"HTTP_PROXY="+proxy.URL, "http_proxy="+proxy.URL, "NO_PROXY=", "no_proxy=")
	if out, err := helper.CombinedOutput(); err != nil {
		t.Fatalf("the helper: %v\n%s", err, out)
	}
	mu.Lock()
	defer mu.Unlock()
	if want := []string{"http://api.localhost:" + port + "/control"}; !slices.Equal(taken, want) {
		t.Errorf("the proxy was given %q, want %q", taken, want)
	}
	if len(keys) == 0 {
		t.Fatal("nothing reached the server")
	}
	for _, key := range keys {
		if key != "Bearer "+testKey {
			t.Errorf("the server was sent %q", key)
		}
	}
}

// TestProxyEnvironmentHelper is the process TestAProxyTheEnvironmentNamesIsPassedForThisMachine starts.
func TestProxyEnvironmentHelper(t *testing.T) {
	port := os.Getenv("TELMONI_PROXY_HELPER")
	if port == "" {
		t.Skip("started by TestAProxyTheEnvironmentNamesIsPassedForThisMachine")
	}
	if resp, err := http.Post("http://api.localhost:"+port+"/control", "application/json", strings.NewReader("{}")); err == nil {
		_ = resp.Body.Close()
	}
	// And an application that dials every connection through it, as a SOCKS dialer does.
	if base, ok := http.DefaultTransport.(*http.Transport); ok {
		dialer := &net.Dialer{}
		base.DialContext = func(ctx context.Context, network, _ string) (net.Conn, error) {
			return dialer.DialContext(ctx, network, os.Getenv("TELMONI_PROXY_DIAL"))
		}
	}
	client, err := telmoni.New(telmoni.Config{Endpoint: "http://LOCALHOST:" + port, APIKey: testKey})
	if err != nil {
		t.Fatalf("building the client: %v", err)
	}
	_, run := client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	ctx, cancel := context.WithTimeout(context.Background(), waitTimeout)
	defer cancel()
	if err := client.Shutdown(ctx); err != nil {
		t.Fatalf("shutting down: %v", err)
	}
}

// An answer whose head is cut before its blank line is no answer, sent three
// times as in every SDK: a status line alone decides nothing.
func TestAnAnswerWhoseHeadIsCutIsSentAgain(t *testing.T) {
	var mu sync.Mutex
	tries := map[string]int{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		mu.Lock()
		tries[string(body)]++
		mu.Unlock()
		conn, sent, err := w.(http.Hijacker).Hijack()
		if err != nil {
			return
		}
		defer conn.Close()
		_, _ = sent.WriteString("HTTP/1.1 413 Payload Too Large\r\nContent-Ty")
		_ = sent.Flush()
	}))
	defer server.Close()
	sleeps, logs := &sleepLog{}, &logSink{}
	client, err := telmoni.New(telmoni.Config{Endpoint: server.URL, APIKey: testKey},
		telmoni.WithSleep(sleeps.sleep), telmoni.WithLogger(slog.New(logs)))
	if err != nil {
		t.Fatalf("building the client: %v", err)
	}
	_, run := client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	ctx, cancel := context.WithTimeout(context.Background(), waitTimeout)
	defer cancel()
	if err := client.Shutdown(ctx); err != nil {
		t.Errorf("shutting down: %v", err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(tries) == 0 {
		t.Fatal("nothing was sent")
	}
	var want []time.Duration
	for body, n := range tries {
		if n != 3 {
			t.Errorf("a batch was sent %d times, want 3: %.40s", n, body)
		}
		want = append(want, time.Second, 2*time.Second)
	}
	if got := sleeps.all(); !slices.Equal(got, want) {
		t.Errorf("waited %v, want %v", got, want)
	}
	warned := logs.warnings()
	if len(warned) != 1 || !regexp.MustCompile(`^telmoni: export failed after 3 attempts \(\S+\); the batch was dropped$`).MatchString(warned[0]) {
		t.Errorf("warnings %q", warned)
	}
}

// bigSpans is a span named s<n> for each size, an attribute of that many bytes
// beside one of a few.
func bigSpans(sizes ...int) []sdktrace.ReadOnlySpan {
	spans := make([]sdktrace.ReadOnlySpan, len(sizes))
	for n, size := range sizes {
		spans[n] = tracetest.SpanStub{
			Name: fmt.Sprintf("s%d", n),
			SpanContext: trace.NewSpanContext(trace.SpanContextConfig{
				TraceID: trace.TraceID{1}, SpanID: trace.SpanID{1}, TraceFlags: trace.FlagsSampled,
			}),
			StartTime: time.Unix(1, 0),
			EndTime:   time.Unix(2, 0),
			Attributes: []attribute.KeyValue{
				attribute.String("telmoni.tag.big", strings.Repeat("x", size)), attribute.String("app.kept", "yes"),
			},
		}.Snapshot()
	}
	return spans
}

// An export past ingest's 1 MiB, which it refuses whole and whose 413 may
// break an upload still going out, is halved until each part fits.
func TestAnExportPastIngestsMebibyteIsHalvedUntilEachPartFits(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	telmoni.ExportSpans(h.client, bigSpans(300_000, 300_000, 300_000, 300_000, 300_000, 300_000, 300_000, 300_000))
	requests := h.transport.all()
	if len(requests) != 4 {
		t.Fatalf("%d exports, want 4", len(requests))
	}
	for _, request := range requests {
		if len(request.body) > 1<<20 {
			t.Errorf("an export of %d bytes", len(request.body))
		}
	}
	want := []string{"s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7"}
	if got := names(h.transport.spans(t)); !slices.Equal(got, want) {
		t.Errorf("sent %q, want %q", got, want)
	}
	if warned := h.logs.warnings(); len(warned) != 0 {
		t.Errorf("warnings %q", warned)
	}
}

// A span alone past it is sent without what ingest drops for its length, rather
// than lost, since a run's own span carries the run's end.
func TestASpanAlonePastItGoesWithoutWhatIngestDropsForItsLength(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	telmoni.ExportSpans(h.client, bigSpans(100, 1<<20, 100))
	telmoni.ExportSpans(h.client, bigSpans(1<<20))
	var got []string
	for _, span := range h.transport.spans(t) {
		keys := slices.Sorted(maps.Keys(attributesOf(t, span)))
		got = append(got, span["name"].(string)+":"+strings.Join(keys, ","))
	}
	want := []string{"s0:app.kept,telmoni.tag.big", "s1:app.kept", "s2:app.kept,telmoni.tag.big", "s0:app.kept"}
	if !slices.Equal(got, want) {
		t.Errorf("sent %q, want %q", got, want)
	}
	warned := []string{"telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length"}
	if got := h.logs.warnings(); !slices.Equal(got, warned) {
		t.Errorf("warnings %q, want %q", got, warned)
	}
}

func TestASpanWhoseOwnNamePassesItIsDroppedWithOneWarning(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	named := tracetest.SpanStub{
		Name: strings.Repeat("x", 1<<20),
		SpanContext: trace.NewSpanContext(trace.SpanContextConfig{
			TraceID: trace.TraceID{1}, SpanID: trace.SpanID{1}, TraceFlags: trace.FlagsSampled,
		}),
		StartTime: time.Unix(1, 0),
		EndTime:   time.Unix(2, 0),
	}.Snapshot()
	telmoni.ExportSpans(h.client, []sdktrace.ReadOnlySpan{named})
	if sent := h.transport.all(); len(sent) != 0 {
		t.Errorf("%d exports, want none", len(sent))
	}
	want := []string{"telmoni: a span past the 1 MiB an export may hold was dropped"}
	if got := h.logs.warnings(); !slices.Equal(got, want) {
		t.Errorf("warnings %q, want %q", got, want)
	}
}
