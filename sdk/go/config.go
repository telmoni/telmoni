package telmoni

import (
	"errors"
	"fmt"
	"net/netip"
	"os"
	"slices"
	"strconv"
	"strings"
	"unicode"

	sdktrace "go.opentelemetry.io/otel/sdk/trace"
)

// DefaultEndpoint is the machine host; the console's, https://telmoni.com,
// serves people.
const DefaultEndpoint = "https://api.telmoni.com"

// Config is how the SDK reaches Telmoni, and what it sends.
type Config struct {
	// Endpoint is the platform's base URL, read as the CLI reads one: blank is
	// DefaultEndpoint, a host without a scheme gets https://, http:// locally.
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
// unset and the endpoint read as the CLI reads the same variables.
func (c Config) normalized() Config {
	c.Endpoint = endpointOf(c.Endpoint)
	c.OrganizationID = strings.TrimSpace(c.OrganizationID)
	c.APIKey = strings.TrimSpace(c.APIKey)
	c.Environment = strings.TrimSpace(c.Environment)
	c.Release = strings.TrimSpace(c.Release)
	if c.SampleRate == 0 {
		c.SampleRate = 1
	}
	return c
}

const (
	nameChars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-"
	pathChars = nameChars + "~!$&'()*+,;=:@/"
	hexDigits = "0123456789ABCDEFabcdef"
	digits    = "0123456789"
)

// hostOf is the host endpoint names, as written, or false where it is no URL every SDK
// reads alike (sdk/fixtures/endpoints.json): nothing a URL parser fills in or rewrites.
func hostOf(endpoint string) (string, bool) {
	for i := 0; i < len(endpoint); i++ {
		if endpoint[i] < '!' || endpoint[i] > '~' {
			return "", false
		}
	}
	scheme, rest, found := strings.Cut(endpoint, "://")
	if !found || !(strings.EqualFold(scheme, "http") || strings.EqualFold(scheme, "https")) {
		return "", false
	}
	authority, path, _ := strings.Cut(rest, "/")
	segments := strings.Split(path, "/")
	if !only(path, pathChars) || slices.Contains(segments, ".") || slices.Contains(segments, "..") {
		return "", false
	}
	var host, port string
	if strings.HasPrefix(authority, "[") {
		end := strings.IndexByte(authority, ']') + 1
		if end == 0 {
			return "", false
		}
		host, port = authority[:end], authority[end:]
		inner := host[1 : end-1]
		if addr, err := netip.ParseAddr(inner); !only(inner, hexDigits+":.") || err != nil || !addr.Is6() {
			return "", false
		}
	} else {
		name, number, colon := strings.Cut(authority, ":")
		host = name
		if colon {
			port = ":" + number
		}
		// WHATWG's parser reads a name ending in a number as an IPv4 address.
		labels := strings.Split(strings.TrimSuffix(host, "."), ".")
		if host == "" || !only(host, nameChars) || slices.ContainsFunc(labels, badLabel) || (numbered(host) && !isIPv4(host)) {
			return "", false
		}
	}
	return host, portOK(port)
}

// only is whether every byte of s is one of set's.
func only(s, set string) bool {
	for i := 0; i < len(s); i++ {
		if strings.IndexByte(set, s[i]) < 0 {
			return false
		}
	}
	return true
}

// numbered is whether host's last label, a trailing dot aside, is a number.
func numbered(host string) bool {
	host = strings.TrimSuffix(host, ".")
	last := host[strings.LastIndexByte(host, '.')+1:]
	if last != "" && only(last, digits) {
		return true
	}
	return len(last) >= 2 && strings.EqualFold(last[:2], "0x") && only(last[2:], hexDigits)
}

func isIPv4(host string) bool {
	addr, err := netip.ParseAddr(host)
	return err == nil && addr.Is4()
}

// portOK is whether port is none, an empty one, or up to five digits from 1 to
// 65535.
func portOK(port string) bool {
	if port == "" {
		return true
	}
	number := port[1:]
	if port[0] != ':' || len(number) > 5 || !only(number, digits) {
		return false
	}
	if number == "" {
		return true
	}
	n, _ := strconv.Atoi(number)
	return n > 0 && n <= 65535
}

// badLabel is whether a name's label is one DNS or TLS refuses: empty, past
// 63 characters, or with a hyphen at either end.
func badLabel(label string) bool {
	return label == "" || len(label) > 63 || strings.HasPrefix(label, "-") || strings.HasSuffix(label, "-")
}

// isThisMachine is whether host, as hostOf gives it, is localhost, a name under it,
// 127.0.0.0/8 or ::1, an IPv4-mapped address read as its IPv4.
func isThisMachine(host string) bool {
	if strings.HasPrefix(host, "[") {
		addr, err := netip.ParseAddr(host[1 : len(host)-1])
		return err == nil && addr.Unmap().IsLoopback()
	}
	if addr, err := netip.ParseAddr(host); err == nil && addr.Is4() {
		return addr.IsLoopback()
	}
	host = strings.ToLower(host)
	return host == "localhost" || strings.HasSuffix(host, ".localhost")
}

// bypassesProxy is whether endpoint goes straight to this machine, past any proxy
// the environment names: a proxy would take plain HTTP's key off it in the clear,
// and one on another machine cannot reach this one's ports anyway. net/http's
// spares an exact localhost and a loopback IP alone.
func bypassesProxy(endpoint string) bool {
	host, ok := hostOf(endpoint)
	return ok && isThisMachine(host)
}

// endpointOf is the endpoint as the CLI reads one: a host without a scheme
// gets https://, or http:// for this machine.
func endpointOf(given string) string {
	given = strings.TrimSpace(given)
	// Only what follows the :// is trimmed, so a second reading changes nothing.
	if head, tail, found := strings.Cut(given, "://"); found {
		return head + "://" + trimTail(tail)
	}
	endpoint := trimTail(given)
	if endpoint == "" {
		return DefaultEndpoint
	}
	// https:/host or https//host is a scheme mistyped, not a host: checkEndpoint refuses it.
	first := endpoint
	if i := strings.IndexAny(endpoint, ":/"); i >= 0 {
		first = endpoint[:i]
	}
	if strings.EqualFold(first, "http") || strings.EqualFold(first, "https") {
		return endpoint
	}
	if host, ok := hostOf("http://" + endpoint); ok && isThisMachine(host) {
		return "http://" + endpoint
	}
	return "https://" + endpoint
}

// trimTail drops trailing slashes and spaces, so the path appends cleanly.
func trimTail(text string) string {
	return strings.TrimRightFunc(text, func(r rune) bool { return r == '/' || unicode.IsSpace(r) })
}

// checkEndpoint refuses an endpoint the SDKs could read two ways, which fails an export
// or reaches another host, and plain HTTP to another machine, which would bare the key.
func checkEndpoint(endpoint string) error {
	host, ok := hostOf(endpoint)
	if !ok {
		return errors.New("telmoni: the endpoint must be an http:// or https:// URL")
	}
	if scheme, _, _ := strings.Cut(endpoint, "://"); strings.EqualFold(scheme, "http") && !isThisMachine(host) {
		return fmt.Errorf("telmoni: refusing to send the API key over unencrypted HTTP to %q; use HTTPS", host)
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

// Bounds is what a monitor holds a run to; a zero field is not sent, nor one
// ingest would refuse the run for, which Run leaves off with one warning.
type Bounds struct {
	StallAfterS int
	MaxSteps    int
	MaxCostUSD  float64
}

// Usage is a model call's tokens, counted as the conventions count them:
// input tokens include the cache's reads and writes, output tokens include
// reasoning. Where a provider reports parts apart, sum them first. Each count
// from 0 up is sent, a zero among them.
type Usage struct {
	InputTokens           int
	OutputTokens          int
	CacheReadInputTokens  int
	CacheWriteInputTokens int
}
