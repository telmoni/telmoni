# Telmoni Rust SDK

`telmoni`, the Rust SDK for [Telmoni](https://github.com/telmoni/telmoni),
privacy-first, open-source monitoring for AI agents: an agent's runs, steps,
model calls, tool calls and retrievals as OpenTelemetry spans, sent to Telmoni
and nowhere else. The client owns a
tracer provider of its own, built part by part so no `OTEL_*` variable reaches
it, and runs beside whatever OpenTelemetry the application already has: a run
opened while an application span is current joins its trace, and the
application's spans stay with the application's exporter. The client takes prompts,
completions and tool payloads off every span before it leaves the process
unless you ask and the project allows.

## Installation

Rust 1.88 or later:

```console
cargo add telmoni
```

The hosted service at `https://api.telmoni.com`, the SDK's default endpoint,
is not open yet, nor are the docs at telmoni.com/docs. Until they are,
[run Telmoni yourself](https://github.com/telmoni/telmoni#self-hosting) and
point `TELMONI_ENDPOINT` at its machine host; the docs read in
[`web/content/docs`](https://github.com/telmoni/telmoni/tree/main/web/content/docs).

## Quick start

```rust
use telmoni::{
    Config, ModelCallOptions, Reply, RetrievalOptions, RunOptions, Telmoni, ToolCallOptions, Usage,
};

fn main() -> Result<(), telmoni::Error> {
    // Reads TELMONI_ENDPOINT, TELMONI_API_KEY, TELMONI_ORG, TELMONI_ENVIRONMENT, TELMONI_RELEASE,
    // TELMONI_CAPTURE_CONTENT, TELMONI_SCRUB_SECRETS and TELMONI_SCRUB_PII.
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
    let found = run.retrieval(
        "kb-articles",
        RetrievalOptions {
            query: Some("refunds".into()),
            top_k: Some(5),
            ..RetrievalOptions::default()
        },
    )?;
    found.returned(&index.search("refunds", 5)); // your vector store's call, a Vec<serde_json::Value>
    drop(found);
    run.end();

    telmoni.shutdown();
    Ok(())
}
```

Every handle is a guard: its span ends when it is dropped or on `end()`. A
retrieval is a search of a vector store or an index, sent as
`retrieval <source>`, or with `memory: true` of a memory store, sent as
`search_memory`; `returned(&documents)` takes the slice it found and sends how
many it holds in every mode beside the documents themselves, content
([Content](#content)), the last slice given standing whole.
`run` returns `Error::EmptyAgent` for a blank agent name, and `Error::MaxSteps` or
`Error::MaxSpend` for a cap that is no cap ([The guard](#the-guard)). A
negative token count, or a `Usage`'s `cost` below 0 or not finite, is left
off its call with one warning; a run sends its first 32 tags by key, the most
ingest keeps, and warns once of the rest. A tag whose key
is not 1 to 64 characters or whose value is over 256 is left off with one
warning first and takes no place among them. Past ingest's caps, counted in
characters, a name, the agent, a model, a provider, a response model or an
error type is cut to its cap — a step's or a tool's name or a retrieval's
source to 145, the rest to 128 — and a user past 128 or a session past 200 is
left off, since cut it would name another; a step named nothing goes as `step`,
a model call with no model and a retrieval with a blank source are named by
their operation alone, and a model call of an operation ingest reads as no model
call, any but `chat`, `generate_content`, `text_completion` and `embeddings`, goes as `chat`;
each with one warning, where ingest would refuse the span, or the whole run for
its agent, user or session, or file the call elsewhere. `ModelCallOptions`'
`prompt` and `prompt_version` name the prompt the call used, sent on its span in
every content mode, so a prompt's name names a prompt and never a person, who
goes in `user_id`; a blank one is none, and one past ingest's 1,024 characters
is left off, never cut, as is a version whose name is none or was left off, each
with one warning. A recorded call names no prompt.
`fail("ErrorType")` marks a span failed with the error's type, none if empty,
and no message; `cancel()` marks it cancelled by the agent. The `_with`
methods take a closure and return its value, or the `Error` that kept their
handle from opening — `telmoni.run_with(..)`, `run.step_with(..)`,
`model_call_with`, `tool_call_with`, `retrieval_with` — and their `try_` forms, for a closure
whose error converts from `telmoni::Error` and implements `Display`, mark an
`Err` on the span with the error type's name, and its message as an `exception`
event and the status's message, content ([Content](#content)), before returning
it unchanged; `run.on_result(result)` does the same for a run held as a guard:

```rust
// MyError implements From<telmoni::Error> and Display.
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

A step holds its own calls, steps among them, a subagent's own subagents say,
up to 16 deep, each under the step that holds it:
`StepRecord { calls: vec![..], ..StepRecord::new(..) }`. A model call's
`reasoning_effort`, how hard it was asked to reason, is sent in every content
mode. Times are `SystemTime`s, which `chrono`'s and `time`'s convert into with
`SystemTime::from`.

It is sent as the live calls would have sent it, through the same exporter and
content rules, in a trace of its own named by the run's id,
whatever trace is current. Each span's id is derived from the run's id and the
span's place in the record, so the same record sent again is the same bytes,
which ingest answers once: give a run the same id each time you send it. A
record's content is its run's messages (`input_messages`, `output_messages`),
a model call's (`input_messages`, `output_messages`), a tool call's `arguments` and `result`,
and a step's messages, a subagent's task and answer; its tags and token counts go as a live run's do.
A blank agent (`Error::EmptyAgent`), a run id that is not a UUID or is the nil
UUID (`Error::RunId`), an empty step name or model (`Error::EmptyName`), text
past ingest's caps (`Error::TooLong`: a step's or a tool's name 145
characters, the agent, models, providers and error types 128, the user 128,
the session 200, and a reasoning effort 1,024), a step nested past 16 deep
(`Error::TooDeep`), a time
at or before the Unix epoch or 2^64
nanoseconds or more after it, in July 2554 (`Error::RecordedTime`), or a span
that ends before it starts (`Error::EndBeforeStart`) is returned as an error,
and nothing is sent. A record the export queue has no room for is
`Error::QueueFull`, nothing of it queued: flush, or wait, and record it again,
from a clone, since `record_run` takes the record. One of more spans than the
queue holds, 2,048 unless `Hooks::batching` raises it, is never taken, and its
error says so. One made once the client is shut down is `Error::Shutdown`.

## Scores

A score is a name and a number, a yes or no, or a category. On a handle still
open, `run.score(Score::number("relevance", 0.75).range(0.0, 1.0).version("judge-2"))`
adds it to that span, timed now; `Score::boolean` is a yes or no, and
`Score::category` a category. Once it has ended, the client's `score` sends
it, naming what it scores — a run, a run and one of its spans (`run.run_id()`,
`step.span_id()`), or a session:

```rust
telmoni.score(&ScoreTarget::run(&run_id), Score::boolean("thumbs", true));
telmoni.score(&ScoreTarget::span(&run_id, &span_id), Score::number("relevance", 0.5));
telmoni.score(&ScoreTarget::session("conv_7"), Score::category("outcome", "resolved"));
```

It goes with the next export, through the same queue, so `flush` and
`shutdown` hold it. A recorded run, and each step and call in it, takes
`scores: Vec<Score>`, each timed at that span's end. A name, a category or a
version is trimmed and up to 64 characters; a number is finite and within its
range; a span holds up to 16. A score that breaks a rule never panics or
errs: it is not sent, with one warning. A score is no content: it is sent
whatever `capture_content` and the project's mode say, and never handed to
the mask.

## The guard

`RunOptions { max_steps: Some(20), max_spend: Some(0.50), .. }` holds a run
to caps on its steps and on what its model calls cost, in US dollars, counted
in this process with no call to Telmoni. Past one, `step`, `model_call`,
`tool_call` and `retrieval` return `Error::CapExceeded` (its `cap`, `Cap::Steps` or
`Cap::Spend`, its `limit` and what the run had `used`) and open nothing: a
step past the step cap, and anything once the run spent its spend cap or more.
A call counts its reported `cost`; else its tokens at a price handed as
`Config { prices: vec![Price { .. }], .. }`, which the SDK alone holds, so
Telmoni's own reads still price that call by the table; else at the price
table this crate carries, LiteLLM's, by its provider and the model it answered
as; else nothing. A call answered again counts as its span then holds it.
`run.spent()` reads the count; a continued run carries no cap.

## Configuration

`Config::from_env()` reads eight variables, a blank value being unset:

| Variable | What |
|---|---|
| `TELMONI_ENDPOINT` | Where the platform is; `https://api.telmoni.com` unless set. |
| `TELMONI_API_KEY` | The project's API key, with the `ingest` scope. |
| `TELMONI_ORG` | The organization (`org_…`). |
| `TELMONI_ENVIRONMENT` | The deployment this process runs in (`production`, `staging`), sent as `deployment.environment.name`. |
| `TELMONI_RELEASE` | The release it runs (a version or a commit), sent as `service.version`. |
| `TELMONI_CAPTURE_CONTENT` | `1` alone, its blanks about it trimmed, turns `capture_content` on; anything else, or nothing, leaves it off. A `capture_content` set in code over `Config::from_env()` stands. |
| `TELMONI_SCRUB_SECRETS` | `0` alone, trimmed, turns the secrets scrubber off; anything else, or nothing, leaves it on ([Content](#content)). |
| `TELMONI_SCRUB_PII` | `1` alone, trimmed, turns the PII scrubber on; anything else, or nothing, leaves it off. |

Nothing in the SDK reads an `OTEL_*` variable. `Config` also holds
`capture_content` (default `false`), `mask`, the code's own over content
([Content](#content)), which no variable sets and `Debug` shows only as set or
not, `scrub_secrets` and `scrub_pii`, each `None` read from its variable above
however the configuration was built and a `Some` set in code standing over it,
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

Prompts, completions, tool arguments and results, and a retrieval's query and
documents are content, and so are a run's own messages: `RunOptions::input_messages` and
`run.answered(RunReply { output_messages })`, a failure's message, its
`exception` event and its status's message, and every event the application
adds to a span through OpenTelemetry, a score's own keys apart ([Scores](#scores)). They are set on
the span, and the exporter takes them off every span before it leaves the
process unless `capture_content` is on, in code or by `TELMONI_CAPTURE_CONTENT=1`
under `Config::from_env()`, **and** the project's content mode,
asked of the platform before the first content it would send and learned
again from every answer, is `on`; while the mode is unheard, content stays
home. `telmoni.content_mode()` hands it back as
`Some(ContentMode::Off)` or `Some(ContentMode::On)`: `None` before an answer, or
for a word ingest does not send, which keeps content home too.

An image or file a message carries, a `blob` part holding its bytes in base64
in `input_messages`, `output_messages` or `system_instructions`, never travels
inside the span: where content is sent, the exporter puts its bytes to
`PUT {endpoint}/v1/media/{sha256}` with the same key, once a client for each
file, and sends the part as a `file` part whose `file_id` is
`telmoni:<sha256>`, its `modality` and `mime_type` kept. A file past 5 MiB is
not put, and a put that fails is warned of once (`WarningKind::UploadFailed`);
either way the part goes as the file it names. A part whose content is no
base64 goes as it is. A `Transport` of your own is handed both, by its
`Method`: an export's `Post` and a file's `Put`.

To take something out of content before it leaves, a card number or an email
address, set `Config::mask`, a `telmoni::Mask`. Where content is sent, and only
there, the exporter hands it each content field's key and its value —
`gen_ai.input.messages` as its JSON, an event's attribute under its own key, a
failure's message as `exception.message` and its status's as
`otel.status_description`; a value that is no string as its JSON — and sends the
string it answers instead, or the span without that field where it answers
`None`. It runs on the processor's thread, once a field each export, before any
image is put, so a part it takes out is never uploaded. A mask that panics sends
the span without that field, warned once (`WarningKind::MaskFailed`) by the key
alone; under `panic = "abort"` its panic ends the process, as any does. It sees
the content the SDK records, never a span's name, a tag or a user id, which are
metadata kept on every project, nor an attribute your code sets on a span
itself, which leaves the process whatever `capture_content` says and which a
project keeping content keeps too: keep a person out of those in your own code.

```rust
use std::sync::Arc;

let config = telmoni::Config {
    capture_content: true,
    mask: Some(Arc::new(|_key: &str, value: &str| {
        Some(value.replace("4242 4242 4242 4242", "[card]"))
    })),
    ..telmoni::Config::from_env()
};
```

After the mask, and after its images are lifted out, content that leaves goes
through the scrubbers, on the fields the mask is handed. The secrets scrubber,
on unless `scrub_secrets` is `Some(false)`, or unset with
`TELMONI_SCRUB_SECRETS=0`, sends each private key, connection string, JWT,
provider key and value set by a keyword such as `password=` as
`[secret: <kind>]`; the PII scrubber, off unless `scrub_pii` is `Some(true)`, or
unset with `TELMONI_SCRUB_PII=1`, sends each email, IBAN, card number, SSN,
phone number, IP address and home folder as `[pii: <kind>]`. A span they
changed carries `telmoni.scrubbed`, what they replaced by kind as JSON
(`{"pii:email":1,"secret:aws_key":2}`); a span goes without a field they fail
on, counted as `dropped:<field>`, never with it as it was. They take time in
proportion to the text, whatever it holds.
