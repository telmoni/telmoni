package telmoni_test

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"net/http"
	"regexp"
	"strings"
	"testing"

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
