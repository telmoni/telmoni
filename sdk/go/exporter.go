package telmoni

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/sdk"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	"go.opentelemetry.io/otel/trace"
)

const (
	// attempts is how many times one batch is sent before it is dropped.
	attempts       = 3
	requestTimeout = 10 * time.Second
	retryAfterMax  = 60 * time.Second
	// answerMax bounds what is read of an answer: a problem's JSON is small,
	// and an export never pays for more.
	answerMax = 1 << 20
	// warnedMax is how many distinct warnings are remembered before the set
	// starts over.
	warnedMax = 1024
	// contentDocs is where a dropped content field's warning sends the reader.
	contentDocs = "https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door"
)

var (
	droppedLine = regexp.MustCompile(`^span (\S+) dropped (\S+): (\w+)(?:; (.*))?$`)
	refusedLine = regexp.MustCompile(`^span (\S+) refused: (.*)$`)
)

// exporter is OTLP's JSON encoding over net/http.
//
// It takes content off every span unless the code asked and the project's
// mode allows, sends to POST {endpoint}/otlp/v1/traces, waits out a
// Retry-After, and reads Telmoni-Content-Mode and the partial success from
// each answer, warning once for each attribute an answer names dropped.
type exporter struct {
	config    Config
	transport Transport
	sleep     func(time.Duration)
	logger    *slog.Logger
	url       string
	resource  []keyValue

	mu     sync.Mutex
	mode   string
	warned map[string]struct{}
}

func newExporter(cfg Config, transport Transport, sleep func(time.Duration), logger *slog.Logger) *exporter {
	return &exporter{
		config:    cfg,
		transport: transport,
		sleep:     sleep,
		logger:    logger,
		url:       cfg.Endpoint + "/otlp/v1/traces",
		resource: []keyValue{
			stringAttribute(distroName, distro),
			stringAttribute("telemetry.distro.version", Version),
			stringAttribute("telemetry.sdk.language", "go"),
			stringAttribute("telemetry.sdk.name", "opentelemetry"),
			stringAttribute("telemetry.sdk.version", sdk.Version()),
		},
		warned: map[string]struct{}{},
	}
}

// contentMode is the project's mode, as the last answer named it; "" before
// one.
func (e *exporter) contentMode() string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.mode
}

// answer is what one attempt got back.
type answer struct {
	status int
	header http.Header
	body   []byte
}

func (e *exporter) export(spans []sdktrace.ReadOnlySpan) {
	if len(spans) == 0 {
		return
	}
	body, err := json.Marshal(e.encode(spans))
	if err != nil {
		e.warnOnce("encode", "telmoni: an export could not be encoded; the batch was dropped")
		return
	}
	// A request that cannot be built, from an endpoint that is no URL, will
	// not be built on the next attempt either.
	if _, err := e.request(body); err != nil {
		e.warnOnce("request:"+errorTypeName(err),
			fmt.Sprintf("telmoni: export request could not be built (%s); the batch was dropped", errorTypeName(err)))
		return
	}
	var failure string
	for attempt := 0; attempt < attempts; attempt++ {
		got, err := e.send(body)
		if err != nil {
			failure = errorTypeName(err)
			e.wait("", attempt)
			continue
		}
		if got.status >= 200 && got.status < 300 {
			e.read(got)
			return
		}
		if got.status == http.StatusTooManyRequests || got.status == http.StatusServiceUnavailable {
			failure = fmt.Sprintf("%d %s", got.status, problemType(got.body))
			e.wait(got.header.Get("Retry-After"), attempt)
			continue
		}
		e.warnOnce("status:"+strconv.Itoa(got.status),
			fmt.Sprintf("telmoni: export refused with %d %s; the batch was dropped", got.status, problemType(got.body)))
		return
	}
	e.warnOnce("failed:"+failure,
		fmt.Sprintf("telmoni: export failed after %d attempts (%s); the batch was dropped", attempts, failure))
}

func (e *exporter) request(body []byte) (*http.Request, error) {
	req, err := http.NewRequestWithContext(context.Background(), http.MethodPost, e.url, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", "telmoni-go/"+Version)
	if e.config.APIKey != "" {
		req.Header.Set("Authorization", "Bearer "+e.config.APIKey)
	}
	return req, nil
}

func (e *exporter) send(body []byte) (answer, error) {
	req, err := e.request(body)
	if err != nil {
		return answer{}, err
	}
	resp, err := e.transport(req)
	if err != nil {
		return answer{}, err
	}
	if resp == nil {
		return answer{}, errors.New("telmoni: the transport returned no response")
	}
	got := answer{status: resp.StatusCode, header: resp.Header}
	if resp.Body != nil {
		defer resp.Body.Close()
		got.body, _ = io.ReadAll(io.LimitReader(resp.Body, answerMax))
	}
	return got, nil
}

// read takes what a kept export's answer says: the project's content mode,
// and each span refused or attribute dropped.
func (e *exporter) read(got answer) {
	if mode := got.header.Get(contentModeHeader); mode != "" {
		e.mu.Lock()
		e.mode = mode
		e.mu.Unlock()
	}
	var decoded struct {
		PartialSuccess *struct {
			ErrorMessage string `json:"errorMessage"`
		} `json:"partialSuccess"`
	}
	if len(got.body) == 0 || json.Unmarshal(got.body, &decoded) != nil || decoded.PartialSuccess == nil {
		return
	}
	if decoded.PartialSuccess.ErrorMessage != "" {
		e.note(decoded.PartialSuccess.ErrorMessage)
	}
}

func (e *exporter) note(message string) {
	for _, line := range strings.Split(message, "\n") {
		line = strings.TrimRight(line, "\r")
		if m := droppedLine.FindStringSubmatch(line); m != nil {
			attr, reason, rest := m[2], m[3], m[4]
			text := fmt.Sprintf("telmoni: the server dropped %s (%s", attr, reason)
			if rest != "" {
				text += "; " + rest
			}
			text += "); " + remedy(attr, reason)
			e.warnOnce("dropped:"+attr+":"+reason, text)
			continue
		}
		if m := refusedLine.FindStringSubmatch(line); m != nil {
			e.warnOnce("refused:"+m[2], "telmoni: the server refused a span: "+m[2])
			continue
		}
		if line == "" || strings.HasPrefix(line, "and ") {
			continue
		}
		e.warnOnce("line:"+line, "telmoni: the server said: "+line)
	}
}

// wait sleeps before the next attempt, and not after the last: Retry-After's
// seconds when the answer carries them, else 2^attempt, at most a minute.
func (e *exporter) wait(retryAfter string, attempt int) {
	if attempt+1 >= attempts {
		return
	}
	seconds := float64(int(1) << attempt)
	if retryAfter != "" {
		if n, err := strconv.Atoi(strings.TrimSpace(retryAfter)); err == nil {
			seconds = float64(n)
		}
	}
	e.sleep(max(min(time.Duration(seconds*float64(time.Second)), retryAfterMax), 0))
}

func (e *exporter) warnOnce(key, text string) {
	e.mu.Lock()
	_, seen := e.warned[key]
	if !seen {
		// Bounded: a server naming ever-new attributes would otherwise grow
		// it for the life of the process; past the cap, warnings may repeat.
		if len(e.warned) >= warnedMax {
			clear(e.warned)
		}
		e.warned[key] = struct{}{}
	}
	e.mu.Unlock()
	if !seen {
		e.logger.Warn(text)
	}
}

func remedy(attr, reason string) string {
	switch reason {
	case "content":
		// The page, not a setting to change: which modes keep content is the
		// platform's to say, and no project can choose one that does yet.
		return "the project's content mode decides whether content is kept: " + contentDocs
	case "unlisted":
		return "send it as the tag " + tagPrefix + attr + " to keep it"
	case "over_cap":
		return "shorten it to keep it"
	}
	return "see the server's answer"
}

// problemType is the problem type a failure's body opens its message with,
// up to the first colon; "" when the body carries none.
func problemType(body []byte) string {
	var decoded struct {
		Message string `json:"message"`
	}
	if json.Unmarshal(body, &decoded) != nil || !strings.HasPrefix(decoded.Message, "/errors/") {
		return ""
	}
	problem, _, _ := strings.Cut(decoded.Message, ":")
	return problem
}

// OTLP's ExportTraceServiceRequest, in its JSON encoding. Values are pointers
// so an empty string or a zero still travels, as encoding/json's omitempty
// would otherwise drop it.
type exportRequest struct {
	ResourceSpans []resourceSpans `json:"resourceSpans"`
}

type resourceSpans struct {
	Resource   otlpResource `json:"resource"`
	ScopeSpans []scopeSpans `json:"scopeSpans"`
}

type otlpResource struct {
	Attributes []keyValue `json:"attributes"`
}

type scopeSpans struct {
	Scope otlpScope  `json:"scope"`
	Spans []otlpSpan `json:"spans"`
}

type otlpScope struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

type otlpSpan struct {
	TraceID           string      `json:"traceId"`
	SpanID            string      `json:"spanId"`
	ParentSpanID      string      `json:"parentSpanId,omitempty"`
	Name              string      `json:"name"`
	Kind              int         `json:"kind"`
	StartTimeUnixNano string      `json:"startTimeUnixNano"`
	EndTimeUnixNano   string      `json:"endTimeUnixNano"`
	Attributes        []keyValue  `json:"attributes"`
	Status            *otlpStatus `json:"status,omitempty"`
}

type otlpStatus struct {
	Code int `json:"code"`
}

type keyValue struct {
	Key   string   `json:"key"`
	Value anyValue `json:"value"`
}

type anyValue struct {
	StringValue *string     `json:"stringValue,omitempty"`
	BoolValue   *bool       `json:"boolValue,omitempty"`
	IntValue    *string     `json:"intValue,omitempty"`
	DoubleValue *float64    `json:"doubleValue,omitempty"`
	ArrayValue  *arrayValue `json:"arrayValue,omitempty"`
}

type arrayValue struct {
	Values []anyValue `json:"values"`
}

func (e *exporter) encode(spans []sdktrace.ReadOnlySpan) exportRequest {
	e.mu.Lock()
	keepContent := e.config.CaptureContent && e.mode == "on"
	e.mu.Unlock()
	encoded := make([]otlpSpan, 0, len(spans))
	for _, span := range spans {
		encoded = append(encoded, encodeSpan(span, keepContent))
	}
	return exportRequest{ResourceSpans: []resourceSpans{{
		Resource: otlpResource{Attributes: e.resource},
		ScopeSpans: []scopeSpans{{
			Scope: otlpScope{Name: scope, Version: Version},
			Spans: encoded,
		}},
	}}}
}

func encodeSpan(span sdktrace.ReadOnlySpan, keepContent bool) otlpSpan {
	attributes := span.Attributes()
	encoded := make([]keyValue, 0, len(attributes))
	for _, kv := range attributes {
		if !keepContent && content[string(kv.Key)] {
			continue
		}
		encoded = append(encoded, keyValue{Key: string(kv.Key), Value: encodeValue(kv.Value)})
	}
	start, end := span.StartTime(), span.EndTime()
	if end.IsZero() {
		end = start
	}
	sc := span.SpanContext()
	out := otlpSpan{
		TraceID:           sc.TraceID().String(),
		SpanID:            sc.SpanID().String(),
		Name:              span.Name(),
		Kind:              int(trace.ValidateSpanKind(span.SpanKind())),
		StartTimeUnixNano: nanos(start),
		EndTimeUnixNano:   nanos(end),
		Attributes:        encoded,
	}
	if parent := span.Parent(); parent.IsValid() {
		out.ParentSpanID = parent.SpanID().String()
	}
	// Events and the status message are where an exception writes its
	// message and stack trace: never sent.
	if span.Status().Code == codes.Error {
		out.Status = &otlpStatus{Code: 2}
	}
	return out
}

// nanos is OTLP's JSON spelling of a time: its Unix nanoseconds, in decimal,
// as a string.
func nanos(t time.Time) string {
	if t.IsZero() {
		return "0"
	}
	return strconv.FormatInt(t.UnixNano(), 10)
}

func stringAttribute(key, value string) keyValue {
	return keyValue{Key: key, Value: stringValue(value)}
}

func stringValue(s string) anyValue  { return anyValue{StringValue: &s} }
func boolValue(b bool) anyValue      { return anyValue{BoolValue: &b} }
func doubleValue(f float64) anyValue { return anyValue{DoubleValue: &f} }
func intValue(i int64) anyValue      { s := strconv.FormatInt(i, 10); return anyValue{IntValue: &s} }
func arrayOf[T any](values []T, each func(T) anyValue) anyValue {
	out := make([]anyValue, 0, len(values))
	for _, v := range values {
		out = append(out, each(v))
	}
	return anyValue{ArrayValue: &arrayValue{Values: out}}
}

func encodeValue(v attribute.Value) anyValue {
	switch v.Type() {
	case attribute.BOOL:
		return boolValue(v.AsBool())
	case attribute.INT64:
		return intValue(v.AsInt64())
	case attribute.FLOAT64:
		return doubleValue(v.AsFloat64())
	case attribute.STRING:
		return stringValue(v.AsString())
	case attribute.BOOLSLICE:
		return arrayOf(v.AsBoolSlice(), boolValue)
	case attribute.INT64SLICE:
		return arrayOf(v.AsInt64Slice(), intValue)
	case attribute.FLOAT64SLICE:
		return arrayOf(v.AsFloat64Slice(), doubleValue)
	case attribute.STRINGSLICE:
		return arrayOf(v.AsStringSlice(), stringValue)
	}
	return stringValue(v.Emit())
}
