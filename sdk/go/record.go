package telmoni

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"
	"unicode/utf8"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

// RunRecord is a run that has already ended, as RecordRun sends it: its id, a
// UUID the caller gives, and everything in it, each with its own times.
type RunRecord struct {
	// RunID is the run's id: a UUID in its 8-4-4-4-12 form, of any version
	// and in either case, sent in lowercase. Give a run the same id each time
	// it is sent: the same record is then the same bytes.
	RunID string
	// Agent is the agent's name.
	Agent string
	// UserID, SessionID and Tags are as a live run's RunOptions has them.
	UserID    string
	SessionID string
	Tags      map[string]string
	// InputMessages and OutputMessages are content: recorded as JSON, sent
	// only when CaptureContent is on and the project's mode allows.
	InputMessages  any
	OutputMessages any
	// Start and End are when the run began and ended. A zero time is refused:
	// it is a time never set, not one the run had.
	Start time.Time
	End   time.Time
	// Inside is what ran inside the run, in the order it ran.
	Inside []RecordItem
	// ErrorType, when set, marks the run failed with it as its error.type,
	// as FailWith does.
	ErrorType string
	// Cancelled marks the run cancelled by the agent, as Cancel does.
	Cancelled bool
	// Scores score the run, each sent at its end; one the rules refuse is left
	// off with one warning, the record still sent. A step's or call's score its
	// own span the same way.
	Scores []RecordedScore
}

// RecordItem is what a recorded run holds: a StepRecord, a ModelCallRecord or
// a ToolCallRecord, or a pointer to one.
type RecordItem interface {
	recordItem()
}

// CallRecord is a recorded call: a ModelCallRecord or a ToolCallRecord, or a
// pointer to one.
type CallRecord interface {
	RecordItem
	callRecord()
}

// StepRecord is a step that has ended, with what ran inside it, in the order
// it ran: its calls, and its own steps, a subagent's say, up to 16 deep.
// InputMessages and OutputMessages, a subagent's task and answer say, are
// content as a RunRecord's are. ErrorType and Cancelled mark it as a
// RunRecord's mark a run.
type StepRecord struct {
	Name           string
	InputMessages  any
	OutputMessages any
	Start          time.Time
	End            time.Time
	Calls          []RecordItem
	ErrorType      string
	Cancelled      bool
	Scores         []RecordedScore
}

// ModelCallRecord is a model call that has ended: its model, its times and its
// tokens. Provider and Operation are as a live call's ModelCallOptions has
// them, a blank Operation meaning chat, and Usage as its Answer has it.
// ReasoningEffort, how hard the model was asked to reason, is sent in every
// content mode; InputMessages and OutputMessages are content as a RunRecord's
// are. ErrorType and Cancelled mark it as a RunRecord's mark a run.
type ModelCallRecord struct {
	Model           string
	Provider        string
	Operation       string
	ReasoningEffort string
	Start           time.Time
	End             time.Time
	Usage           *Usage
	InputMessages   any
	OutputMessages  any
	ErrorType       string
	Cancelled       bool
	Scores          []RecordedScore
}

// ToolCallRecord is a tool call that has ended: its name and its times.
// Arguments and Result are content as a RunRecord's messages are. ErrorType
// and Cancelled mark it as a RunRecord's mark a run.
type ToolCallRecord struct {
	Name      string
	Arguments any
	Result    any
	Start     time.Time
	End       time.Time
	ErrorType string
	Cancelled bool
	Scores    []RecordedScore
}

func (StepRecord) recordItem()      {}
func (ModelCallRecord) recordItem() {}
func (ToolCallRecord) recordItem()  {}
func (ModelCallRecord) callRecord() {}
func (ToolCallRecord) callRecord()  {}

var errNotAnItem = errors.New("telmoni: a recorded item must be a StepRecord, a ModelCallRecord or a ToolCallRecord")

// errNoName refuses a step's name or a model call's model only when exactly
// empty: ingest takes a name of spaces as given.
var errNoName = errors.New("telmoni: a recorded step's name or model call's model cannot be empty")

// stepDepthMax is how deep steps nest, a run's own at 1: past any agent's
// subagents, and an end to a step that holds itself through a pointer.
const stepDepthMax = 16

var errTooDeep = fmt.Errorf("telmoni: a recorded step must be nested at most %d deep", stepDepthMax)

// ErrQueueFull is returned by RecordRun when the export queue has no room for
// the whole record, so nothing of it was queued: flush, or wait for the queue
// to drain, and record it again; one of more spans than the queue holds, once
// WithBatching raises its size, its error saying so.
var ErrQueueFull = errors.New("telmoni: the export queue has no room for the record; flush or wait, then record it again")

// queueTooSmall is ErrQueueFull for a record of more spans than the whole
// queue holds, which no flush makes room for.
type queueTooSmall struct{ spans, queue int }

func (e queueTooSmall) Error() string {
	return fmt.Sprintf("telmoni: the export queue has no room for the record's %d spans; "+
		"the queue holds %d, so raise its size to record it", e.spans, e.queue)
}

func (queueTooSmall) Is(target error) bool { return target == ErrQueueFull }

// ErrShutdown is returned by RecordRun once the client is shut down, so
// nothing of the record was queued.
var ErrShutdown = errors.New("telmoni: the client is shut down; the record was not sent")

// RecordRun sends a run that has ended as the live calls would have sent it:
// for a job that keeps its own log, or a hook run after the turn it reports.
// It goes through the same exporter and content rules, in a trace of its own
// named by the run's id, whatever span ctx carries. Each span's id derives
// from the run's id and its place in the record, and the spans are queued
// whole once built, in exports of their own, so the same record sent again is
// the same bytes whatever else is sent meanwhile, and ingest answers it once.
//
// It returns an error, sending nothing, for a blank agent, a run id that is
// not a UUID, text past ingest's caps, a time at or before the Unix epoch or
// 2^64 nanoseconds or more after it, an empty step or model, a nil item, a
// step nested past 16 deep or a span that ends before it starts, ErrQueueFull
// when the queue has no room for it, and ErrShutdown once the client is shut
// down. A record is handed over whole, so it is refused whole, where a live
// run's mistakes are warned of.
func (c *Client) RecordRun(ctx context.Context, record RunRecord) error {
	if !c.built() {
		return errors.New("telmoni: a Client must come from New; nothing is sent")
	}
	if strings.TrimSpace(record.Agent) == "" {
		return errors.New("telmoni: a run needs its agent's name")
	}
	runUUID, ok := parseUUID(record.RunID)
	if !ok {
		return errors.New("telmoni: a recorded run's id must be a UUID")
	}
	// The run's id is its trace's, and a trace id of zeros is no trace at all.
	if runUUID == [16]byte{} {
		return errors.New("telmoni: a recorded run's id must be a UUID other than the nil UUID")
	}
	// Checked whole before anything is queued, so a record refused sends
	// nothing rather than half a run.
	if err := checkRecord(record); err != nil {
		return err
	}
	if ctx == nil {
		ctx = context.Background()
	}
	inherited := inheritedAttributes(formatUUID(runUUID), record.Agent, record.UserID, record.SessionID)
	name, attrs := c.runOpening(record.Agent, inherited, record.Tags)
	if record.InputMessages != nil {
		attrs = append(attrs, attribute.String(attrInputMessages, asJSON(record.InputMessages)))
	}
	if record.OutputMessages != nil {
		attrs = append(attrs, attribute.String(attrOutputMessages, asJSON(record.OutputMessages)))
	}
	// Before the hold: a logger that records a run would wait on it for good.
	c.warnUsage(record)
	scores := c.recordScores(record)
	// Before the hold too: a MarshalJSON of the caller's may record a run.
	contents := recordContents(record)
	// Queued whole or not at all: a record that outgrows the queue's room
	// would lose its tail, the run's own span first.
	release, err := c.processor.hold(trace.TraceID(runUUID), recordSpans(record))
	if err != nil {
		return err
	}
	built := false
	defer func() { release(built) }()
	r := &recorder{tracer: c.tracer, runID: runUUID, inherited: inherited, contents: contents, scores: scores}
	// No span in it: whatever span ctx carries, the record's trace is its
	// own.
	root := trace.ContextWithSpanContext(ctx, trace.SpanContext{})
	run, runScores := r.open(root, name, trace.SpanKindInternal, attrs, record.Start)
	r.items(trace.ContextWithSpan(root, run), record.Inside)
	endRecorded(run, runScores, record.End, record.ErrorType, record.Cancelled)
	built = true
	return nil
}

// recorder opens a record's spans, each with the ids its place gives it: 0 the
// run, then each step and call from 1 in the record's order, depth first, a
// step before what it holds.
type recorder struct {
	tracer    trace.Tracer
	runID     [16]byte
	inherited []attribute.KeyValue
	contents  [][]attribute.KeyValue
	scores    [][][]attribute.KeyValue
	place     int
}

// open starts the span at the next place, and is the scores it ends with.
func (r *recorder) open(parent context.Context, name string, kind trace.SpanKind, attrs []attribute.KeyValue, start time.Time) (trace.Span, [][]attribute.KeyValue) {
	ctx := withIDs(parent, trace.TraceID(r.runID), recordedSpanID(r.runID, r.place))
	attrs = slices.Concat(attrs, r.contents[r.place])
	scores := r.scores[r.place]
	r.place++
	_, span := r.tracer.Start(ctx, name,
		trace.WithSpanKind(kind),
		trace.WithAttributes(attrs...),
		trace.WithTimestamp(start),
	)
	return span, scores
}

// recordContents is each span's content by its place in the record: none for
// the run, whose own goes with its opening, then each step and call as
// eachItem places them.
func recordContents(record RunRecord) [][]attribute.KeyValue {
	contents := [][]attribute.KeyValue{nil}
	eachItem(record.Inside, func(item RecordItem) {
		contents = append(contents, contentOf(item))
	})
	return contents
}

// recordScores is each span's scores by its place in the record, as
// recordContents places content, each refused one warned of.
func (c *Client) recordScores(record RunRecord) [][][]attribute.KeyValue {
	scores := [][][]attribute.KeyValue{c.recordedScores(record.Scores)}
	eachItem(record.Inside, func(item RecordItem) {
		scores = append(scores, c.recordedScores(scoresOf(item)))
	})
	return scores
}

// eachItem visits every item a record holds as a value, in the record's order,
// depth first, a step before what it holds: the order of their places. Only
// for a record checkRecord passed, whose steps end.
func eachItem(items []RecordItem, visit func(RecordItem)) {
	for _, item := range items {
		item = itemValue(item)
		visit(item)
		if step, ok := item.(StepRecord); ok {
			eachItem(step.Calls, visit)
		}
	}
}

func scoresOf(item RecordItem) []RecordedScore {
	switch item := itemValue(item).(type) {
	case StepRecord:
		return item.Scores
	case ModelCallRecord:
		return item.Scores
	case ToolCallRecord:
		return item.Scores
	}
	return nil
}

// contentOf is a recorded step's or call's content as JSON, which the
// exporter's lock sends or keeps back.
func contentOf(item RecordItem) []attribute.KeyValue {
	var out []attribute.KeyValue
	add := func(key string, value any) {
		if value != nil {
			out = append(out, attribute.String(key, asJSON(value)))
		}
	}
	switch item := itemValue(item).(type) {
	case StepRecord:
		add(attrInputMessages, item.InputMessages)
		add(attrOutputMessages, item.OutputMessages)
	case ModelCallRecord:
		add(attrInputMessages, item.InputMessages)
		add(attrOutputMessages, item.OutputMessages)
	case ToolCallRecord:
		add(attrToolCallArguments, item.Arguments)
		add(attrToolCallResult, item.Result)
	}
	return out
}

// items opens what a run or a step holds under parent, each step's own under
// it, so each ends before what holds it.
func (r *recorder) items(parent context.Context, items []RecordItem) {
	for _, item := range items {
		switch item := itemValue(item).(type) {
		case StepRecord:
			step, scores := r.open(parent, item.Name, trace.SpanKindInternal, r.inherited, item.Start)
			r.items(trace.ContextWithSpan(parent, step), item.Calls)
			endRecorded(step, scores, item.End, item.ErrorType, item.Cancelled)
		case CallRecord:
			r.call(parent, item)
		}
	}
}

// call opens a call and ends it at once, so it is queued as it ended: before
// the next call, and before the step it is in.
func (r *recorder) call(parent context.Context, call CallRecord) {
	switch call := itemValue(call).(type) {
	case ModelCallRecord:
		operation, _ := modelOperation(call.Operation)
		name, own := modelCallOpening(call.Model, operation, call.Provider, "", "")
		if call.ReasoningEffort != "" {
			own = append(own, attribute.String(attrRequestReasoningLevel, call.ReasoningEffort))
		}
		// No warning under the hold: RecordRun gave it before.
		attrs := slices.Concat(r.inherited, own, usageAttributes(call.Usage, nil))
		span, scores := r.open(parent, name, trace.SpanKindClient, attrs, call.Start)
		endRecorded(span, scores, call.End, call.ErrorType, call.Cancelled)
	case ToolCallRecord:
		name, own := toolCallOpening(call.Name)
		span, scores := r.open(parent, name, trace.SpanKindInternal, slices.Concat(r.inherited, own), call.Start)
		endRecorded(span, scores, call.End, call.ErrorType, call.Cancelled)
	}
}

// endRecorded marks a span as its record says it ended, its scores timed at
// its end so the same record is the same bytes, and ends it there.
func endRecorded(span trace.Span, scores [][]attribute.KeyValue, end time.Time, errorType string, cancelled bool) {
	for _, attrs := range scores {
		span.AddEvent(eventEvaluation, trace.WithTimestamp(end), trace.WithAttributes(attrs...))
	}
	if errorType != "" {
		span.SetAttributes(attribute.String(attrErrorType, errorType))
		span.SetStatus(codes.Error, "")
	}
	if cancelled {
		span.SetAttributes(attribute.Bool(attrCancelled, true))
	}
	span.End(trace.WithTimestamp(end))
}

// itemValue is item as a value: one given by pointer is read through it, and
// a nil pointer is nil.
func itemValue(item RecordItem) RecordItem {
	switch item := item.(type) {
	case *StepRecord:
		if item == nil {
			return nil
		}
		return *item
	case *ModelCallRecord:
		if item == nil {
			return nil
		}
		return *item
	case *ToolCallRecord:
		if item == nil {
			return nil
		}
		return *item
	}
	return item
}

func checkRecord(record RunRecord) error {
	if err := checkRun(record); err != nil {
		return err
	}
	return checkItems(record.Inside, 1)
}

// checkItems checks what a run or a step holds, each step in it depth deep.
func checkItems(items []RecordItem, depth int) error {
	for _, item := range items {
		switch item := itemValue(item).(type) {
		case StepRecord:
			if depth > stepDepthMax {
				return errTooDeep
			}
			if err := checkStep(item); err != nil {
				return err
			}
			if err := checkItems(item.Calls, depth+1); err != nil {
				return err
			}
		case CallRecord:
			if err := checkCall(item); err != nil {
				return err
			}
		default:
			return errNotAnItem
		}
	}
	return nil
}

// recordSpans counts the spans a record queues: the run, then each step and
// each call.
func recordSpans(record RunRecord) int {
	n := 1
	eachItem(record.Inside, func(RecordItem) { n++ })
	return n
}

// warnUsage warns of each token count the record's calls leave off, and of
// an operation sent as chat.
func (c *Client) warnUsage(record RunRecord) {
	eachItem(record.Inside, func(item RecordItem) {
		if call, ok := item.(ModelCallRecord); ok {
			usageAttributes(call.Usage, c.warnCount)
			if _, setAside := modelOperation(call.Operation); setAside {
				c.warnOperation()
			}
		}
	})
}

func checkRun(record RunRecord) error {
	err := checkCaps("run", []capped{
		{"Agent", record.Agent, agentMax},
		{"UserID", record.UserID, userIDMax},
		{"SessionID", record.SessionID, sessionIDMax},
		{"ErrorType", record.ErrorType, errorTypeMax},
	})
	if err != nil {
		return err
	}
	return checkTimes(record.Start, record.End)
}

func checkStep(step StepRecord) error {
	if step.Name == "" {
		return errNoName
	}
	if err := checkCaps("step", []capped{{"Name", step.Name, nameMax}, {"ErrorType", step.ErrorType, errorTypeMax}}); err != nil {
		return err
	}
	return checkTimes(step.Start, step.End)
}

func checkCall(call CallRecord) error {
	switch call := itemValue(call).(type) {
	case ModelCallRecord:
		if call.Model == "" {
			return errNoName
		}
		err := checkCaps("model call", []capped{
			{"Model", call.Model, modelMax},
			{"Provider", call.Provider, providerMax},
			{"ReasoningEffort", call.ReasoningEffort, attributeValueMax},
			{"ErrorType", call.ErrorType, errorTypeMax},
		})
		if err != nil {
			return err
		}
		return checkTimes(call.Start, call.End)
	case ToolCallRecord:
		// A tool's empty name ingest reads as none, keeping the call.
		if err := checkCaps("tool call", []capped{{"Name", call.Name, nameMax}, {"ErrorType", call.ErrorType, errorTypeMax}}); err != nil {
			return err
		}
		return checkTimes(call.Start, call.End)
	}
	return errNotAnItem
}

// capped is a recorded field by its Go name, and ingest's cap on it.
type capped struct {
	field, value string
	max          int
}

// checkCaps refuses the first field past its cap, counting characters as
// ingest does: JSON sends each invalid byte as one U+FFFD.
func checkCaps(kind string, fields []capped) error {
	for _, f := range fields {
		if utf8.RuneCountInString(f.value) > f.max {
			return fmt.Errorf("telmoni: a recorded %s's %s must be up to %d characters", kind, f.field, f.max)
		}
	}
	return nil
}

// checkTimes compares the times as they are sent, in Unix nanoseconds: a time
// read from time.Now carries the monotonic clock too, which Before would
// compare instead.
func checkTimes(start, end time.Time) error {
	if start.IsZero() || end.IsZero() {
		return errors.New("telmoni: a recorded time cannot be zero")
	}
	from, fromOK := unixNanos(start)
	to, toOK := unixNanos(end)
	if !fromOK || !toOK {
		return errors.New("telmoni: a recorded time must fall after the Unix epoch and less than 2^64 nanoseconds after it")
	}
	if to < from {
		return errors.New("telmoni: a recorded span cannot end before it starts")
	}
	return nil
}
