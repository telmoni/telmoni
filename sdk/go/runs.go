package telmoni

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

// RunOptions is what a run is opened with; each field is optional.
type RunOptions struct {
	// UserID is the end user the run is for, as the application's own id for
	// them, sent as given for ingest to hash.
	UserID string
	// SessionID is the session the run belongs to, as the application's own
	// id for it.
	SessionID string
	// Tags travel as telmoni.tag.<key>.
	Tags map[string]string
	// Bounds are what a monitor holds the run to.
	Bounds *Bounds
	// SampleRate is this run's share of being sent, above 0 and up to 1; 0
	// means the client's.
	SampleRate float64
}

// ModelCallOptions is what a model call is opened with.
type ModelCallOptions struct {
	// Provider is the model's provider, as the conventions name it: openai,
	// anthropic, ...
	Provider string
	// Operation is chat, generate_content, text_completion or embeddings;
	// blank means chat.
	Operation string
	// InputMessages and SystemInstructions are content: recorded as JSON,
	// sent only when CaptureContent is on and the project's mode allows.
	InputMessages      any
	SystemInstructions any
}

// ToolCallOptions is what a tool call is opened with.
type ToolCallOptions struct {
	// Arguments are content: recorded as JSON, sent only when CaptureContent
	// is on and the project's mode allows.
	Arguments any
}

// Answer is what a model answered: its name, its tokens and its messages.
type Answer struct {
	ResponseModel string
	Usage         *Usage
	// OutputMessages are content: recorded as JSON, sent only when
	// CaptureContent is on and the project's mode allows.
	OutputMessages any
}

// handle is a span the SDK opened, until its End. A handle opened outside a
// run has no span and does nothing.
type handle struct {
	span trace.Span
	once sync.Once
}

// End ends the span; a second End does nothing.
func (h *handle) End() {
	if h == nil || h.span == nil {
		return
	}
	h.once.Do(func() { h.span.End() })
}

// Fail marks the span failed with err's Go type and never its message: the
// message is content. An error that is or wraps context.Canceled marks it
// cancelled instead: the agent gave the call up. A deadline that ran out is
// a failure, as a timeout is in the other SDKs, since a stalled call is what
// a monitor is for; it fails with its own type.
func (h *handle) Fail(err error) {
	if h == nil || h.span == nil || err == nil {
		return
	}
	if errors.Is(err, context.Canceled) {
		h.Cancel()
		return
	}
	h.FailWith(errorTypeName(err))
}

// FailWith marks the span failed with errorType as its error.type, and
// status ERROR with no message.
func (h *handle) FailWith(errorType string) {
	if h == nil || h.span == nil {
		return
	}
	h.span.SetAttributes(attribute.String(attrErrorType, errorType))
	h.span.SetStatus(codes.Error, "")
}

// Cancel marks the span cancelled by the agent, which is neither ok nor an
// error.
func (h *handle) Cancel() {
	if h == nil || h.span == nil {
		return
	}
	h.span.SetAttributes(attribute.Bool(attrCancelled, true))
}

// errorTypeName is err's Go type without its pointer star: errors.errorString,
// net.OpError.
func errorTypeName(err error) string {
	return strings.TrimPrefix(fmt.Sprintf("%T", err), "*")
}

// Run is a run of an agent: a span of its own in whatever trace was current,
// opened by telmoni.run.start. Every span inside it carries the run's id,
// agent, user and session, so ingest knows each span's run without
// waiting for the trace's tree.
type Run struct {
	handle
	client    *Client
	id        string
	agent     string
	inherited []attribute.KeyValue
}

// Step is a step of a run.
type Step struct {
	handle
}

// ModelCall is a call to a model inside a run.
type ModelCall struct {
	handle
}

// ToolCall is a call to a tool inside a run.
type ToolCall struct {
	handle
}

type runKey struct{}

func contextWithRun(ctx context.Context, r *Run) context.Context {
	return context.WithValue(ctx, runKey{}, r)
}

// RunFromContext is the run open in ctx, or nil outside one. A nil Run opens
// handles that do nothing, and its own End, Fail, FailWith and Cancel do
// nothing, so nothing is sent outside a run and nothing panics.
func RunFromContext(ctx context.Context) *Run {
	if ctx == nil {
		return nil
	}
	r, _ := ctx.Value(runKey{}).(*Run)
	return r
}

// Run opens a run of agent: the span "invoke_agent <agent>", under the span in
// ctx or at the root of a trace of its own, and sends telmoni.run.start at
// once. The context returned carries the run and its span, so spans the
// application opens with it nest under the run. End it with run.End, usually
// deferred.
//
// A blank agent name opens no run: the handle returned does nothing, nothing
// is sent, and the client warns once. A SampleRate outside (0, 1] is replaced
// by the client's, with one warning. Neither panics: telemetry never takes
// the agent down, where Python and TypeScript raise for the same mistakes.
func (c *Client) Run(ctx context.Context, agent string, opts RunOptions) (context.Context, *Run) {
	if ctx == nil {
		ctx = context.Background()
	}
	if strings.TrimSpace(agent) == "" {
		c.exporter.warnOnce("run:agent", "telmoni: a run needs its agent's name; nothing is sent for one without")
		return ctx, &Run{client: c}
	}
	rate := c.config.SampleRate
	if opts.SampleRate != 0 {
		if opts.SampleRate > 0 && opts.SampleRate <= 1 {
			rate = opts.SampleRate
		} else {
			c.exporter.warnOnce("run:rate", "telmoni: a run's SampleRate must be above 0 and up to 1; the client's is used")
		}
	}
	runID := newUUID()
	inherited := []attribute.KeyValue{
		attribute.String(attrRunID, runID),
		attribute.String(attrAgent, agent),
	}
	if opts.UserID != "" {
		inherited = append(inherited, attribute.String(attrUserID, opts.UserID))
	}
	if opts.SessionID != "" {
		inherited = append(inherited, attribute.String(attrSessionID, opts.SessionID))
	}
	common := append(boundAttributes(opts.Bounds), attribute.Float64(attrSampleRate, rate))
	attrs := make([]attribute.KeyValue, 0, 2+len(inherited)+len(common)+len(opts.Tags))
	attrs = append(attrs, attribute.String(attrOperation, runOperation), attribute.String(attrAgentName, agent))
	attrs = append(attrs, inherited...)
	attrs = append(attrs, common...)
	for key, value := range opts.Tags {
		attrs = append(attrs, attribute.String(tagPrefix+key, value))
	}
	start := time.Now()
	ctx, span := c.tracer.Start(ctx, runOperation+" "+agent,
		trace.WithSpanKind(trace.SpanKindInternal),
		trace.WithAttributes(attrs...),
		trace.WithTimestamp(start),
	)
	if rate < 1 && c.random() >= rate {
		c.processor.sampleOut(runID)
	}
	// OpenTelemetry sends a span only once it ends; without this one a run
	// stalled in its first call would never be heard.
	openerAttrs := make([]attribute.KeyValue, 0, len(inherited)+len(common))
	openerAttrs = append(openerAttrs, inherited...)
	openerAttrs = append(openerAttrs, common...)
	_, opener := c.tracer.Start(ctx, runStart,
		trace.WithSpanKind(trace.SpanKindInternal),
		trace.WithAttributes(openerAttrs...),
		trace.WithTimestamp(start),
	)
	opener.End(trace.WithTimestamp(start))
	run := &Run{handle: handle{span: span}, client: c, id: runID, agent: agent, inherited: inherited}
	return contextWithRun(ctx, run), run
}

// ID is the run's id, a UUID, carried by every span inside it.
func (r *Run) ID() string {
	if r == nil {
		return ""
	}
	return r.id
}

// Agent is the agent's name.
func (r *Run) Agent() string {
	if r == nil {
		return ""
	}
	return r.agent
}

// End ends the run's span; a second End does nothing. End, Fail, FailWith and
// Cancel are the handle's, declared again on Run so a nil Run does nothing:
// reached through the embedded handle, the nil Run would be dereferenced
// before the handle's own check.
func (r *Run) End() {
	if r == nil {
		return
	}
	r.handle.End()
}

// Fail marks the run failed with err's Go type, never its message, or
// cancelled for an error that is or wraps context.Canceled, as a step's Fail
// does.
func (r *Run) Fail(err error) {
	if r == nil {
		return
	}
	r.handle.Fail(err)
}

// FailWith marks the run failed with errorType as its error.type, and status
// ERROR with no message.
func (r *Run) FailWith(errorType string) {
	if r == nil {
		return
	}
	r.handle.FailWith(errorType)
}

// Cancel marks the run cancelled by the agent, which is neither ok nor an
// error.
func (r *Run) Cancel() {
	if r == nil {
		return
	}
	r.handle.Cancel()
}

// Step opens a step named name under the span in ctx: the run's, or a step's
// to nest under it. The context returned carries the step's span.
func (r *Run) Step(ctx context.Context, name string) (context.Context, *Step) {
	if r == nil || r.span == nil {
		return ctx, &Step{}
	}
	ctx, span := r.child(ctx, name, trace.SpanKindInternal, nil)
	return ctx, &Step{handle: handle{span: span}}
}

// ModelCall opens a call to model under the span in ctx; Answered records
// what the model answered.
func (r *Run) ModelCall(ctx context.Context, model string, opts ModelCallOptions) (context.Context, *ModelCall) {
	if r == nil || r.span == nil {
		return ctx, &ModelCall{}
	}
	operation := modelOperation(opts.Operation)
	attrs := []attribute.KeyValue{
		attribute.String(attrOperation, operation),
		attribute.String(attrRequestModel, model),
	}
	if opts.Provider != "" {
		attrs = append(attrs, attribute.String(attrProvider, opts.Provider))
	}
	if opts.InputMessages != nil {
		attrs = append(attrs, attribute.String(attrInputMessages, asJSON(opts.InputMessages)))
	}
	if opts.SystemInstructions != nil {
		attrs = append(attrs, attribute.String(attrSystemInstructions, asJSON(opts.SystemInstructions)))
	}
	ctx, span := r.child(ctx, operation+" "+model, trace.SpanKindClient, attrs)
	return ctx, &ModelCall{handle: handle{span: span}}
}

// ToolCall opens a call to the tool named name under the span in ctx;
// Returned records what the tool returned.
func (r *Run) ToolCall(ctx context.Context, name string, opts ToolCallOptions) (context.Context, *ToolCall) {
	if r == nil || r.span == nil {
		return ctx, &ToolCall{}
	}
	attrs := []attribute.KeyValue{
		attribute.String(attrOperation, toolOperation),
		attribute.String(attrToolName, name),
	}
	if opts.Arguments != nil {
		attrs = append(attrs, attribute.String(attrToolCallArguments, asJSON(opts.Arguments)))
	}
	ctx, span := r.child(ctx, toolOperation+" "+name, trace.SpanKindInternal, attrs)
	return ctx, &ToolCall{handle: handle{span: span}}
}

func (r *Run) child(ctx context.Context, name string, kind trace.SpanKind, attrs []attribute.KeyValue) (context.Context, trace.Span) {
	if ctx == nil {
		ctx = context.Background()
	}
	// A context with no span in it, a fresh goroutine's say, puts the span
	// under the run itself rather than at the root of a trace of its own.
	if !trace.SpanContextFromContext(ctx).IsValid() {
		ctx = trace.ContextWithSpan(ctx, r.span)
	}
	all := make([]attribute.KeyValue, 0, len(r.inherited)+len(attrs))
	all = append(all, r.inherited...)
	all = append(all, attrs...)
	ctx, span := r.client.tracer.Start(ctx, name, trace.WithSpanKind(kind), trace.WithAttributes(all...))
	return contextWithRun(ctx, r), span
}

// Answered records what the model answered: its name, its tokens, and its
// messages.
func (m *ModelCall) Answered(a Answer) {
	if m == nil || m.span == nil {
		return
	}
	var attrs []attribute.KeyValue
	if a.ResponseModel != "" {
		attrs = append(attrs, attribute.String(attrResponseModel, a.ResponseModel))
	}
	attrs = append(attrs, usageAttributes(a.Usage)...)
	if a.OutputMessages != nil {
		attrs = append(attrs, attribute.String(attrOutputMessages, asJSON(a.OutputMessages)))
	}
	if len(attrs) > 0 {
		m.span.SetAttributes(attrs...)
	}
}

// Returned records what the tool returned: content, sent only when asked and
// allowed.
func (t *ToolCall) Returned(result any) {
	if t == nil || t.span == nil {
		return
	}
	t.span.SetAttributes(attribute.String(attrToolCallResult, asJSON(result)))
}

// modelOperation is a model call's operation, chat when blank: sent blank, the
// span would be named " <model>" and ingest would read it as a step.
func modelOperation(operation string) string {
	if strings.TrimSpace(operation) == "" {
		return defaultModelOperation
	}
	return operation
}

func usageAttributes(u *Usage) []attribute.KeyValue {
	if u == nil {
		return nil
	}
	return []attribute.KeyValue{
		attribute.Int(attrInputTokens, u.InputTokens),
		attribute.Int(attrOutputTokens, u.OutputTokens),
		attribute.Int(attrCacheReadInputTokens, u.CacheReadInputTokens),
		attribute.Int(attrCacheWriteInputTokens, u.CacheWriteInputTokens),
	}
}

func boundAttributes(b *Bounds) []attribute.KeyValue {
	if b == nil {
		return nil
	}
	var out []attribute.KeyValue
	if b.StallAfterS != 0 {
		out = append(out, attribute.Int(attrBoundStallAfter, b.StallAfterS))
	}
	if b.MaxSteps != 0 {
		out = append(out, attribute.Int(attrBoundMaxSteps, b.MaxSteps))
	}
	if b.MaxCostUSD != 0 {
		out = append(out, attribute.Float64(attrBoundMaxCostUSD, b.MaxCostUSD))
	}
	return out
}

// asJSON is a content value as the span carries it: a string as given,
// anything else as JSON, and what JSON cannot encode as fmt prints it.
func asJSON(value any) string {
	if s, ok := value.(string); ok {
		return s
	}
	var buf bytes.Buffer
	encoder := json.NewEncoder(&buf)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(value); err != nil {
		return fmt.Sprint(value)
	}
	return strings.TrimSuffix(buf.String(), "\n")
}

// newUUID is a version 4 UUID in its lowercase 8-4-4-4-12 form.
func newUUID() string {
	var b [16]byte
	// crypto/rand.Read never returns an error since Go 1.24; it crashes the
	// program instead.
	_, _ = rand.Read(b[:])
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return formatUUID(b)
}
