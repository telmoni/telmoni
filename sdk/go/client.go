package telmoni

import (
	"context"
	"errors"
	"log/slog"
	"math/rand/v2"
	"net/http"
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

// WithSleep waits through sleep, instead of time.Sleep, before an export is
// sent again.
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
}

// Telmoni is a convenience alias for Client.
type Telmoni = Client

// New builds a client from cfg. It fails when SampleRate is not above 0 and
// up to 1.
func New(cfg Config, opts ...Option) (*Client, error) {
	cfg = cfg.normalized()
	if !(cfg.SampleRate > 0 && cfg.SampleRate <= 1) {
		return nil, errors.New("telmoni: SampleRate must be above 0 and up to 1")
	}
	if err := refuseCleartext(cfg.Endpoint); err != nil {
		return nil, err
	}
	o := options{
		transport: (&http.Client{Timeout: requestTimeout, CheckRedirect: noRedirect}).Do,
		sleep:     time.Sleep,
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
	// here, so the exporter writes a resource of its own. The default id
	// generator reads nothing.
	provider := sdktrace.NewTracerProvider(
		sdktrace.WithResource(resource.NewSchemaless(
			attribute.String(distroName, distro),
			attribute.String("telemetry.distro.version", Version),
		)),
		sdktrace.WithSampler(recordingSampler{root: root}),
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
	return &Client{
		config:    cfg,
		exporter:  exp,
		processor: proc,
		provider:  provider,
		tracer:    provider.Tracer(scope, trace.WithInstrumentationVersion(Version)),
		random:    o.random,
	}, nil
}

// NewFromEnv builds a client from ConfigFromEnv.
func NewFromEnv() (*Client, error) {
	return New(ConfigFromEnv())
}

// Config returns the configuration as the client holds it.
func (c *Client) Config() Config {
	return c.config
}

// ContentMode is the project's content mode, learned from the last answer:
// "" before the first one.
func (c *Client) ContentMode() string {
	return c.exporter.contentMode()
}

// Flush sends everything queued and waits until the platform has it, or ctx
// ends.
func (c *Client) Flush(ctx context.Context) error {
	return c.provider.ForceFlush(ctx)
}

// Shutdown flushes, then stops the exporter's worker. Nothing is sent after
// it. It waits for ctx, or 30 seconds when ctx has no deadline, and a batch
// still retrying past that is left to the worker, which the process exit
// takes with it.
func (c *Client) Shutdown(ctx context.Context) error {
	return c.provider.Shutdown(ctx)
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
