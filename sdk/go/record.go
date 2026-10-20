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
}

// RecordItem is what a recorded run holds: a StepRecord, a ModelCallRecord or
// a ToolCallRecord, or a pointer to one.
type RecordItem interface {
	recordItem()
}

// CallRecord is what a recorded step holds: a ModelCallRecord or a
// ToolCallRecord, or a pointer to one.
type CallRecord interface {
	RecordItem
	callRecord()
}

// StepRecord is a step that has ended, with the calls made inside it, in the
// order they ran. ErrorType and Cancelled mark it as a RunRecord's mark a run.
type StepRecord struct {
	Name      string
	Start     time.Time
	End       time.Time
	Calls     []CallRecord
	ErrorType string
	Cancelled bool
}

// ModelCallRecord is a model call that has ended: its model, its times and its
// tokens. Provider and Operation are as a live call's ModelCallOptions has
// them, a blank Operation meaning chat, and Usage as its Answer has it.
// ErrorType and Cancelled mark it as a RunRecord's mark a run.
type ModelCallRecord struct {
	Model     string
	Provider  string
	Operation string
	Start     time.Time
	End       time.Time
	Usage     *Usage
	ErrorType string
	Cancelled bool
}

// ToolCallRecord is a tool call that has ended: its name and its times.
// ErrorType and Cancelled mark it as a RunRecord's mark a run.
type ToolCallRecord struct {
	Name      string
	Start     time.Time
	End       time.Time
	ErrorType string
	Cancelled bool
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

// ErrQueueFull is returned by RecordRun when the export queue has no room for
// the whole record, so nothing of it was queued: flush, or wait for the queue
// to drain, and record it again.
var ErrQueueFull = errors.New("telmoni: the export queue has no room for the record; flush or wait, then record it again")

// RecordRun sends a run that has ended as the live calls would have sent it:
// for a job that keeps its own log, or a hook run after the turn it reports.
// It goes through the same exporter, content rules and sample rate, in a
// trace of its own named by the run's id, whatever span ctx carries. Each
// span's id is derived from the run's id and the span's place in the record,
// and the spans are queued whole once built, to travel in exports of their
// own, so the same record sent again is the same bytes, whatever else is
// sent meanwhile, and ingest answers it once. A record holds no content.
//
// It returns an error, sending nothing, for a blank agent, a run id that is
// not a UUID, text past ingest's caps, a time at or before the Unix epoch or
// 2^64 nanoseconds or more after it, an empty step or model, a nil item or a
// span that ends before it starts, and ErrQueueFull when the queue has no
// room for it. A record is handed over whole, so it is refused whole, where
// a live run's mistakes are warned of.
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
	runID := formatUUID(runUUID)
	rate := c.config.SampleRate
	inherited := []attribute.KeyValue{
		attribute.String(attrRunID, runID),
		attribute.String(attrAgent, record.Agent),
	}
	if record.UserID != "" {
		inherited = append(inherited, attribute.String(attrUserID, record.UserID))
	}
	if record.SessionID != "" {
		inherited = append(inherited, attribute.String(attrSessionID, record.SessionID))
	}
	sampleRate := attribute.Float64(attrSampleRate, rate)
	// By key: a map ranges in a new order each time, and the same record must
	// be the same bytes.
	tags := c.tagAttributes(record.Tags)
	attrs := make([]attribute.KeyValue, 0, 3+len(inherited)+len(tags))
	attrs = append(attrs, attribute.String(attrOperation, runOperation), attribute.String(attrAgentName, record.Agent))
	attrs = append(attrs, inherited...)
	attrs = append(attrs, sampleRate)
	attrs = append(attrs, tags...)
	// Warned of before the hold, as the tags are: a logger that records a run
	// would wait on it for good.
	c.warnUsage(record)
	// Sampled out, no span is built: none of a record's ends after it, and
	// the same record sent again draws again.
	if rate < 1 && c.random() >= rate {
		return nil
	}
	// Queued whole or not at all: a record that outgrows the queue's room
	// would lose its tail, the run's own span first.
	release, ok := c.processor.hold(trace.TraceID(runUUID), recordSpans(record))
	if !ok {
		return ErrQueueFull
	}
	built := false
	defer func() { release(built) }()
	r := &recorder{tracer: c.tracer, runID: runUUID, inherited: inherited}
	// No span in it: whatever span ctx carries, the record's trace is its
	// own.
	root := trace.ContextWithSpanContext(ctx, trace.SpanContext{})
	run := r.open(root, runOperation+" "+record.Agent, trace.SpanKindInternal, attrs, record.Start)
	inRun := trace.ContextWithSpan(root, run)
	opener := r.open(inRun, runStart, trace.SpanKindInternal, slices.Concat(inherited, []attribute.KeyValue{sampleRate}), record.Start)
	opener.End(trace.WithTimestamp(record.Start))
	for _, item := range record.Inside {
		switch item := itemValue(item).(type) {
		case StepRecord:
			step := r.open(inRun, item.Name, trace.SpanKindInternal, inherited, item.Start)
			inStep := trace.ContextWithSpan(root, step)
			for _, call := range item.Calls {
				r.call(inStep, call)
			}
			endRecorded(step, item.End, item.ErrorType, item.Cancelled)
		case CallRecord:
			r.call(inRun, item)
		}
	}
	endRecorded(run, record.End, record.ErrorType, record.Cancelled)
	built = true
	return nil
}

// recorder opens a record's spans, each with the ids its place gives it: 0 the
// run, 1 its start, then each step and call in the record's order, a step
// before its calls.
type recorder struct {
	tracer    trace.Tracer
	runID     [16]byte
	inherited []attribute.KeyValue
	place     int
}

func (r *recorder) open(parent context.Context, name string, kind trace.SpanKind, attrs []attribute.KeyValue, start time.Time) trace.Span {
	ctx := withIDs(parent, trace.TraceID(r.runID), recordedSpanID(r.runID, r.place))
	r.place++
	_, span := r.tracer.Start(ctx, name,
		trace.WithSpanKind(kind),
		trace.WithAttributes(attrs...),
		trace.WithTimestamp(start),
	)
	return span
}

// call opens a call and ends it at once, so it is queued as it ended: before
// the next call, and before the step it is in.
func (r *recorder) call(parent context.Context, call CallRecord) {
	switch call := itemValue(call).(type) {
	case ModelCallRecord:
		operation := modelOperation(call.Operation)
		attrs := slices.Concat(r.inherited, []attribute.KeyValue{
			attribute.String(attrOperation, operation),
			attribute.String(attrRequestModel, call.Model),
		})
		if call.Provider != "" {
			attrs = append(attrs, attribute.String(attrProvider, call.Provider))
		}
		// No warning under the hold: RecordRun gave it before.
		attrs = append(attrs, usageAttributes(call.Usage, nil)...)
		span := r.open(parent, operation+" "+call.Model, trace.SpanKindClient, attrs, call.Start)
		endRecorded(span, call.End, call.ErrorType, call.Cancelled)
	case ToolCallRecord:
		attrs := slices.Concat(r.inherited, []attribute.KeyValue{
			attribute.String(attrOperation, toolOperation),
			attribute.String(attrToolName, call.Name),
		})
		span := r.open(parent, toolOperation+" "+call.Name, trace.SpanKindInternal, attrs, call.Start)
		endRecorded(span, call.End, call.ErrorType, call.Cancelled)
	}
}

// endRecorded marks a span as its record says it ended, and ends it at the
// record's end.
func endRecorded(span trace.Span, end time.Time, errorType string, cancelled bool) {
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
	for _, item := range record.Inside {
		switch item := itemValue(item).(type) {
		case StepRecord:
			if err := checkStep(item); err != nil {
				return err
			}
			for _, call := range item.Calls {
				if err := checkCall(call); err != nil {
					return err
				}
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

// recordSpans counts the spans a record queues: the run and its start, then
// each step and each call.
func recordSpans(record RunRecord) int {
	n := 2
	for _, item := range record.Inside {
		n++
		if step, ok := itemValue(item).(StepRecord); ok {
			n += len(step.Calls)
		}
	}
	return n
}

// warnUsage warns of each token count the record's calls leave off.
func (c *Client) warnUsage(record RunRecord) {
	for _, item := range record.Inside {
		var calls []CallRecord
		switch item := itemValue(item).(type) {
		case StepRecord:
			calls = item.Calls
		case CallRecord:
			calls = []CallRecord{item}
		}
		for _, call := range calls {
			if call, ok := itemValue(call).(ModelCallRecord); ok {
				usageAttributes(call.Usage, c.warnCount)
			}
		}
	}
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
			{"ErrorType", call.ErrorType, errorTypeMax},
		})
		if err != nil {
			return err
		}
		// Its span is named "<operation> <model>", a name ingest caps too.
		if utf8.RuneCountInString(modelOperation(call.Operation))+utf8.RuneCountInString(call.Model) > nameMax-1 {
			return fmt.Errorf("telmoni: a recorded model call's operation and model must be up to %d characters together", nameMax-1)
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
