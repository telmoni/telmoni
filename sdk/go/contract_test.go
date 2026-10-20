package telmoni_test

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"math"
	"net/http"
	"net/http/httptest"
	"regexp"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

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

func TestPlainHTTPIsRefusedExceptToThisMachine(t *testing.T) {
	for _, endpoint := range []string{
		"http://localhost:8080",
		"http://api.localhost",
		"http://127.0.0.1:8080/",
		"http://[::1]:8080",
		"https://api.example.com",
	} {
		if got := mustNew(t, telmoni.Config{Endpoint: endpoint}).Config().Endpoint; got != strings.TrimSuffix(endpoint, "/") {
			t.Errorf("Endpoint %q became %q", endpoint, got)
		}
	}
	for _, endpoint := range []string{"http://api.example.com", "http://10.0.0.5:8080"} {
		if _, err := telmoni.New(telmoni.Config{Endpoint: endpoint}); err == nil || !strings.Contains(err.Error(), "unencrypted HTTP") {
			t.Errorf("New(%q) = %v, want a refusal of unencrypted HTTP", endpoint, err)
		}
	}
}

func TestAHostWithoutASchemeIsReadAsTheCLIReadsIt(t *testing.T) {
	for given, want := range map[string]string{
		"api.example.com":           "https://api.example.com",
		"localhost:8080/":           "http://localhost:8080",
		"LOCALHOST:8080":            "http://LOCALHOST:8080",
		"127.0.0.1:8080":            "http://127.0.0.1:8080",
		"[::1]:8080":                "http://[::1]:8080",
		"localhost.example.com":     "https://localhost.example.com",
		"https://api.example.com /": "https://api.example.com",
		"api.example.com /":         "https://api.example.com",
		"/":                         telmoni.DefaultEndpoint,
	} {
		if got := mustNew(t, telmoni.Config{Endpoint: given}).Config().Endpoint; got != want {
			t.Errorf("Endpoint %q became %q, want %q", given, got, want)
		}
		if got := mustNew(t, telmoni.Config{Endpoint: want}).Config().Endpoint; got != want {
			t.Errorf("Endpoint %q, read twice, became %q", want, got)
		}
	}
	for _, endpoint := range []string{
		"ftp://api.example.com", "https://", "file:///tmp/telmoni", "localhost:abc", "localhost:99999",
		"https:/api.example.com", "https//api.example.com", "https://api .example.com",
	} {
		if _, err := telmoni.New(telmoni.Config{Endpoint: endpoint}); err == nil || !strings.Contains(err.Error(), "http:// or https://") {
			t.Errorf("New(%q) = %v, want a refusal of the endpoint", endpoint, err)
		}
	}
	// From the environment, normalized twice on its way to New.
	t.Setenv("TELMONI_ENDPOINT", "https://")
	if _, err := telmoni.NewFromEnv(); err == nil || !strings.Contains(err.Error(), "http:// or https://") {
		t.Errorf("NewFromEnv() with https:// = %v, want a refusal of the endpoint", err)
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
