package telmoni_test

import (
	"context"
	"encoding/json"
	"errors"
	"reflect"
	"strings"
	"sync"
	"testing"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

var retrieved = []map[string]any{{"id": "kb-112", "score": 0.91}, {"id": "kb-087", "score": 0.84}}

func searches(t *testing.T, h *harness) []map[string]any {
	t.Helper()
	var out []map[string]any
	for _, span := range h.transport.spans(t) {
		if name := span["name"].(string); strings.HasPrefix(name, "retrieval") || name == "search_memory" {
			out = append(out, span)
		}
	}
	return out
}

// keeping is a client whose content leaves: asked for, and the project's
// mode on.
func keeping(t *testing.T, mask func(key, value string) (string, bool)) *harness {
	t.Helper()
	on := make([]any, 16)
	for i := range on {
		on[i] = contentOn()
	}
	return newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true, Mask: mask}, newFakeTransport(t, on...))
}

// decodedAs is a content attribute's JSON, decoded.
func decodedAs(t *testing.T, attributes map[string]any, key string) any {
	t.Helper()
	text, ok := attributes[key].(string)
	if !ok {
		t.Fatalf("%s was not sent: %v", key, attributes)
	}
	var out any
	if err := json.Unmarshal([]byte(text), &out); err != nil {
		t.Fatalf("%s is no JSON: %q", key, text)
	}
	return out
}

func TestARetrievalCarriesTheConventionsNames(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{UserID: "user"})
	_, found := run.Retrieval(ctx, "kb-articles", telmoni.RetrievalOptions{Query: "refunds", TopK: 5})
	found.Returned(retrieved)
	found.End()
	run.End()
	h.flush()
	spans := searches(t, h)
	if len(spans) != 1 {
		t.Fatalf("want one retrieval, got %v", names(spans))
	}
	if spans[0]["name"] != "retrieval kb-articles" || spans[0]["kind"] != float64(3) {
		t.Errorf("name %v kind %v, want retrieval kb-articles of kind 3", spans[0]["name"], spans[0]["kind"])
	}
	want := map[string]any{
		"telmoni.run_id":             run.ID(),
		"telmoni.agent":              "bot",
		"user.id":                    "user",
		"gen_ai.operation.name":      "retrieval",
		"gen_ai.data_source.id":      "kb-articles",
		"gen_ai.retrieval.top_k":     int64(5),
		"telmoni.retrieval.returned": int64(2),
	}
	if got := attributesOf(t, spans[0]); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
	if text := h.transport.text(); strings.Contains(text, "refunds") || strings.Contains(text, "kb-112") {
		t.Error("content left with capture off")
	}
}

func TestAMemorySearchGoesUnderTheMemoryNames(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, found := run.Retrieval(ctx, "preferences", telmoni.RetrievalOptions{TopK: 3, Memory: true})
	found.Returned([]string{"mem-41"})
	found.End()
	run.End()
	h.flush()
	spans := searches(t, h)
	if len(spans) != 1 || spans[0]["name"] != "search_memory" || spans[0]["kind"] != float64(3) {
		t.Fatalf("want one search_memory of kind 3, got %v", names(spans))
	}
	want := map[string]any{
		"telmoni.run_id":             run.ID(),
		"telmoni.agent":              "bot",
		"gen_ai.operation.name":      "search_memory",
		"gen_ai.memory.store.id":     "preferences",
		"gen_ai.retrieval.top_k":     int64(3),
		"gen_ai.memory.record.count": int64(1),
	}
	if got := attributesOf(t, spans[0]); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
}

// Outside a run, a retrieval does nothing, and nothing panics.
func TestNothingIsSentForARetrievalOutsideARun(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx := context.Background()
	opened, found := telmoni.RunFromContext(ctx).Retrieval(ctx, "kb", telmoni.RetrievalOptions{Query: "q", TopK: 5})
	if opened != ctx {
		t.Error("a retrieval outside a run changed the context")
	}
	var never *telmoni.Retrieval
	for _, nothing := range []*telmoni.Retrieval{found, never} {
		nothing.Returned(retrieved)
		nothing.Score("relevance", telmoni.Number(0.5), telmoni.ScoreOptions{})
		if id := nothing.SpanID(); id != "" {
			t.Errorf("SpanID() = %q", id)
		}
		nothing.Fail(errors.New("x"))
		nothing.FailWith("x")
		nothing.Cancel()
		nothing.End()
	}
	h.flush()
	if sent := len(h.transport.all()) + len(h.transport.allOpenings()); sent != 0 {
		t.Errorf("sent %d exports outside a run", sent)
	}
	expectWarnings(t, h)
}

func TestABlankSourceIsNamedByItsOperationAloneWarnedOnce(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for _, source := range []string{"", " 　"} {
		_, found := run.Retrieval(ctx, source, telmoni.RetrievalOptions{})
		found.End()
	}
	_, found := run.Retrieval(ctx, "\u0085", telmoni.RetrievalOptions{Memory: true})
	found.End()
	run.End()
	h.flush()
	spans := searches(t, h)
	if got := names(spans); !reflect.DeepEqual(got, []string{"retrieval", "retrieval", "search_memory"}) {
		t.Errorf("names = %v", got)
	}
	for _, span := range spans {
		sent := attributesOf(t, span)
		if _, ok := sent["gen_ai.data_source.id"]; ok {
			t.Errorf("%v names a data source", span["name"])
		}
		if _, ok := sent["gen_ai.memory.store.id"]; ok {
			t.Errorf("%v names a store", span["name"])
		}
	}
	expectWarnings(t, h, "telmoni: a retrieval needs its source; one without is sent named by its operation alone")
}

func TestASourcePastIngestsCapIsCutWarnedOnce(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for _, source := range []string{strings.Repeat("😀", 146), strings.Repeat("😀", 146), strings.Repeat("😀", 145)} {
		_, found := run.Retrieval(ctx, source, telmoni.RetrievalOptions{})
		found.End()
	}
	_, found := run.Retrieval(ctx, strings.Repeat("😀", 146), telmoni.RetrievalOptions{Memory: true})
	found.End()
	run.End()
	h.flush()
	spans := searches(t, h)
	cut := strings.Repeat("😀", 145)
	if got, want := names(spans), []string{"retrieval " + cut, "retrieval " + cut, "retrieval " + cut, "search_memory"}; !reflect.DeepEqual(got, want) {
		t.Errorf("names = %v", got)
	}
	for _, span := range spans[:3] {
		if got := attributesOf(t, span)["gen_ai.data_source.id"]; got != cut {
			t.Errorf("source = %v", got)
		}
	}
	if got := attributesOf(t, spans[3])["gen_ai.memory.store.id"]; got != cut {
		t.Errorf("store = %v", got)
	}
	expectWarnings(t, h, "telmoni: a retrieval's source must be up to 145 characters; it is sent cut to its first 145")
}

func TestATopKBelow0IsLeftOffWithOneWarning(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for _, topK := range []int{7, 0, -1, -2} {
		_, found := run.Retrieval(ctx, "kb", telmoni.RetrievalOptions{TopK: topK})
		found.End()
	}
	run.End()
	h.flush()
	var sent []any
	for _, span := range searches(t, h) {
		sent = append(sent, attributesOf(t, span)["gen_ai.retrieval.top_k"])
	}
	// Go has no None: a TopK of 0 is none.
	if want := []any{int64(7), nil, nil, nil}; !reflect.DeepEqual(sent, want) {
		t.Errorf("top_k = %v, want %v", sent, want)
	}
	expectWarnings(t, h, "telmoni: gen_ai.retrieval.top_k must be a whole number from 0 within 64 bits; the retrieval is sent without it")
	expectFields(t, h, map[string]any{"kind": "input"})
}

func TestReturnedRefusesAnythingButAListWhole(t *testing.T) {
	for name, given := range map[string]any{
		"text":              "kb-112",
		"bytes":             []byte("kb-112"),
		"raw JSON":          json.RawMessage(`[{"id":"kb-112"}]`),
		"an array of bytes": [2]byte{1, 2},
		"an untyped nil":    nil,
		"a map":             map[string]any{"id": "kb-112"},
		"a pointer":         &retrieved,
		"a number":          3,
	} {
		t.Run(name, func(t *testing.T) {
			h := keeping(t, nil)
			ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
			_, found := run.Retrieval(ctx, "kb", telmoni.RetrievalOptions{})
			found.Returned(given)
			found.End()
			run.End()
			h.flush()
			sent := attributesOf(t, searches(t, h)[0])
			for _, key := range []string{"telmoni.retrieval.returned", "gen_ai.retrieval.documents"} {
				if value, ok := sent[key]; ok {
					t.Errorf("%s sent as %v", key, value)
				}
			}
			expectWarnings(t, h, "telmoni: a retrieval's documents must be a list; they are neither sent nor counted")
			expectFields(t, h, map[string]any{"kind": "input"})
		})
	}
}

func TestATypedNilSliceIsAnEmptyListAndAnArrayAList(t *testing.T) {
	h := keeping(t, nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	var none []map[string]any
	_, empty := run.Retrieval(ctx, "kb", telmoni.RetrievalOptions{})
	empty.Returned(none)
	empty.End()
	_, pair := run.Retrieval(ctx, "pair", telmoni.RetrievalOptions{})
	pair.Returned([2]string{"kb-112", "kb-087"})
	pair.End()
	run.End()
	h.flush()
	spans := searches(t, h)
	nothing, two := attributesOf(t, spans[0]), attributesOf(t, spans[1])
	if nothing["telmoni.retrieval.returned"] != int64(0) || nothing["gen_ai.retrieval.documents"] != "[]" {
		t.Errorf("a nil slice sent %v", nothing)
	}
	if two["telmoni.retrieval.returned"] != int64(2) || two["gen_ai.retrieval.documents"] != `["kb-112","kb-087"]` {
		t.Errorf("an array sent %v", two)
	}
	expectWarnings(t, h)
}

func TestTheLastListWinsWholeARefusedOneLeavingTheLast(t *testing.T) {
	h := keeping(t, nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	more := append([]map[string]any{{"id": "kb-203", "score": 0.77}}, retrieved...)
	for _, given := range []struct {
		source string
		lists  []any
	}{
		{"then-refused", []any{retrieved, "kb-112"}},
		{"refused-then", []any{[]byte("kb-112"), retrieved}},
		{"twice", []any{more, retrieved[:1]}},
	} {
		_, found := run.Retrieval(ctx, given.source, telmoni.RetrievalOptions{})
		for _, list := range given.lists {
			found.Returned(list)
		}
		found.End()
	}
	run.End()
	h.flush()
	got := map[string][2]any{}
	for _, span := range searches(t, h) {
		sent := attributesOf(t, span)
		got[span["name"].(string)] = [2]any{len(decodedAs(t, sent, "gen_ai.retrieval.documents").([]any)), sent["telmoni.retrieval.returned"]}
	}
	want := map[string][2]any{
		"retrieval then-refused": {2, int64(2)},
		"retrieval refused-then": {2, int64(2)},
		"retrieval twice":        {1, int64(1)},
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("documents and counts\n got: %v\nwant: %v", got, want)
	}
}

func TestAnEmptyListIsCountedZeroAndSentAsOne(t *testing.T) {
	h := keeping(t, nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, found := run.Retrieval(ctx, "kb", telmoni.RetrievalOptions{})
	found.Returned([]any{})
	found.End()
	_, recalled := run.Retrieval(ctx, "prefs", telmoni.RetrievalOptions{Memory: true})
	recalled.Returned([]any{})
	recalled.End()
	run.End()
	h.flush()
	spans := searches(t, h)
	retrieval, memory := attributesOf(t, spans[0]), attributesOf(t, spans[1])
	if retrieval["telmoni.retrieval.returned"] != int64(0) || retrieval["gen_ai.retrieval.documents"] != "[]" {
		t.Errorf("retrieval sent %v", retrieval)
	}
	if memory["gen_ai.memory.record.count"] != int64(0) || memory["gen_ai.memory.records"] != "[]" {
		t.Errorf("memory search sent %v", memory)
	}
}

func TestTheQueryAndDocumentsLeaveOnlyUnderBothLocks(t *testing.T) {
	h := keeping(t, nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, found := run.Retrieval(ctx, "kb", telmoni.RetrievalOptions{Query: "refunds"})
	found.Returned(retrieved)
	found.End()
	_, recalled := run.Retrieval(ctx, "prefs", telmoni.RetrievalOptions{Query: map[string]string{"about": "tone"}, Memory: true})
	recalled.Returned(retrieved[:1])
	recalled.End()
	run.End()
	h.flush()
	spans := searches(t, h)
	retrieval, memory := attributesOf(t, spans[0]), attributesOf(t, spans[1])
	if retrieval["gen_ai.retrieval.query.text"] != "refunds" || memory["gen_ai.memory.query.text"] != `{"about":"tone"}` {
		t.Errorf("queries sent %v and %v", retrieval, memory)
	}
	if got := decodedAs(t, retrieval, "gen_ai.retrieval.documents"); len(got.([]any)) != 2 {
		t.Errorf("documents sent %v", got)
	}
	if got := decodedAs(t, memory, "gen_ai.memory.records"); len(got.([]any)) != 1 {
		t.Errorf("records sent %v", got)
	}
	if _, ok := memory["gen_ai.retrieval.query.text"]; ok {
		t.Error("a memory search sent a retrieval's query")
	}
}

// The count is taken from the list handed, before the mask runs.
func TestAMaskThatTakesDocumentsOutLeavesTheCount(t *testing.T) {
	h := keeping(t, func(key, value string) (string, bool) {
		if key == "gen_ai.retrieval.documents" {
			return "[]", true
		}
		return value, true
	})
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, found := run.Retrieval(ctx, "kb", telmoni.RetrievalOptions{})
	found.Returned(retrieved)
	found.End()
	run.End()
	h.flush()
	sent := attributesOf(t, searches(t, h)[0])
	if sent["gen_ai.retrieval.documents"] != "[]" || sent["telmoni.retrieval.returned"] != int64(2) {
		t.Errorf("sent %v", sent)
	}
}

// Lists handed at once leave one call's documents beside that call's count.
func TestListsReturnedAtOnceLeaveOneCallsPair(t *testing.T) {
	const rounds = 400
	h := keeping(t, nil)
	ctx, run := h.client.Run(context.Background(), "raced", telmoni.RunOptions{})
	for range rounds {
		_, found := run.Retrieval(ctx, "kb", telmoni.RetrievalOptions{})
		start := make(chan struct{})
		var wg sync.WaitGroup
		for _, size := range []int{1, 3} {
			wg.Add(1)
			go func() {
				defer wg.Done()
				<-start
				found.Returned(make([]int, size))
			}()
		}
		close(start)
		wg.Wait()
		found.End()
	}
	run.End()
	h.flush()
	spans := searches(t, h)
	if len(spans) != rounds {
		t.Fatalf("sent %d retrievals, want %d", len(spans), rounds)
	}
	astray := 0
	for _, span := range spans {
		sent := attributesOf(t, span)
		if int64(len(decodedAs(t, sent, "gen_ai.retrieval.documents").([]any))) != sent["telmoni.retrieval.returned"] {
			astray++
		}
	}
	if astray != 0 {
		t.Errorf("%d of %d retrievals sent one call's documents beside another's count", astray, rounds)
	}
}

func TestACallInsideARetrievalNestsUnderIt(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	searchCtx, found := run.Retrieval(ctx, "kb", telmoni.RetrievalOptions{})
	_, call := run.ModelCall(searchCtx, "embed-small", telmoni.ModelCallOptions{Operation: "embeddings"})
	call.End()
	found.End()
	run.End()
	h.flush()
	spans := byName(h.transport.spans(t))
	if spans["embeddings embed-small"]["parentSpanId"] != spans["retrieval kb"]["spanId"] {
		t.Error("the call inside the retrieval is not under it")
	}
}
