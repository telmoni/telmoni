# Telmoni Go SDK

`github.com/telmoni/telmoni/sdk/go`: an agent's runs, steps, model calls
and tool calls as OpenTelemetry spans, sent to Telmoni and nowhere else. The
client runs a tracer provider of its own beside whatever OpenTelemetry your
application already has, sends nothing outside a run, and takes prompts,
completions and tool payloads off every span before it leaves the process
unless you ask and the project allows. Go 1.26 or later.

## Installation

Not published as a tagged module yet. Use it from this repository:

```console
go get github.com/telmoni/telmoni/sdk/go
```

## Quick start

```go
package main

import (
	"context"
	"log"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

func main() {
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
	reply, err := askTheModel(stepCtx) // your provider's call
	if err != nil {
		call.Fail(err) // error.type alone, never the message
	} else {
		call.Answered(telmoni.Answer{
			ResponseModel: reply.Model,
			Usage:         &telmoni.Usage{InputTokens: reply.InputTokens, OutputTokens: reply.OutputTokens},
		})
	}
	call.End()
	step.End()

	_, tool := run.ToolCall(ctx, "search", telmoni.ToolCallOptions{})
	if _, err := search(ctx, reply.Query); err != nil {
		tool.Fail(err)
	}
	tool.End()
}
```

Each opener returns a context carrying its span and the run, so your own
OpenTelemetry spans started with that context nest under the run, and
`telmoni.RunFromContext(ctx)` finds the run deep in a call stack. End each
handle with `End()`, usually deferred. `Fail(err)` marks a span failed with the
error's Go type (`errors.errorString`, `net.OpError`), never its message, and
marks it cancelled for `context.Canceled`; a deadline that ran out fails with
its own type, since a stalled call is what a monitor is for.
`FailWith("TimeoutError")` and `Cancel()` do the same by hand.

The SDK never panics: a `Client` not from `New`, the nil a failed `New`
returns among them, sends nothing, its `RecordRun` returning an error; a
blank agent name opens a run that does nothing and warns once, a run's
`SampleRate` outside (0, 1] falls back to the client's with one warning, and
of what ingest would refuse or drop, these are left off with one warning: a
`StallAfterS` or `MaxSteps` below 0 or past 4294967295, or a
`MaxCostUSD` below 0 or not finite, off its run (a zero bound is unset), a
token count below zero off its call, and a tag whose key is not 1 to 64
characters or whose value is over 256, which takes no place among a run's 32.
A run sends its first 32 tags by key, the most ingest keeps, and warns once of
the rest. Past ingest's caps, counted in characters, a name, the agent, a
model, a provider, a response model or an error type is cut to its cap — a
step's or a tool's name, and a model call's `<operation> <model>`, to 145, the
rest to 128 — and a user past 128 or a session past 200 is left off, since cut
it would name another; a step named nothing goes as `step`, and a model call
with no model is named by its operation alone; each with one warning, where
ingest would refuse the span, or the whole run for its agent, user or session.
`New` returns an error for a client-wide
`SampleRate` outside (0, 1], for a plain-HTTP endpoint whose host is not this
machine, and for one of any scheme but HTTP and HTTPS; `RecordRun`, below, for
a record it refuses. An endpoint without a scheme gets `https://`, or
`http://` for this machine, as the CLI reads one.

## A run recorded after it ended

A job that keeps its own log, or a hook that runs once the turn it reports is
over, hands the finished run to `RecordRun` as data:

```go
start := time.Date(2026, 10, 12, 9, 0, 0, 0, time.UTC)
err := client.RecordRun(ctx, telmoni.RunRecord{
	RunID:     "3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d", // yours, kept with the log
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
client.Flush(ctx)
```

It is sent as the live calls would have sent it, through the same exporter,
content rules and sample rate, in a trace of its own named by the run's id,
whatever span `ctx` carries. Each span's id is derived from the run's id and
the span's place in the record, so the same record sent again is the same
bytes, which ingest answers once: give a run the same id each time you send it.
A record holds no content; a tag or token count a live run would leave off is
left off with one warning. A blank agent, a run id that is not a UUID or is the
nil UUID, text past ingest's caps (a step's or a tool's name 145 characters,
the agent, the model, provider and error types 128, the user 128, the session
200, a model call's operation and model 144 together), a step with an empty
name or a model call
with an empty model, a nil item, a span that ends before it starts, or a time
that is zero, at or before the Unix epoch, or 2^64 nanoseconds or more after
it, in July 2554, returns an error, and nothing is sent: a record is handed
over whole, so it is refused whole. One the export queue has no room for is
`ErrQueueFull`, nothing of it queued: flush, or wait, and record it again.

## Content

Prompts, completions, system instructions, tool arguments and results
(`InputMessages`, `SystemInstructions`, `OutputMessages`, `Arguments`,
`Returned(...)`) are recorded on the span as JSON and taken off by the exporter
before anything leaves the process. They are sent only when the client is built
with `CaptureContent: true` **and** the project's content mode, which the SDK
learns from the server's first answer, is `on`.

## Configuration

`telmoni.Config{Endpoint, OrganizationID, APIKey, CaptureContent, SampleRate,
Sampler, Environment, Release}`, or `telmoni.ConfigFromEnv()`, which reads
`TELMONI_ENDPOINT` (default `https://api.telmoni.com`), `TELMONI_API_KEY`,
`TELMONI_ORG`, `TELMONI_ENVIRONMENT` and `TELMONI_RELEASE`, a blank value
counting as unset. `Environment` and `Release` name the deployment every span
is sent from, as the resource's `deployment.environment.name` and
`service.version`. `SampleRate` is the share of runs sent (0 means 1), dropped
whole in the SDK's processor and never through the trace's sampled flag; `Sampler`
is the OpenTelemetry sampler whose decision a run takes when it starts a
trace of its own. The key never appears in `%v`, `%+v`, `%#v` or
`json.Marshal` of the configuration or the client. Options for tests:
`telmoni.WithTransport`, `telmoni.WithSleep`, `telmoni.WithLogger`.

## A run across services

Before a request leaves a run, `client.Inject(ctx, req.Header)` writes the
W3C `traceparent` of the span `ctx` carries and the run's id and agent as
`baggage` members. In the service that answers,
`ctx := client.Extract(r.Context(), r.Header)` carries the run continued
there, which `telmoni.RunFromContext(ctx)` finds: the steps and calls opened
with it nest under the caller's span and carry the caller's run, and nothing
of the run itself is sent from this side. Headers that name no run leave
`ctx` as given, and a step opened then is outside a run.

Spans are batched (a queue of 2048, batches of 512, every five seconds; a run's
start goes at once) and sent as OTLP/JSON to `POST {endpoint}/otlp/v1/traces`
over HTTPS, following no redirect, each attempt carrying `Telmoni-Sent-At`,
this clock as it leaves, by which ingest moves a run from a clock a zone or a
date out as one. A `429` or `503` is retried after `Retry-After`, three
attempts in all; an answer whose body breaks after its status is decided by
that status, as a whole one is. Every warning is logged once per client
through `log/slog`. `client.Flush(ctx)` sends what is queued;
`client.Shutdown(ctx)` drains and stops. Each waits for `ctx`, or 30 seconds
when it has no deadline; a `Shutdown` whose `ctx` is already done still stops,
waiting for nothing.

The variables, the attributes each span carries, and the other SDKs are
documented at [telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks).
