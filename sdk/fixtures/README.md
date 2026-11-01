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
5. The one warning is for the call whose operation is `rerank`, which ingest reads as no model call, so it goes as `chat`: `telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat`, its `kind` `input` (`answers/`, below).

The same record sent twice must be the same bytes, which ingest's claim on a body answers once.

## `failure.json`

A failure in full, under the content lock. For each of `cases`, a client is built with its `capture_content`, learns its `content_mode` from the answer to an empty run's export (`Telmoni-Content-Mode`), then opens a run of `scenario`'s `agent` in which the application adds `event` to the run's span through the OpenTelemetry API, and the run ends by the language's own failure path with an error of the type `error` whose message is `message`: Python's exception raised out of the `with`, TypeScript's error thrown from the callback, Go's error handed to `Fail`, Rust's `Err` returned from a `try_` closure. The run's span, as sent, must hold `status` and `events` as `kept` or `withheld` say, whichever the case's `sends` names; `withheld` has no `events` key, and the span must carry none.

Before comparing, a suite normalizes the run's span:

1. Every event's `timeUnixNano` is checked to be a decimal string from the span's start to its end, then replaced by `<time>`; events keep their order, the application's first.
2. Every event's `attributes` array is sorted by `key`.
3. `exception.type` is checked to be the span's `error.type`, as each language names an error's type, then replaced by `<error.type>`.
4. `exception.stacktrace` is checked to be a non-empty string and taken out in Python and TypeScript, which give one cheaply; Go and Rust send none.

## `media.json`

The images and files a message carries, lifted out of it under the content lock. For each of `cases`, a client keyed `k` is built with its `capture_content`, learns its `content_mode` from the answer to an empty run's export (`Telmoni-Content-Mode`), then opens two runs of `scenario`'s `agent` one after the other, each flushed, each holding one model call of its `model` and `provider` opened with `input_messages` and `system_instructions` and answered with `output_messages`. Every upload, a `PUT`, is answered `204`.

Where `sends`, the uploads, sorted by path, are `uploads` — each its path under the endpoint, its `Content-Type`, and its body in base64 — each with `Authorization: Bearer k`, made once however many spans and exports name the file; and each model call's span holds the three fields, each parsed from its JSON and compared as JSON, as `sent`: each `blob` part a `file` part naming its file by `telmoni:` and the SHA-256 of its bytes, its `modality` and `mime_type` kept where given, and a part whose content is no base64, a `uri` or `file` part, or a part that is no object, as it was. A blob with no `mime_type`, or one blank or holding anything but printable ASCII, is put as `application/octet-stream`, its part keeping the type as given. Where not, nothing is put and neither model call carries any of the three.

`max_bytes` is the largest file put, which every suite holds its SDK's own cap to, as the server's tests hold its lane's, with its path and its `telmoni:` names. The rest each suite proves on its own: a failed upload, a `413` or no answer, still sends the part as its `file` part and warns once (`upload_failed`, with the status where one came), and its hash is tried again on the next export, being remembered only once kept; a file past `max_bytes` is never put, and its part is sent as its `file` part all the same.

## `ask.json`

A client asks the project's content mode before the first content it would send, so a process that sends once, a hook's, still sends its content. For each of `cases`, a client keyed `k` is built with its `capture_content`; its fake transport answers each request in turn from `answers` — a word, `200` with `Telmoni-Content-Mode` naming it and a body of `{}`; a number, that status with no header and no body — then as `kept.json` does. The client runs two runs of `agent`, each flushed, each holding one model call of `model`, opened with `input_messages` where `content` is true and with none where it is false.

`requests` is how many requests the client made in all. Where `asked`, the first was the ask: a `POST` to the traces lane with the export's headers and `body`, an export of no spans, which ingest answers under the project's mode as it is then, never as it answered an earlier one; where not, nothing was asked. `sent` is whether the first run's export carries `gen_ai.input.messages` on its model call. A client asks once and silently, before an export holding content, while no answer has named the mode; an ask met with anything but a `2xx` leaves the mode unheard, and the export goes without its content.

## `capture_content.json`

What each SDK makes of `TELMONI_CAPTURE_CONTENT` as it builds its configuration from the environment (`Config.from_env`, `Telmoni.fromEnv`, `ConfigFromEnv`, `Config::from_env`). Each of `cases` sets `variable`, `TELMONI_CAPTURE_CONTENT`, to `given`, or leaves it unset where that is `null`, and the configuration must hold `capture_content` as it says: the value, Unicode's White_Space trimmed from its ends as every variable is, turns content on when it is `1` alone; `0`, `true`, `yes` and every other value, a blank one or none leave it off, so no spelling of "false" lets content out. Each suite proves on its own that a setting made in code stands over the variable, on or off.

## `content.json`

The attribute names every SDK takes off a span unless the code asked for content and the project's mode allows: `content` holds each, and each suite holds its own list to it, as the server's `contract::CONTENT` is held to it, so a name added on one side alone fails the other. An attribute an application sets under one of these names through the OpenTelemetry API is held back as the SDK's own content is.

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

Each file is one answer from `POST /otlp/v1/traces` — `status`, `headers`, `body` — and `expect`, what the SDK must do with it. A suite's fake transport answers from these files; the asserted behaviour is the `expect` line. `warning` is the one line the SDK logs, word for word, when every attempt of an export meets that answer, or `null` for none, and `fields` what it hands beside that line, so a reader decides what became of its spans without parsing the words: the warning's `kind`, and for an export's own failure the `status` it last met and the `problem_type` its body named, each left out where there is none.

Every warning any SDK gives carries one of these kinds, the same in all four:

| `kind` | What became of the spans | `status`, `problem_type` |
|---|---|---|
| `refused` | an export refused for good, never retried, and its batch dropped; Rust's endpoint refused too, nothing sent | the answer's, where one came |
| `failed` | an export failed every attempt, and its batch dropped | the last attempt's, as its line names them; none where that attempt met a transport error, whatever earlier ones met |
| `queue_full` | spans dropped before sending, the queue full | — |
| `export_error` | a batch its own export dropped: one it could not encode, or whose export raised, threw or panicked — a hook the application gave among them | — |
| `too_large` | a span alone past an export's 1 MiB dropped, its own name too long to trim | — |
| `trimmed` | a span alone past 1 MiB sent without its longest content, named in `telmoni.content_cut`, or without the attributes ingest drops for their length | — |
| `dropped` | an attribute the server dropped, the span kept | — |
| `span_refused` | a span the server refused, the rest of the export kept | — |
| `said` | a line the server gave naming neither | — |
| `input` | something the SDK was handed fixed, cut or left off as it sent the rest, or not sent at all — a field past its cap, a count that is none, a call outside a run | — |
| `upload_failed` | spans sent, an image or file their messages carried not put: its `file` part names it all the same (`media.json`) | the upload's `status`, where an answer came |

| File | Answer | The SDK |
|---|---|---|
| `kept.json` | `200`, `Telmoni-Content-Mode: off`, no partial success | logs nothing; its content mode is now `off` |
| `dropped.json` | `200`, a partial success naming an attribute dropped as content | warns once, naming the attribute, the project's content mode that decides the drop, and the page that says which modes keep content (`https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door`); the same attribute named again is not logged again |
| `unlisted.json` | `200`, a partial success naming a text attribute the inventory does not name | warns once, naming the tag that would keep it |
| `cardinality.json` | `200`, a partial success naming a tag past its project's cap for the day | warns once, naming the tag, and that tags keep to a few keys of a few values each |
| `refused.json` | `200`, a partial success refusing one span | warns once with the server's line; never retries |
| `rate-limited.json` | `429` with `Retry-After` | waits `Retry-After` seconds, through its injected sleep, then sends the same bytes again |
| `rate-limited-long.json` | `429` with a `Retry-After` of 20 digits, past what any of the four could hold as a whole number | waits the cap, 60 seconds, then sends the same bytes again: the header is read as text, ASCII digits with a sign allowed, below 0 none and past 60 the cap |
| `unavailable.json` | `503` with `Retry-After` | waits and retries, three attempts in all, then drops the batch with one warning |
| `bad-gateway.json` | `502` from a load balancer, an HTML body and no `Retry-After` | waits 2^attempt seconds and retries, three attempts in all, as OTLP retries a `502`, then drops the batch with one warning naming the status alone |
| `gateway-timeout.json` | `504` from a load balancer, a plain-text body | retries as for a `502` |
| `forbidden.json` | `403` | never retries; warns once naming the problem type; later batches are still sent |
| `said.json` | `200`, a partial success whose line names neither a drop nor a refusal | warns once with the server's line, as it said it |

Partial success lines are the server's: `span <id> refused: <why>` and `span <id> dropped <attribute>: <reason>; <what lifts it>`, with `and N more` once 4 KiB is used (`crates/telemetry/src/ingest/report.rs`). A failure's body is OTLP's `Status`, its `message` opening with the problem type, which the SDK's warning names.
