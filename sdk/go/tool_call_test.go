package telmoni_test

import (
	"context"
	"reflect"
	"strings"
	"testing"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

type tookTooLong struct{}

func (tookTooLong) Error() string { return "took 30 s" }

func tools(t *testing.T, h *harness) []map[string]any {
	t.Helper()
	return withPrefix(h.transport.spans(t), "execute_tool ")
}

func TestAToolCallCarriesTheConventionsNames(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{UserID: "user"})
	_, tool := run.ToolCall(ctx, "search", telmoni.ToolCallOptions{Arguments: map[string]string{"q": "weather"}})
	tool.Returned(map[string]int{"temp": 21})
	tool.End()
	run.End()
	h.flush()
	spans := tools(t, h)
	if len(spans) != 1 {
		t.Fatalf("want one tool call, got %v", names(spans))
	}
	span := spans[0]
	if span["name"] != "execute_tool search" || span["kind"] != float64(1) {
		t.Errorf("name %v kind %v, want execute_tool search of kind 1", span["name"], span["kind"])
	}
	want := map[string]any{
		"telmoni.run_id":        run.ID(),
		"telmoni.agent":         "bot",
		"user.id":               "user",
		"gen_ai.operation.name": "execute_tool",
		"gen_ai.tool.name":      "search",
	}
	if got := attributesOf(t, span); !reflect.DeepEqual(got, want) {
		t.Errorf("attributes\n got: %v\nwant: %v", got, want)
	}
	for _, secret := range []string{"weather", "temp"} {
		if strings.Contains(h.transport.text(), secret) {
			t.Errorf("%q left the process", secret)
		}
	}
}

func TestAFailingToolRecordsItsErrorType(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, search := run.ToolCall(ctx, "search", telmoni.ToolCallOptions{})
	search.Fail(tookTooLong{})
	search.End()
	_, lookup := run.ToolCall(ctx, "lookup", telmoni.ToolCallOptions{})
	lookup.FailWith("NotFound")
	lookup.End()
	run.End()
	h.flush()
	spans := byName(tools(t, h))
	if got := attributesOf(t, spans["execute_tool search"])["error.type"]; got != "telmoni_test.tookTooLong" {
		t.Errorf("error.type = %v", got)
	}
	if !reflect.DeepEqual(spans["execute_tool search"]["status"], map[string]any{"code": float64(2)}) {
		t.Errorf("status = %v, want {code: 2}", spans["execute_tool search"]["status"])
	}
	if got := attributesOf(t, spans["execute_tool lookup"])["error.type"]; got != "NotFound" {
		t.Errorf("error.type = %v", got)
	}
	if strings.Contains(h.transport.text(), "took 30 s") {
		t.Error("the error's message was sent")
	}
}

func TestToolContentTravelsWhenAskedAndAllowed(t *testing.T) {
	on := contentOn()
	h := newHarness(t, telmoni.Config{APIKey: "k", CaptureContent: true}, newFakeTransport(t, on, on, on, on))
	_, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	run.End()
	h.flush()
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	_, search := run.ToolCall(ctx, "search", telmoni.ToolCallOptions{Arguments: map[string]string{"q": "weather"}})
	search.Returned(map[string]int{"temp": 21})
	search.End()
	_, echo := run.ToolCall(ctx, "echo", telmoni.ToolCallOptions{Arguments: "plain"})
	echo.Returned(nil)
	echo.End()
	run.End()
	h.flush()
	spans := byName(tools(t, h))
	searchAttributes := attributesOf(t, spans["execute_tool search"])
	if searchAttributes["gen_ai.tool.call.arguments"] != `{"q":"weather"}` {
		t.Errorf("gen_ai.tool.call.arguments = %v, want JSON", searchAttributes["gen_ai.tool.call.arguments"])
	}
	if searchAttributes["gen_ai.tool.call.result"] != `{"temp":21}` {
		t.Errorf("gen_ai.tool.call.result = %v, want JSON", searchAttributes["gen_ai.tool.call.result"])
	}
	echoAttributes := attributesOf(t, spans["execute_tool echo"])
	if echoAttributes["gen_ai.tool.call.arguments"] != "plain" {
		t.Errorf("gen_ai.tool.call.arguments = %v, want the string as given", echoAttributes["gen_ai.tool.call.arguments"])
	}
	if echoAttributes["gen_ai.tool.call.result"] != "null" {
		t.Errorf("gen_ai.tool.call.result = %v, want null", echoAttributes["gen_ai.tool.call.result"])
	}
}

func TestAFunctionWrappedInAToolCall(t *testing.T) {
	h := newHarness(t, testConfig(), nil)
	ctx, run := h.client.Run(context.Background(), "bot", telmoni.RunOptions{})
	search := func(ctx context.Context, q string) string {
		_, tool := telmoni.RunFromContext(ctx).ToolCall(ctx, "search", telmoni.ToolCallOptions{Arguments: q})
		defer tool.End()
		runes := []rune(q)
		for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
			runes[i], runes[j] = runes[j], runes[i]
		}
		return string(runes)
	}
	if got := search(ctx, "abc"); got != "cba" {
		t.Errorf("search(abc) = %q", got)
	}
	run.End()
	h.flush()
	if got := len(tools(t, h)); got != 1 {
		t.Errorf("want one tool call, got %d", got)
	}
}
