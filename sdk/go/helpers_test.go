package telmoni_test

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

const (
	testKey     = "telmoni_test_key"
	waitTimeout = 5 * time.Second
)

var fixturesDir = filepath.Join("..", "fixtures")

func loadFixture(t *testing.T, name string) []byte {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(fixturesDir, name))
	if err != nil {
		t.Fatalf("reading the fixture %s: %v", name, err)
	}
	return raw
}

// scripted is one answer the fake transport gives: an answers/*.json fixture,
// or one a test makes.
type scripted struct {
	status  int
	headers map[string]string
	body    []byte
}

func answerFrom(t *testing.T, name string) scripted {
	t.Helper()
	var fixture struct {
		Status  int               `json:"status"`
		Headers map[string]string `json:"headers"`
		Body    json.RawMessage   `json:"body"`
	}
	if err := json.Unmarshal(loadFixture(t, name), &fixture); err != nil {
		t.Fatalf("decoding the fixture %s: %v", name, err)
	}
	return scripted{status: fixture.Status, headers: fixture.Headers, body: fixture.Body}
}

func kept(t *testing.T) scripted {
	t.Helper()
	return answerFrom(t, "answers/kept.json")
}

type request struct {
	url    string
	header http.Header
	body   []byte
}

// fakeTransport records every export and answers from a script, then with
// kept.
type fakeTransport struct {
	mu       sync.Mutex
	received *sync.Cond
	fallback scripted
	answers  []any
	requests []request
}

func newFakeTransport(t *testing.T, answers ...any) *fakeTransport {
	t.Helper()
	f := &fakeTransport{fallback: kept(t), answers: answers}
	f.received = sync.NewCond(&f.mu)
	return f
}

func (f *fakeTransport) send(req *http.Request) (*http.Response, error) {
	body, err := io.ReadAll(req.Body)
	if err != nil {
		return nil, err
	}
	f.mu.Lock()
	f.requests = append(f.requests, request{url: req.URL.String(), header: req.Header.Clone(), body: body})
	var next any = f.fallback
	if len(f.answers) > 0 {
		next, f.answers = f.answers[0], f.answers[1:]
	}
	f.received.Broadcast()
	f.mu.Unlock()
	if err, ok := next.(error); ok {
		return nil, err
	}
	a := next.(scripted)
	header := http.Header{}
	for key, value := range a.headers {
		header.Set(key, value)
	}
	return &http.Response{
		StatusCode: a.status,
		Status:     strconv.Itoa(a.status) + " " + http.StatusText(a.status),
		Header:     header,
		Body:       io.NopCloser(bytes.NewReader(a.body)),
		Request:    req,
	}, nil
}

// waitFor is true once count requests have been received, false after
// timeout.
func (f *fakeTransport) waitFor(count int, timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	// Cond has no timed wait; a broadcast at the deadline ends this one.
	timer := time.AfterFunc(timeout, func() {
		f.mu.Lock()
		f.received.Broadcast()
		f.mu.Unlock()
	})
	defer timer.Stop()
	f.mu.Lock()
	defer f.mu.Unlock()
	for len(f.requests) < count {
		if !time.Now().Before(deadline) {
			return false
		}
		f.received.Wait()
	}
	return true
}

func (f *fakeTransport) all() []request {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]request(nil), f.requests...)
}

func (f *fakeTransport) bodies(t *testing.T) []map[string]any {
	t.Helper()
	var out []map[string]any
	for _, req := range f.all() {
		var body map[string]any
		if err := json.Unmarshal(req.body, &body); err != nil {
			t.Fatalf("decoding an export: %v", err)
		}
		out = append(out, body)
	}
	return out
}

func (f *fakeTransport) spans(t *testing.T) []map[string]any {
	t.Helper()
	var out []map[string]any
	for _, body := range f.bodies(t) {
		out = append(out, spansIn(body)...)
	}
	return out
}

// text is every export's bytes together, for asserting what never left.
func (f *fakeTransport) text() string {
	var b strings.Builder
	for _, req := range f.all() {
		b.Write(req.body)
	}
	return b.String()
}

func spansIn(body map[string]any) []map[string]any {
	var out []map[string]any
	for _, rs := range body["resourceSpans"].([]any) {
		for _, ss := range rs.(map[string]any)["scopeSpans"].([]any) {
			for _, span := range ss.(map[string]any)["spans"].([]any) {
				out = append(out, span.(map[string]any))
			}
		}
	}
	return out
}

// attributesOf reads a span's typed attributes: an intValue as int64, a
// doubleValue as float64, the rest as given.
func attributesOf(t *testing.T, span map[string]any) map[string]any {
	t.Helper()
	out := map[string]any{}
	for _, raw := range span["attributes"].([]any) {
		attr := raw.(map[string]any)
		key := attr["key"].(string)
		value := attr["value"].(map[string]any)
		if s, ok := value["intValue"]; ok {
			n, err := strconv.ParseInt(s.(string), 10, 64)
			if err != nil {
				t.Fatalf("%s: intValue %v is not a decimal string", key, s)
			}
			out[key] = n
			continue
		}
		for _, v := range value {
			out[key] = v
		}
	}
	return out
}

// attributesUnder is the attributes whose keys start with prefix.
func attributesUnder(attributes map[string]any, prefix string) map[string]any {
	out := map[string]any{}
	for key, value := range attributes {
		if strings.HasPrefix(key, prefix) {
			out[key] = value
		}
	}
	return out
}

// numberedTags is n tags keyed k000, k001 and on, so their order by key is
// their number's.
func numberedTags(n int) map[string]string {
	tags := map[string]string{}
	for i := range n {
		tags[fmt.Sprintf("k%03d", i)] = "v"
	}
	return tags
}

// droppedTags is a tag of each kind ingest drops, each sorting before k000 so
// it would crowd a numbered tag out of the 32.
func droppedTags() map[string]string {
	return map[string]string{"": "v", strings.Repeat("a", 65): "v", "b": strings.Repeat("v", 257)}
}

// tagsOf is the keys of the tags a span carries, in order.
func tagsOf(attributes map[string]any) []string {
	var out []string
	for key := range attributes {
		if tag, ok := strings.CutPrefix(key, "telmoni.tag."); ok {
			out = append(out, tag)
		}
	}
	slices.Sort(out)
	return out
}

func names(spans []map[string]any) []string {
	out := make([]string, 0, len(spans))
	for _, span := range spans {
		out = append(out, span["name"].(string))
	}
	return out
}

func sortedNames(spans []map[string]any) []string {
	out := names(spans)
	sort.Strings(out)
	return out
}

func byName(spans []map[string]any) map[string]map[string]any {
	out := map[string]map[string]any{}
	for _, span := range spans {
		out[span["name"].(string)] = span
	}
	return out
}

func withPrefix(spans []map[string]any, prefix string) []map[string]any {
	var out []map[string]any
	for _, span := range spans {
		if strings.HasPrefix(span["name"].(string), prefix) {
			out = append(out, span)
		}
	}
	return out
}

// logSink is the slog handler the tests give the client: it keeps every
// warning's message.
type logSink struct {
	mu       sync.Mutex
	messages []string
}

func (s *logSink) Enabled(context.Context, slog.Level) bool { return true }

func (s *logSink) Handle(_ context.Context, r slog.Record) error {
	if r.Level >= slog.LevelWarn {
		s.mu.Lock()
		s.messages = append(s.messages, r.Message)
		s.mu.Unlock()
	}
	return nil
}

func (s *logSink) WithAttrs([]slog.Attr) slog.Handler { return s }
func (s *logSink) WithGroup(string) slog.Handler      { return s }

func (s *logSink) warnings() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]string(nil), s.messages...)
}

// sleepLog is the sleep the tests inject: it records, and waits for nothing.
type sleepLog struct {
	mu        sync.Mutex
	durations []time.Duration
}

func (l *sleepLog) sleep(d time.Duration) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.durations = append(l.durations, d)
}

func (l *sleepLog) all() []time.Duration {
	l.mu.Lock()
	defer l.mu.Unlock()
	return append([]time.Duration(nil), l.durations...)
}

// harness is one client with its fake transport, recorded sleeps and captured
// warnings, shut down when the test ends.
type harness struct {
	t         *testing.T
	client    *telmoni.Client
	transport *fakeTransport
	sleeps    *sleepLog
	logs      *logSink
}

func newHarness(t *testing.T, cfg telmoni.Config, transport *fakeTransport, opts ...telmoni.Option) *harness {
	t.Helper()
	if transport == nil {
		transport = newFakeTransport(t)
	}
	h := &harness{t: t, transport: transport, sleeps: &sleepLog{}, logs: &logSink{}}
	all := []telmoni.Option{
		telmoni.WithTransport(transport.send),
		telmoni.WithSleep(h.sleeps.sleep),
		telmoni.WithLogger(slog.New(h.logs)),
	}
	client, err := telmoni.New(cfg, append(all, opts...)...)
	if err != nil {
		t.Fatalf("building the client: %v", err)
	}
	h.client = client
	t.Cleanup(h.shutdown)
	return h
}

func testConfig() telmoni.Config {
	return telmoni.Config{APIKey: testKey}
}

func (h *harness) flush() {
	h.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), waitTimeout)
	defer cancel()
	if err := h.client.Flush(ctx); err != nil {
		h.t.Fatalf("flushing: %v", err)
	}
}

func (h *harness) shutdown() {
	h.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), waitTimeout)
	defer cancel()
	if err := h.client.Shutdown(ctx); err != nil {
		h.t.Errorf("shutting down: %v", err)
	}
}

// mustNew builds a client on the real transport, for tests that send nothing.
func mustNew(t *testing.T, cfg telmoni.Config) *telmoni.Client {
	t.Helper()
	client, err := telmoni.New(cfg)
	if err != nil {
		t.Fatalf("building the client: %v", err)
	}
	t.Cleanup(func() { _ = client.Shutdown(context.Background()) })
	return client
}
