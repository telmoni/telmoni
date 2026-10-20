# Telmoni Python SDK

`telmoni`: an agent's runs, steps, model calls and tool calls as OpenTelemetry
spans, sent to Telmoni and nowhere else. It runs a tracer provider of its own
beside whatever OpenTelemetry your application already has, sends nothing
outside a run, and takes prompts, completions and tool payloads off every span
before it leaves the process unless you ask and the project allows.

## Installation

Not published yet. Install it from a checkout of this repository (Python 3.10
or later):

```console
pip install ./sdk/python
```

## Quick start

```python
from telmoni import Bounds, Telmoni, Usage

# Reads TELMONI_ENDPOINT, TELMONI_API_KEY (a key with the ingest scope) and
# TELMONI_ORG, and nothing else.
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
    bounds=Bounds(stall_after_s=60, max_steps=10, max_cost_usd=1.5),
) as run:
    ...

telmoni.flush()  # before the process exits, if it exits soon after a run
```

Each of `run`, `step`, `model_call` and `tool_call` is a context manager, an
`async with`, and a decorator. An exception that escapes one marks the span
failed with the exception's class, never its message, and is raised again; a
`KeyboardInterrupt` or `asyncio.CancelledError` marks it cancelled. `fail()`
and `cancel()` do the same by hand. `run` raises `ValueError` for a blank agent
name or a sample rate outside (0, 1]. A bound or a token count is read from a
number or its text, a stall's, steps' or count's fraction cut, and of what
ingest would refuse, these are left off with one warning: a bound outside 1 to
4294967295, a cost not above 0, a token count below 0 or past 64 bits, a tag
whose key is not 1 to 64 characters or whose value is past 256, and the tags
past the first 32 kept, the most ingest keeps; an empty user, session,
provider or response model is left off as unset. A name, model, provider,
user, session or response model that is no string goes as its text with one
warning, as does a tag that is no string, finite number or bool. Text past
ingest's caps, below, and a step or model named nothing still go as given,
and ingest refuses the span they are on, or the whole run for its agent, user
or session.

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

It is sent as the live calls would have sent it, through the same exporter,
content rules and sample rate, in a trace of its own named by the run's id,
whatever trace is current. Each span's id is derived from the run's id and the
span's place in the record, so the same record sent again is the same bytes,
which ingest answers once: give a run the same id each time you send it. A
record holds no content. A blank agent, a run id that is neither a hyphenated
UUID nor 32 hex digits, or is the nil UUID, a time without its timezone or
outside what ingest reads (after the Unix epoch, and less than 2^64 nanoseconds
after it, in July 2554), an empty step's name or model, text longer than
ingest keeps (a name, the agent, a model, a provider, a user or an error type
past 128 characters, a session past 200, or a model call's operation and model
past 127 together), or a span that ends before it starts raises `ValueError`. A
field of the wrong type raises `TypeError` — a time that is no `datetime`, an
item of another class, `inside` or `calls` that is no sequence (a list or a
tuple, in the order it ran) — save a token count, read as a whole number where
it can be and else left off with one warning, and an `operation` of `None`, a
chat. Either sends nothing. A record the export queue has no room for raises
`QueueFullError`, nothing of it queued: flush, or wait, and record it again.

## Content

Prompts, completions, tool arguments and results (`input_messages`,
`output_messages`, `arguments`, `returned(...)`) are recorded on the span and
taken off by the exporter before anything leaves the process. They are sent
only when the client is built with `Config(capture_content=True)` **and** the
project's content mode, which the SDK learns from the server's first answer,
is `on`.

## Configuration

`Config(endpoint, organization_id, api_key, capture_content, sample_rate, sampler,
environment, release)`, or `Config.from_env()`, which reads `TELMONI_ENDPOINT`
(default `https://api.telmoni.com`), `TELMONI_API_KEY`, `TELMONI_ORG`,
`TELMONI_ENVIRONMENT` and `TELMONI_RELEASE`, a blank value counting as unset.
`environment` and `release` name the deployment every span is sent from, as
the resource's `deployment.environment.name` and `service.version`.
`sample_rate` is the share of runs sent, dropped whole in the SDK's processor
and never through the trace's sampled flag. The key never appears in `repr`,
`str` or `dataclasses.asdict`, and `pickle` refuses it. It travels over HTTPS
only: a plain-HTTP endpoint is refused with a `ValueError` unless its host is
this machine, and no redirect is followed. An endpoint without a scheme gets
`https://`, or `http://` for this machine, as the CLI reads one; any other
scheme is refused with a `ValueError`.

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
