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

// Reads TELMONI_ENDPOINT, TELMONI_API_KEY, TELMONI_ORG, TELMONI_ENVIRONMENT, TELMONI_RELEASE
// and TELMONI_CAPTURE_CONTENT
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
(`error.type`), records an `Error`'s message and stack on the span as an
`exception` event and the status's message, content ([Content](#content)), and
is rethrown; an error named `AbortError`
marks the span cancelled instead. `run.fail('RateLimit')` and `run.cancel()`
mark a span by hand. Steps nest: `step.step(...)`, `step.modelCall(...)` and
`step.toolCall(...)` open under the step; the same methods on `run` open under
the run. `run` throws a `TypeError` for a blank agent name and a `RangeError`
for a cap that is no cap
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
`fail()` goes as its class's name, an `Error`'s message and stack recorded as
content. Past ingest's caps,
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
holds its own `calls`, steps among them, a subagent's own subagents say, up to
16 deep, each under the step that holds it. A model call's `reasoningEffort`,
how hard it was asked to reason, is sent in every content mode. The run is sent as the live calls would have sent it,
through the same exporter and content rules, in a trace of its own
named by the run's id, whatever context is active. Each span's id is derived
from the run's id and the span's place in the record, so the same record sent
again is the same bytes, which ingest answers once: give a run the same id each
time you send it. A record's content is its run's messages (`inputMessages`,
`outputMessages`), a model call's (`inputMessages`, `outputMessages`), a tool call's
`arguments` and `result`, and a step's messages, a subagent's task and answer. A blank agent, a run id that is
neither a hyphenated UUID nor 32 hex digits, or is the nil UUID, or an empty
step's name or model throws a `TypeError`, as does a field of the wrong type —
a time that is not one, an item of no known `type`, `inside` or `calls` that is
no array, in the order it ran, tags or a usage that is no object (an array or a
`Map`, say) — save a token count, read as a whole number where it can be and
else left off with one warning. A time outside what ingest reads (after the
Unix epoch, and less than 2^64 nanoseconds after it, in July 2554), text longer
than ingest keeps (a step's or a tool's name past 145 characters, the agent, a
model, a provider, a user or an error type past 128, a session past 200, or a
reasoning effort past 1,024, counted in characters, not UTF-16's units), a step
nested past 16 deep, or a span
that ends before it starts throws a `RangeError`. Either sends nothing. A
record the export queue has no room for throws `QueueFullError`, nothing of it
queued: flush, or wait, and record it again. One of more spans than the queue
holds, 2,048 unless the `processor` hook's `maxQueueSize` raises it, is never
taken, and its error says so. One made once the client is shut down throws
`ShutdownError`.

## Scores

A score is a name and a number, a yes or no, or a category. On a handle still
open, `run.score('relevance', 0.75, { range: [0, 1], version: 'judge-2' })`
adds it to that span, timed now; `true` or `false` is a yes or no, and text a
category. Once it has ended, the client's `score` sends it, naming what it
scores — a run, a run and one of its spans (`run.runId`, `step.spanId`), or a
session:

```ts
telmoni.score('thumbs', true, { runId });
telmoni.score('relevance', 0.5, { runId, spanId, range: [0, 1] });
telmoni.score('outcome', 'resolved', { sessionId: 'conv_7' });
```

It goes with the next export, through the same queue, so `flush` and
`shutdown` hold it. A recorded run, and each step and call in it, takes
`scores: [{ name, value, range?, version? }]`, each timed at that span's end. A
name, a category or a version is trimmed and up to 64 characters; a number is
finite and within its range; a span holds up to 16. A score that breaks a rule
is never thrown: it is not sent, with one warning. A score is no content: it
is sent whatever `captureContent` and the project's mode say, and never handed
to the mask.

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
  environment: 'production', // the deployment, sent as deployment.environment.name
  release: '2026.10.18', // its release, sent as service.version
});
```

`Telmoni.fromEnv()` reads `TELMONI_ENDPOINT`, `TELMONI_API_KEY`,
`TELMONI_ORG`, `TELMONI_ENVIRONMENT`, `TELMONI_RELEASE` and
`TELMONI_CAPTURE_CONTENT`; a blank value is unset, and a blank endpoint falls
back to the default. `TELMONI_CAPTURE_CONTENT` turns `captureContent` on with
`1` alone, its blanks about it trimmed; anything else, or nothing, leaves it
off. A client built with `new Telmoni(config)` reads no variable, so its
`captureContent` is what the code says. No `OTEL_*` variable
is read. `prices` hands [the guard](#the-guard) prices of your own, a
`TypeError` or `RangeError` for one that names no provider or model or is no
number from 0; nothing of them is sent. The key is never printed or serialized: `console.log`,
`util.inspect` and `JSON.stringify` of the client or its configuration leave
it out.

## A run across services

Before a request leaves a run, `telmoni.inject(headers, step)` writes the W3C
`traceparent` of the step's span and the run's id and agent as
`baggage` members into a record or a `Headers`; without a handle it writes from the
active context, where your application runs OpenTelemetry's context manager.
In the service that answers, `telmoni.extract(req.headers)` gives the run
back, continued there: its steps and calls nest under the caller's span and
carry the caller's run, and nothing of the run itself is sent from this side.
Headers that name no run give `undefined`.

## Content

Prompts, completions, system instructions, tool arguments and tool results —
`inputMessages`, `systemInstructions`, `outputMessages`, `arguments` and
`returned(...)`, and a run's own `inputMessages` and
`run.answered({ outputMessages })` — are content, and so are a failure's message
and stack, its `exception` event and its status's message, and every event the
application adds to a span through OpenTelemetry, a score's own keys apart
([Scores](#scores)): without them, a failed span
goes with its `error.type` and an error status alone. They leave the process only when
`captureContent: true`, or `TELMONI_CAPTURE_CONTENT=1` under `Telmoni.fromEnv()`,
**and** the project's content mode is `on`, which the
SDK asks the platform for before the first content it would send and learns
again from every answer (a `captureContent` that is no boolean, such as
`'false'` from the environment, throws a `TypeError`); while the mode is
unheard, and on an `off` project, the exporter takes them off every span. The server drops content
on an `off` project too, so the SDK's strip is the first lock, not the only one.

An image or file a message carries, a `blob` part holding its bytes in base64
in `inputMessages`, `outputMessages` or `systemInstructions`, never travels
inside the span: where content is sent, the exporter puts its bytes to
`PUT {endpoint}/v1/media/{sha256}` with the same key, once a client for each
file, and sends the part as a `file` part whose `file_id` is
`telmoni:<sha256>`, its `modality` and `mime_type` kept. A file past 5 MiB is
not put, and a put that fails is warned of once (`upload_failed`); either way
the part goes as the file it names. A part whose content is no base64 goes as
it is.

To take something out of content before it leaves, a card number or an email
address, give the client a mask. Where content is sent, and only there, the
exporter hands it each content field's key and its value — `gen_ai.input.messages`
as its JSON, an event's attribute under its own key, a failure's message as
`exception.message` and its status's as `otel.status_description`; a value that is
no string as its JSON — and sends the string it answers instead, or the span
without that field where it answers `undefined` or `null`. It runs once a field
each export, before any image is put, so a part it takes out is never uploaded,
and must answer at once: a mask that throws, or answers anything but a string, a
`Promise` among them, sends the span without that field, warned once
(`mask_failed`) by the key alone. It sees the content the SDK records, never a
name, an id, a tag or an attribute your code sets on a span itself, which a
project keeping content keeps too: keep a person out of those in your own code.
`Telmoni.fromEnv()` sets none; give it to
the constructor.

```ts
const CARD = /\b\d(?:[ -]?\d){12,18}\b/g;

const telmoni = new Telmoni({
  apiKey: process.env.TELMONI_API_KEY,
  captureContent: true,
  mask: (key, value) => value.replace(CARD, '[card]'),
});
```

## Beside your own OpenTelemetry

The SDK builds a tracer provider of its own, part by part, and sends only to
Telmoni: an application's own provider, exporters and sampling are untouched. A
run opened while one of the application's spans is current joins its trace, as
a child of that span; with OpenTelemetry's Node context manager registered,
the application's spans opened inside a callback nest under ours, and
`currentRun()` finds the run. Without a context manager, nest explicitly:
`tracer.startSpan('db.query', {}, run.context)`.

A run takes the sampled flag of the trace it joins, so the application's own
sampling rate holds inside runs as outside them. Every run is sent whatever the
flag says, one inside a trace the application does not sample too. The optional
`sampler` is an OpenTelemetry `Sampler` whose decision a run takes when it
starts a trace of its own.

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
one. A `429`, `502`, `503` or `504` is retried, after `Retry-After` where one
came, three attempts in all; an
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
processor })`, where `transport` has `fetch`'s shape, handed an export's
`POST` or a file's `PUT`, `sleep` is the wait
between retries, handed a signal that aborts once a shutdown gives the retry up,
and `processor` sizes the queue.

The variables, their fallbacks and the other SDKs are documented at
[telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks).
