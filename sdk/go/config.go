package telmoni

import (
	"fmt"
	"net"
	"net/url"
	"os"
	"strings"

	sdktrace "go.opentelemetry.io/otel/sdk/trace"
)

// DefaultEndpoint is the machine host; the console's, https://telmoni.com,
// serves people.
const DefaultEndpoint = "https://api.telmoni.com"

// Config is how the SDK reaches Telmoni, and what it sends.
type Config struct {
	// Endpoint is the platform's base URL. Blank means DefaultEndpoint, and
	// trailing slashes are stripped.
	Endpoint       string `json:"endpoint"`
	OrganizationID string `json:"organization_id,omitempty"`
	// APIKey is the API key (telmoni_…) with the ingest scope. It is never
	// marshalled, and String and GoString redact it: a configuration logged
	// or written out would otherwise carry the key.
	APIKey string `json:"-"`
	// CaptureContent lets prompts, completions, tool arguments and results
	// leave the process, once the project's content mode allows it too.
	// Otherwise the exporter takes them off every span.
	CaptureContent bool `json:"capture_content"`
	// SampleRate is the share of runs sent, above 0 and up to 1; 0 means 1.
	// A run is dropped whole, in the processor, never through the trace's
	// sampled flag, so an application's own sampling is untouched.
	SampleRate float64 `json:"sample_rate"`
	// Sampler decides a run that starts a trace of its own, as the
	// application's root sampler would decide its own spans; nil samples
	// every such run. Nothing is dropped by it: a run it does not sample is
	// still sent, with the trace's sampled flag unset.
	Sampler sdktrace.Sampler `json:"-"`
	// Environment is the deployment this process runs in (production,
	// staging), sent on every span's resource as deployment.environment.name;
	// blank sends nothing.
	Environment string `json:"environment,omitempty"`
	// Release is the release this process runs (a version or a commit), sent
	// on every span's resource as service.version; blank sends nothing.
	Release string `json:"release,omitempty"`
}

// String formats the configuration for %v and %s, with the API key redacted.
func (c Config) String() string {
	return fmt.Sprintf("{Endpoint:%s OrganizationID:%s APIKey:%s CaptureContent:%t SampleRate:%g Environment:%s Release:%s}",
		c.Endpoint, c.OrganizationID, redacted(c.APIKey), c.CaptureContent, c.SampleRate, c.Environment, c.Release)
}

// GoString formats the configuration for %#v, with the API key redacted.
func (c Config) GoString() string {
	return fmt.Sprintf("telmoni.Config{Endpoint:%q, OrganizationID:%q, APIKey:%q, CaptureContent:%t, SampleRate:%g, Environment:%q, Release:%q}",
		c.Endpoint, c.OrganizationID, redacted(c.APIKey), c.CaptureContent, c.SampleRate, c.Environment, c.Release)
}

func redacted(secret string) string {
	if secret == "" {
		return ""
	}
	return "<redacted>"
}

// normalized is the configuration as the client holds it: a blank value is
// unset, as the CLI reads the same variables, and the endpoint carries no
// trailing slash since the path is appended to it.
func (c Config) normalized() Config {
	c.Endpoint = strings.TrimRight(strings.TrimSpace(c.Endpoint), "/")
	if c.Endpoint == "" {
		c.Endpoint = DefaultEndpoint
	}
	c.OrganizationID = strings.TrimSpace(c.OrganizationID)
	c.APIKey = strings.TrimSpace(c.APIKey)
	c.Environment = strings.TrimSpace(c.Environment)
	c.Release = strings.TrimSpace(c.Release)
	if c.SampleRate == 0 {
		c.SampleRate = 1
	}
	return c
}

// isLoopbackHost is whether host is this machine: localhost, a name under it,
// or a loopback address.
func isLoopbackHost(host string) bool {
	if ip := net.ParseIP(host); ip != nil {
		return ip.IsLoopback()
	}
	return host == "localhost" || strings.HasSuffix(host, ".localhost")
}

// refuseCleartext keeps plain HTTP to this machine: every export carries the
// key.
func refuseCleartext(endpoint string) error {
	parsed, err := url.Parse(endpoint)
	if err != nil {
		return nil
	}
	if parsed.Scheme == "http" && !isLoopbackHost(parsed.Hostname()) {
		return fmt.Errorf("telmoni: refusing to send the API key over unencrypted HTTP to %q; use HTTPS", parsed.Hostname())
	}
	return nil
}

// DefaultConfig returns the default SDK configuration.
func DefaultConfig() Config {
	return Config{Endpoint: DefaultEndpoint, SampleRate: 1}
}

// ConfigFromEnv reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG, and
// TELMONI_ENVIRONMENT and TELMONI_RELEASE for the deployment, a blank value
// counting as unset. No OTEL_* variable is read: the SDK's tracer provider is
// built part by part.
func ConfigFromEnv() Config {
	return Config{
		Endpoint:       os.Getenv("TELMONI_ENDPOINT"),
		OrganizationID: os.Getenv("TELMONI_ORG"),
		APIKey:         os.Getenv("TELMONI_API_KEY"),
		Environment:    os.Getenv("TELMONI_ENVIRONMENT"),
		Release:        os.Getenv("TELMONI_RELEASE"),
	}.normalized()
}

// Bounds is what a monitor holds a run to; a zero field is not sent.
type Bounds struct {
	StallAfterS int
	MaxSteps    int
	MaxCostUSD  float64
}

// Usage is a model call's tokens, counted as the conventions count them:
// input tokens include the cache's reads and writes, output tokens include
// reasoning. Where a provider reports parts apart, sum them first. All four
// counts of a Usage are sent, a zero among them.
type Usage struct {
	InputTokens           int
	OutputTokens          int
	CacheReadInputTokens  int
	CacheWriteInputTokens int
}
