# SDK fixtures

One contract across the four SDKs: what each must send for the same run, and what each must do with the answers the server gives. Every language's suite replays these files, so no SDK ships a behaviour the others lack (`ARCHITECTURE.md`, *The SDKs*). The server's side of the contract is the core's `crates/telemetry/src/contract.rs` and its ingest handler; a disagreement is settled there, never here.

## `run.json`

`scenario` is the run every suite performs, and how the client is built for it: `capture_content`, and the `environment` and `release` the client names its deployment by, which the resource carries as `deployment.environment.name` and `service.version`; `export` is the OTLP/JSON body the SDK must have sent for it, in `ExportTraceServiceRequest`'s JSON encoding, after the normalization below. The run's spans may travel in more than one export, so a suite gathers every body its fake transport received and compares their spans together.

Before comparing, a suite normalizes what it captured:

1. Spans are sorted by `name`, and every `attributes` array by `key`.
2. Every `traceId` is checked to be the same 32 lowercase hex characters, then replaced by `<trace>`.
3. Each `spanId` is replaced by `<span:NAME>`, its span's name; each `parentSpanId` by `<span:NAME>` of the span it names, which must be in the export. A root span has no `parentSpanId`.
4. `startTimeUnixNano` and `endTimeUnixNano` are checked to be decimal strings with end no earlier than start, then replaced by `<time>`.
5. `telmoni.run_id` is checked to be one UUID (8-4-4-4-12 lowercase hex, version 4) on every span, then replaced by `<uuid>`.
6. `telemetry.sdk.language` is replaced by `<language>`; `telemetry.sdk.version`, `telemetry.distro.version` and the scope's `version` by `<version>`.
7. Everything else is compared as JSON: names, kinds, every attribute's key and typed value (`stringValue`; `intValue` as a decimal string; `doubleValue` as a number; `boolValue`), each span's `status`, the resource and the scope's name. A span with no error carries no `status`; one that failed carries `{"code": 2}`.

The content the scenario hands the model call (`gen_ai.input.messages`, `gen_ai.output.messages`) is absent from the export: the exporter takes content off every span unless the code asked for it and the project's mode allows.

Every request carries `Telmoni-Sent-At`, the SDK's clock as the attempt leaves, in decimal nanoseconds since the Unix epoch, stamped anew on each attempt, by which ingest moves an export from a clock a zone or a date out.

## `recorded.json`

`scenario.record` is a run that has already ended, handed whole to `record_run` (`recordRun`, `RecordRun`): its id, its agent, user, session and tags, its times, and what ran inside it, a step holding its own calls. Times are RFC 3339 in UTC, to the millisecond, which every language holds exactly. `export` is the one body the SDK must send for it: a record travels in a single export when it holds no more spans than a batch, each span as it ended — a step after its calls — and the run's own last.

Nothing is derived at random, so the export is compared nearly whole:

1. Exactly one body was sent.
2. Every `attributes` array is sorted by `key`, the resource's too; spans keep the order they were sent in.
3. `telemetry.sdk.language` is replaced by `<language>`; `telemetry.sdk.version`, `telemetry.distro.version` and the scope's `version` by `<version>`.
4. Everything else is compared as JSON, ids and times included. The trace's id is the run's, without its hyphens. Each span's id is the first eight bytes, in hex, of the version 5 UUID (RFC 9562) named by the span's place in the record, in decimal, under the run's id: `0` the run, then each step and call from `1` in the record's order, a step before its calls.
5. The one warning is for the call whose operation is `rerank`, which ingest reads as no model call, so it goes as `chat`: `telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat`.

The same record sent twice must be the same bytes, which ingest's claim on a body answers once.

## `headers.json`

A run across services: what `inject` writes into an outgoing request's headers, and what `extract` reads back from an incoming one's, so headers one SDK writes are read by each of the other three.

`inject` is performed from inside a step of a run of `agent`: `traceparent` and `baggage` are what the headers must hold afterwards, `<trace>` and `<span>` standing for the step's trace and span ids as the export spells them, and `<uuid>` for the run's id. `beside` gives a `baggage` the headers held already (`given`) and what they hold afterwards (`written`): the carrier's own members kept, in their order, and ours — `telmoni.run_id` and `telmoni.agent` — replaced where they were there and written last, the agent percent-encoded as the W3C baggage grammar asks (its `baggage-octet` kept as it is, every other byte of the UTF-8 as `%XX`, and `%` itself encoded, since a reader decodes it).

`extract` is a list of cases, each the `headers` given — their names in whatever case, matched as the language's headers are (Rust's `HashMap` carrier lowercases them on both sides, so its suite lowercases the fixture's) — and the `run` read, or `null` where the headers name none: its `run_id` (lowercase, whatever case it was given in), its `agent` (percent-decoded, a character given unescaped taken as its UTF-8, bytes that are no UTF-8 read one U+FFFD for each maximal subpart, as Unicode's practice has it, a byte-order mark kept as a character, and cut to ingest's 128 characters), and the `trace_id`, `parent_span_id` and `sampled` flag its steps and calls open under. A run is read only from a well-formed `traceparent` — a version other than `ff`, lowercase hex, a trace id and a span id that are not zeros, nothing after the flags under version `00` — and a `baggage` holding both members, the id a UUID and the agent not blank.

## `guard.json`

The guard, which counts a run's steps and spend in the agent's own process, with no call to Telmoni. `prices` are handed to the client, which counts by them before the table and never sends them; each SDK carries the price table beside them, a byte-for-byte copy of the core's `crates/telemetry/model_prices.json`, which its suite holds to the core's.

`calls` are model calls, each opened inside a run of its own with no cap, each with its `provider` and `model` (either may be absent), and answered with its `response_model` and `usage` where given, or once with each of its `answers` in order. `counted` is the run's spend once the call is answered, in US dollars, compared exactly, as its span then holds it, each attribute as last sent: its cost as reported where it is a number from 0, else its tokens at the price handed for its provider and the model it answered as, else at the table's, else nothing — its input less the cache's at the input price, each cache count at its own (a cache price not given being the input's), and its output at the output price, the four products added in that order, each product and each sum rounded as it is made, as the server rounds them. A multiply fused into its add, as Go compiles one on arm64 unless each product is converted, sums the call of 1,000 tokens in, 800 out, 400 read from the cache and 200 written to it as 0.014070000000000001, not 0.01407. Go's `Usage` sends its four counts whole, a 0 among them, so no case answered again rests on a count a later answer left off. None of these names is the table's but `openai`'s `gpt-4o`, which a price handed shadows, so no case moves when the pin does.

`runs` are runs opened with `max_steps` and `max_spend` where given, performing `ops` in order directly under the run: `step` opens a step and ends it, `tool_call` a tool call, and `{"model_call": n}` the model call `calls[n]`, answered as there. `refused` is the op that raises — before it opens anything, and `null` where none does — with the cap it names (`steps` or `spend`), the cap's `limit`, and what the run had `used` of it: its steps opened, or its spend. A run reaches its spend cap once its spend is the cap or more, and then whatever it opens raises; it reaches its step cap once it has opened that many steps, and then its next step raises. `spent` is the run's spend once its ops are done, or at the refusal. A run's amounts are compared to within 1e-12.

`refusals` are refusals built from a `cap`, its `limit` and what the run had `used`, and what each `says`, word for word the same in all four: each amount the shortest decimal that reads back as it, written out with no exponent.

## `endpoints.json`

What each SDK makes of an endpoint, configured or from `TELMONI_ENDPOINT`. `taken` lists each as `given` and as the SDK holds it (`endpoint`), which, given again, it holds unchanged. `refused` lists each the SDK refuses — at construction in Python, TypeScript and Go, and in Rust at the first export, which sends nothing — `as` `url`, no endpoint any SDK takes (`the endpoint must be an http:// or https:// URL`), or `cleartext`, plain HTTP to another machine (`refusing to send the API key over unencrypted HTTP`).

One rule in all four, written out by each rather than left to its URL parser, since those read shorthand, escapes, credentials and a missing host four ways: Unicode's White_Space trimmed from the ends, as Go and Rust trim; printable ASCII; HTTP or HTTPS, in any case; a host that is a name of letters, digits, dots, hyphens and underscores, in labels of 1 to 63 characters with no hyphen at either end, its last label no number unless the whole is an IPv4 address in four decimal parts, or an IPv6 address in brackets; a port of up to five digits from 1 to 65535, or an empty one; a path of the same plain characters and `~!$&'()*+,;=:@`, with no `.` or `..` segment; and plain HTTP only to this machine: `localhost`, a name under it, 127.0.0.0/8 or `::1`, an IPv4-mapped address read as its IPv4.

## `answers/`

Each file is one answer from `POST /otlp/v1/traces` — `status`, `headers`, `body` — and `expect`, what the SDK must do with it. A suite's fake transport answers from these files; the asserted behaviour is the `expect` line.

| File | Answer | The SDK |
|---|---|---|
| `kept.json` | `200`, `Telmoni-Content-Mode: off`, no partial success | logs nothing; its content mode is now `off` |
| `dropped.json` | `200`, a partial success naming an attribute dropped as content | warns once, naming the attribute, the project's content mode that decides the drop, and the page that says which modes keep content (`https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door`); the same attribute named again is not logged again |
| `unlisted.json` | `200`, a partial success naming a text attribute the inventory does not name | warns once, naming the tag that would keep it |
| `cardinality.json` | `200`, a partial success naming a tag past its project's cap for the day | warns once, naming the tag, and that tags keep to a few keys of a few values each |
| `refused.json` | `200`, a partial success refusing one span | warns once with the server's line; never retries |
| `rate-limited.json` | `429` with `Retry-After` | waits `Retry-After` seconds, through its injected sleep, then sends the same bytes again |
| `unavailable.json` | `503` with `Retry-After` | waits and retries, three attempts in all, then drops the batch with one warning |
| `forbidden.json` | `403` | never retries; warns once naming the problem type; later batches are still sent |

Partial success lines are the server's: `span <id> refused: <why>` and `span <id> dropped <attribute>: <reason>; <what lifts it>`, with `and N more` once 4 KiB is used (`crates/telemetry/src/ingest/report.rs`). A failure's body is OTLP's `Status`, its `message` opening with the problem type, which the SDK's warning names.
