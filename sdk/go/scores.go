package telmoni

import (
	"context"
	"encoding/hex"
	"math"
	"strings"
	"unicode/utf8"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/trace"
)

// ScoreValue is what a score says: a Number, a Bool or a Category. The zero
// ScoreValue is none, and its score is not sent.
type ScoreValue struct {
	kind   scoreKind
	number float64
	label  string
}

type scoreKind int

const (
	scoreNone scoreKind = iota
	scoreNumber
	scoreBoolean
	scoreCategory
)

// Number is a score of a number, finite and within its range where it has one.
func Number(value float64) ScoreValue { return ScoreValue{kind: scoreNumber, number: value} }

// Bool is a yes or no, sent as 1 or 0.
func Bool(value bool) ScoreValue {
	if value {
		return ScoreValue{kind: scoreBoolean, number: 1}
	}
	return ScoreValue{kind: scoreBoolean}
}

// Category is a score of one label, 1 to 64 characters once trimmed.
func Category(label string) ScoreValue { return ScoreValue{kind: scoreCategory, label: label} }

// ScoreOptions is what a score is given beside its value; each is optional.
type ScoreOptions struct {
	// Range is a number's least and most, min below max, both inclusive.
	Range *[2]float64
	// Version names the code that scored, up to 64 characters; blank is none.
	Version string
}

// ScoreTarget is what a late score scores: a trace by its id, one of its spans
// by the trace's id and the span's, or a session by its id.
type ScoreTarget struct {
	TraceID   string
	SpanID    string
	SessionID string
}

// RecordedScore is a score a recorded span holds, sent at the span's end.
type RecordedScore struct {
	Name    string
	Value   ScoreValue
	Options ScoreOptions
}

// Why a score is not sent, each warned of once.
const (
	whyName     = "its name is 1 to 64 characters"
	whyNumber   = "a number score is a finite number within its range"
	whyCategory = "its category is 1 to 64 characters"
	whyVersion  = "its version is up to 64 characters"
	whyRange    = "a range is a number score's alone"
	whyValue    = "its value is a number, a yes or no, or a category"
	whyTarget   = "it names a trace by 32 hex digits, a trace and one of its spans by 16, or a session of up to 200 characters"
	whyEnded    = "its span had ended; send it with the client's score, naming the span"
	whyFull     = "its span holds 16 scores already"
)

// score is one held to the rules, its text trimmed.
type score struct {
	name, label, version string
	kind                 scoreKind
	value                float64
	ranged               *[2]float64
}

// checkScore is the score as sent, or why it is not.
func checkScore(name string, value ScoreValue, opts ScoreOptions) (score, string) {
	s := score{name: strings.TrimSpace(name), kind: value.kind, value: value.number, version: strings.TrimSpace(opts.Version)}
	if s.name == "" || utf8.RuneCountInString(s.name) > scoreNameMax {
		return s, whyName
	}
	switch value.kind {
	case scoreNone:
		return s, whyValue
	case scoreNumber:
		if r := opts.Range; r != nil {
			if !finite(r[0]) || !finite(r[1]) || r[0] >= r[1] || s.value < r[0] || s.value > r[1] {
				return s, whyNumber
			}
			s.ranged = &[2]float64{r[0], r[1]}
		}
		if !finite(s.value) {
			return s, whyNumber
		}
	default:
		if opts.Range != nil {
			return s, whyRange
		}
	}
	if value.kind == scoreCategory {
		s.label = strings.TrimSpace(value.label)
		if s.label == "" || utf8.RuneCountInString(s.label) > scoreCategoryMax {
			return s, whyCategory
		}
	}
	if utf8.RuneCountInString(s.version) > scoreVersionMax {
		return s, whyVersion
	}
	return s, ""
}

func finite(f float64) bool { return !math.IsNaN(f) && !math.IsInf(f, 0) }

// attributes are the score as its evaluation event carries it.
func (s score) attributes() []attribute.KeyValue {
	attrs := []attribute.KeyValue{attribute.String(attrEvaluationName, s.name)}
	switch s.kind {
	case scoreCategory:
		attrs = append(attrs, attribute.String(attrEvaluationLabel, s.label))
	case scoreBoolean:
		attrs = append(attrs, attribute.Float64(attrEvaluationValue, s.value), attribute.String(attrScoreType, "boolean"))
	default:
		attrs = append(attrs, attribute.Float64(attrEvaluationValue, s.value))
		if s.ranged != nil {
			attrs = append(attrs, attribute.Float64(attrScoreRangeMin, s.ranged[0]), attribute.Float64(attrScoreRangeMax, s.ranged[1]))
		}
	}
	if s.version != "" {
		attrs = append(attrs, attribute.String(attrScoreVersion, s.version))
	}
	return attrs
}

// scoreItem is one score of a POST to scoresPath; an absent key is left out.
type scoreItem struct {
	ID         string   `json:"id"`
	TraceID    string   `json:"trace_id,omitempty"`
	SpanID     string   `json:"span_id,omitempty"`
	SessionID  string   `json:"session_id,omitempty"`
	Name       string   `json:"name"`
	DataType   string   `json:"data_type"`
	Value      *float64 `json:"value,omitempty"`
	Categories []string `json:"categories,omitempty"`
	RangeMin   *float64 `json:"range_min,omitempty"`
	RangeMax   *float64 `json:"range_max,omitempty"`
	Version    string   `json:"version,omitempty"`
}

func (s score) item(target ScoreTarget) scoreItem {
	// A fresh id each call, so a retry replaces itself.
	item := scoreItem{ID: newUUID(), TraceID: target.TraceID, SpanID: target.SpanID, SessionID: target.SessionID, Name: s.name, Version: s.version}
	switch s.kind {
	case scoreCategory:
		item.DataType, item.Categories = "category", []string{s.label}
	case scoreBoolean:
		item.DataType, item.Value = "boolean", &s.value
	default:
		item.DataType, item.Value = "number", &s.value
		if s.ranged != nil {
			item.RangeMin, item.RangeMax = &s.ranged[0], &s.ranged[1]
		}
	}
	return item
}

// checkTarget is the target as sent, its ids lowercase and its session
// trimmed; false for anything but a trace, a trace's span or a session.
func checkTarget(t ScoreTarget) (ScoreTarget, bool) {
	if t.SessionID != "" {
		session := strings.TrimSpace(t.SessionID)
		if t.TraceID != "" || t.SpanID != "" || session == "" || utf8.RuneCountInString(session) > sessionIDMax {
			return t, false
		}
		return ScoreTarget{SessionID: session}, true
	}
	traceID, ok := parseTraceID(t.TraceID)
	if !ok {
		return t, false
	}
	out := ScoreTarget{TraceID: traceID.String()}
	if t.SpanID != "" {
		if _, err := hex.DecodeString(t.SpanID); err != nil || len(t.SpanID) != 16 {
			return t, false
		}
		out.SpanID = strings.ToLower(t.SpanID)
	}
	return out, true
}

// refuseScore warns once of a score not sent, by why.
func (c *Client) refuseScore(why string) {
	c.exporter.warnOnce("score:"+why, WarningInput, "telmoni: a score was not sent: "+why)
}

// Score sends a score of a trace, one of its spans or a session that may have
// ended, no trace needed. It travels the export queue as a span of its own,
// which the exporter posts to /v1/scores instead; a score or target the rules
// refuse is not sent, with one warning.
func (c *Client) Score(target ScoreTarget, name string, value ScoreValue, opts ScoreOptions) {
	if !c.built() {
		return
	}
	s, why := checkScore(name, value, opts)
	if why == "" {
		var ok bool
		if target, ok = checkTarget(target); !ok {
			why = whyTarget
		}
	}
	if why != "" {
		c.refuseScore(why)
		return
	}
	// From the root, so no trace or its attributes reach it.
	_, carrier := c.tracer.Start(context.Background(), scoreSpanName,
		trace.WithAttributes(attribute.String(attrScoreSent, asJSON(s.item(target)))))
	carrier.End()
}

// score adds the score's evaluation event to the span, timed now: refused,
// with one warning, once the span ended or holds 16.
func (h *handle) score(name string, value ScoreValue, opts ScoreOptions) {
	if h.span == nil {
		return
	}
	s, why := checkScore(name, value, opts)
	if why == "" {
		h.mu.Lock()
		switch {
		case h.ended:
			why = whyEnded
		case h.scored >= scoresPerSpanMax:
			why = whyFull
		default:
			h.scored++
			h.span.AddEvent(eventEvaluation, trace.WithAttributes(s.attributes()...))
		}
		h.mu.Unlock()
	}
	// Given once the lock is released: the logger is the application's.
	if why != "" {
		h.client.refuseScore(why)
	}
}

func (h *handle) spanID() string {
	if h.span == nil {
		return ""
	}
	return h.span.SpanContext().SpanID().String()
}

// Score scores the trace on the agent's span, as the client's Score would,
// timed now; once the trace ended, send it with the client's Score instead.
func (r *Trace) Score(name string, value ScoreValue, opts ScoreOptions) {
	if r == nil {
		return
	}
	r.handle.score(name, value, opts)
}

// Score scores the step as its trace's Score would.
func (s *Step) Score(name string, value ScoreValue, opts ScoreOptions) {
	if s == nil {
		return
	}
	s.handle.score(name, value, opts)
}

// Score scores the call as its trace's Score would.
func (m *ModelCall) Score(name string, value ScoreValue, opts ScoreOptions) {
	if m == nil {
		return
	}
	m.handle.score(name, value, opts)
}

// Score scores the call as its trace's Score would.
func (t *ToolCall) Score(name string, value ScoreValue, opts ScoreOptions) {
	if t == nil {
		return
	}
	t.handle.score(name, value, opts)
}

// Score scores the retrieval as its trace's Score would.
func (r *Retrieval) Score(name string, value ScoreValue, opts ScoreOptions) {
	if r == nil {
		return
	}
	r.handle.score(name, value, opts)
}

// SpanID is the agent's span's id, 16 lowercase hex digits, which a late
// score names; "" for a handle that does nothing.
func (r *Trace) SpanID() string {
	if r == nil {
		return ""
	}
	return r.handle.spanID()
}

// SpanID is the step's span's id, as its trace's SpanID is.
func (s *Step) SpanID() string {
	if s == nil {
		return ""
	}
	return s.handle.spanID()
}

// SpanID is the call's span's id, as its trace's SpanID is.
func (m *ModelCall) SpanID() string {
	if m == nil {
		return ""
	}
	return m.handle.spanID()
}

// SpanID is the call's span's id, as its trace's SpanID is.
func (t *ToolCall) SpanID() string {
	if t == nil {
		return ""
	}
	return t.handle.spanID()
}

// SpanID is the retrieval's span's id, as its trace's SpanID is.
func (r *Retrieval) SpanID() string {
	if r == nil {
		return ""
	}
	return r.handle.spanID()
}

// recordedScores is a recorded span's scores as events' attributes, each
// refused one and each past the 16th left off with one warning.
func (c *Client) recordedScores(given []RecordedScore) [][]attribute.KeyValue {
	var out [][]attribute.KeyValue
	for _, each := range given {
		s, why := checkScore(each.Name, each.Value, each.Options)
		if why == "" && len(out) >= scoresPerSpanMax {
			why = whyFull
		}
		if why != "" {
			c.refuseScore(why)
			continue
		}
		out = append(out, s.attributes())
	}
	return out
}
