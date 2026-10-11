package telmoni_test

import (
	"context"
	"errors"
	"log"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

// lookUp stands for a tool the agent calls.
func lookUp(context.Context, string) (string, error) {
	return "", errors.New("took 30 s")
}

func Example() {
	// Reads TELMONI_ENDPOINT, TELMONI_API_KEY (a key with the ingest scope)
	// and TELMONI_ORG, and nothing else.
	client, err := telmoni.NewFromEnv()
	if err != nil {
		log.Fatal(err)
	}
	defer client.Shutdown(context.Background())

	ctx, run := client.Run(context.Background(), "support-bot", telmoni.RunOptions{
		UserID:         "user_42",
		ConversationID: "conv_7",
		Tags:           map[string]string{"env": "prod"},
		Bounds:         &telmoni.Bounds{StallAfterS: 60, MaxSteps: 10, MaxCostUSD: 1.5},
	})
	defer run.End()

	stepCtx, step := run.Step(ctx, "plan")
	_, call := run.ModelCall(stepCtx, "gpt-4o-mini", telmoni.ModelCallOptions{Provider: "openai"})
	// The provider's call goes here. Its reply is recorded without its text
	// unless CaptureContent is on and the project's content mode allows.
	call.Answered(telmoni.Answer{
		ResponseModel: "gpt-4o-mini-2024-07-18",
		Usage:         &telmoni.Usage{InputTokens: 12, OutputTokens: 7},
	})
	call.End()
	step.End()

	_, tool := run.ToolCall(ctx, "search", telmoni.ToolCallOptions{Arguments: map[string]string{"q": "weather"}})
	result, err := lookUp(ctx, "weather")
	if err != nil {
		tool.Fail(err) // error.type alone, never the message
	} else {
		tool.Returned(result)
	}
	tool.End()
}

func ExampleRunFromContext() {
	// Deep in a call stack that has only the context, the run is found from
	// it; outside a run, the handle opened does nothing.
	search := func(ctx context.Context, q string) (string, error) {
		_, tool := telmoni.RunFromContext(ctx).ToolCall(ctx, "search", telmoni.ToolCallOptions{})
		defer tool.End()
		result, err := lookUp(ctx, q)
		if err != nil {
			tool.Fail(err)
		}
		return result, err
	}
	_, _ = search(context.Background(), "weather")
}
