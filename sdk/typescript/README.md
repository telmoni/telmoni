# Telmoni TypeScript / JavaScript SDK

An agent's runs, steps, model calls and tool calls as OpenTelemetry spans,
sent to Telmoni and nowhere else. Node 18.19 or 20.6 and later.

## Installation

```console
npm install telmoni
```

The hosted service at `https://api.telmoni.com`, the SDK's default endpoint,
is not open yet, nor are the docs at telmoni.com/docs. Until they are,
[run Telmoni yourself](https://github.com/telmoni/telmoni#-self-hosting) and
point `TELMONI_ENDPOINT` at its machine host; the docs read in
[`web/content/docs`](https://github.com/telmoni/telmoni/tree/main/web/content/docs).

## Quick Start

```ts
import { Telmoni } from 'telmoni';

// Reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG
const telmoni = Telmoni.fromEnv();

const reply = await telmoni.run(
  'support-bot',
  { userId: 'user_42', sessionId: 'conv_7' },
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
the run. `run` throws a `TypeError` for a blank agent name and a `RangeError`
for a sample rate that is no number or outside (0, 1], or a cap that is no cap
([The guard](#the-guard)). A token count is read from a number, a bigint or a
decimal numeral's text, its fraction cut, and the usage's `cost` is what the
call cost in US dollars, where your provider reports it; of what ingest would
refuse, these are left off with one warning: a token count below 0 or past 64
bits, a cost below 0, not finite, or no number, a tag whose key is not 1 to 64 characters or whose
value is past 256, and the tags past the first 32 kept,
the most ingest keeps; an empty user, session, provider or response model is
left off as unset. A name, model, provider, user, session, response model or
error type that is no string goes as its text with one warning, as does a tag
that is no string, finite number or boolean; an error, or a class, handed to
`fail()` goes as its class's name, never its message. Past ingest's caps,
counted in characters, a name, the agent, a model, a provider, a response
model or an error type is cut to its cap — a step's or a tool's name to 145,
the rest to 128 — and a user past 128 or a session past 200 is left off, since
cut it would name another; a step named nothing goes as `step`, a model call
with no model is named by its operation alone, and one of an operation ingest
reads as no model call, any but `chat`, `generate_content`, `text_completion` and `embeddings`, goes
as `chat`; each with one warning, where ingest would refuse the span, or the
whole run for its agent, user or session, or file the call elsewhere.

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

## A run recorded after it ended

A job that keeps its own log, or a hook that runs once the turn it reports is
over, hands the finished run to `recordRun` as data:

```ts
telmoni.recordRun({
  runId: '3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d', // yours, kept with the log
  agent: 'nightly-triage',
  sessionId: 'conv_7',
  start: new Date('2026-10-12T09:00:00.000Z'),
  end: new Date('2026-10-12T09:00:09.000Z'),
  inside: [
    {
      type: 'model_call',
      model: 'gpt-4o-mini',
      provider: 'openai',
      start: new Date('2026-10-12T09:00:00.100Z'),
      end: new Date('2026-10-12T09:00:02.000Z'),
      usage: { inputTokens: 1200, outputTokens: 85 },
    },
    {
      type: 'tool_call',
      name: 'search',
      start: new Date('2026-10-12T09:00:02.000Z'),
      end: new Date('2026-10-12T09:00:03.000Z'),
      errorType: 'TimeoutError',
    },
  ],
});
await telmoni.flush();
```

A time is a `Date` or milliseconds since the Unix epoch; a step (`type: 'step'`)
holds its own `calls`. The run is sent as the live calls would have sent it,
through the same exporter, content rules and sample rate, in a trace of its own
named by the run's id, whatever context is active. Each span's id is derived
from the run's id and the span's place in the record, so the same record sent
again is the same bytes, which ingest answers once: give a run the same id each
time you send it. A record holds no content. A blank agent, a run id that is
neither a hyphenated UUID nor 32 hex digits, or is the nil UUID, or an empty
step's name or model throws a `TypeError`, as does a field of the wrong type —
a time that is not one, an item of no known `type`, `inside` or `calls` that is
no array, in the order it ran, tags or a usage that is no object (an array or a
`Map`, say) — save a token count, read as a whole number where it can be and
else left off with one warning. A time outside what ingest reads (after the
Unix epoch, and less than 2^64 nanoseconds after it, in July 2554), text longer
than ingest keeps (a step's or a tool's name past 145 characters, the agent, a
model, a provider, a user or an error type past 128, or a session past 200,
counted in characters, not UTF-16's units), or a span
that ends before it starts throws a `RangeError`. Either sends nothing. A
record the export queue has no room for throws `QueueFullError`, nothing of it
queued: flush, or wait, and record it again. One of more spans than the queue
holds, 2,048 unless the `processor` hook's `maxQueueSize` raises it, is never
taken, and its error says so. One made once the client is shut down throws
`ShutdownError`.

## The guard

`telmoni.run('support-bot', { maxSteps: 20, maxSpend: 0.5 }, callback)` holds
a run to caps on its steps and on what its model calls cost, in US dollars,
counted in this process with no call to Telmoni. Past one, `step`, `modelCall`
and `toolCall` throw `CapExceededError`, naming the `cap` (`steps` or
`spend`), its `limit` and what the run had `used`, before their callback runs:
a step past the step cap, and anything once the run spent its spend cap or
more. A call counts its reported `cost`; else its tokens at a price handed as
`prices: [{ provider, model, input, output }]`, which the SDK alone holds, so
Telmoni's own reads still price that call by the table; else at the price
table this package carries, LiteLLM's, by its provider and the model it
answered as; else nothing. A call answered again counts as its span then
holds it. `run.spent` reads the count; a continued run carries no cap.

## Configuration

```ts
const telmoni = new Telmoni({
  endpoint: 'https://api.telmoni.com', // the default
  apiKey: 'tm_…',
  organizationId: 'org_…',
  captureContent: false, // the default
  sampleRate: 1, // the default: every run
  environment: 'production', // the deployment, sent as deployment.environment.name
  release: '2026.10.18', // its release, sent as service.version
});
```

`Telmoni.fromEnv()` reads `TELMONI_ENDPOINT`, `TELMONI_API_KEY`,
`TELMONI_ORG`, `TELMONI_ENVIRONMENT` and `TELMONI_RELEASE`; a blank value is
unset, and a blank endpoint falls back to the default. No `OTEL_*` variable
is read. `prices` hands [the guard](#the-guard) prices of your own, a
`TypeError` or `RangeError` for one that names no provider or model or is no
number from 0; nothing of them is sent. The key is never printed or serialized: `console.log`,
`util.inspect` and `JSON.stringify` of the client or its configuration leave
it out.

## A run across services

Before a request leaves a run, `telmoni.inject(headers, step)` writes the W3C
`traceparent` of the step's span and the run's id and agent as `baggage`
members into a record or a `Headers`; without a handle it writes from the
active context, where your application runs OpenTelemetry's context manager.
In the service that answers, `telmoni.extract(req.headers)` gives the run
back, continued there: its steps and calls nest under the caller's span and
carry the caller's run, and nothing of the run itself is sent from this side.
Headers that name no run give `undefined`.

## Content

Prompts, completions, system instructions, tool arguments and tool results —
`inputMessages`, `systemInstructions`, `outputMessages`, `arguments` and
`returned(...)` — are content. They leave the process only when
`captureContent: true` **and** the project's content mode is `on`, which the
SDK learns from the platform's first answer (a `captureContent` that is no
boolean, such as `'false'` from the environment, throws a `TypeError`); before that answer, and on an
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

Spans are batched (a queue of 2048, batches of 512, every five seconds; a
recorded run goes at once) and sent as OTLP/JSON to `POST {endpoint}/otlp/v1/traces`
over HTTPS: a plain-HTTP endpoint is refused with a `TypeError` unless its host
is this machine — `localhost`, a name under it, 127.0.0.0/8 or `::1`, an
IPv4-mapped address read as its IPv4 — and no redirect is followed, since every
export carries the key. A request to this machine goes through no proxy, even
under `NODE_USE_ENV_PROXY`: it is sent with Node's own `http`. An endpoint
without a scheme gets `https://`, or `http://` for this machine, as the CLI
reads one; any other scheme is refused
with a `TypeError`, as is one the four SDKs could read two ways: one not in
printable ASCII, or holding credentials, an escape, a query, a fragment, a dot
segment or a character a URL parser would escape, or a port of 0 or past
65535, or naming a host that is neither a name of letters, digits, dots,
hyphens and underscores, in labels of 1 to 63 characters with no hyphen at
either end and the last no number, nor an IPv4 address in four decimal parts,
nor an IPv6 one in brackets. Each attempt carries `Telmoni-Sent-At`, this clock
as it leaves, by which ingest moves a run from a clock a zone or a date out as
one. A `429` or `503` is retried after `Retry-After`, three attempts in all; an
answer whose body breaks after its status is decided by that status, as a whole
one is, and one whose head breaks off is no answer, sent again. A batch whose
body would pass the 1 MiB ingest takes, or that holds more than its 1,000
spans, is sent in halves, each within both, and a span alone past 1 MiB is sent
without the attributes ingest drops for their length, with one warning. Every
warning — an attribute the server dropped, a batch given up on — is logged once
per client through `console.warn`. `await telmoni.flush()` waits for what is
queued and resolves `true` once nothing is left to send, kept or not (a batch
refused or dropped is warned of), and at once after `shutdown()`; `false` past
its timeout. `await telmoni.shutdown()` drains and stops. The SDK's worker never keeps the
process alive on its own. A process that ends of itself is drained as Node's
loop empties, but `process.exit()` or a signal left unhandled waits for
nothing, so call `shutdown()` before exiting.

For tests, inject the seams: `new Telmoni(config, { transport, sleep, warn,
processor })`, where `transport` has `fetch`'s shape, `sleep` is the wait
between retries and `processor` sizes the queue.

The variables, their fallbacks and the other SDKs are documented at
[telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks).
