package telmoni

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"reflect"
	"slices"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
)

// TraceOptions is what a trace is opened with; each field is optional.
type TraceOptions struct {
	// UserID is the end user the trace is for, as the application's own id
	// for them, sent as given for ingest to hash.
	UserID string
	// SessionID is the session the trace belongs to, as the application's own
	// id for it.
	SessionID string
	// Tags travel as telmoni.tag.<key>.
	Tags map[string]string
	// MaxSteps and MaxSpend, in US dollars, are the guard's caps, 0 for none:
	// once the trace reaches one, what it opens next opens nothing, and the
	// context it returns is cancelled with a *CapExceededError as its cause.
	MaxSteps int
	MaxSpend float64
	// InputMessages are content: recorded as JSON, sent only when
	// CaptureContent is on and the project's mode allows.
	InputMessages any
}

// TraceReply is what a trace answered: its messages.
type TraceReply struct {
	// OutputMessages are content: recorded as JSON, sent only when
	// CaptureContent is on and the project's mode allows.
	OutputMessages any
}

// ModelCallOptions is what a model call is opened with.
type ModelCallOptions struct {
	// Provider is the model's provider, as the conventions name it: openai,
	// anthropic, ...
	Provider string
	// Operation is chat, generate_content, text_completion or embeddings;
	// blank means chat, and any other goes as chat with one warning.
	Operation string
	// Prompt and PromptVersion name the prompt it used, sent in every content
	// mode: a prompt, never a person, who goes in TraceOptions.UserID.
	Prompt        string
	PromptVersion string
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

// RetrievalOptions is what a retrieval is opened with.
type RetrievalOptions struct {
	// Query is what it searched for, content: recorded as JSON, a string as
	// given, sent only when CaptureContent is on and the project's mode allows.
	Query any
	// TopK is how many results it asked for, 0 for none; one below 0 is left
	// off with one warning.
	TopK int
	// Memory makes it a search of a memory store, search_memory, rather than
	// of a vector store or an index.
	Memory bool
}

// Answer is what a model answered: its name, its tokens and its messages.
type Answer struct {
	ResponseModel string
	Usage         *Usage
	// OutputMessages are content: recorded as JSON, sent only when
	// CaptureContent is on and the project's mode allows.
	OutputMessages any
	// TimeToFirstToken is from the call's start to its answer's first token,
	// 0 for none; one below 0 is left off with one warning.
	TimeToFirstToken time.Duration
}

// handle is a span the SDK opened, until its End. A handle opened outside a
// trace has no span and does nothing.
type handle struct {
	span   trace.Span
	client *Client
	once   sync.Once
	// mu holds ended and the scores the span holds, so none lands past its end.
	mu     sync.Mutex
	ended  bool
	scored int
}

// End ends the span; a second End does nothing.
func (h *handle) End() {
	if h.span == nil {
		return
	}
	h.once.Do(func() {
		h.mu.Lock()
		h.ended = true
		h.mu.Unlock()
		h.span.End()
	})
}

// Fail marks the span failed with err's Go type; its message is content, kept
// on the span as an exception event and the status's for the exporter's lock.
// An error that is or wraps context.Canceled marks it cancelled instead: the
// agent gave the call up. A deadline that ran out is a failure, as a timeout
// is in the other SDKs; it fails with its own type.
func (h *handle) Fail(err error) {
	if h.span == nil || err == nil {
		return
	}
	if errors.Is(err, context.Canceled) {
		h.Cancel()
		return
	}
	errorType := h.errorType(errorTypeName(err))
	// fmt recovers an Error method that panics, as a nil receiver's may.
	message := fmt.Sprint(err)
	h.span.AddEvent(eventException, trace.WithAttributes(
		attribute.String(attrExceptionType, errorType),
		attribute.String(attrExceptionMessage, message),
	))
	h.span.SetStatus(codes.Error, message)
}

// FailWith marks the span failed with errorType as its error.type, none if
// empty, and status ERROR with no message.
func (h *handle) FailWith(errorType string) {
	if h.span == nil {
		return
	}
	h.errorType(errorType)
	h.span.SetStatus(codes.Error, "")
}

// errorType sets error.type, cut to ingest's cap, and returns what it set.
func (h *handle) errorType(errorType string) string {
	// Ingest refuses the span over an empty one, and with the agent's, the trace.
	if errorType == "" {
		return ""
	}
	errorType = h.client.cut(errorType, errorTypeMax, "span's error type")
	h.span.SetAttributes(attribute.String(attrErrorType, errorType))
	return errorType
}

// Cancel marks the span cancelled by the agent, which is neither ok nor an
// error.
func (h *handle) Cancel() {
	if h.span == nil {
		return
	}
	h.span.SetAttributes(attribute.Bool(attrCancelled, true))
}

// errorTypeName is err's Go type without its pointer star: errors.errorString,
// net.OpError.
func errorTypeName(err error) string {
	return strings.TrimPrefix(fmt.Sprintf("%T", err), "*")
}

// Trace is one trace of an agent: a trace of its own, its agent's span at the
// root, linked to whatever span the application had current. Every span
// inside it carries the agent, user and session, so ingest knows each span's
// trace without waiting for the trace's tree.
type Trace struct {
	handle
	agent     string
	inherited []attribute.KeyValue
	// guard is held by every trace with a span, a continued trace's with no
	// caps.
	guard *guard
	// displaced is the provider instrumentation opened spans on where the
	// trace was opened, which its contexts hand out in place of the SDK's.
	displaced trace.TracerProvider
	// ended is whether End was called, under the guard's lock.
	ended bool
}

// inContext is a span of the SDK's as the contexts it returns carry it: its
// TracerProvider the one the trace displaced, so instrumentation that opens
// spans on the current span's provider, as otelhttp does unless given one,
// stays on the application's.
type inContext struct {
	trace.Span
	displaced trace.TracerProvider
}

func (s inContext) TracerProvider() trace.TracerProvider { return s.displaced }

// providerOf is the provider instrumentation opens spans on in ctx: the
// current span's, or the global one where ctx holds none.
func providerOf(ctx context.Context) trace.TracerProvider {
	if span := trace.SpanFromContext(ctx); span.SpanContext().IsValid() {
		return span.TracerProvider()
	}
	return otel.GetTracerProvider()
}

// Step is a step of a trace.
type Step struct {
	handle
	trace *Trace
}

// ModelCall is a call to a model inside a trace.
type ModelCall struct {
	handle
	trace *Trace
	// What it is priced by and what its span holds, as sent; what it last
	// counted to its trace's spend; and whether End was called.
	provider, model string
	held            tokens
	reported        *float64
	counted         float64
	ended           bool
}

// ToolCall is a call to a tool inside a trace.
type ToolCall struct {
	handle
	trace *Trace
}

// Retrieval is a search inside a trace, of a vector store or an index, or of
// a memory store.
type Retrieval struct {
	handle
	trace *Trace
	// memory is whether it searched a memory, whose names its finds go under.
	memory bool
}

type traceKey struct{}

func contextWithTrace(ctx context.Context, tr *Trace) context.Context {
	return context.WithValue(ctx, traceKey{}, tr)
}

// TraceFromContext is the trace open in ctx, or nil outside one. A nil Trace
// opens handles that do nothing, and its own End, Fail, FailWith and Cancel
// do nothing, so nothing is sent outside a trace and nothing panics.
func TraceFromContext(ctx context.Context) *Trace {
	if ctx == nil {
		return nil
	}
	tr, _ := ctx.Value(traceKey{}).(*Trace)
	return tr
}

// Trace opens a trace of agent: the span "invoke_agent <agent>" at the root
// of a trace of its own, linked to the span in ctx where there is one. The
// context returned carries the trace and its span, so spans the application
// opens with it nest under the agent's span. End it with trace.End, usually
// deferred.
//
// A blank agent name opens no trace: its handle does nothing, and the client
// warns once. An agent past 128 characters is cut to them, and a user or
// session id past its cap, or a tag ingest drops or past the 32nd by key, is
// left off, each with one warning. None panics: telemetry never takes the
// agent down, where Python and TypeScript raise for a blank agent.
func (c *Client) Trace(ctx context.Context, agent string, opts TraceOptions) (context.Context, *Trace) {
	if ctx == nil {
		ctx = context.Background()
	}
	if !c.built() {
		return ctx, &Trace{}
	}
	if strings.TrimSpace(agent) == "" {
		c.exporter.warnOnce("trace:agent", WarningInput, "telmoni: a trace needs its agent's name; nothing is sent for one without")
		return ctx, &Trace{handle: handle{client: c}}
	}
	agent = c.cut(agent, agentMax, "trace's agent")
	caps := &guard{}
	if opts.MaxSteps > 0 {
		caps.maxSteps = opts.MaxSteps
	} else if opts.MaxSteps < 0 {
		c.exporter.warnOnce("trace:steps", WarningInput, "telmoni: a trace's MaxSteps must be above 0; the trace has no step cap")
	}
	if opts.MaxSpend > 0 && dollars(opts.MaxSpend) {
		caps.maxSpend = opts.MaxSpend
	} else if opts.MaxSpend != 0 {
		c.exporter.warnOnce("trace:spend", WarningInput, "telmoni: a trace's MaxSpend must be a finite number of US dollars above 0; the trace has no spend cap")
	}
	userID, sessionID := opts.UserID, opts.SessionID
	if userID != "" && !c.fits(userID, userIDMax, "user id", "trace") {
		userID = ""
	}
	if sessionID != "" && !c.fits(sessionID, sessionIDMax, "session id", "trace") {
		sessionID = ""
	}
	inherited := inheritedAttributes(agent, userID, sessionID)
	name, attrs := c.agentOpening(agent, inherited, opts.Tags)
	if opts.InputMessages != nil {
		attrs = append(attrs, attribute.String(attrInputMessages, asJSON(opts.InputMessages)))
	}
	displaced := providerOf(ctx)
	starts := []trace.SpanStartOption{trace.WithSpanKind(trace.SpanKindInternal), trace.WithAttributes(attrs...)}
	// A root of its own trace, linked to the application's span rather than
	// under it: the trace is the unit, and the application's trace stays its own.
	if current := trace.SpanContextFromContext(ctx); current.IsValid() {
		starts = append(starts, trace.WithLinks(trace.Link{SpanContext: current}))
	}
	spanCtx, span := c.tracer.Start(trace.ContextWithSpanContext(ctx, trace.SpanContext{}), name, starts...)
	// The trace's opening, under its span and at its start: the processor
	// sends it at once, so the trace is seen open before it ends.
	if opened, ok := span.(sdktrace.ReadOnlySpan); ok {
		at := trace.WithTimestamp(opened.StartTime())
		_, start := c.tracer.Start(spanCtx, traceStart, trace.WithSpanKind(trace.SpanKindInternal), trace.WithAttributes(inherited...), at)
		start.End(at)
	}
	tr := &Trace{handle: handle{span: span, client: c}, agent: agent, inherited: inherited, guard: caps, displaced: displaced}
	return contextWithTrace(trace.ContextWithSpan(ctx, inContext{span, displaced}), tr), tr
}

// Spent is what the trace's model calls have cost so far in US dollars,
// counted as its spend cap counts them, in this process.
func (r *Trace) Spent() float64 {
	if r == nil || r.guard == nil {
		return 0
	}
	r.guard.mu.Lock()
	defer r.guard.mu.Unlock()
	return r.guard.spent
}

// Err is the *CapExceededError the trace's last refused opening was cancelled
// with, nil while nothing was refused.
func (r *Trace) Err() error {
	if r == nil || r.guard == nil {
		return nil
	}
	r.guard.mu.Lock()
	defer r.guard.mu.Unlock()
	return r.guard.refused
}

// opens is nil where the trace may open what it asks, else why not: a cap it
// reached, or the refusal ctx carries, a refused opening's context handed on.
func (r *Trace) opens(ctx context.Context, step bool) error {
	var refusal *CapExceededError
	if ctx != nil && errors.As(context.Cause(ctx), &refusal) {
		return refusal
	}
	return r.guard.opens(step)
}

// TraceID is the trace's id, 32 lowercase hex digits, which every span inside
// it is in; "" for a handle that does nothing.
func (r *Trace) TraceID() string {
	if r == nil || r.span == nil {
		return ""
	}
	return r.span.SpanContext().TraceID().String()
}

// Agent is the agent's name as the trace sends it, cut to 128 characters.
func (r *Trace) Agent() string {
	if r == nil {
		return ""
	}
	return r.agent
}

// End ends the agent's span; a second End does nothing. End, Fail, FailWith
// and Cancel are the handle's, declared again on each handle so a nil one does
// nothing: reached through the embedded handle, it would be dereferenced.
func (r *Trace) End() {
	if r == nil {
		return
	}
	if r.guard != nil {
		// Under the lock its answers are held under, so none lands past the end.
		r.guard.mu.Lock()
		r.ended = true
		r.guard.mu.Unlock()
	}
	r.handle.End()
}

// Answered records what the trace answered: its messages; one given after
// End is not sent.
func (r *Trace) Answered(a TraceReply) {
	if r == nil || r.span == nil {
		return
	}
	var attrs []attribute.KeyValue
	if a.OutputMessages != nil {
		attrs = append(attrs, attribute.String(attrOutputMessages, asJSON(a.OutputMessages)))
	}
	r.guard.mu.Lock()
	ended := r.ended
	if !ended && len(attrs) > 0 {
		r.span.SetAttributes(attrs...)
	}
	r.guard.mu.Unlock()
	// Given once the lock is released: the logger is the application's.
	if ended {
		r.client.exporter.warnOnce("trace:late", WarningInput, "telmoni: a trace must be answered before it ends; an answer given after is not sent")
	}
}

// Fail marks the trace failed with err's Go type and its message, or
// cancelled for an error that is or wraps context.Canceled, as a step's Fail
// does; once the trace was refused, such an error fails it as the refusal.
func (r *Trace) Fail(err error) {
	if r == nil {
		return
	}
	r.handle.Fail(r.failure(err))
}

// failure is err, but the trace's refusal for a cancellation once it was
// refused: the cancellation a refused opening's context ended a request with.
func (r *Trace) failure(err error) error {
	if refusal := r.Err(); refusal != nil && errors.Is(err, context.Canceled) {
		return refusal
	}
	return err
}

// Fail marks the step failed as its trace's Fail would.
func (s *Step) Fail(err error) {
	if s == nil {
		return
	}
	s.handle.Fail(s.trace.failure(err))
}

// Fail marks the call failed as its trace's Fail would.
func (m *ModelCall) Fail(err error) {
	if m == nil {
		return
	}
	m.handle.Fail(m.trace.failure(err))
}

// Fail marks the call failed as its trace's Fail would.
func (t *ToolCall) Fail(err error) {
	if t == nil {
		return
	}
	t.handle.Fail(t.trace.failure(err))
}

// Fail marks the retrieval failed as its trace's Fail would.
func (r *Retrieval) Fail(err error) {
	if r == nil {
		return
	}
	r.handle.Fail(r.trace.failure(err))
}

// End ends the step's span; a second End does nothing.
func (s *Step) End() {
	if s == nil {
		return
	}
	s.handle.End()
}

// End ends the call's span; a second End does nothing.
func (t *ToolCall) End() {
	if t == nil {
		return
	}
	t.handle.End()
}

// End ends the retrieval's span; a second End does nothing.
func (r *Retrieval) End() {
	if r == nil {
		return
	}
	r.handle.End()
}

// FailWith marks the step failed as its trace's FailWith would.
func (s *Step) FailWith(errorType string) {
	if s == nil {
		return
	}
	s.handle.FailWith(errorType)
}

// FailWith marks the call failed as its trace's FailWith would.
func (m *ModelCall) FailWith(errorType string) {
	if m == nil {
		return
	}
	m.handle.FailWith(errorType)
}

// FailWith marks the call failed as its trace's FailWith would.
func (t *ToolCall) FailWith(errorType string) {
	if t == nil {
		return
	}
	t.handle.FailWith(errorType)
}

// FailWith marks the retrieval failed as its trace's FailWith would.
func (r *Retrieval) FailWith(errorType string) {
	if r == nil {
		return
	}
	r.handle.FailWith(errorType)
}

// Cancel marks the step cancelled as its trace's Cancel would.
func (s *Step) Cancel() {
	if s == nil {
		return
	}
	s.handle.Cancel()
}

// Cancel marks the call cancelled as its trace's Cancel would.
func (m *ModelCall) Cancel() {
	if m == nil {
		return
	}
	m.handle.Cancel()
}

// Cancel marks the call cancelled as its trace's Cancel would.
func (t *ToolCall) Cancel() {
	if t == nil {
		return
	}
	t.handle.Cancel()
}

// Cancel marks the retrieval cancelled as its trace's Cancel would.
func (r *Retrieval) Cancel() {
	if r == nil {
		return
	}
	r.handle.Cancel()
}

// FailWith marks the trace failed with errorType as its error.type, none if
// empty, and status ERROR with no message.
func (r *Trace) FailWith(errorType string) {
	if r == nil {
		return
	}
	r.handle.FailWith(errorType)
}

// Cancel marks the trace cancelled by the agent, which is neither ok nor an
// error.
func (r *Trace) Cancel() {
	if r == nil {
		return
	}
	r.handle.Cancel()
}

// Step opens a step named name under the span in ctx when ctx carries the
// trace, a step's to nest under it, else under the agent's span. The context
// returned carries the step's span. Past a cap, or under a refused opening's
// context, it opens nothing, and the context returned is cancelled, its cause
// the *CapExceededError.
func (r *Trace) Step(ctx context.Context, name string) (context.Context, *Step) {
	if r == nil || r.span == nil {
		return ctx, &Step{}
	}
	if err := r.opens(ctx, true); err != nil {
		return contextWithTrace(refused(ctx, err), r), &Step{}
	}
	// Ingest refuses a span named nothing.
	if name == "" {
		r.client.exporter.warnOnce("step:name", WarningInput, "telmoni: a step needs its name; one without is sent named step")
		name = "step"
	}
	ctx, span := r.child(ctx, r.client.cut(name, nameMax, "step's name"), trace.SpanKindInternal, nil)
	return ctx, &Step{handle: handle{span: span, client: r.client}, trace: r}
}

// ModelCall opens a call to model where Step opens a step; Answered records
// what the model answered. Past a cap it opens nothing, as Step does.
func (r *Trace) ModelCall(ctx context.Context, model string, opts ModelCallOptions) (context.Context, *ModelCall) {
	if r == nil || r.span == nil {
		return ctx, &ModelCall{}
	}
	if err := r.opens(ctx, false); err != nil {
		return contextWithTrace(refused(ctx, err), r), &ModelCall{}
	}
	c := r.client
	// Ingest refuses a call over an empty model, but keeps one with none.
	if model == "" {
		c.exporter.warnOnce("call:model", WarningInput, "telmoni: a model call needs its model; one without is sent named by its operation alone")
	} else {
		model = c.cut(model, modelMax, "model")
	}
	provider := ""
	if opts.Provider != "" {
		provider = c.cut(opts.Provider, providerMax, "provider")
	}
	operation, setAside := modelOperation(opts.Operation)
	if setAside {
		c.warnOperation()
	}
	prompt, promptVersion := c.prompt(opts.Prompt, opts.PromptVersion)
	name, attrs := modelCallOpening(model, operation, provider, prompt, promptVersion)
	if opts.InputMessages != nil {
		attrs = append(attrs, attribute.String(attrInputMessages, asJSON(opts.InputMessages)))
	}
	if opts.SystemInstructions != nil {
		attrs = append(attrs, attribute.String(attrSystemInstructions, asJSON(opts.SystemInstructions)))
	}
	ctx, span := r.child(ctx, name, trace.SpanKindClient, attrs)
	return ctx, &ModelCall{handle: handle{span: span, client: c}, trace: r, provider: provider, model: model}
}

// ToolCall opens a call to the tool named name where Step opens a step;
// Returned records what the tool returned.
func (r *Trace) ToolCall(ctx context.Context, name string, opts ToolCallOptions) (context.Context, *ToolCall) {
	if r == nil || r.span == nil {
		return ctx, &ToolCall{}
	}
	if err := r.opens(ctx, false); err != nil {
		return contextWithTrace(refused(ctx, err), r), &ToolCall{}
	}
	spanName, attrs := toolCallOpening(r.client.cut(name, nameMax, "tool's name"))
	if opts.Arguments != nil {
		attrs = append(attrs, attribute.String(attrToolCallArguments, asJSON(opts.Arguments)))
	}
	ctx, span := r.child(ctx, spanName, trace.SpanKindInternal, attrs)
	return ctx, &ToolCall{handle: handle{span: span, client: r.client}, trace: r}
}

// Retrieval opens a search of source, or with opts.Memory of a memory store,
// where Step opens a step; Returned records what it found.
func (r *Trace) Retrieval(ctx context.Context, source string, opts RetrievalOptions) (context.Context, *Retrieval) {
	if r == nil || r.span == nil {
		return ctx, &Retrieval{}
	}
	if err := r.opens(ctx, false); err != nil {
		return contextWithTrace(refused(ctx, err), r), &Retrieval{}
	}
	c := r.client
	operation, sourceKey, queryKey := retrievalOperation, attrDataSourceID, attrRetrievalQueryText
	if opts.Memory {
		operation, sourceKey, queryKey = searchMemory, attrMemoryStoreID, attrMemoryQueryText
	}
	name := operation
	attrs := []attribute.KeyValue{attribute.String(attrOperation, operation)}
	// Ingest names a retrieval by its source, and one with none by its span.
	if strings.TrimSpace(source) == "" {
		c.exporter.warnOnce("retrieval:source", WarningInput, "telmoni: a retrieval needs its source; one without is sent named by its operation alone")
	} else {
		source = c.cut(source, nameMax, "retrieval's source")
		attrs = append(attrs, attribute.String(sourceKey, source))
		if !opts.Memory {
			name = operation + " " + source
		}
	}
	if opts.TopK > 0 {
		attrs = append(attrs, attribute.Int(attrRetrievalTopK, opts.TopK))
	} else if opts.TopK < 0 {
		c.exporter.warnOnce("retrieval:"+attrRetrievalTopK, WarningInput, "telmoni: "+attrRetrievalTopK+" must be a whole number from 0 within 64 bits; the retrieval is sent without it")
	}
	if opts.Query != nil {
		attrs = append(attrs, attribute.String(queryKey, asJSON(opts.Query)))
	}
	ctx, span := r.child(ctx, name, trace.SpanKindClient, attrs)
	return ctx, &Retrieval{handle: handle{span: span, client: c}, trace: r, memory: opts.Memory}
}

// inheritedAttributes is what every span of a trace carries, live or
// recorded: its agent, and its user and session where it has them.
func inheritedAttributes(agent, userID, sessionID string) []attribute.KeyValue {
	inherited := []attribute.KeyValue{attribute.String(attrAgent, agent)}
	if userID != "" {
		inherited = append(inherited, attribute.String(attrUserID, userID))
	}
	if sessionID != "" {
		inherited = append(inherited, attribute.String(attrSessionID, sessionID))
	}
	return inherited
}

// agentOpening is the agent's span name and attributes, live or recorded,
// its tags by key: a map ranges in a new order each time, and the same record
// must be the same bytes.
func (c *Client) agentOpening(agent string, inherited []attribute.KeyValue, tags map[string]string) (string, []attribute.KeyValue) {
	kept := c.tagAttributes(tags)
	attrs := make([]attribute.KeyValue, 0, 2+len(inherited)+len(kept))
	attrs = append(attrs, attribute.String(attrOperation, agentOperation), attribute.String(attrAgentName, agent))
	attrs = append(attrs, inherited...)
	return agentOperation + " " + agent, append(attrs, kept...)
}

// modelCallOpening is a model call's span name and own attributes, live or
// recorded, its operation one ingest reads and an empty model, provider or
// prompt left off.
func modelCallOpening(model, operation, provider, prompt, promptVersion string) (string, []attribute.KeyValue) {
	attrs := []attribute.KeyValue{attribute.String(attrOperation, operation)}
	name := operation
	if model != "" {
		attrs = append(attrs, attribute.String(attrRequestModel, model))
		name = operation + " " + model
	}
	if provider != "" {
		attrs = append(attrs, attribute.String(attrProvider, provider))
	}
	if prompt != "" {
		attrs = append(attrs, attribute.String(attrPromptName, prompt))
		if promptVersion != "" {
			attrs = append(attrs, attribute.String(attrPromptVersion, promptVersion))
		}
	}
	return name, attrs
}

// prompt is a live call's prompt and its version: a blank one is none, and one
// past its cap or a version without its name is left off with one warning.
func (c *Client) prompt(name, version string) (string, string) {
	kept := func(text, what string) string {
		if strings.TrimSpace(text) == "" || !c.fits(text, attributeValueMax, what, "call") {
			return ""
		}
		return text
	}
	name, version = kept(name, "prompt's name"), kept(version, "prompt's version")
	if version != "" && name == "" {
		c.exporter.warnOnce("call:prompt_version", WarningInput, "telmoni: a prompt's version needs its prompt's name; the call is sent without it")
		return "", ""
	}
	return name, version
}

// toolCallOpening is a tool call's span name and own attributes, live or
// recorded.
func toolCallOpening(name string) (string, []attribute.KeyValue) {
	return toolOperation + " " + name, []attribute.KeyValue{
		attribute.String(attrOperation, toolOperation),
		attribute.String(attrToolName, name),
	}
}

func (r *Trace) child(ctx context.Context, name string, kind trace.SpanKind, attrs []attribute.KeyValue) (context.Context, trace.Span) {
	if ctx == nil {
		ctx = context.Background()
	}
	// A context the trace is not in, a fresh goroutine's or the one the trace
	// was opened from, puts the span under the agent's, in its trace.
	if TraceFromContext(ctx) != r || !trace.SpanContextFromContext(ctx).IsValid() {
		ctx = trace.ContextWithSpan(ctx, r.span)
	}
	all := make([]attribute.KeyValue, 0, len(r.inherited)+len(attrs))
	all = append(all, r.inherited...)
	all = append(all, attrs...)
	ctx, span := r.client.tracer.Start(ctx, name, trace.WithSpanKind(kind), trace.WithAttributes(all...))
	return contextWithTrace(trace.ContextWithSpan(ctx, inContext{span, r.displaced}), r), span
}

// Answered records what the model answered: its name, its tokens, and its
// messages; one given after End is not sent, and counts nothing.
func (m *ModelCall) Answered(a Answer) {
	if m == nil || m.span == nil {
		return
	}
	var attrs []attribute.KeyValue
	answeredAs := ""
	if a.ResponseModel != "" {
		answeredAs = m.client.cut(a.ResponseModel, modelMax, "response model")
		attrs = append(attrs, attribute.String(attrResponseModel, answeredAs))
	}
	usage := usageAttributes(a.Usage, m.client.warnCount)
	attrs = append(attrs, usage...)
	if a.OutputMessages != nil {
		attrs = append(attrs, attribute.String(attrOutputMessages, asJSON(a.OutputMessages)))
	}
	// Ingest refuses the call over one below 0, as over a negative count.
	if a.TimeToFirstToken > 0 {
		attrs = append(attrs, attribute.Float64(attrTimeToFirstChunk, a.TimeToFirstToken.Seconds()))
	} else if a.TimeToFirstToken < 0 {
		m.client.exporter.warnOnce("answer:"+attrTimeToFirstChunk, WarningInput, "telmoni: "+attrTimeToFirstChunk+" must be a duration from 0; the call is sent without it")
	}
	// Given once the trace's lock is released: the logger is the application's.
	if w := m.hold(attrs, answeredAs, usage); w.key != "" {
		m.client.exporter.warnOnce(w.key, WarningInput, w.text)
	}
}

// warning is one the client gives once, by its key.
type warning struct{ key, text string }

// End ends the call's span; a second End does nothing.
func (m *ModelCall) End() {
	if m == nil || m.span == nil {
		return
	}
	// Under the lock its answers are held under, so none lands past the end.
	g := m.trace.guard
	g.mu.Lock()
	m.ended = true
	g.mu.Unlock()
	m.handle.End()
}

// hold sets an answer on the span and counts its cost under the trace's lock,
// so answers count one at a time and none after End; it is the warning it
// calls for.
func (m *ModelCall) hold(attrs []attribute.KeyValue, answeredAs string, usage []attribute.KeyValue) warning {
	g := m.trace.guard
	g.mu.Lock()
	defer g.mu.Unlock()
	if m.ended {
		return warning{"call:late", "telmoni: a model call must be answered before it ends; an answer given after is not sent and counts nothing toward the trace's spend"}
	}
	if len(attrs) > 0 {
		m.span.SetAttributes(attrs...)
	}
	if answeredAs != "" {
		// The name the call is priced by, as ingest reads it.
		m.model = answeredAs
	}
	for _, kv := range usage {
		switch string(kv.Key) {
		case attrInputTokens:
			m.held.input = int(kv.Value.AsInt64())
		case attrOutputTokens:
			m.held.output = int(kv.Value.AsInt64())
		case attrCacheReadInputTokens:
			m.held.cacheRead = int(kv.Value.AsInt64())
		case attrCacheCreationInputTokens:
			m.held.cacheWrite = int(kv.Value.AsInt64())
		case attrCost:
			cost := kv.Value.AsFloat64()
			m.reported = &cost
		}
	}
	t := m.held
	var cost float64
	var priced bool
	if m.reported != nil {
		cost, priced = *m.reported, true
	} else {
		cost, priced = m.client.priced(m.provider, m.model, t)
	}
	var w warning
	if !priced && g.maxSpend > 0 && t != (tokens{}) {
		model, provider := m.model, m.provider
		if model == "" {
			model = "a call with no model"
		}
		if provider == "" {
			provider = "no provider"
		}
		w = warning{"unpriced:" + m.provider + ":" + m.model,
			"telmoni: no price for " + model + " (" + provider + "), which reported no cost; it counts nothing toward the trace's spend cap"}
	}
	g.spent += cost - m.counted
	m.counted = cost
	return w
}

// Returned records what the tool returned: content, sent only when asked and
// allowed.
func (t *ToolCall) Returned(result any) {
	if t == nil || t.span == nil {
		return
	}
	t.span.SetAttributes(attribute.String(attrToolCallResult, asJSON(result)))
}

// Returned records what the search found, a slice or an array: the documents
// as content and how many in every mode; anything else, bytes too, is refused.
func (r *Retrieval) Returned(documents any) {
	if r == nil || r.span == nil {
		return
	}
	list := reflect.ValueOf(documents)
	kind := list.Kind()
	// Bytes are no documents, and JSON would send them as base64 text.
	if (kind != reflect.Slice && kind != reflect.Array) || list.Type().Elem().Kind() == reflect.Uint8 {
		// Whole: a count once set stays, beside any documents set after.
		r.client.exporter.warnOnce("retrieval:documents", WarningInput, "telmoni: a retrieval's documents must be a list; they are neither sent nor counted")
		return
	}
	found, count := attrRetrievalDocuments, attrRetrievalReturned
	if r.memory {
		found, count = attrMemoryRecords, attrMemoryRecordCount
	}
	// A nil slice is an empty list, which JSON would write as null.
	sent := "[]"
	if kind == reflect.Array || !list.IsNil() {
		sent = asJSON(documents)
	}
	// One call, under the span's one lock, so the last list sets both.
	r.span.SetAttributes(attribute.String(found, sent), attribute.Int(count, list.Len()))
}

// modelOperation is a model call's operation as ingest reads one: chat for a
// blank one, and for one ingest would file as a step, an agent or a tool call,
// which setAside reports for the caller to warn of (warnOperation).
func modelOperation(operation string) (read string, setAside bool) {
	if strings.TrimSpace(operation) == "" {
		return defaultModelOperation, false
	}
	if slices.Contains(modelOperations, operation) {
		return operation, false
	}
	return defaultModelOperation, true
}

// warnOperation warns once of an operation sent as chat.
func (c *Client) warnOperation() {
	c.exporter.warnOnce("call:operation", WarningInput, "telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat")
}

// usageAttributes is a call's token counts but any below zero, and its cost
// but one below zero or not finite, for which ingest would refuse the call;
// warn, when given, is told each one's name.
func usageAttributes(u *Usage, warn func(name string)) []attribute.KeyValue {
	if u == nil {
		return nil
	}
	var out []attribute.KeyValue
	for _, count := range []struct {
		name  string
		value int
	}{
		{attrInputTokens, u.InputTokens},
		{attrOutputTokens, u.OutputTokens},
		{attrCacheReadInputTokens, u.CacheReadInputTokens},
		{attrCacheCreationInputTokens, u.CacheWriteInputTokens},
		{attrReasoningOutputTokens, u.ReasoningOutputTokens},
	} {
		if count.value >= 0 {
			out = append(out, attribute.Int(count.name, count.value))
		} else if warn != nil {
			warn(count.name)
		}
	}
	if u.Cost != nil {
		if dollars(*u.Cost) {
			out = append(out, attribute.Float64(attrCost, *u.Cost))
		} else if warn != nil {
			warn(attrCost)
		}
	}
	return out
}

func (c *Client) warnCount(name string) {
	if name == attrCost {
		c.exporter.warnOnce("usage:"+name, WarningInput, "telmoni: "+name+" must be a finite number of US dollars from 0; the call is sent without it")
		return
	}
	c.exporter.warnOnce("usage:"+name, WarningInput, "telmoni: "+name+" must be a whole number from 0 within 64 bits; the call is sent without it")
}

// cut is text up to ingest's cap, in characters, cut there with one warning,
// since ingest refuses a span past it. JSON sends each invalid byte as one
// U+FFFD, which range counts as one rune too.
func (c *Client) cut(text string, most int, what string) string {
	// A string's bytes are never fewer than its runes.
	if len(text) <= most || utf8.RuneCountInString(text) <= most {
		return text
	}
	if c.built() {
		c.exporter.warnOnce("cut:"+what, WarningInput, fmt.Sprintf("telmoni: a %s must be up to %d characters; it is sent cut to its first %d", what, most, most))
	}
	runes := 0
	for at := range text {
		if runes == most {
			return text[:at]
		}
		runes++
	}
	return text
}

// fits reports whether an id is within ingest's cap; one past it is left off
// its trace or call with one warning, since cut it would name another.
func (c *Client) fits(id string, most int, what, span string) bool {
	if len(id) <= most || utf8.RuneCountInString(id) <= most {
		return true
	}
	if c.built() {
		c.exporter.warnOnce("long:"+what, WarningInput, fmt.Sprintf("telmoni: a %s must be up to %d characters; the %s is sent without it", what, most, span))
	}
	return false
}

// tagAttributes is a trace's first 32 tags by key within ingest's caps; more
// would crowd the marks a trace ends with out of the span's 128 attributes.
func (c *Client) tagAttributes(tags map[string]string) []attribute.KeyValue {
	var keys []string
	for _, key := range slices.Sorted(maps.Keys(tags)) {
		// Ingest drops it before counting, so it takes no place among the 32.
		if key == "" || utf8.RuneCountInString(key) > tagKeyMax || utf8.RuneCountInString(tags[key]) > tagValueMax {
			c.exporter.warnOnce("trace:tag", WarningInput, "telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent")
			continue
		}
		keys = append(keys, key)
	}
	if len(keys) > tagsMax {
		c.exporter.warnOnce("trace:tags", WarningInput, "telmoni: a trace carries up to 32 tags, the most ingest keeps; the rest are not sent")
		keys = keys[:tagsMax]
	}
	out := make([]attribute.KeyValue, 0, len(keys))
	for _, key := range keys {
		out = append(out, attribute.String(tagPrefix+key, tags[key]))
	}
	return out
}

// asJSON is a content value as the span carries it: a string as given,
// anything else as JSON, and what JSON cannot encode as its type.
func asJSON(value any) string {
	if s, ok := value.(string); ok {
		return s
	}
	var buf bytes.Buffer
	encoder := json.NewEncoder(&buf)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(value); err != nil {
		// Its type alone: fmt's %v walks a map or slice that holds itself
		// until the stack overflows.
		return fmt.Sprintf("%T", value)
	}
	return strings.TrimSuffix(buf.String(), "\n")
}

// newUUID is a version 4 UUID in its lowercase 8-4-4-4-12 form: a late
// score's id.
func newUUID() string {
	var b [16]byte
	// crypto/rand.Read never returns an error since Go 1.24; it crashes the
	// program instead.
	_, _ = rand.Read(b[:])
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return formatUUID(b)
}
