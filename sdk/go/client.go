package telmoni

import (
	"context"
	"errors"
	"log/slog"
	"math/rand/v2"
	"net"
	"net/http"
	"runtime"
	"time"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/sdk/resource"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
)

// Transport sends one export and returns its answer. An error means the
// platform could not be reached, and the batch is sent again. An
// *http.Client's Do and an http.RoundTripper's RoundTrip are both one.
type Transport func(*http.Request) (*http.Response, error)

// Option configures a Client beyond its Config: the transport, the sleep
// between attempts and the logger, which a test replaces.
type Option func(*options)

type options struct {
	transport Transport
	sleep     func(time.Duration)
	logger    *slog.Logger
	random    func() float64
	limits    queueLimits
}

// noRedirect answers a redirect as the 3xx it is: followed, it would resend
// the bearer to wherever it points.
func noRedirect(*http.Request, []*http.Request) error {
	return http.ErrUseLastResponse
}

// WithTransport sends every export through t instead of an http.Client with
// a 10 s timeout that follows no redirect.
func WithTransport(t Transport) Option {
	return func(o *options) {
		if t != nil {
			o.transport = t
		}
	}
}

// WithSleep waits through sleep before an export is sent again, instead of the
// SDK's own wait, which a shutdown giving up ends at once.
func WithSleep(sleep func(time.Duration)) Option {
	return func(o *options) {
		if sleep != nil {
			o.sleep = sleep
		}
	}
}

// WithLogger sends the SDK's warnings to logger instead of slog.Default().
// Each distinct warning is logged once per client.
func WithLogger(logger *slog.Logger) Option {
	return func(o *options) {
		if logger != nil {
			o.logger = logger
		}
	}
}

// withRandom replaces the source the sample rate is drawn against, so a test
// decides which runs are kept.
func withRandom(random func() float64) Option {
	return func(o *options) {
		if random != nil {
			o.random = random
		}
	}
}

// withQueue sizes the processor's queue and batch and sets its delay, so a
// test fills a queue of two or a batch of three.
func withQueue(maxQueue, maxBatch int, delay time.Duration) Option {
	return func(o *options) {
		o.limits = queueLimits{maxQueue: maxQueue, maxBatch: maxBatch, delay: delay}
	}
}

// Client is the SDK's entry point: a tracer provider of its own, and the runs
// it records. The provider is built part by part, resource, sampler, limits
// and processor, so no OTEL_* variable reaches what is sent, and an
// application's own OpenTelemetry is neither replaced, sent to Telmoni, nor
// changed.
type Client struct {
	config    Config
	exporter  *exporter
	processor *processor
	provider  *sdktrace.TracerProvider
	tracer    trace.Tracer
	random    func() float64
	// prices are the project's own, which the guard counts a call by first.
	prices map[name]rate
}

// Telmoni is a convenience alias for Client.
type Telmoni = Client

// defaultClient is net/http's, through any proxy the environment names, and to
// this machine through none (bypassesProxy): its default transport cloned with no
// Proxy and a dialer of its own, since an application may dial through one, SOCKS
// from ALL_PROXY say.
func defaultClient(endpoint string) *http.Client {
	client := &http.Client{Timeout: requestTimeout, CheckRedirect: noRedirect}
	if bypassesProxy(endpoint) {
		direct := new(http.Transport)
		if base, ok := http.DefaultTransport.(*http.Transport); ok {
			direct = base.Clone()
		}
		direct.Proxy = nil
		direct.DialContext = (&net.Dialer{Timeout: 30 * time.Second, KeepAlive: 30 * time.Second}).DialContext
		direct.Dial, direct.DialTLS, direct.DialTLSContext = nil, nil, nil
		client.Transport = direct
	}
	return client
}

// New builds a client from cfg. It fails on a SampleRate outside (0, 1], on
// an endpoint no export could use or that would carry the key in the clear,
// and on a price that names no model or is no number of dollars from 0.
func New(cfg Config, opts ...Option) (*Client, error) {
	cfg = cfg.normalized()
	if !(cfg.SampleRate > 0 && cfg.SampleRate <= 1) {
		return nil, errors.New("telmoni: SampleRate must be above 0 and up to 1")
	}
	if err := checkEndpoint(cfg.Endpoint); err != nil {
		return nil, err
	}
	prices, err := ownRates(cfg.Prices)
	if err != nil {
		return nil, err
	}
	o := options{
		transport: defaultClient(cfg.Endpoint).Do,
		logger:    slog.Default(),
		random:    rand.Float64,
		limits:    defaultQueueLimits,
	}
	for _, opt := range opts {
		opt(&o)
	}
	exp := newExporter(cfg, o.transport, o.sleep, o.logger)
	proc := newProcessor(exp, o.logger, o.limits)
	root := cfg.Sampler
	if root == nil {
		root = sdktrace.AlwaysSample()
	}
	// WithSampler and WithRawSpanLimits replace what OTEL_TRACES_SAMPLER and
	// the OTEL_SPAN_* variables would have set. OpenTelemetry merges
	// OTEL_RESOURCE_ATTRIBUTES into the provider's resource whatever is given
	// here, so the exporter writes a resource of its own. The id generator is
	// ours, for a record's derived ids, and reads nothing either.
	provider := sdktrace.NewTracerProvider(
		sdktrace.WithResource(resource.NewSchemaless(
			attribute.String(distroName, distro),
			attribute.String("telemetry.distro.version", Version),
		)),
		sdktrace.WithSampler(recordingSampler{root: root}),
		sdktrace.WithIDGenerator(ids{}),
		sdktrace.WithRawSpanLimits(sdktrace.SpanLimits{
			AttributeValueLengthLimit:   -1,
			AttributeCountLimit:         128,
			EventCountLimit:             0,
			LinkCountLimit:              0,
			AttributePerEventCountLimit: 0,
			AttributePerLinkCountLimit:  0,
		}),
		sdktrace.WithSpanProcessor(proc),
	)
	// Dropped without Shutdown, the worker sends what is queued and leaves once
	// nothing reaches the provider, no copy of the client nor a span; proc never does.
	runtime.AddCleanup(provider, func(p *processor) { p.stop() }, proc)
	return &Client{
		config:    cfg,
		exporter:  exp,
		processor: proc,
		provider:  provider,
		tracer:    provider.Tracer(scope, trace.WithInstrumentationVersion(Version)),
		random:    o.random,
		prices:    prices,
	}, nil
}

// NewFromEnv builds a client from ConfigFromEnv.
func NewFromEnv() (*Client, error) {
	return New(ConfigFromEnv())
}

// built is whether c came from New: a zero Client, or the nil one a failed New
// returns, sends nothing rather than panic.
func (c *Client) built() bool {
	return c != nil && c.exporter != nil
}

// Config returns the configuration as the client holds it.
func (c *Client) Config() Config {
	if c == nil {
		return Config{}
	}
	return c.config
}

// ContentMode is the project's content mode, learned from the last answer:
// "" before the first one.
func (c *Client) ContentMode() string {
	if !c.built() {
		return ""
	}
	return c.exporter.contentMode()
}

// Flush sends everything queued and waits until nothing is left to send, kept
// or not, returning at once after a shutdown: a batch refused or dropped is
// logged, never returned. It waits for ctx, or 30 seconds when ctx has no
// deadline, as the other SDKs wait at most, and then returns ctx's error; a
// batch still being sent is left to the worker.
func (c *Client) Flush(ctx context.Context) error {
	if !c.built() {
		return nil
	}
	if ctx == nil {
		ctx = context.Background()
	}
	return c.provider.ForceFlush(ctx)
}

// Shutdown flushes, then stops the exporter's worker. Nothing ended after it
// is sent, even when ctx is already done. It waits for ctx, or 30 seconds
// when ctx has no deadline; past that, or at once for a ctx already done,
// what is left is given up: the request in flight is cancelled and no batch
// or attempt follows. A client dropped without it sends what it queued once
// collected, though an exit waits for neither.
func (c *Client) Shutdown(ctx context.Context) error {
	if !c.built() {
		return nil
	}
	if ctx == nil {
		ctx = context.Background()
	}
	// Ours first: the provider's returns before reaching it when ctx is
	// already done, and never runs twice. Its error can only be ctx's.
	err := c.processor.Shutdown(ctx)
	_ = c.provider.Shutdown(ctx)
	return err
}

// String formats the client for %v and %s, with the API key redacted. fmt
// reaches an unexported field by reflection, past Config's own String, so
// without this the client would print the key.
func (c Client) String() string {
	return "{config:" + c.config.String() + "}"
}

// GoString formats the client for %#v, with the API key redacted.
func (c Client) GoString() string {
	return "telmoni.Client{config:" + c.config.GoString() + "}"
}
