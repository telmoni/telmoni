# Telmoni TypeScript / JavaScript SDK

An agent's runs, steps, model calls and tool calls as OpenTelemetry spans,
sent to Telmoni and nowhere else. Node 18.19 or 20.6 and later.

## Installation

Not published yet. Build it from a checkout of this repository, then depend on
it by path:

```console
cd sdk/typescript && npm ci && npm run build
```

```json
{ "dependencies": { "telmoni": "file:../telmoni/sdk/typescript" } }
```

## Quick Start

```ts
import { Telmoni } from 'telmoni';

// Reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG
const telmoni = Telmoni.fromEnv();

const reply = await telmoni.run(
  'support-bot',
  { userId: 'user_42', conversationId: 'conv_7', bounds: { maxSteps: 10, maxCostUsd: 1.5 } },
  async (run) => {
    const plan = await run.step('plan', (step) =>
      step.modelCall('gpt-4o-mini', { provider: 'openai', inputMessages: messages }, async (call) => {
        const completion = await openai.chat.completions.create({ model: 'gpt-4o-mini', messages });
        call.answered({
          responseModel: completion.model,
          usage: {
            inputTokens: completion.usage?.prompt_tokens,
            outputTokens: completion.usage?.completion_tokens,
          },
          outputMessages: completion.choices,
        });
        return completion.choices[0]?.message.content ?? '';
      }),
    );
    return run.toolCall('search', { arguments: { q: plan } }, async (tool) => {
      const found = await search(plan);
      tool.returned(found);
      return found;
    });
  },
);

await telmoni.shutdown(); // before the process exits: sends what is still queued
```

Each callback gets a handle and runs with its span current; the span ends when
the callback settles, and the callback's result is returned, so a synchronous
callback returns synchronously and an `async` one returns its promise. An error
thrown or rejected inside marks the span with the error's class name
(`error.type`), never its message, and is rethrown; an error named `AbortError`
marks the span cancelled instead. `run.fail('RateLimit')` and `run.cancel()`
mark a span by hand. Steps nest: `step.step(...)`, `step.modelCall(...)` and
`step.toolCall(...)` open under the step; the same methods on `run` open under
the run.

`telmoni.wrap(agent, options, fn)` returns `fn` opening a run per call, with
the run's handle as `fn`'s last argument:

```ts
import type { Run } from 'telmoni';

const answer = telmoni.wrap('support-bot', async (question: string, run: Run) => {
  return run.step('think', () => think(question));
});
await answer('why is the sky blue?');
```

Nothing can be opened outside a run: steps and calls exist on a run's handle,
or a step's, and nothing is sent outside one.

## Configuration

```ts
const telmoni = new Telmoni({
  endpoint: 'https://api.telmoni.com', // the default
  apiKey: 'telmoni_…',
  organizationId: 'org_…',
  captureContent: false, // the default
  sampleRate: 1, // the default: every run
});
```

`Telmoni.fromEnv()` reads `TELMONI_ENDPOINT`, `TELMONI_API_KEY` and
`TELMONI_ORG`; a blank value is unset, and a blank endpoint falls back to the
default. No `OTEL_*` variable is read. The key is never printed or serialized:
`console.log`, `util.inspect` and `JSON.stringify` of the client or its
configuration leave it out.

## Content

Prompts, completions, system instructions, tool arguments and tool results —
`inputMessages`, `systemInstructions`, `outputMessages`, `arguments` and
`returned(...)` — are content. They leave the process only when
`captureContent: true` **and** the project's content mode is `on`, which the
SDK learns from the platform's first answer; before that answer, and on an
`off` project, the exporter takes them off every span. The server drops content
on an `off` project too, so the SDK's strip is the first lock, not the only one.

## Beside your own OpenTelemetry

The SDK builds a tracer provider of its own, part by part, and sends only to
Telmoni: an application's own provider, exporters and sampling are untouched. A
run opened while one of the application's spans is current joins its trace, as
a child of that span; with OpenTelemetry's Node context manager registered,
the application's spans opened inside a callback nest under ours, and
`currentRun()` finds the run. Without a context manager, nest explicitly:
`tracer.startSpan('db.query', {}, run.context)`.

`sampleRate` drops whole runs inside the SDK, never through the trace's
sampled flag, so the application's own sampling rate holds inside runs as
outside them. A run inside a trace the application does not sample is still
sent. The optional `sampler` is an OpenTelemetry `Sampler` whose decision a run
takes when it starts a trace of its own.

## Sending

Spans are batched (a queue of 2048, batches of 512, every five seconds; a run's
start goes at once) and sent as OTLP/JSON to `POST {endpoint}/otlp/v1/traces`
over HTTPS: a plain-HTTP endpoint is refused with a `TypeError` unless its host
is this machine, and no redirect is followed, since every export carries the
key. A `429` or `503` is retried after `Retry-After`, three attempts in all. Every
warning — an attribute the server dropped, a batch given up on — is logged once
per client through `console.warn`. `await telmoni.flush()` sends what is queued
and resolves `true` once the platform has it; `await telmoni.shutdown()` drains
and stops. The SDK's worker never keeps the process alive on its own, so call
`shutdown()` before exiting.

For tests, inject the seams: `new Telmoni(config, { transport, sleep, warn,
processor })`, where `transport` has `fetch`'s shape, `sleep` is the wait
between retries and `processor` sizes the queue.

The variables, their fallbacks and the other SDKs are documented at
[telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks).
