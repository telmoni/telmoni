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
and `cancel()` do the same by hand.

## Content

Prompts, completions, tool arguments and results (`input_messages`,
`output_messages`, `arguments`, `returned(...)`) are recorded on the span and
taken off by the exporter before anything leaves the process. They are sent
only when the client is built with `Config(capture_content=True)` **and** the
project's content mode, which the SDK learns from the server's first answer,
is `on`.

## Configuration

`Config(endpoint, organization_id, api_key, capture_content, sample_rate, sampler)`,
or `Config.from_env()`, which reads `TELMONI_ENDPOINT` (default
`https://api.telmoni.com`), `TELMONI_API_KEY` and `TELMONI_ORG`, a blank value
counting as unset. `sample_rate` is the share of runs sent, dropped whole in
the exporter and never through the trace's sampled flag. The key never appears
in `repr`, `str` or `dataclasses.asdict`, and `pickle` refuses it. It travels
over HTTPS only: a plain-HTTP endpoint is refused with a `ValueError` unless
its host is this machine, and no redirect is followed.

The variables, the attributes each span carries, and the other SDKs are
documented at [telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks).
