# SDK fixtures

One contract across the four SDKs: what each must send for the same run, and what each must do with the answers the server gives. Every language's suite replays these files, so no SDK ships a behaviour the others lack (`ARCHITECTURE.md`, *The SDKs*). The server's side of the contract is the core's `crates/telemetry/src/contract.rs` and its ingest handler; a disagreement is settled there, never here.

## `run.json`

`scenario` is the run every suite performs; `export` is the OTLP/JSON body the SDK must have sent for it, in `ExportTraceServiceRequest`'s JSON encoding, after the normalization below. The SDK sends `telmoni.run.start` at once, in an export of its own, and the rest when the run ends, so a suite gathers every body its fake transport received and compares their spans together.

Before comparing, a suite normalizes what it captured:

1. Spans are sorted by `name`, and every `attributes` array by `key`.
2. Every `traceId` is checked to be the same 32 lowercase hex characters, then replaced by `<trace>`.
3. Each `spanId` is replaced by `<span:NAME>`, its span's name; each `parentSpanId` by `<span:NAME>` of the span it names, which must be in the export. A root span has no `parentSpanId`.
4. `startTimeUnixNano` and `endTimeUnixNano` are checked to be decimal strings with end no earlier than start, and equal on `telmoni.run.start`, then replaced by `<time>`.
5. `telmoni.run_id` is checked to be one UUID (8-4-4-4-12 lowercase hex, version 4) on every span, then replaced by `<uuid>`.
6. `telemetry.sdk.language` is replaced by `<language>`; `telemetry.sdk.version`, `telemetry.distro.version` and the scope's `version` by `<version>`.
7. Everything else is compared as JSON: names, kinds, every attribute's key and typed value (`stringValue`; `intValue` as a decimal string; `doubleValue` as a number; `boolValue`), each span's `status`, the resource and the scope's name. A span with no error carries no `status`; one that failed carries `{"code": 2}`.

The content the scenario hands the model call (`gen_ai.input.messages`, `gen_ai.output.messages`) is absent from the export: the exporter takes content off every span unless the code asked for it and the project's mode allows.

## `recorded.json`

`scenario.record` is a run that has already ended, handed whole to `record_run` (`recordRun`, `RecordRun`): its id, its agent, user, session and tags, its times, and what ran inside it, a step holding its own calls. Times are RFC 3339 in UTC, to the millisecond, which every language holds exactly. `export` is the one body the SDK must send for it: a record travels in a single export when it holds no more spans than a batch, its `telmoni.run.start` first, then each span as it ended — a step after its calls — and the run's own last.

Nothing is derived at random, so the export is compared nearly whole:

1. Exactly one body was sent.
2. Every `attributes` array is sorted by `key`, the resource's too; spans keep the order they were sent in.
3. `telemetry.sdk.language` is replaced by `<language>`; `telemetry.sdk.version`, `telemetry.distro.version` and the scope's `version` by `<version>`.
4. Everything else is compared as JSON, ids and times included. The trace's id is the run's, without its hyphens. Each span's id is the first eight bytes, in hex, of the version 5 UUID (RFC 9562) named by the span's place in the record, in decimal, under the run's id: `0` the run, `1` its start, then each step and call in the record's order, a step before its calls.

The same record sent twice must be the same bytes, which ingest's claim on a body answers once.

## `answers/`

Each file is one answer from `POST /otlp/v1/traces` — `status`, `headers`, `body` — and `expect`, what the SDK must do with it. A suite's fake transport answers from these files; the asserted behaviour is the `expect` line.

| File | Answer | The SDK |
|---|---|---|
| `kept.json` | `200`, `Telmoni-Content-Mode: off`, no partial success | logs nothing; its content mode is now `off` |
| `dropped.json` | `200`, a partial success naming an attribute dropped as content | warns once, naming the attribute, the project's content mode that decides the drop, and the page that says which modes keep content (`https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door`); the same attribute named again is not logged again |
| `unlisted.json` | `200`, a partial success naming a text attribute the inventory does not name | warns once, naming the tag that would keep it |
| `refused.json` | `200`, a partial success refusing one span | warns once with the server's line; never retries |
| `rate-limited.json` | `429` with `Retry-After` | waits `Retry-After` seconds, through its injected sleep, then sends the same bytes again |
| `unavailable.json` | `503` with `Retry-After` | waits and retries, three attempts in all, then drops the batch with one warning |
| `forbidden.json` | `403` | never retries; warns once naming the problem type; later batches are still sent |

Partial success lines are the server's: `span <id> refused: <why>` and `span <id> dropped <attribute>: <reason>; <what lifts it>`, with `and N more` once 4 KiB is used (`crates/telemetry/src/ingest/report.rs`). A failure's body is OTLP's `Status`, its `message` opening with the problem type, which the SDK's warning names.
