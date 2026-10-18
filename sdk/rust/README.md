# Telmoni Rust SDK

`telmoni-sdk`: an agent's runs, steps, model calls and tool calls as
OpenTelemetry spans, sent to Telmoni and nowhere else. The client owns a
tracer provider of its own, built part by part so no `OTEL_*` variable reaches
it, and runs beside whatever OpenTelemetry the application already has: a run
opened while an application span is current joins its trace, and the
application's spans stay with the application's exporter.

## Installation

Not published yet. Depend on it from this repository:

```toml
[dependencies]
telmoni-sdk = { git = "https://github.com/telmoni/telmoni" }
```

## Quick start

```rust
use telmoni_sdk::{Config, ModelCallOptions, Reply, RunOptions, Telmoni, ToolCallOptions, Usage};

fn main() -> Result<(), telmoni_sdk::Error> {
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
        let step = run.step("plan");
        let call = step.model_call(
            "gpt-4o-mini",
            ModelCallOptions {
                provider: Some("openai".into()),
                ..ModelCallOptions::default()
            },
        );
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
    let tool = run.tool_call("search", ToolCallOptions::default());
    tool.returned(serde_json::json!({ "results": 3 }));
    drop(tool);
    run.end();

    telmoni.shutdown();
    Ok(())
}
```

Every handle is a guard: its span ends when it is dropped or on `end()`.
`fail("ErrorType")` marks a span failed with the error's type and no message;
`cancel()` marks it cancelled by the agent. The `_with` methods take a closure
and return its value — `telmoni.run_with(..)`, `run.step_with(..)`,
`model_call_with`, `tool_call_with` — and their `try_` forms mark an `Err` on
the span with the error type's name before returning it unchanged:

```rust
let answer: Result<String, MyError> = telmoni.try_run_with("support-bot", RunOptions::default(), |run| {
    run.try_step_with("plan", |_step| plan())
});
```

A run's `context()` carries it into a future (`FutureExt::with_context`) or
another thread, and `attach()` makes it current for synchronous code, so the
application's own spans nest under it.

## A run recorded after it ended

A job that keeps its own log, or a hook that runs once the turn it reports is
over, hands the finished run to `record_run` as data:

```rust
use std::time::{Duration, SystemTime};

use telmoni_sdk::{ModelCallRecord, RunRecord, ToolCallRecord, Usage};

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
record holds no content. A blank agent (`Error::EmptyAgent`), a run id that is
not a UUID or is the nil UUID (`Error::RunId`) or a span that ends before it starts
(`Error::EndBeforeStart`) is returned as an error, and nothing is sent. A record
the export queue has no room for is `Error::QueueFull`, nothing of it queued:
flush, or wait, and record it again.

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
a trace of its own, and `environment` and `release`, the deployment the
variables above name. The API key is never printed or serialized: `Debug`
shows it redacted, and `Serialize` leaves it out. It travels over HTTPS only:
with a plain-HTTP endpoint whose host is not this machine, nothing is sent and
the first export warns once, and no redirect is followed.

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
