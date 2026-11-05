# Telmoni Python SDK

`telmoni`: an agent's runs, steps, model calls and tool calls as OpenTelemetry
spans, sent to Telmoni and nowhere else. It runs a tracer provider of its own
beside whatever OpenTelemetry your application already has, sends nothing
outside a run, and takes prompts, completions and tool payloads off every span
before it leaves the process unless you ask and the project allows.

## Installation

Python 3.10 or later:

```console
pip install telmoni
```

The hosted service at `https://api.telmoni.com`, the SDK's default endpoint,
is not open yet, nor are the docs at telmoni.com/docs. Until they are,
[run Telmoni yourself](https://github.com/telmoni/telmoni#-self-hosting) and
point `TELMONI_ENDPOINT` at its machine host; the docs read in
[`web/content/docs`](https://github.com/telmoni/telmoni/tree/main/web/content/docs).

## Quick start

```python
from telmoni import Telmoni, Usage

# Reads TELMONI_ENDPOINT, TELMONI_API_KEY (a key with the ingest scope),
# TELMONI_ORG, TELMONI_ENVIRONMENT, TELMONI_RELEASE and TELMONI_CAPTURE_CONTENT,
# and nothing else.
telmoni = Telmoni.from_env()


@telmoni.run("support-bot")
def answer(question: str) -> str:
    with telmoni.step("plan"):
        with telmoni.model_call("gpt-4o-mini", provider="openai") as call:
            reply = ask_the_model(question)  # your provider's call
            call.answered(
                response_model=reply.model,
                usage=Usage(
                    input_tokens=reply.usage.input, output_tokens=reply.usage.output
                ),
            )
    with telmoni.tool_call("search"):
        results = search(reply.query)  # an exception marks the call failed
    return reply.text


with telmoni.run(
    "support-bot",
    user_id="user_42",
    session_id="conv_7",
    tags={"env": "prod"},
) as run:
    ...

telmoni.flush()  # before the process exits soon after a run, or a pool's task returns
```

Each of `run`, `step`, `model_call` and `tool_call` is a context manager, an
`async with`, and a decorator; on a generator, sync or async, the decorator's
span covers its iteration. An exception that escapes one marks the span failed
with the exception's class, records its message and stack trace on the span as
an `exception` event and the status's message, content
([Content](#content)), and is raised again; a
`KeyboardInterrupt` or `asyncio.CancelledError`, or a generator closed before
its end, marks it cancelled. `fail()` and `cancel()` do the same by hand. `run` raises `ValueError` for a blank agent
name or a cap that is no cap
([The guard](#the-guard)). A token count is read from a number or its text,
its fraction cut, and `Usage`'s `cost` is what the call cost in US dollars,
where your provider reports it; of what ingest would refuse, these are left off
with one warning: a token count below 0 or past 64 bits, a cost below 0, not
finite, or text, a tag whose key is not
1 to 64 characters or whose value is past 256, and the tags
past the first 32 kept, the most ingest keeps; an empty user, session,
provider or response model is left off as unset. A name, model, provider,
user, session, response model or error type that is no string goes as its
text with one warning, as does a tag that is no string, finite number or bool;
an exception or an exception's class handed to `fail()` goes as the class's
name, and an exception's message and stack trace are recorded as content. Past ingest's caps, counted in characters, a name, the agent, a model, a
provider, a response model or an error type is cut to its cap — a step's or a
tool's name to 145, the rest to 128 — and a user past 128 or a session past 200
is left off, since cut it would name another; a step named nothing goes as
`step`, a model call with no model is named by its operation alone, and one of
an operation ingest reads as no model call, any but `chat`, `generate_content`, `text_completion` and `embeddings`,
goes as `chat`; each with one warning, where ingest would refuse the span,
or the whole run for its agent, user or session, or file the call elsewhere.

## A run recorded after it ended

A job that keeps its own log, or a hook that runs once the turn it reports is
over, hands the finished run to `record_run` as data:

```python
from datetime import datetime, timezone

from telmoni import ModelCallRecord, RunRecord, StepRecord, ToolCallRecord, Usage

telmoni.record_run(
    RunRecord(
        run_id="3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d",  # yours, kept with the log
        agent="nightly-triage",
        session_id="conv_7",
        start=datetime(2026, 10, 12, 9, 0, tzinfo=timezone.utc),
        end=datetime(2026, 10, 12, 9, 0, 9, tzinfo=timezone.utc),
        inside=[
            ModelCallRecord(
                model="gpt-4o-mini",
                provider="openai",
                start=datetime(2026, 10, 12, 9, 0, 0, 100000, tzinfo=timezone.utc),
                end=datetime(2026, 10, 12, 9, 0, 2, tzinfo=timezone.utc),
                usage=Usage(input_tokens=1200, output_tokens=85),
            ),
            ToolCallRecord(
                name="search",
                start=datetime(2026, 10, 12, 9, 0, 2, tzinfo=timezone.utc),
                end=datetime(2026, 10, 12, 9, 0, 3, tzinfo=timezone.utc),
                error_type="TimeoutError",
            ),
        ],
    )
)
telmoni.flush()
```

A `StepRecord` holds its `calls`, steps among them, a subagent's own subagents
say, up to 16 deep, each under the step that holds it. A model call's
`reasoning_effort`, how hard it was asked to reason, is sent in every content
mode. It is sent as the live calls would have sent it, through the same exporter
and content rules, in a trace of its own named by the run's id,
whatever trace is current. Each span's id is derived from the run's id and the
span's place in the record, so the same record sent again is the same bytes,
which ingest answers once: give a run the same id each time you send it. A
record's content is its run's messages (`input_messages`, `output_messages`),
a model call's (`input_messages`, `output_messages`), a tool call's `arguments` and `result`,
and a step's messages, a subagent's task and answer. A blank agent, a run id that is neither a hyphenated
UUID nor 32 hex digits, or is the nil UUID, a time without its timezone or
outside what ingest reads (after the Unix epoch, and less than 2^64 nanoseconds
after it, in July 2554), an empty step's name or model, text longer than
ingest keeps (a step's or a tool's name past 145 characters, the agent, a
model, a provider, a user or an error type past 128, a session past 200, or a
reasoning effort past 1,024), a step nested past 16 deep, or a span that ends
before it starts raises `ValueError`. A
field of the wrong type raises `TypeError` — a time that is no `datetime`, an
item of another class, `inside` or `calls` that is no sequence (a list or a
tuple, in the order it ran) — save a token count, read as a whole number where
it can be and else left off with one warning, and an `operation` of `None`, a
chat. Either sends nothing. A record the export queue has no room for raises
`QueueFullError`, nothing of it queued: flush, or wait, and record it again.
One of more spans than the queue holds, 2,048 unless
`Batching(max_queue_size=...)` raises it, is never taken, and its error says so.
One made once the client is shut down raises `ShutdownError`.

## Scores

A score is a name and a number, a yes or no, or a category. On a handle still
open, `run.score("relevance", 0.75, range=(0, 1), version="judge-2")` adds it
to that span, timed now; `True` or `False` is a yes or no, and a `str` a
category. Once it has ended, the client's `score` sends it, naming what it
scores — a run, a run and one of its spans (`run.run_id`, `step.span_id`), or a
session:

```python
telmoni.score("thumbs", True, run_id=run_id)
telmoni.score("relevance", 0.5, run_id=run_id, span_id=span_id, range=(0, 1))
telmoni.score("outcome", "resolved", session_id="conv_7")
```

It goes with the next export, through the same queue, so `flush()` and
`shutdown()` hold it. A recorded run, and each step and call in it, takes
`scores=[{"name": ..., "value": ..., "range": ..., "version": ...}]`, each timed
at that span's end. A name, a category or a version is trimmed and up to 64
characters; a number is finite and within its range; a span holds up to 16. A
score that breaks a rule is never raised: it is not sent, with one warning. A
score is no content: it is sent whatever `capture_content` and the project's
mode say, and never handed to the mask.

## The guard

`telmoni.run("support-bot", max_steps=20, max_spend=0.50)` holds a run to caps
on its steps and on what its model calls cost, in US dollars, counted in this
process with no call to Telmoni. Past one, the next step, model call or tool
call raises `CapExceededError`, a `RuntimeError` naming the `cap` (`steps` or
`spend`), its `limit` and what the run had `used`, before it opens: a step past
the step cap, and anything once the run spent its spend cap or more. A call
counts its reported `cost`; else its tokens at a price handed as
`Config(prices=[Price(provider=..., model=..., input=..., output=...)])`, which
the SDK alone holds, so Telmoni's own reads still price that call by the
table; else at the price table this package carries, LiteLLM's, by its
provider and the model it answered as; else nothing. A call answered again
counts as its span then holds it. `run.spent` reads the count; a continued run
carries no cap.

## Content

Prompts, completions, tool arguments and results (`input_messages`,
`output_messages`, `arguments`, `returned(...)`, and a run's own
`run(input_messages=)` and `run.answered(output_messages=)`) are recorded on
the span and taken off by the exporter before anything leaves the process. They are sent
only when the client is built with `Config(capture_content=True)`, or from
`Config.from_env()` with `TELMONI_CAPTURE_CONTENT=1`, **and** the
project's content mode, which the SDK asks the server for before the first
content it would send and learns again from every answer, is `on`. A failure's message and stack trace, its `exception` event and its
status's message, and every event the application adds to a span through
OpenTelemetry, a score's own keys apart ([Scores](#scores)), are content too: without both, a failed span goes with its
`error.type` and an error status alone. A `capture_content` that is no `bool`,
such as `"false"` from the environment, raises `TypeError`.

An image or file a message carries, a `blob` part holding its bytes in base64
in `input_messages`, `output_messages` or `system_instructions`, never travels
inside the span: where content is sent, the exporter puts its bytes to
`PUT {endpoint}/v1/media/{sha256}` with the same key, once a client for each
file, and sends the part as a `file` part whose `file_id` is
`telmoni:<sha256>`, its `modality` and `mime_type` kept. A file past 5 MiB is
not put, and a put that fails is warned of once (`upload_failed`); either way
the part goes as the file it names. A part whose content is no base64 goes as
it is.

To take something out of content before it leaves, a card number or an email
address, give the client a mask: `Config(capture_content=True, mask=mask)`. Where
content is sent, and only there, the exporter hands it each content field's key
and its value — `gen_ai.input.messages` as its JSON, an event's attribute under
its own key, a failure's message as `exception.message` and its status's as
`otel.status_description`; a value that is no string as its JSON — and sends the
string it answers instead, or the span without that field where it answers
`None`. It runs on the exporter's thread, once a field each export, before any
image is put, so a part it takes out is never uploaded. A mask that raises, or
answers anything but a `str`, sends the span without that field, warned once
(`mask_failed`) by the key alone. It sees the content the SDK records, never a
name, an id, a tag or an attribute your code sets on a span itself, which a
project keeping content keeps too: keep a person out of those in your own code.

```python
import re

CARD = re.compile(r"\b\d(?:[ -]?\d){12,18}\b")


def mask(key: str, value: str) -> str | None:
    return CARD.sub("[card]", value)


config = Config.from_env()
config.mask = mask
telmoni = Telmoni(config)
```

## Configuration

`Config(endpoint, organization_id, api_key, capture_content, mask, sampler,
environment, release, prices)`, or `Config.from_env()`, which reads `TELMONI_ENDPOINT`
(default `https://api.telmoni.com`), `TELMONI_API_KEY`, `TELMONI_ORG`,
`TELMONI_ENVIRONMENT`, `TELMONI_RELEASE` and `TELMONI_CAPTURE_CONTENT`, a blank
value counting as unset. `TELMONI_CAPTURE_CONTENT` turns `capture_content` on
with `1` alone, its blanks about it trimmed; anything else, or nothing, leaves
it off, and a `capture_content` set in code afterwards stands.
`environment` and `release` name the deployment every span is sent from, as
the resource's `deployment.environment.name` and `service.version`. `prices`
hands [the guard](#the-guard) prices of your own, a `ValueError` for one that
names no provider or model or is no number from 0; nothing of them is sent.
The key never appears in `repr`,
`str` or `dataclasses.asdict`, and `pickle` refuses it. It travels over HTTPS
only: a plain-HTTP endpoint is refused with a `ValueError` unless its host is
this machine — `localhost`, a name under it, 127.0.0.0/8 or `::1`, an
IPv4-mapped address read as its IPv4 — and no redirect is followed. A request
to this machine goes through no proxy, whatever the environment names. An
endpoint without a scheme gets `https://`, or `http://` for this machine, as
the CLI reads one; any other scheme is refused with a `ValueError`, as is one
the four SDKs could read two ways: one not in printable ASCII, or holding
credentials, an escape, a query, a fragment, a dot segment or a character a
URL parser would escape, or a port of 0 or past 65535, or naming a host that
is neither a name of letters, digits, dots, hyphens and underscores, in labels
of 1 to 63 characters with no hyphen at either end and the last no number, nor
an IPv4 address in four decimal parts, nor an IPv6 one in brackets.

## A run across services

Before a request leaves a run, `telmoni.inject(headers)` writes the W3C
`traceparent` of the span current and the run's id and agent as `baggage`
members into any mutable mapping of headers. In the service that answers,
`run = telmoni.extract(request.headers)` gives the run back, continued there:
`telmoni.step("work", run=run)` and the calls inside nest under the caller's
span and carry the caller's run, and nothing of the run itself is sent from
this side. Headers that name no run give `None`, and a step opened with it
is outside a run.

The variables, the attributes each span carries, and the other SDKs are
documented at [telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks).
