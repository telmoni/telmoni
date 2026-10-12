package telmoni_test

import (
	"context"
	"errors"
	"log"
	"time"

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
		UserID:    "user_42",
		SessionID: "conv_7",
		Tags:      map[string]string{"env": "prod"},
		Bounds:    &telmoni.Bounds{StallAfterS: 60, MaxSteps: 10, MaxCostUSD: 1.5},
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

func ExampleClient_RecordRun() {
	client, err := telmoni.NewFromEnv()
	if err != nil {
		log.Fatal(err)
	}
	defer client.Shutdown(context.Background())

	// A run read back from the agent's own log, under the id kept with it:
	// sent again, it is the same bytes, which ingest answers once.
	start := time.Date(2026, 10, 12, 9, 0, 0, 0, time.UTC)
	err = client.RecordRun(context.Background(), telmoni.RunRecord{
		RunID:     "3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d",
		Agent:     "nightly-triage",
		SessionID: "conv_7",
		Start:     start,
		End:       start.Add(9 * time.Second),
		Inside: []telmoni.RecordItem{
			telmoni.ModelCallRecord{
				Model:    "gpt-4o-mini",
				Provider: "openai",
				Start:    start.Add(100 * time.Millisecond),
				End:      start.Add(2 * time.Second),
				Usage:    &telmoni.Usage{InputTokens: 1200, OutputTokens: 85},
			},
			telmoni.StepRecord{
				Name:  "look-up",
				Start: start.Add(2 * time.Second),
				End:   start.Add(3 * time.Second),
				Calls: []telmoni.CallRecord{
					telmoni.ToolCallRecord{
						Name:      "search",
						Start:     start.Add(2 * time.Second),
						End:       start.Add(3 * time.Second),
						ErrorType: "TimeoutError",
					},
				},
			},
		},
	})
	if err != nil {
		log.Print(err) // refused whole: nothing was sent
	}
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
