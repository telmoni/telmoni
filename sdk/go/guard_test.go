package telmoni_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

// guardAnswer is one answer to a call of sdk/fixtures/guard.json.
type guardAnswer struct {
	ResponseModel string `json:"response_model"`
	Usage         *struct {
		InputTokens           int      `json:"input_tokens"`
		OutputTokens          int      `json:"output_tokens"`
		CacheReadInputTokens  int      `json:"cache_read_input_tokens"`
		CacheWriteInputTokens int      `json:"cache_write_input_tokens"`
		Cost                  *float64 `json:"cost"`
	} `json:"usage"`
}

// guardFixture is sdk/fixtures/guard.json.
type guardFixture struct {
	Prices []telmoni.Price `json:"prices"`
	Calls  []struct {
		guardAnswer
		Name     string        `json:"name"`
		Provider string        `json:"provider"`
		Model    string        `json:"model"`
		Answers  []guardAnswer `json:"answers"`
		Counted  float64       `json:"counted"`
	} `json:"calls"`
	Runs []struct {
		Name     string            `json:"name"`
		MaxSteps int               `json:"max_steps"`
		MaxSpend float64           `json:"max_spend"`
		Ops      []json.RawMessage `json:"ops"`
		Refused  *struct {
			At    int     `json:"at"`
			Cap   string  `json:"cap"`
			Limit float64 `json:"limit"`
			Used  float64 `json:"used"`
		} `json:"refused"`
		Spent float64 `json:"spent"`
	} `json:"runs"`
	Refusals []struct {
		telmoni.CapExceededError
		Says string `json:"says"`
	} `json:"refusals"`
}

func loadGuard(t *testing.T) guardFixture {
	t.Helper()
	var fixture guardFixture
	if err := json.Unmarshal(loadFixture(t, "guard.json"), &fixture); err != nil {
		t.Fatalf("decoding guard.json: %v", err)
	}
	return fixture
}

func near(a, b float64) bool { return math.Abs(a-b) <= 1e-12 }

// call opens fixture.Calls[n] under run and answers it as the fixture does,
// answering the context it opened with.
func (f guardFixture) call(ctx context.Context, run *telmoni.Run, n int) context.Context {
	spec := f.Calls[n]
	callCtx, call := run.ModelCall(ctx, spec.Model, telmoni.ModelCallOptions{Provider: spec.Provider})
	answers := spec.Answers
	if answers == nil {
		answers = []guardAnswer{spec.guardAnswer}
	}
	for _, given := range answers {
		answer := telmoni.Answer{ResponseModel: given.ResponseModel}
		if u := given.Usage; u != nil {
			answer.Usage = &telmoni.Usage{
				InputTokens:           u.InputTokens,
				OutputTokens:          u.OutputTokens,
				CacheReadInputTokens:  u.CacheReadInputTokens,
				CacheWriteInputTokens: u.CacheWriteInputTokens,
				Cost:                  u.Cost,
			}
		}
		call.Answered(answer)
	}
	call.End()
	return callCtx
}

// perform does op under run, answering the context it opened with.
func (f guardFixture) perform(t *testing.T, ctx context.Context, run *telmoni.Run, op json.RawMessage) context.Context {
	t.Helper()
	var word string
	if json.Unmarshal(op, &word) == nil {
		switch word {
		case "step":
			opened, step := run.Step(ctx, "step")
			step.End()
			return opened
		case "tool_call":
			opened, tool := run.ToolCall(ctx, "lookup", telmoni.ToolCallOptions{})
			tool.End()
			return opened
		}
		t.Fatalf("an op the fixture has no word for: %s", word)
	}
	var call struct {
		ModelCall int `json:"model_call"`
	}
	if err := json.Unmarshal(op, &call); err != nil {
		t.Fatalf("an op: %s", op)
	}
	return f.call(ctx, run, call.ModelCall)
}

func TestTheGuardCountsEachCallAsTheServerCountsIt(t *testing.T) {
	fixture := loadGuard(t)
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret(testKey), Prices: fixture.Prices}, nil)
	for n, spec := range fixture.Calls {
		ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{})
		fixture.call(ctx, run, n)
		// Exactly: a multiply fused into its add, as arm64 may, ends a bit off.
		if got := run.Spent(); got != spec.Counted {
			t.Errorf("%s: counted %v, want %v", spec.Name, got, spec.Counted)
		}
		run.End()
	}
}

func TestARunPastACapIsRefusedWhatItOpensNext(t *testing.T) {
	fixture := loadGuard(t)
	for _, spec := range fixture.Runs {
		t.Run(spec.Name, func(t *testing.T) {
			h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret(testKey), Prices: fixture.Prices}, nil)
			ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{MaxSteps: spec.MaxSteps, MaxSpend: spec.MaxSpend})
			performed := len(spec.Ops)
			for at, op := range spec.Ops {
				opened := fixture.perform(t, ctx, run, op)
				if spec.Refused == nil || at != spec.Refused.At {
					if opened.Err() != nil {
						t.Fatalf("op %d was refused: %v", at, context.Cause(opened))
					}
					continue
				}
				var refusal *telmoni.CapExceededError
				if opened.Err() == nil || !errors.As(context.Cause(opened), &refusal) {
					t.Fatalf("op %d opened past its cap: %v", at, context.Cause(opened))
				}
				if refusal.Cap != spec.Refused.Cap || !near(refusal.Limit, spec.Refused.Limit) || !near(refusal.Used, spec.Refused.Used) {
					t.Errorf("refused as %+v, want %+v", *refusal, *spec.Refused)
				}
				if !errors.Is(run.Err(), refusal) {
					t.Errorf("the run's Err is %v, want its refusal", run.Err())
				}
				performed = at
				break
			}
			if !near(run.Spent(), spec.Spent) {
				t.Errorf("spent %g, want %g", run.Spent(), spec.Spent)
			}
			run.End()
			h.flush()
			opened := 0
			for _, span := range h.transport.spans(t) {
				if span["name"] != "invoke_agent guarded" {
					opened++
				}
			}
			if opened != performed {
				t.Errorf("%d spans opened, want %d: a refused op opens nothing", opened, performed)
			}
		})
	}
}

func TestARefusalSaysWhatTheOtherSDKsSay(t *testing.T) {
	for _, spec := range loadGuard(t).Refusals {
		if got := spec.CapExceededError.Error(); got != spec.Says {
			t.Errorf("says %q, want %q", got, spec.Says)
		}
	}
}

func TestARefusedOpeningsContextIsCancelledWithTheCap(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{MaxSteps: 1})
	_, first := run.Step(ctx, "first")
	first.End()
	stepCtx, second := run.Step(ctx, "second")
	second.End()
	if !errors.Is(stepCtx.Err(), context.Canceled) {
		t.Fatalf("the refused step's context is %v, want cancelled", stepCtx.Err())
	}
	if !strings.Contains(context.Cause(stepCtx).Error(), "its step cap of 1") {
		t.Errorf("cause %v", context.Cause(stepCtx))
	}
	if telmoni.RunFromContext(stepCtx) != run {
		t.Error("the refused context lost its run")
	}
	if ctx.Err() != nil {
		t.Error("the run's own context was cancelled")
	}
	run.End()
}

func TestNothingOpensUnderARefusedOpeningsContext(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{MaxSteps: 1})
	_, first := run.Step(ctx, "first")
	first.End()
	stepCtx, second := run.Step(ctx, "second")
	second.End()
	callCtx, call := run.ModelCall(stepCtx, "m", telmoni.ModelCallOptions{})
	call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 10}})
	call.End()
	toolCtx, tool := run.ToolCall(stepCtx, "lookup", telmoni.ToolCallOptions{})
	tool.End()
	for _, opened := range []context.Context{callCtx, toolCtx} {
		if context.Cause(opened) != context.Cause(stepCtx) || telmoni.RunFromContext(opened) != run {
			t.Errorf("opened under %v, want the step's refusal and its run", context.Cause(opened))
		}
	}
	run.End()
	h.flush()
	if got := sortedNames(h.transport.spans(t)); !slices.Equal(got, []string{"first", "invoke_agent guarded"}) {
		t.Errorf("spans sent: %v", got)
	}
}

// A run failed with the cancellation of a refused opening fails as the refusal.
func TestARefusalEscapingTheRunFailsItWithItsType(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{MaxSteps: 1})
	_, first := run.Step(ctx, "first")
	first.End()
	stepCtx, second := run.Step(ctx, "second")
	second.End()
	run.Fail(fmt.Errorf("asking the model: %w", stepCtx.Err()))
	run.End()
	h.flush()
	span := runSpan(t, h)
	attributes := attributesOf(t, span)
	if attributes["error.type"] != "telmoni.CapExceededError" || attributes["telmoni.cancelled"] != nil {
		t.Errorf("error.type %v, telmoni.cancelled %v", attributes["error.type"], attributes["telmoni.cancelled"])
	}
	if !reflect.DeepEqual(span["status"], map[string]any{"code": float64(2)}) {
		t.Errorf("status = %v, want {code: 2}", span["status"])
	}
}

// A step or call failed with that cancellation fails as the refusal too.
func TestARefusalPassingThroughAStepOrCallFailsItWithItsType(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{MaxSteps: 1})
	stepCtx, step := run.Step(ctx, "outer")
	toolCtx, tool := run.ToolCall(stepCtx, "lookup", telmoni.ToolCallOptions{})
	callCtx, call := run.ModelCall(toolCtx, "m", telmoni.ModelCallOptions{})
	innerCtx, _ := run.Step(callCtx, "inner")
	for _, opened := range []interface {
		Fail(error)
		End()
	}{call, tool, step} {
		opened.Fail(fmt.Errorf("asking the model: %w", innerCtx.Err()))
		opened.End()
	}
	run.End()
	h.flush()
	var failed []string
	for _, span := range h.transport.spans(t) {
		attributes := attributesOf(t, span)
		if attributes["error.type"] == "telmoni.CapExceededError" && attributes["telmoni.cancelled"] == nil {
			failed = append(failed, span["name"].(string))
		}
	}
	slices.Sort(failed)
	if want := []string{"chat m", "execute_tool lookup", "outer"}; !slices.Equal(failed, want) {
		t.Errorf("failed with the refusal: %v, want %v", failed, want)
	}
}

func TestATableNameIsPricedByTheTableTheReleaseCarries(t *testing.T) {
	var file struct {
		Prices []telmoni.Price `json:"prices"`
	}
	raw, err := os.ReadFile("model_prices.json")
	if err != nil || json.Unmarshal(raw, &file) != nil {
		t.Fatalf("reading the price file: %v", err)
	}
	i := slices.IndexFunc(file.Prices, func(p telmoni.Price) bool { return p.Provider == "openai" && p.Model == "gpt-4o-mini" })
	if i < 0 {
		t.Fatal("the price file prices no gpt-4o-mini")
	}
	var spent []float64
	for _, prices := range [][]telmoni.Price{nil, {file.Prices[i]}} {
		h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret(testKey), Prices: prices}, nil)
		ctx, run := h.client.Run(context.Background(), "priced", telmoni.RunOptions{})
		_, call := run.ModelCall(ctx, "gpt-4o-mini", telmoni.ModelCallOptions{Provider: "openai"})
		call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 10_000, OutputTokens: 2_000, CacheReadInputTokens: 4_000}})
		call.End()
		spent = append(spent, run.Spent())
		run.End()
	}
	if spent[0] <= 0 || spent[0] != spent[1] {
		t.Errorf("the table priced it at %g, the same row handed at %g", spent[0], spent[1])
	}
}

func TestACallAnsweredAgainCountsWhatItsSpanHolds(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret(testKey), Prices: []telmoni.Price{{Provider: "p", Model: "m", Input: 1e-6, Output: 1e-5}}}, nil)
	ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{})
	_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{Provider: "p"})
	// Answers given at once, from goroutines of the agent's own.
	var wg sync.WaitGroup
	for range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 1000}})
		}()
	}
	wg.Wait()
	// A stream's counts, answered as they arrive: Go sends a Usage's four
	// counts whole, so the second's input of 0 is what the span holds.
	call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 2000}})
	call.Answered(telmoni.Answer{Usage: &telmoni.Usage{OutputTokens: 100}})
	call.End()
	spent := run.Spent()
	run.End()
	h.flush()
	var held map[string]any
	for _, span := range h.transport.spans(t) {
		if span["name"] == "chat m" {
			held = attributesOf(t, span)
		}
	}
	input, _ := held["gen_ai.usage.input_tokens"].(int64)
	output, _ := held["gen_ai.usage.output_tokens"].(int64)
	if !near(spent, float64(input)*1e-6+float64(output)*1e-5) || !near(spent, 0.001) {
		t.Errorf("spent %g, the span holding %d in and %d out", spent, input, output)
	}
}

func TestAnAnswerAfterItsCallEndedCountsNothingWithOneWarning(t *testing.T) {
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret(testKey), Prices: []telmoni.Price{{Provider: "p", Model: "m", Input: 1e-6}}}, nil)
	ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{})
	_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{Provider: "p"})
	call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 1000}})
	call.End()
	for range 2 {
		call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 5000}})
	}
	spent := run.Spent()
	run.End()
	h.flush()
	var held map[string]any
	for _, span := range h.transport.spans(t) {
		if span["name"] == "chat m" {
			held = attributesOf(t, span)
		}
	}
	if held["gen_ai.usage.input_tokens"] != int64(1000) || !near(spent, 0.001) {
		t.Errorf("spent %g, the span holding %v", spent, held["gen_ai.usage.input_tokens"])
	}
	want := []string{"telmoni: a model call must be answered before it ends; an answer given after is not sent and counts nothing toward the run's spend"}
	if got := h.logs.warnings(); !slices.Equal(got, want) {
		t.Errorf("warned %q", got)
	}
}

func TestTheReleaseCarriesThePriceFileWhole(t *testing.T) {
	carried, err := os.ReadFile("model_prices.json")
	if err != nil {
		t.Fatal(err)
	}
	core, err := os.ReadFile(filepath.Join("..", "..", "crates", "telemetry", "model_prices.json"))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(carried, core) {
		t.Error("model_prices.json is not the core's: make prices writes it")
	}
}

func TestACallWithNeitherUnderASpendCapIsWarnedOfOnce(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{MaxSpend: 1})
	for range 3 {
		_, call := run.ModelCall(ctx, "my-model", telmoni.ModelCallOptions{Provider: "vllm"})
		call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 10}})
		call.End()
	}
	run.End()
	ctx, other := h.client.Run(context.Background(), "unguarded", telmoni.RunOptions{})
	_, call := other.ModelCall(ctx, "other-model", telmoni.ModelCallOptions{Provider: "vllm"})
	call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 10}})
	call.End()
	other.End()
	var warned []string
	for _, w := range h.logs.warnings() {
		if strings.Contains(w, "counts nothing") {
			warned = append(warned, w)
		}
	}
	want := []string{"telmoni: no price for my-model (vllm), which reported no cost; it counts nothing toward the run's spend cap"}
	if !slices.Equal(warned, want) {
		t.Errorf("warned %q", warned)
	}
}

// spendReader is a logger's handler that reads the run's spend from another
// goroutine on each warning, and notes whether the read was held up.
type spendReader struct {
	run     atomic.Pointer[telmoni.Run]
	stalled atomic.Bool
}

func (s *spendReader) Enabled(context.Context, slog.Level) bool { return true }
func (s *spendReader) WithAttrs([]slog.Attr) slog.Handler       { return s }
func (s *spendReader) WithGroup(string) slog.Handler            { return s }

func (s *spendReader) Handle(context.Context, slog.Record) error {
	if run := s.run.Load(); run != nil {
		read := make(chan struct{})
		go func() { run.Spent(); close(read) }()
		select {
		case <-read:
		case <-time.After(waitTimeout):
			s.stalled.Store(true)
		}
	}
	return nil
}

// The application's logger runs with the run's lock released.
func TestALoggerReadingTheRunsSpendIsNeverHeldUp(t *testing.T) {
	reader := &spendReader{}
	h := newHarness(t, testConfig(), nil, telmoni.WithLogger(slog.New(reader)))
	ctx, run := h.client.Run(context.Background(), "guarded", telmoni.RunOptions{MaxSpend: 1})
	reader.run.Store(run)
	_, call := run.ModelCall(ctx, "my-model", telmoni.ModelCallOptions{Provider: "vllm"})
	call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 10}})
	call.End()
	run.End()
	if reader.stalled.Load() {
		t.Error("the logger ran under the run's lock")
	}
}

func TestACapThatIsNoCapIsNoneWithOneWarning(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	for _, opts := range []telmoni.RunOptions{{MaxSteps: -1}, {MaxSpend: -1}, {MaxSpend: math.NaN()}, {MaxSpend: math.Inf(1)}} {
		ctx, run := h.client.Run(context.Background(), "guarded", opts)
		for range 3 {
			stepCtx, step := run.Step(ctx, "step")
			if stepCtx.Err() != nil {
				t.Errorf("%+v: a step was refused", opts)
			}
			step.End()
		}
		run.End()
	}
	if got := len(h.logs.warnings()); got != 2 {
		t.Errorf("%d warnings, want one for MaxSteps and one for MaxSpend: %q", got, h.logs.warnings())
	}
}

func TestAPriceHandedIsANameAndNumbersFrom0(t *testing.T) {
	bad := -1.0
	for _, price := range []telmoni.Price{
		{Model: "m", Input: 1e-6, Output: 1e-6},
		{Provider: "p", Input: 1e-6, Output: 1e-6},
		{Provider: "p", Model: "m", Input: -1e-6, Output: 1e-6},
		{Provider: "p", Model: "m", Input: 1e-6, Output: math.NaN()},
		{Provider: "p", Model: "m", Input: 1e-6, Output: 1e-6, CacheRead: &bad},
	} {
		if _, err := telmoni.New(telmoni.Config{Prices: []telmoni.Price{price}}); err == nil {
			t.Errorf("%+v was taken", price)
		}
	}
}

func TestACostGoesAsADoubleAndOneThatIsNoCostIsLeftOff(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	for _, cost := range []float64{3, 0.0123, -0.5, math.NaN(), math.Inf(1)} {
		_, call := run.ModelCall(ctx, "m", telmoni.ModelCallOptions{})
		call.Answered(telmoni.Answer{Usage: &telmoni.Usage{InputTokens: 1, Cost: &cost}})
		call.End()
	}
	run.End()
	h.flush()
	var sent []any
	for _, span := range h.transport.spans(t) {
		if span["name"] == "chat m" {
			sent = append(sent, attributesOf(t, span)["gen_ai.usage.cost"])
		}
	}
	if !slices.Equal(sent, []any{3.0, 0.0123, nil, nil, nil}) {
		t.Errorf("sent %v", sent)
	}
	want := []string{"telmoni: gen_ai.usage.cost must be a finite number of US dollars from 0; the call is sent without it"}
	if got := h.logs.warnings(); !slices.Equal(got, want) {
		t.Errorf("warned %q", got)
	}
}
