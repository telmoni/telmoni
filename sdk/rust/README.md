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

## Configuration

`Config::from_env()` reads three variables, a blank value being unset:

| Variable | What |
|---|---|
| `TELMONI_ENDPOINT` | Where the platform is; `https://api.telmoni.com` unless set. |
| `TELMONI_API_KEY` | The project's API key, with the `ingest` scope. |
| `TELMONI_ORG` | The organization (`org_…`). |

Nothing in the SDK reads an `OTEL_*` variable. `Config` also holds
`capture_content` (default `false`), `sample_rate` (default `1.0`; above 0 and
up to 1, decided per run and never through the trace's sampled flag) and
`sampler`, the OpenTelemetry sampler whose decision a run takes when it starts
a trace of its own. The API key is never printed or serialized: `Debug` shows
it redacted, and `Serialize` leaves it out. It travels over HTTPS only: with a
plain-HTTP endpoint whose host is not this machine, nothing is sent and the
first export warns once, and no redirect is followed.

## Content

Prompts, completions, tool arguments and results are content. They are set on
the span, and the exporter takes them off every span before it leaves the
process unless `capture_content` is on **and** the project's content mode,
learned from the platform's answers, is `on`. Before the first answer the mode
is unknown, so content stays home.
