# SDK fixtures

One contract across the four SDKs: what each must send for the same trace, and what each must do with the answers the server gives. Every language's suite replays these files, so no SDK ships a behaviour the others lack (`ARCHITECTURE.md`, *The SDKs*). The server's side of the contract is the core's `crates/telemetry/src/contract.rs` and its ingest handler; a disagreement is settled there, never here.

## `trace.json`

`scenario` is the trace every suite performs, and how the client is built for it: `capture_content`, and the `environment` and `release` the client names its deployment by, which the resource carries as `deployment.environment.name` and `service.version`; `export` is the OTLP/JSON body the SDK must have sent for it, in `ExportTraceServiceRequest`'s JSON encoding, after the normalization below. The trace's spans may travel in more than one export, so a suite gathers every body its fake transport received and compares their spans together.

Before comparing, a suite normalizes what it captured:

1. Spans are sorted by `name`, and every `attributes` array by `key`.
2. Every `traceId` is checked to be the same 32 lowercase hex characters, then replaced by `<trace>`.
3. Each `spanId` is replaced by `<span:NAME>`, its span's name; each `parentSpanId` by `<span:NAME>` of the span it names, which must be in the export. A root span has no `parentSpanId`.
4. `startTimeUnixNano` and `endTimeUnixNano` are checked to be decimal strings with end no earlier than start, then replaced by `<time>`.
5. `telemetry.sdk.language` is replaced by `<language>`; `telemetry.sdk.version`, `telemetry.distro.version` and the scope's `version` by `<version>`.
6. Everything else is compared as JSON: names, kinds, every attribute's key and typed value (`stringValue`; `intValue` as a decimal string; `doubleValue` as a number; `boolValue`), each span's `status`, the resource and the scope's name. A span with no error carries no `status`; one that failed carries `{"code": 2}`.

A trace opened while no application span is current carries no `links`; one opened while an application span is current is a trace of its own, linked to that span, and its agent's span carries one link, the span's `traceId` and `spanId`, which each suite proves on its own.

The content the scenario hands the model call (`gen_ai.input.messages`, `gen_ai.output.messages`) is absent from the export: the exporter takes content off every span unless the code asked for it and the project's mode allows. The prompt it names, `prompt` and `prompt_version`, is metadata, so it is there with content off: `gen_ai.prompt.name` and `gen_ai.prompt.version`, on the call's span alone.

The rest each suite proves on its own: a blank prompt or version is none, without a word; one past ingest's 1,024 characters, counted in characters, is left off, never cut, and so is a version whose name is none or was left off, each with one warning; in Python and TypeScript one that is no string goes as its text, with one warning; with content on, both leave as given, never handed to the mask or the scrubbers; and a recorded call names no prompt.

Every request carries `Telmoni-Sent-At`, the SDK's clock as the attempt leaves, in decimal nanoseconds since the Unix epoch, stamped anew on each attempt, by which ingest moves an export from a clock a zone or a date out.

## `trace-start.json`

A trace is seen from its start, not only from its end. `scenario` opens a trace of `trace` and holds it open, on a client whose batch waits `schedule_delay_ms`, far past the test, and that names its deployment by `environment` and `release`. Before the trace ends, within `sent_within_ms`, exactly one body has been sent, holding one span: `first_export`, after `trace.json`'s normalization, but for its parent, `<span:agent>`, the agent's own span, which is not in it. The span is of no length: its start and end are the same instant.

The suite then ends the trace, flushes, and checks the agent's own span, `invoke_agent <agent>`: its `spanId` is the start's `parentSpanId`, its `startTimeUnixNano` the start's, and its `traceId` the start's.

No other trace sends one: a trace continued from another service's headers sends nothing of its own, and a recorded trace sends exactly `recorded.json`.

## `retrieval.json`

A trace's searches, of a vector store or an index, or of a memory, and what each found. `scenario.trace` is a trace of `agent` for `user_id` and `session_id` performing `inside` in order, directly under the agent's span: each a retrieval of its `source`, opened with its `query` and `top_k`, a memory search where `memory` is true, then answered with `returned(documents)` where it holds `documents`, or failed by its type alone, as `trace.json`'s tool call is, where it holds `fails_with`. For each of `cases`, a client built with its `capture_content`, whose fake transport answers every export naming `content_mode`, performs the trace and flushes; the spans every body carried, gathered into the first body holding any, the ask before the first content (`ask.json`) holding none, and normalized as `trace.json`'s are, are the export `sends` names.

A retrieval goes as `retrieval <source>` and a memory search as `search_memory`, each `CLIENT`, with its source as `gen_ai.data_source.id` or `gen_ai.memory.store.id`, its `gen_ai.retrieval.top_k`, and how many it found, `telmoni.retrieval.returned` or `gen_ai.memory.record.count`, counted from the list it was handed: metadata, in both exports. Its query and what it found, `gen_ai.retrieval.query.text` and `gen_ai.retrieval.documents` or `gen_ai.memory.query.text` and `gen_ai.memory.records`, the list as its compact JSON, are content, in `content` alone. One that found nothing counts 0 and sends `[]`; one that failed returned nothing, so it carries no count.

The rest each suite proves on its own: a retrieval opened outside a trace sends nothing, as a tool call does; a blank source is sent named by its operation alone, a source past 145 characters cut to them, and a `top_k` that is no whole number from 0 left off, each with one warning; `returned` takes a list alone and refuses anything else whole, with one `input` warning — Python's text, bytes, a mapping or a generator, TypeScript's typed array, Go's `[]byte`, `json.RawMessage` or untyped `nil`, where Go's typed nil slice is an empty list — and sets the list and its count in one call, so the last list wins whole and a refused one leaves the last; a mask that takes documents out leaves the count; and in Go and Rust, lists of two sizes handed at once from two threads leave one call's documents beside that call's count.

## `recorded.json`

`scenario.record` is a trace that has already ended, handed whole to `record_trace` (`recordTrace`, `RecordTrace`): its id, its agent, user, session and tags, its times, and what ran inside it, a step holding its own calls and a step of its own, whose model call names its reasoning effort, sent as `gen_ai.request.reasoning.level` whatever the content mode, and holds its input messages, content the export lacks, as `capture_content` is off. Times are RFC 3339 in UTC, to the millisecond, which every language holds exactly. `export` is the one body the SDK must send for it: a record travels in a single export when it holds no more spans than a batch, each span as it ended — a step after what it holds — and the agent's own last.

Nothing is derived at random, so the export is compared nearly whole:

1. Exactly one body was sent.
2. Every `attributes` array is sorted by `key`, the resource's too; spans keep the order they were sent in.
3. `telemetry.sdk.language` is replaced by `<language>`; `telemetry.sdk.version`, `telemetry.distro.version` and the scope's `version` by `<version>`.
4. Everything else is compared as JSON, ids and times included. The trace's id is the record's, 32 hex digits sent in lowercase however they were given. Each span's id is the first eight bytes, in hex, of the version 5 UUID (RFC 9562) named by the span's place in the record, in decimal, under the trace's sixteen bytes as a UUID: `0` the agent's span, then each step and call from `1` in the record's order, depth first, a step before what it holds.
5. The one warning is for the call whose operation is `rerank`, which ingest reads as no model call, so it goes as `chat`: `telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat`, its `kind` `input` (`answers/`, below).

The same record sent twice must be the same bytes, which ingest's claim on a body answers once. A record whose id is not 32 hex digits, a UUID's hyphenated spelling among them, or is all zeros, is refused whole, nothing sent.

## `failure.json`

A failure in full, under the content lock. For each of `cases`, a client is built with its `capture_content`, learns its `content_mode` from the answer to an empty trace's export (`Telmoni-Content-Mode`), then opens a trace of `scenario`'s `agent` in which the application adds `event` to the agent's span through the OpenTelemetry API, and the trace ends by the language's own failure path with an error of the type `error` whose message is `message`: Python's exception raised out of the `with`, TypeScript's error thrown from the callback, Go's error handed to `Fail`, Rust's `Err` returned from a `try_` closure. The agent's span, as sent, must hold `status` and `events` as `kept` or `withheld` say, whichever the case's `sends` names; `withheld` has no `events` key, and the span must carry none.

Before comparing, a suite normalizes the agent's span:

1. Every event's `timeUnixNano` is checked to be a decimal string from the span's start to its end, then replaced by `<time>`; events keep their order, the application's first.
2. Every event's `attributes` array is sorted by `key`.
3. `exception.type` is checked to be the span's `error.type`, as each language names an error's type, then replaced by `<error.type>`.
4. `exception.stacktrace` is checked to be a non-empty string and taken out in Python and TypeScript, which give one cheaply; Go and Rust send none.

## `media.json`

The images and files a message carries, lifted out of it under the content lock. For each of `cases`, a client keyed `k` is built with its `capture_content`, learns its `content_mode` from the answer to an empty trace's export (`Telmoni-Content-Mode`), then opens two traces of `scenario`'s `agent` one after the other, each flushed, each holding one model call of its `model` and `provider` opened with `input_messages` and `system_instructions` and answered with `output_messages`. Every upload, a `PUT`, is answered `204`.

Where `sends`, the uploads, sorted by path, are `uploads` — each its path under the endpoint, its `Content-Type`, and its body in base64 — each with `Authorization: Bearer k`, made once however many spans and exports name the file; and each model call's span holds the three fields, each parsed from its JSON and compared as JSON, as `sent`: each `blob` part a `file` part naming its file by `telmoni:` and the SHA-256 of its bytes, its `modality` and `mime_type` kept where given, and a part whose content is no base64, a `uri` or `file` part, or a part that is no object, as it was. A blob with no `mime_type`, or one blank or holding anything but printable ASCII, is put as `application/octet-stream`, its part keeping the type as given. Where not, nothing is put and neither model call carries any of the three.

`max_bytes` is the largest file put, which every suite holds its SDK's own cap to, as the server's tests hold its lane's, with its path and its `telmoni:` names. The rest each suite proves on its own: a failed upload, a `413` or no answer, still sends the part as its `file` part and warns once (`upload_failed`, with the status where one came), and its hash is tried again on the next export, being remembered only once kept; a file past `max_bytes` is never put, and its part is sent as its `file` part all the same.

## `mask.json`

The code's mask over content, under the content lock. For each of `cases`, a client keyed `k` is built with its `capture_content` and a mask that keeps every key and value it is handed and answers as `mask`'s rule for the key says — `replace`, that string; `drop`, none, the span sent without the field; `fail`, the language's own failure, Python's exception raised, TypeScript's error thrown, Go's and Rust's panic, its words holding the value; no rule, the value as handed — then learns its `content_mode` from the answer to an empty trace's export. It opens one trace of `scenario`'s `agent` for its `user` and `session` with its `tags`, holding one model call of `model` and `provider` opened with `input_messages` and `system_instructions` and answered with `output_messages`; the application adds `event` to the agent's span, and the trace ends by the language's own failure path, as `failure.json`'s does, with an error of the type `error` whose message is `message`.

Where `masks`, the mask was handed `handed`, each key once — Python and TypeScript handing `exception.stacktrace` besides, which the suites leave out before comparing — the keys of `handed_text` as their strings, `attempt`, an integer, as its JSON, and the three message fields as the JSON of what the code gave, blob parts and all; nothing else, so a span's name, the trace's agent, user, session and tags never reach it. The model call's span holds `sent` of the three fields, parsed from its JSON; the agent's span, normalized as `failure.json`'s README sets out, is `kept`: an answer the same as the value handed, as `attempt`'s, keeps the value as it was, an integer still. Nothing is put, the one image left being in the field that failed, and the one warning is `warnings`, kind `mask_failed`, naming the key alone. Where not, the mask was handed nothing, the call carries none of the three fields, the agent's span is `withheld`, and nothing is warned. In every case no request and no warning holds `4242 `: what the mask took out stays home.

The rest each suite proves on its own: a mask that fails on a field is warned of once however many exports it fails in, and is asked again each export; Python's and TypeScript's mask answering anything but a string, or none, fails as a raise does, TypeScript's `Promise` among them; and a mask that is no function is refused as the client is built, in the two languages that could be handed one.


## `canary.json`

The scrubbers over content, secrets and PII, and the corpus every way out is held to. `planted` is one fake but realistic value of each kind — an email, a phone number, a Luhn-valid test card, an SSN-shaped number, an IBAN, a cloud key, a JWT, a PEM key, a connection string, an IP address and a home folder's path — which the server's suite sends through every way out. `modes` names the switches: `sdk_default` (secrets on, PII off, as a client built with neither set), `plugin_default` (both on, a coding-agent plugin's default) and `off`. For each of `cases`, each mode's scrubber turns `text` into `expect.<mode>.text` exactly, and counts what it replaced as `expect.<mode>.scrubbed`, keyed `<family>:<kind>`, which a span whose content a scrubber changed carries as `telmoni.scrubbed`: that object as JSON, its keys sorted, with no spaces. A rule replaces each match with `[secret: <kind>]` or `[pii: <kind>]`, secrets' rules first and in their order, then PII's; a scrubber that fails on a field drops the field and counts `dropped:<field>` instead.

## `scores.json`

Scores, held to the server's own rules, which its tests read from this file too (`crates/telemetry/src/handler/scores.rs`). `limits` are the caps every SDK holds: a name, a category and a version in characters, Unicode code points after Unicode's White_Space is trimmed from the ends; a session's id; scores a span (`per_span_max`, half the 32 events a span holds, so a failure's event still fits); and scores a body of `/v1/scores`.

- `scores`: each case a score given to a live trace's `score`, on a client whose content stays home. `value` is a number, a word for one JSON has none for (`NaN`, `Infinity`, `-Infinity`), `true` or `false` for a yes or no, or a category's label; `range` and `version` where given. Where `kept`, the agent's span carries one `gen_ai.evaluation.result` event naming `kept`'s `name`, its `value` (as a double; 1 or 0 for a yes or no, with `telmoni.score.type` `boolean`) or its `label`, its range where given, and `telmoni.score.version` where `kept.version` is not empty; where `refused`, none, and one warning of kind `input`.
- `targets`: each case a number score sent with the client's `score` naming what it scores. Where `kept`, one `POST /v1/scores` whose one item holds exactly `kept`'s ids, in lowercase: a trace's id is 32 hex digits, leading zeros kept, a span's 16; where `refused`, no request and one `input` warning.
- `late`: three scores sent with the client's `score`, then a flush: exactly one request, a `POST` to `/v1/scores` with the export's headers and no OTLP export at all, whose body is `sent` once each `id` is checked to be a UUID v4 and replaced by `<uuid>`, numbers compared as numbers.
- `live`: three scores on one trace, its span's events, each attributes array sorted by `key` and each `timeUnixNano` checked to fall within the span, then replaced by `<time>`, being `events`.

A score's own keys are metadata, sent in every content mode and never handed to the mask; any other attribute of an evaluation event, an application's `gen_ai.evaluation.explanation` say, is content.
## `ask.json`

A client asks the project's content mode before the first content it would send, so a process that sends once, a hook's, still sends its content. For each of `cases`, a client keyed `k` is built with its `capture_content`; its fake transport answers each request in turn from `answers` — a word, `200` with `Telmoni-Content-Mode` naming it and a body of `{}`; a number, that status with no header and no body — then as `kept.json` does. The client opens two traces of `agent`, each flushed, each holding one model call of `model`, opened with `input_messages` where `content` is true and with none where it is false.

`requests` is how many requests the client made in all. Where `asked`, the first was the ask: a `POST` to the traces lane with the export's headers and `body`, an export of no spans, which ingest answers under the project's mode as it is then, never as it answered an earlier one; where not, nothing was asked. `sent` is whether the first trace's export carries `gen_ai.input.messages` on its model call. A client asks once and silently, before an export holding content, while no answer has named the mode; an ask met with anything but a `2xx` leaves the mode unheard, and the export goes without its content.

## `capture_content.json`

What each SDK makes of `TELMONI_CAPTURE_CONTENT` as it builds its configuration from the environment (`Config.from_env`, `Telmoni.fromEnv`, `ConfigFromEnv`, `Config::from_env`). Each of `cases` sets `variable`, `TELMONI_CAPTURE_CONTENT`, to `given`, or leaves it unset where that is `null`, and the configuration must hold `capture_content` as it says: the value, Unicode's White_Space trimmed from its ends as every variable is, turns content on when it is `1` alone; `0`, `true`, `yes` and every other value, a blank one or none leave it off, so no spelling of "false" lets content out. Each suite proves on its own that a setting made in code stands over the variable, on or off.

## `content.json`

The attribute names every SDK takes off a span unless the code asked for content and the project's mode allows: `content` holds each, and each suite holds its own list to it, as the server's `contract::CONTENT` is held to it, so a name added on one side alone fails the other. An attribute an application sets under one of these names through the OpenTelemetry API is held back as the SDK's own content is.

## `headers.json`

A trace across services: what `inject` writes into an outgoing request's headers, and what `extract` reads back from an incoming one's, so headers one SDK writes are read by each of the other three.

`inject` is performed from inside a step of a trace of `agent`: `traceparent` and `baggage` are what the headers must hold afterwards, `<trace>` and `<span>` standing for the step's trace and span ids as the export spells them. `beside` gives a `baggage` the headers held already (`given`) and what they hold afterwards (`written`): the carrier's own members kept, in their order, and ours, `telmoni.agent`, replaced where it was there and written last, the agent percent-encoded as the W3C baggage grammar asks (its `baggage-octet` kept as it is, every other byte of the UTF-8 as `%XX`, and `%` itself encoded, since a reader decodes it).

`extract` is a list of cases, each the `headers` given — their names in whatever case, matched as the language's headers are (Rust's `HashMap` carrier lowercases them on both sides, so its suite lowercases the fixture's) — and the `trace` read, or `null` where the headers name none: its `agent` (percent-decoded, a character given unescaped taken as its UTF-8, bytes that are no UTF-8 read one U+FFFD for each maximal subpart, as Unicode's practice has it, a byte-order mark kept as a character, and cut to ingest's 128 characters), and the `trace_id`, `parent_span_id` and `sampled` flag its steps and calls open under, the trace id kept whole with its leading zeros. A trace is read only from a well-formed `traceparent` — a version other than `ff`, lowercase hex, a trace id and a span id that are not zeros, nothing after the flags under version `00` — and a `baggage` holding the agent, not blank.

## `guard.json`

The guard, which counts a trace's steps and spend in the agent's own process, with no call to Telmoni. `prices` are handed to the client, which counts by them before the table and never sends them; each SDK carries the price table beside them, a byte-for-byte copy of the core's `crates/telemetry/model_prices.json`, which its suite holds to the core's.

`calls` are model calls, each opened inside a trace of its own with no cap, each with its `provider` and `model` (either may be absent), and answered with its `response_model` and `usage` where given, or once with each of its `answers` in order. `counted` is the trace's spend once the call is answered, in US dollars, compared exactly, as its span then holds it, each attribute as last sent: its cost as reported where it is a number from 0, else its tokens at the price handed for its provider and the model it answered as, else at the table's, else nothing — its input less the cache's at the input price, each cache count at its own (a cache price not given being the input's), and its output at the output price, the four products added in that order, each product and each sum rounded as it is made, as the server rounds them. A multiply fused into its add, as Go compiles one on arm64 unless each product is converted, sums the call of 1,000 tokens in, 800 out, 400 read from the cache and 200 written to it as 0.014070000000000001, not 0.01407. Go's `Usage` sends its four counts whole, a 0 among them, so no case answered again rests on a count a later answer left off. None of these names is the table's but `openai`'s `gpt-4o`, which a price handed shadows, so no case moves when the pin does.

`traces` are traces opened with `max_steps` and `max_spend` where given, performing `ops` in order directly under the agent's span: `step` opens a step and ends it, `tool_call` a tool call, `retrieval` a retrieval, and `{"model_call": n}` the model call `calls[n]`, answered as there. `refused` is the op that raises — before it opens anything, and `null` where none does — with the cap it names (`steps` or `spend`), the cap's `limit`, and what the trace had `used` of it: its steps opened, or its spend. A trace reaches its spend cap once its spend is the cap or more, and then whatever it opens raises; it reaches its step cap once it has opened that many steps, a call or a retrieval being none, and then its next step raises. `spent` is the trace's spend once its ops are done, or at the refusal. A trace's amounts are compared to within 1e-12.

`refusals` are refusals built from a `cap`, its `limit` and what the trace had `used`, and what each `says`, word for word the same in all four: each amount the shortest decimal that reads back as it, written out with no exponent.

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
| `input` | something the SDK was handed fixed, cut or left off as it sent the rest, or not sent at all — a field past its cap, a count that is none, a call outside a trace | — |
| `upload_failed` | spans sent, an image or file their messages carried not put: its `file` part names it all the same (`media.json`) | the upload's `status`, where an answer came |
| `mask_failed` | a span sent without a content field the code's mask failed on (`mask.json`) | — |

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
