# Telmoni Rust SDK

`telmoni`: an agent's runs, steps, model calls and tool calls as
OpenTelemetry spans, sent to Telmoni and nowhere else. The client owns a
tracer provider of its own, built part by part so no `OTEL_*` variable reaches
it, and runs beside whatever OpenTelemetry the application already has: a run
opened while an application span is current joins its trace, and the
application's spans stay with the application's exporter.

## Installation

Rust 1.99.1 or later:

```console
cargo add telmoni
```

The hosted service at `https://api.telmoni.com`, the SDK's default endpoint,
is not open yet, nor are the docs at telmoni.com/docs. Until they are,
[run Telmoni yourself](https://github.com/telmoni/telmoni#-self-hosting) and
point `TELMONI_ENDPOINT` at its machine host; the docs read in
[`web/content/docs`](https://github.com/telmoni/telmoni/tree/main/web/content/docs).

## Quick start

```rust
use telmoni::{Config, ModelCallOptions, Reply, RunOptions, Telmoni, ToolCallOptions, Usage};

fn main() -> Result<(), telmoni::Error> {
    // Reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG.
    let telmoni = Telmoni::new(Config::from_env());

    let run = telmoni.run(
        "support-bot",
        RunOptions {
            user_id: Some("user_42".into()),
            ..RunOptions::default()
        },
    )?;
    {
        let step = run.step("plan")?;
        let call = step.model_call(
            "gpt-4o-mini",
            ModelCallOptions {
                provider: Some("openai".into()),
                ..ModelCallOptions::default()
            },
        )?;
        call.answered(Reply {
            response_model: Some("gpt-4o-mini-2024-07-18".into()),
            usage: Some(Usage {
                input_tokens: Some(12),
                output_tokens: Some(7),
                ..Usage::default()
            }),
            ..Reply::default()
        });
    }
    let tool = run.tool_call("search", ToolCallOptions::default())?;
    tool.returned(serde_json::json!({ "results": 3 }));
    drop(tool);
    run.end();

    telmoni.shutdown();
    Ok(())
}
```

Every handle is a guard: its span ends when it is dropped or on `end()`.
`run` returns `Error::EmptyAgent` for a blank agent name,
`Error::SampleRate` for a rate outside (0, 1], and `Error::MaxSteps` or
`Error::MaxSpend` for a cap that is no cap ([The guard](#the-guard)). A
negative token count, or a `Usage`'s `cost` below 0 or not finite, is left
off its call with one warning; a run sends its first 32 tags by key, the most
ingest keeps, and warns once of the rest. A tag whose key
is not 1 to 64 characters or whose value is over 256 is left off with one
warning first and takes no place among them. Past ingest's caps, counted in
characters, a name, the agent, a model, a provider, a response model or an
error type is cut to its cap — a step's or a tool's name to 145, the rest to
128 — and a user past 128 or a session past 200 is left off, since cut it would
name another; a step named nothing goes as `step`, a model call with no model is
named by its operation alone, and one of an operation ingest reads as no model
call, any but `chat`, `generate_content`, `text_completion` and `embeddings`, goes as `chat`;
each with one warning, where ingest would refuse the span, or the whole run for
its agent, user or session, or file the call elsewhere.
`fail("ErrorType")` marks a span failed with the error's type, none if empty,
and no message; `cancel()` marks it cancelled by the agent. The `_with`
methods take a closure and return its value, or the `Error` that kept their
handle from opening — `telmoni.run_with(..)`, `run.step_with(..)`,
`model_call_with`, `tool_call_with` — and their `try_` forms, for a closure
whose error converts from `telmoni::Error`, mark an `Err` on the span with the
error type's name before returning it unchanged:

```rust
// MyError implements From<telmoni::Error>.
let answer: Result<String, MyError> = telmoni.try_run_with("support-bot", RunOptions::default(), |run| {
    run.try_step_with("plan", |_step| plan())
});
```

A run's `context()` carries it into a future (`FutureExt::with_context`) or
another thread, and `attach()` makes it current for synchronous code, so the
application's own spans nest under it. That, and a run joining the trace the
application has current, take the application on the SDK's `opentelemetry`
minor, re-exported as `telmoni::opentelemetry`: cargo builds each 0.x minor
apart, each with a current context of its own.

## A run recorded after it ended

A job that keeps its own log, or a hook that runs once the turn it reports is
over, hands the finished run to `record_run` as data:

```rust
use std::time::{Duration, SystemTime};

use telmoni::{ModelCallRecord, RunRecord, ToolCallRecord, Usage};

// 2026-10-12T09:00:00Z, and what came after it.
let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_795_600);
let at = |millis| start + Duration::from_millis(millis);

telmoni.record_run(RunRecord {
    session_id: Some("conv_7".into()),
    inside: vec![
        ModelCallRecord {
            provider: Some("openai".into()),
            usage: Some(Usage {
                input_tokens: Some(1200),
                output_tokens: Some(85),
                ..Usage::default()
            }),
            ..ModelCallRecord::new("gpt-4o-mini", at(100), at(2_000))
        }
        .into(),
        ToolCallRecord {
            error_type: Some("TimeoutError".into()),
            ..ToolCallRecord::new("search", at(2_000), at(3_000))
        }
        .into(),
    ],
    // Yours, kept with the log.
    ..RunRecord::new("3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d", "nightly-triage", start, at(9_000))
})?;
telmoni.flush();
```

A step holds its own calls: `StepRecord { calls: vec![..], ..StepRecord::new(..) }`.
Times are `SystemTime`s, which `chrono`'s and `time`'s convert into with
`SystemTime::from`.

It is sent as the live calls would have sent it, through the same exporter,
content rules and sample rate, in a trace of its own named by the run's id,
whatever trace is current. Each span's id is derived from the run's id and the
span's place in the record, so the same record sent again is the same bytes,
which ingest answers once: give a run the same id each time you send it. A
record holds no content, and its tags and token counts go as a live run's do.
A blank agent (`Error::EmptyAgent`), a run id that is not a UUID or is the nil
UUID (`Error::RunId`), an empty step name or model (`Error::EmptyName`), text
past ingest's caps (`Error::TooLong`: a step's or a tool's name 145
characters, the agent, models, providers and error types 128, the user 128,
and the session 200), a time
at or before the Unix epoch or 2^64
nanoseconds or more after it, in July 2554 (`Error::RecordedTime`), or a span
that ends before it starts (`Error::EndBeforeStart`) is returned as an error,
and nothing is sent. A record the export queue has no room for is
`Error::QueueFull`, nothing of it queued: flush, or wait, and record it again,
from a clone, since `record_run` takes the record. One made once the client is
shut down is `Error::Shutdown`.

## The guard

`RunOptions { max_steps: Some(20), max_spend: Some(0.50), .. }` holds a run
to caps on its steps and on what its model calls cost, in US dollars, counted
in this process with no call to Telmoni. Past one, `step`, `model_call` and
`tool_call` return `Error::CapExceeded` (its `cap`, `Cap::Steps` or
`Cap::Spend`, its `limit` and what the run had `used`) and open nothing: a
step past the step cap, and anything once the run spent its spend cap or more.
A call counts its reported `cost`; else its tokens at a price handed as
`Config { prices: vec![Price { .. }], .. }`, which the SDK alone holds, so
Telmoni's own reads still price that call by the table; else at the price
table this crate carries, LiteLLM's, by its provider and the model it answered
as; else nothing. A call answered again counts as its span then holds it.
`run.spent()` reads the count; a continued run carries no cap.

## Configuration

`Config::from_env()` reads five variables, a blank value being unset:

| Variable | What |
|---|---|
| `TELMONI_ENDPOINT` | Where the platform is; `https://api.telmoni.com` unless set. |
| `TELMONI_API_KEY` | The project's API key, with the `ingest` scope. |
| `TELMONI_ORG` | The organization (`org_…`). |
| `TELMONI_ENVIRONMENT` | The deployment this process runs in (`production`, `staging`), sent as `deployment.environment.name`. |
| `TELMONI_RELEASE` | The release it runs (a version or a commit), sent as `service.version`. |

Nothing in the SDK reads an `OTEL_*` variable. `Config` also holds
`capture_content` (default `false`), `sample_rate` (default `1.0`; above 0 and
up to 1, decided per run and never through the trace's sampled flag),
`sampler`, the OpenTelemetry sampler whose decision a run takes when it starts
a trace of its own, `environment` and `release`, the deployment the
variables above name, and `prices`, the guard's prices of your own, a price
that names no provider or model, or is no number from 0, left out with one
warning; nothing of them is sent. The API key is never printed or serialized: `Debug`
shows it redacted, and `Serialize` leaves it out. It travels over HTTPS only:
with a plain-HTTP endpoint whose host is not this machine — `localhost`, a
name under it, 127.0.0.0/8 or `::1`, an IPv4-mapped address read as its IPv4 —
one of any scheme but HTTP and HTTPS, or one the four SDKs could read two ways
— one not in printable ASCII, or holding credentials, an escape, a query, a
fragment, a dot segment or a character a URL parser would escape, or a port of
0 or past 65535, or naming a host that is neither a name of letters, digits,
dots, hyphens and underscores, in labels of 1 to 63 characters with no hyphen
at either end and the last no number, nor an IPv4 address in four decimal
parts, nor an IPv6 one in brackets — nothing is sent and the first export
warns once, and no redirect is followed. A request to this machine goes through
no proxy, whatever the environment names. An endpoint without a scheme gets
`https://`, or `http://` for this machine, as the CLI reads one.

## A run across services

Before a request leaves a run, `telmoni.inject(&mut headers)` writes the W3C
`traceparent` of the span current — what a handle's `attach()` or
`FutureExt::with_context` made so — and the run's id and agent as `baggage`
members; `telmoni.inject_from(&step.context(), &mut headers)` writes from a
handle's context instead. In the service that answers,
`telmoni.extract(&headers)` gives `Some(run)`, continued there: its steps and
calls nest under the caller's span and carry the caller's run, and nothing of
the run itself is sent from this side, so ending it sends nothing. Both take
any `Carrier`, a type that is both an OpenTelemetry `Injector` and
`Extractor`, as `HashMap<String, String>` is. Headers that name no run give
`None`.

## Content

Prompts, completions, tool arguments and results are content. They are set on
the span, and the exporter takes them off every span before it leaves the
process unless `capture_content` is on **and** the project's content mode,
learned from the platform's answers, is `on`. Before the first answer the mode
is unknown, so content stays home.
