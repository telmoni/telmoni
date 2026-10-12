package telmoni

import (
	"context"
	"errors"
	"maps"
	"slices"
	"strings"
	"time"

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

// RecordRun sends a run that has already ended, from its record, as the live
// calls would have sent it: for a job that keeps its own log, or a hook that
// runs once the turn it reports is over. It goes through the same exporter,
// content rules and sample rate, in a trace of its own named by the run's id,
// whatever span ctx carries. Each span's id is derived from the run's id and
// the span's place in the record, and the spans are queued together, the
// run's start first and the run's own last, so the same record sent again is
// the same bytes, which ingest answers once. A record holds no content.
//
// It returns an error, and sends nothing, for a blank agent, a run id that is
// not a UUID, a zero time, a nil item, or a span that ends before it starts.
// A record is handed over whole, so it is refused whole, where a live run's
// mistakes are warned of and the run goes on.
func (c *Client) RecordRun(ctx context.Context, record RunRecord) error {
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
	attrs := make([]attribute.KeyValue, 0, 3+len(inherited)+len(record.Tags))
	attrs = append(attrs, attribute.String(attrOperation, runOperation), attribute.String(attrAgentName, record.Agent))
	attrs = append(attrs, inherited...)
	attrs = append(attrs, sampleRate)
	// By key: a map ranges in a new order each time, and the same record must
	// be the same bytes.
	for _, key := range slices.Sorted(maps.Keys(record.Tags)) {
		attrs = append(attrs, attribute.String(tagPrefix+key, record.Tags[key]))
	}
	if rate < 1 && c.random() >= rate {
		c.processor.sampleOut(runID)
	}
	r := &recorder{tracer: c.tracer, runID: runUUID, inherited: inherited}
	// No span in it: whatever span ctx carries, the record's trace is its
	// own.
	root := trace.ContextWithSpanContext(ctx, trace.SpanContext{})
	release := c.processor.hold()
	defer release()
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
		attrs = append(attrs, usageAttributes(call.Usage)...)
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
	if err := checkTimes(record.Start, record.End); err != nil {
		return err
	}
	for _, item := range record.Inside {
		switch item := itemValue(item).(type) {
		case StepRecord:
			if err := checkTimes(item.Start, item.End); err != nil {
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

func checkCall(call CallRecord) error {
	switch call := itemValue(call).(type) {
	case ModelCallRecord:
		return checkTimes(call.Start, call.End)
	case ToolCallRecord:
		return checkTimes(call.Start, call.End)
	}
	return errNotAnItem
}

// checkTimes compares the times as they are sent, in Unix nanoseconds: a time
// read from time.Now carries the monotonic clock too, which Before would
// compare instead.
func checkTimes(start, end time.Time) error {
	if start.IsZero() || end.IsZero() {
		return errors.New("telmoni: a recorded time cannot be zero")
	}
	if end.UnixNano() < start.UnixNano() {
		return errors.New("telmoni: a recorded span cannot end before it starts")
	}
	return nil
}
