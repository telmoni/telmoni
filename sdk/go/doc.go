// Package telmoni is the Go SDK for Telmoni: an agent's runs, steps, model
// calls and tool calls as OpenTelemetry spans, sent to Telmoni and nowhere
// else.
//
// A client runs a tracer provider of its own beside whatever OpenTelemetry the
// application already has, sends nothing outside a run, and takes prompts,
// completions and tool payloads off every span before it leaves the process
// unless the code asks and the project allows.
//
//	client, err := telmoni.NewFromEnv()
//	if err != nil {
//		return err
//	}
//	defer client.Shutdown(context.Background())
//
//	ctx, run := client.Run(ctx, "support-bot", telmoni.RunOptions{UserID: "user_42"})
//	defer run.End()
//
//	stepCtx, step := run.Step(ctx, "plan")
//	defer step.End()
//
// The context a run or step returns carries its span, so a span the
// application opens with it nests under the run, and the run is found from it
// again with RunFromContext.
package telmoni
