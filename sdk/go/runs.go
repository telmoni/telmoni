package telmoni

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"slices"
	"strings"
	"sync"
	"unicode/utf8"

	"go.opentelemetry.io/otel"
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
	// SampleRate is this run's share of being sent, above 0 and up to 1; 0
	// means the client's.
	SampleRate float64
	// MaxSteps and MaxSpend, in US dollars, are the guard's caps, 0 for none:
	// once the run reaches one, what it opens next opens nothing, and the
	// context it returns is cancelled with a *CapExceededError as its cause.
	MaxSteps int
	MaxSpend float64
}

// ModelCallOptions is what a model call is opened with.
type ModelCallOptions struct {
	// Provider is the model's provider, as the conventions name it: openai,
	// anthropic, ...
	Provider string
	// Operation is chat, generate_content, text_completion or embeddings;
	// blank means chat, and any other goes as chat with one warning.
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
	span   trace.Span
	client *Client
	once   sync.Once
}

// End ends the span; a second End does nothing.
func (h *handle) End() {
	if h.span == nil {
		return
	}
	h.once.Do(func() { h.span.End() })
}

// Fail marks the span failed with err's Go type and never its message: the
// message is content. An error that is or wraps context.Canceled marks it
// cancelled instead: the agent gave the call up. A deadline that ran out is
// a failure, as a timeout is in the other SDKs; it fails with its own type.
func (h *handle) Fail(err error) {
	if h.span == nil || err == nil {
		return
	}
	if errors.Is(err, context.Canceled) {
		h.Cancel()
		return
	}
	h.FailWith(errorTypeName(err))
}

// FailWith marks the span failed with errorType as its error.type, none if
// empty, and status ERROR with no message.
func (h *handle) FailWith(errorType string) {
	if h.span == nil {
		return
	}
	// Ingest refuses the span over an empty one, and with a run's, the run.
	if errorType != "" {
		h.span.SetAttributes(attribute.String(attrErrorType, h.client.cut(errorType, errorTypeMax, "span's error type")))
	}
	h.span.SetStatus(codes.Error, "")
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

// Run is a run of an agent: a span of its own in whatever trace was current.
// Every span inside it carries the run's id, agent, user and session, so
// ingest knows each span's run without waiting for the trace's tree.
type Run struct {
	handle
	id        string
	agent     string
	inherited []attribute.KeyValue
	// sampledOut is whether the sample rate kept this run out: nothing of it
	// is sent, nor handed to another service.
	sampledOut bool
	// guard is held by every run with a span, a continued run's with no caps.
	guard *guard
	// displaced is the provider instrumentation opened spans on where the run
	// was opened, which its contexts hand out in place of the SDK's.
	displaced trace.TracerProvider
}

// inContext is a span of the SDK's as the contexts it returns carry it: its
// TracerProvider the one the run displaced, so instrumentation that opens
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

// Step is a step of a run.
type Step struct {
	handle
	run *Run
}

// ModelCall is a call to a model inside a run.
type ModelCall struct {
	handle
	run *Run
	// What it is priced by and what its span holds, as sent; what it last
	// counted to its run's spend; and whether End was called.
	provider, model string
	held            tokens
	reported        *float64
	counted         float64
	ended           bool
}

// ToolCall is a call to a tool inside a run.
type ToolCall struct {
	handle
	run *Run
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
// ctx or at the root of a trace of its own. The context returned carries the
// run and its span, so spans the application opens with it nest under the
// run. End it with run.End, usually deferred.
//
// A blank agent name opens no run: its handle does nothing, and the client
// warns once. A SampleRate outside (0, 1] falls back to the client's; an
// agent past 128 characters is cut to them; and a user or session id past its
// cap, or a tag ingest drops or past the 32nd by key, is left off, each with
// one warning. None panics: telemetry never takes the agent down, where
// Python and TypeScript raise for a blank agent or a bad rate.
func (c *Client) Run(ctx context.Context, agent string, opts RunOptions) (context.Context, *Run) {
	if ctx == nil {
		ctx = context.Background()
	}
	if !c.built() {
		return ctx, &Run{}
	}
	if strings.TrimSpace(agent) == "" {
		c.exporter.warnOnce("run:agent", "telmoni: a run needs its agent's name; nothing is sent for one without")
		return ctx, &Run{handle: handle{client: c}}
	}
	agent = c.cut(agent, agentMax, "run's agent")
	rate := c.config.SampleRate
	if opts.SampleRate != 0 {
		if opts.SampleRate > 0 && opts.SampleRate <= 1 {
			rate = opts.SampleRate
		} else {
			c.exporter.warnOnce("run:rate", "telmoni: a run's SampleRate must be above 0 and up to 1; the client's is used")
		}
	}
	sampledOut := rate < 1 && c.random() >= rate
	caps := &guard{}
	if opts.MaxSteps > 0 {
		caps.maxSteps = opts.MaxSteps
	} else if opts.MaxSteps < 0 {
		c.exporter.warnOnce("run:steps", "telmoni: a run's MaxSteps must be above 0; the run has no step cap")
	}
	if opts.MaxSpend > 0 && dollars(opts.MaxSpend) {
		caps.maxSpend = opts.MaxSpend
	} else if opts.MaxSpend != 0 {
		c.exporter.warnOnce("run:spend", "telmoni: a run's MaxSpend must be a finite number of US dollars above 0; the run has no spend cap")
	}
	runID := newUUID()
	userID, sessionID := opts.UserID, opts.SessionID
	if userID != "" && !c.fits(userID, userIDMax, "user id") {
		userID = ""
	}
	if sessionID != "" && !c.fits(sessionID, sessionIDMax, "session id") {
		sessionID = ""
	}
	inherited := inheritedAttributes(runID, agent, userID, sessionID)
	// On every span of the run from its start, for the processor to drop: a
	// sampler's drop would clear the flag the application's spans inherit.
	if sampledOut {
		inherited = append(inherited, attribute.Bool(attrSampledOut, true))
	}
	name, attrs := c.runOpening(agent, inherited, rate, opts.Tags)
	displaced := providerOf(ctx)
	ctx, span := c.tracer.Start(ctx, name,
		trace.WithSpanKind(trace.SpanKindInternal),
		trace.WithAttributes(attrs...),
	)
	run := &Run{handle: handle{span: span, client: c}, id: runID, agent: agent, inherited: inherited, sampledOut: sampledOut, guard: caps, displaced: displaced}
	return contextWithRun(trace.ContextWithSpan(ctx, inContext{span, displaced}), run), run
}

// Spent is what the run's model calls have cost so far in US dollars,
// counted as its spend cap counts them, in this process.
func (r *Run) Spent() float64 {
	if r == nil || r.guard == nil {
		return 0
	}
	r.guard.mu.Lock()
	defer r.guard.mu.Unlock()
	return r.guard.spent
}

// Err is the *CapExceededError the run's last refused opening was cancelled
// with, nil while nothing was refused.
func (r *Run) Err() error {
	if r == nil || r.guard == nil {
		return nil
	}
	r.guard.mu.Lock()
	defer r.guard.mu.Unlock()
	return r.guard.refused
}

// opens is nil where the run may open what it asks, else why not: a cap it
// reached, or the refusal ctx carries, a refused opening's context handed on.
func (r *Run) opens(ctx context.Context, step bool) error {
	var refusal *CapExceededError
	if ctx != nil && errors.As(context.Cause(ctx), &refusal) {
		return refusal
	}
	return r.guard.opens(step)
}

// ID is the run's id, a UUID, carried by every span inside it.
func (r *Run) ID() string {
	if r == nil {
		return ""
	}
	return r.id
}

// Agent is the agent's name as the run sends it, cut to 128 characters.
func (r *Run) Agent() string {
	if r == nil {
		return ""
	}
	return r.agent
}

// End ends the run's span; a second End does nothing. End, Fail, FailWith and
// Cancel are the handle's, declared again on each handle so a nil one does
// nothing: reached through the embedded handle, it would be dereferenced.
func (r *Run) End() {
	if r == nil {
		return
	}
	r.handle.End()
}

// Fail marks the run failed with err's Go type, never its message, or
// cancelled for an error that is or wraps context.Canceled, as a step's Fail
// does; once the run was refused, such an error fails it as the refusal.
func (r *Run) Fail(err error) {
	if r == nil {
		return
	}
	r.handle.Fail(r.failure(err))
}

// failure is err, but the run's refusal for a cancellation once it was
// refused: the cancellation a refused opening's context ended a request with.
func (r *Run) failure(err error) error {
	if refusal := r.Err(); refusal != nil && errors.Is(err, context.Canceled) {
		return refusal
	}
	return err
}

// Fail marks the step failed as its run's Fail would.
func (s *Step) Fail(err error) {
	if s == nil {
		return
	}
	s.handle.Fail(s.run.failure(err))
}

// Fail marks the call failed as its run's Fail would.
func (m *ModelCall) Fail(err error) {
	if m == nil {
		return
	}
	m.handle.Fail(m.run.failure(err))
}

// Fail marks the call failed as its run's Fail would.
func (t *ToolCall) Fail(err error) {
	if t == nil {
		return
	}
	t.handle.Fail(t.run.failure(err))
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

// FailWith marks the step failed as its run's FailWith would.
func (s *Step) FailWith(errorType string) {
	if s == nil {
		return
	}
	s.handle.FailWith(errorType)
}

// FailWith marks the call failed as its run's FailWith would.
func (m *ModelCall) FailWith(errorType string) {
	if m == nil {
		return
	}
	m.handle.FailWith(errorType)
}

// FailWith marks the call failed as its run's FailWith would.
func (t *ToolCall) FailWith(errorType string) {
	if t == nil {
		return
	}
	t.handle.FailWith(errorType)
}

// Cancel marks the step cancelled as its run's Cancel would.
func (s *Step) Cancel() {
	if s == nil {
		return
	}
	s.handle.Cancel()
}

// Cancel marks the call cancelled as its run's Cancel would.
func (m *ModelCall) Cancel() {
	if m == nil {
		return
	}
	m.handle.Cancel()
}

// Cancel marks the call cancelled as its run's Cancel would.
func (t *ToolCall) Cancel() {
	if t == nil {
		return
	}
	t.handle.Cancel()
}

// FailWith marks the run failed with errorType as its error.type, none if
// empty, and status ERROR with no message.
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

// Step opens a step named name under the span in ctx when ctx carries the
// run, a step's to nest under it, else under the run. The context returned
// carries the step's span. Past a cap, or under a refused opening's context,
// it opens nothing, and the context returned is cancelled, its cause the
// *CapExceededError.
func (r *Run) Step(ctx context.Context, name string) (context.Context, *Step) {
	if r == nil || r.span == nil {
		return ctx, &Step{}
	}
	if err := r.opens(ctx, true); err != nil {
		return contextWithRun(refused(ctx, err), r), &Step{}
	}
	// Ingest refuses a span named nothing.
	if name == "" {
		r.client.exporter.warnOnce("step:name", "telmoni: a step needs its name; one without is sent named step")
		name = "step"
	}
	ctx, span := r.child(ctx, r.client.cut(name, nameMax, "step's name"), trace.SpanKindInternal, nil)
	return ctx, &Step{handle: handle{span: span, client: r.client}, run: r}
}

// ModelCall opens a call to model where Step opens a step; Answered records
// what the model answered. Past a cap it opens nothing, as Step does.
func (r *Run) ModelCall(ctx context.Context, model string, opts ModelCallOptions) (context.Context, *ModelCall) {
	if r == nil || r.span == nil {
		return ctx, &ModelCall{}
	}
	if err := r.opens(ctx, false); err != nil {
		return contextWithRun(refused(ctx, err), r), &ModelCall{}
	}
	c := r.client
	// Ingest refuses a call over an empty model, but keeps one with none.
	if model == "" {
		c.exporter.warnOnce("call:model", "telmoni: a model call needs its model; one without is sent named by its operation alone")
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
	name, attrs := modelCallOpening(model, operation, provider)
	if opts.InputMessages != nil {
		attrs = append(attrs, attribute.String(attrInputMessages, asJSON(opts.InputMessages)))
	}
	if opts.SystemInstructions != nil {
		attrs = append(attrs, attribute.String(attrSystemInstructions, asJSON(opts.SystemInstructions)))
	}
	ctx, span := r.child(ctx, name, trace.SpanKindClient, attrs)
	return ctx, &ModelCall{handle: handle{span: span, client: c}, run: r, provider: provider, model: model}
}

// ToolCall opens a call to the tool named name where Step opens a step;
// Returned records what the tool returned.
func (r *Run) ToolCall(ctx context.Context, name string, opts ToolCallOptions) (context.Context, *ToolCall) {
	if r == nil || r.span == nil {
		return ctx, &ToolCall{}
	}
	if err := r.opens(ctx, false); err != nil {
		return contextWithRun(refused(ctx, err), r), &ToolCall{}
	}
	spanName, attrs := toolCallOpening(r.client.cut(name, nameMax, "tool's name"))
	if opts.Arguments != nil {
		attrs = append(attrs, attribute.String(attrToolCallArguments, asJSON(opts.Arguments)))
	}
	ctx, span := r.child(ctx, spanName, trace.SpanKindInternal, attrs)
	return ctx, &ToolCall{handle: handle{span: span, client: r.client}, run: r}
}

// inheritedAttributes is what every span of a run carries, live or recorded:
// its id and agent, and its user and session where it has them.
func inheritedAttributes(runID, agent, userID, sessionID string) []attribute.KeyValue {
	inherited := []attribute.KeyValue{
		attribute.String(attrRunID, runID),
		attribute.String(attrAgent, agent),
	}
	if userID != "" {
		inherited = append(inherited, attribute.String(attrUserID, userID))
	}
	if sessionID != "" {
		inherited = append(inherited, attribute.String(attrSessionID, sessionID))
	}
	return inherited
}

// runOpening is a run's span name and attributes, live or recorded, its tags
// by key: a map ranges in a new order each time, and the same record must be
// the same bytes.
func (c *Client) runOpening(agent string, inherited []attribute.KeyValue, rate float64, tags map[string]string) (string, []attribute.KeyValue) {
	kept := c.tagAttributes(tags)
	attrs := make([]attribute.KeyValue, 0, 3+len(inherited)+len(kept))
	attrs = append(attrs, attribute.String(attrOperation, runOperation), attribute.String(attrAgentName, agent))
	attrs = append(attrs, inherited...)
	attrs = append(attrs, attribute.Float64(attrSampleRate, rate))
	return runOperation + " " + agent, append(attrs, kept...)
}

// modelCallOpening is a model call's span name and own attributes, live or
// recorded, its operation one ingest reads and an empty model or provider
// left off.
func modelCallOpening(model, operation, provider string) (string, []attribute.KeyValue) {
	attrs := []attribute.KeyValue{attribute.String(attrOperation, operation)}
	name := operation
	if model != "" {
		attrs = append(attrs, attribute.String(attrRequestModel, model))
		name = operation + " " + model
	}
	if provider != "" {
		attrs = append(attrs, attribute.String(attrProvider, provider))
	}
	return name, attrs
}

// toolCallOpening is a tool call's span name and own attributes, live or
// recorded.
func toolCallOpening(name string) (string, []attribute.KeyValue) {
	return toolOperation + " " + name, []attribute.KeyValue{
		attribute.String(attrOperation, toolOperation),
		attribute.String(attrToolName, name),
	}
}

func (r *Run) child(ctx context.Context, name string, kind trace.SpanKind, attrs []attribute.KeyValue) (context.Context, trace.Span) {
	if ctx == nil {
		ctx = context.Background()
	}
	// A context the run is not in, a fresh goroutine's or the one the run was
	// opened from, puts the span under the run, in its trace.
	if RunFromContext(ctx) != r || !trace.SpanContextFromContext(ctx).IsValid() {
		ctx = trace.ContextWithSpan(ctx, r.span)
	}
	all := make([]attribute.KeyValue, 0, len(r.inherited)+len(attrs))
	all = append(all, r.inherited...)
	all = append(all, attrs...)
	ctx, span := r.client.tracer.Start(ctx, name, trace.WithSpanKind(kind), trace.WithAttributes(all...))
	return contextWithRun(trace.ContextWithSpan(ctx, inContext{span, r.displaced}), r), span
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
	// Given once the run's lock is released: the logger is the application's.
	if w := m.hold(attrs, answeredAs, usage); w.key != "" {
		m.client.exporter.warnOnce(w.key, w.text)
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
	g := m.run.guard
	g.mu.Lock()
	m.ended = true
	g.mu.Unlock()
	m.handle.End()
}

// hold sets an answer on the span and counts its cost under the run's lock, so
// answers count one at a time and none after End; it is the warning it calls for.
func (m *ModelCall) hold(attrs []attribute.KeyValue, answeredAs string, usage []attribute.KeyValue) warning {
	g := m.run.guard
	g.mu.Lock()
	defer g.mu.Unlock()
	if m.ended {
		return warning{"call:late", "telmoni: a model call must be answered before it ends; an answer given after is not sent and counts nothing toward the run's spend"}
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
		case attrCacheWriteInputTokens:
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
			"telmoni: no price for " + model + " (" + provider + "), which reported no cost; it counts nothing toward the run's spend cap"}
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

// modelOperation is a model call's operation as ingest reads one: chat for a
// blank one, and for one ingest would file as a step, a run or a tool call,
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
	c.exporter.warnOnce("call:operation", "telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat")
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
		{attrCacheWriteInputTokens, u.CacheWriteInputTokens},
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
		c.exporter.warnOnce("usage:"+name, "telmoni: "+name+" must be a finite number of US dollars from 0; the call is sent without it")
		return
	}
	c.exporter.warnOnce("usage:"+name, "telmoni: "+name+" must be a whole number from 0 within 64 bits; the call is sent without it")
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
		c.exporter.warnOnce("cut:"+what, fmt.Sprintf("telmoni: a %s must be up to %d characters; it is sent cut to its first %d", what, most, most))
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
// with one warning, since cut it would name another.
func (c *Client) fits(id string, most int, what string) bool {
	if len(id) <= most || utf8.RuneCountInString(id) <= most {
		return true
	}
	if c.built() {
		c.exporter.warnOnce("long:"+what, fmt.Sprintf("telmoni: a %s must be up to %d characters; the run is sent without it", what, most))
	}
	return false
}

// tagAttributes is a run's first 32 tags by key within ingest's caps; more
// would crowd the marks a run ends with out of the span's 128 attributes.
func (c *Client) tagAttributes(tags map[string]string) []attribute.KeyValue {
	var keys []string
	for _, key := range slices.Sorted(maps.Keys(tags)) {
		// Ingest drops it before counting, so it takes no place among the 32.
		if key == "" || utf8.RuneCountInString(key) > tagKeyMax || utf8.RuneCountInString(tags[key]) > tagValueMax {
			c.exporter.warnOnce("run:tag", "telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent")
			continue
		}
		keys = append(keys, key)
	}
	if len(keys) > tagsMax {
		c.exporter.warnOnce("run:tags", "telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent")
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
