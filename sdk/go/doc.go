// Package telmoni is the Go SDK for Telmoni, privacy-first, open-source
// monitoring for AI agents: an agent's traces, steps, model calls, tool calls
// and retrievals as OpenTelemetry spans, sent to Telmoni and nowhere else.
//
// A client runs a tracer provider of its own beside whatever OpenTelemetry the
// application already has, sends nothing outside a trace, and takes prompts,
// completions and tool payloads off every span before it leaves the process
// unless the code asks and the project allows.
//
// Telmoni is self-hosted (https://github.com/telmoni/telmoni#self-hosting):
// TELMONI_ENDPOINT, or Config.Endpoint, names your deployment's endpoint, and
// there is no default.
//
//	client, err := telmoni.NewFromEnv()
//	if err != nil {
//		return err
//	}
//	defer client.Shutdown(context.Background())
//
//	ctx, tr := client.Trace(ctx, "support-bot", telmoni.TraceOptions{UserID: "user_42"})
//	defer tr.End()
//
//	stepCtx, step := tr.Step(ctx, "plan")
//	defer step.End()
//
// Each Trace is a trace of its own, linked to whatever span the application
// had current. The context a trace or step returns carries its span, so a span
// the application opens with it nests under the agent's, and the trace is
// found from it again with TraceFromContext.
//
// A trace that has already ended, kept in a job's own log or reported by a
// hook once its turn is over, is sent whole from its record with RecordTrace.
//
// The guide to it, beside the other three SDKs, is the SDKs page of Telmoni's
// book: https://github.com/telmoni/telmoni/blob/main/web/content/docs/api/sdks.mdx
package telmoni
