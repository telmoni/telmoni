import type { Sampler } from '@opentelemetry/sdk-trace-base';

/**
 * How the SDK reaches Telmoni, and what it sends.
 */
export interface Config {
  /**
   * Telmoni API endpoint URL (default: `https://api.telmoni.com`).
   */
  endpoint?: string | undefined;

  /**
   * The API key (`tm_…`); `/v1` takes no other bearer.
   */
  apiKey?: string | undefined;

  /**
   * Organization identifier (`org_…`).
   */
  organizationId?: string | undefined;

  /**
   * Content (prompts, completions, tool arguments and results) leaves the
   * process only when asked here and the project's mode allows (default: false).
   */
  captureContent?: boolean | undefined;

  /**
   * Handed each content field's key and its value, a string or any other
   * value's JSON, where content leaves; answers the string to send instead, or
   * `undefined` or `null` to send its span without it. A throw, or an answer
   * that is no string, sends the span without that field.
   */
  mask?: ((key: string, value: string) => string | null | undefined) | undefined;

  /**
   * Where content leaves, after the mask, each secret in it — a key, a token, a
   * password, a private key, a connection string — is replaced by
   * `[secret: <kind>]`. Unset, `TELMONI_SCRUB_SECRETS` decides: `0` turns it
   * off, and anything else, or nothing, leaves it on.
   */
  scrubSecrets?: boolean | undefined;

  /**
   * As `scrubSecrets`, for personal data — an email, a phone number, a card, an
   * SSN, an IBAN, an IP address, a home folder — as `[pii: <kind>]`. Unset,
   * `TELMONI_SCRUB_PII` decides: `1` turns it on, and anything else, or nothing,
   * leaves it off.
   */
  scrubPii?: boolean | undefined;

  /**
   * The sampler whose root decision each trace takes as it opens; without
   * one, every trace samples.
   */
  sampler?: Sampler | undefined;

  /**
   * The deployment this process runs in — `production`, `staging` — sent on
   * every span as `deployment.environment.name`; nothing when unset.
   */
  environment?: string | undefined;

  /**
   * The release this process runs — a version or a commit — sent on every
   * span as `service.version`; nothing when unset.
   */
  release?: string | undefined;

  /**
   * The project's own prices, which a trace's spend cap counts a call by before
   * the table this release carries; a name given twice takes its last.
   */
  prices?: readonly Price[] | undefined;
}

/**
 * A project's own price for one of a provider's models, as the provider and
 * the model are sent: each in US dollars a token, a cache price not given
 * being the input price.
 */
export interface Price {
  provider: string;
  model: string;
  input: number;
  output: number;
  cacheRead?: number | undefined;
  cacheWrite?: number | undefined;
}

/** A client's configuration, every blank read as unset and every default filled. */
export interface ResolvedConfig {
  readonly endpoint: string;
  readonly organizationId: string | undefined;
  /** Not enumerable: `console.log`, `util.inspect` and `JSON.stringify` skip it. */
  readonly apiKey: string | undefined;
  readonly captureContent: boolean;
  readonly mask: ((key: string, value: string) => string | null | undefined) | undefined;
  readonly scrubSecrets: boolean;
  readonly scrubPii: boolean;
  readonly sampler: Sampler | undefined;
  readonly environment: string | undefined;
  readonly release: string | undefined;
  readonly prices: readonly Price[];
}

/**
 * Headers as `inject` writes them and `extract` reads them: a record, as
 * Node's `IncomingHttpHeaders` or a plain object, or anything with `get` and
 * `set`, as the Fetch `Headers` is. Names are matched whatever their case.
 */
export type HeaderCarrier = Record<string, string | readonly string[] | undefined> | HeaderMethods;

/** Headers read and written by method, as the Fetch `Headers` is. */
export interface HeaderMethods {
  get(name: string): string | null | undefined;
  set(name: string, value: string): void;
}

/**
 * A model call's tokens, as the conventions count them: input includes the
 * cache's reads and writes, output reasoning; sum parts reported apart. Each
 * goes as a whole number, a fraction cut, one below 0 or past 64 bits left off
 * the call with one warning. `cost` is what the call cost in US dollars, where
 * the provider reports it; one below 0 or not finite is left off with one
 * warning, and a call without one is priced by its tokens. OpenAI's reasoning is
 * already in completion tokens; Gemini's output is candidates plus thoughts.
 * Anthropic reports no separate reasoning count.
 */
export interface Usage {
  inputTokens?: number | undefined;
  outputTokens?: number | undefined;
  cacheReadInputTokens?: number | undefined;
  cacheWriteInputTokens?: number | undefined;
  reasoningOutputTokens?: number | undefined;
  cost?: number | undefined;
}

export interface TraceOptions {
  /** The end user the trace is for, as your own id for them; ingest hashes it. */
  userId?: string | undefined;
  sessionId?: string | undefined;
  tags?: Readonly<Record<string, string | number | boolean>> | undefined;
  /** The guard's cap on the trace's steps: its next step past it throws `CapExceededError`. */
  maxSteps?: number | undefined;
  /** The guard's cap on the trace's spend, in US dollars: reached, what it opens next throws. */
  maxSpend?: number | undefined;
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  inputMessages?: unknown;
}

/** What the agent answered: its messages. */
export interface TraceAnswer {
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  outputMessages?: unknown;
}

export interface ModelCallOptions {
  provider?: string | undefined;
  /** `chat` (default, and what a blank one means), `generate_content`, `text_completion` or `embeddings`; any other goes as `chat`, with one warning. */
  operation?: string | undefined;
  /** The prompt it used, by name: sent in every content mode, so a prompt, never a person, who goes in the trace's `userId`. */
  prompt?: string | undefined;
  /** The prompt's version, sent only beside its name. */
  promptVersion?: string | undefined;
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  inputMessages?: unknown;
  /** Content, as above. */
  systemInstructions?: unknown;
}

/** What the model answered: its name, its tokens, and its messages. */
export interface ModelAnswer {
  responseModel?: string | undefined;
  usage?: Usage | undefined;
  /** Seconds from the call's start to the answer's first token; one below 0 or not finite is left off with one warning. */
  timeToFirstToken?: number | undefined;
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  outputMessages?: unknown;
}

export interface ToolCallOptions {
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  arguments?: unknown;
}

export interface RetrievalOptions {
  /** What it searched for. Content: sent only with `captureContent` and the project's mode `on`. */
  query?: unknown;
  /** How many results it asked for, read as a token count is; one that is none is left off with one warning. */
  topK?: number | undefined;
  /** A search of a memory store, sent as `search_memory`, rather than of a vector store or an index. */
  memory?: boolean | undefined;
}

/** When something in a record happened: a `Date`, or milliseconds since the Unix epoch. */
export type RecordTime = Date | number;

/** What a recorded span ended as; neither field set is ok. */
export interface RecordOutcome {
  /** The error's class, never its message; marks the span failed. */
  errorType?: string | undefined;
  /** Cancelled by the agent, which is neither ok nor an error. */
  cancelled?: boolean | undefined;
  /** Its scores, each timed at its end; one refused is left off with one warning. */
  scores?: readonly RecordedScore[] | undefined;
}

/** A score's value: a number, a yes or no, or a category's label. */
export type ScoreValue = number | boolean | string;

export interface ScoreOptions {
  /** A number score's range, its ends included. */
  range?: readonly [min: number, max: number] | undefined;
  /** The version of what scored, up to 64 characters. */
  version?: string | undefined;
}

/** What a score of something ended scores: a trace, a trace and one of its spans, or a session. */
export interface ScoreTarget {
  /** 32 hex digits, either case, sent lowercase. */
  traceId?: string | undefined;
  spanId?: string | undefined;
  sessionId?: string | undefined;
}

/** A score in a record, held to the rules a live one is. */
export interface RecordedScore extends ScoreOptions {
  name: string;
  value: ScoreValue;
}

/** A model call that has ended: its model, its times and its tokens. */
export interface ModelCallRecord extends RecordOutcome {
  type: 'model_call';
  model: string;
  provider?: string | undefined;
  /** `chat` (default, and what a blank one means), `generate_content`, `text_completion` or `embeddings`; any other goes as `chat`, with one warning. */
  operation?: string | undefined;
  /** How hard the model was asked to reason, as the provider names it: metadata, sent in every mode. */
  reasoningEffort?: string | undefined;
  start: RecordTime;
  end: RecordTime;
  usage?: Usage | undefined;
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  inputMessages?: unknown;
  /** Content, as above. */
  outputMessages?: unknown;
}

/** A tool call that has ended: its name and its times. */
export interface ToolCallRecord extends RecordOutcome {
  type: 'tool_call';
  name: string;
  start: RecordTime;
  end: RecordTime;
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  arguments?: unknown;
  /** Content, as above. */
  result?: unknown;
}

export type CallRecord = ModelCallRecord | ToolCallRecord;

/** A step that has ended, with the calls made inside it and its own steps, a subagent's say, up to 16 deep. */
export interface StepRecord extends RecordOutcome {
  type: 'step';
  name: string;
  start: RecordTime;
  end: RecordTime;
  calls?: readonly (StepRecord | CallRecord)[] | undefined;
  /** Content, a subagent's task say: sent only with `captureContent` and the project's mode `on`. */
  inputMessages?: unknown;
  /** Content, its answer: as above. */
  outputMessages?: unknown;
}

/**
 * A trace that has ended, as `recordTrace` sends it: its id, 32 hex digits
 * the caller gives and gives again whenever the same trace is sent, and
 * everything in it, each with its own times.
 */
export interface TraceRecord extends RecordOutcome {
  traceId: string;
  agent: string;
  /** The end user the trace was for, as your own id for them; ingest hashes it. */
  userId?: string | undefined;
  sessionId?: string | undefined;
  tags?: Readonly<Record<string, string>> | undefined;
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  inputMessages?: unknown;
  /** Content, as above. */
  outputMessages?: unknown;
  start: RecordTime;
  end: RecordTime;
  inside?: readonly (StepRecord | CallRecord)[] | undefined;
}

/** What the transport gives back: the shape of `fetch`'s `Response` the exporter reads. */
export interface TransportResponse {
  status: number;
  headers: { get(name: string): string | null };
  text(): Promise<string>;
}

/** An export, or an image or file put by its hash. */
export type TransportRequest =
  | { method: 'POST'; headers: Record<string, string>; body: string }
  | { method: 'PUT'; headers: Record<string, string>; body: Uint8Array };

/** Sends one request; rejects when the platform could not be reached. */
export type Transport = (url: string, init: TransportRequest) => Promise<TransportResponse>;

/** The export queue's bounds; the defaults are OpenTelemetry's. */
export interface ProcessorOptions {
  maxQueueSize?: number | undefined;
  maxBatchSize?: number | undefined;
  scheduleDelaySeconds?: number | undefined;
}

/** What became of the spans a warning tells of, the same in every SDK (`fixtures/README.md`, `answers/`). */
export type WarningKind =
  | 'refused'
  | 'failed'
  | 'queue_full'
  | 'export_error'
  | 'too_large'
  | 'trimmed'
  | 'dropped'
  | 'span_refused'
  | 'said'
  | 'input'
  | 'upload_failed'
  | 'mask_failed';

/** A warning as data, beside its line; `status` and `problemType` only for an export's own failure, `status` a failed upload's. */
export interface Warning {
  kind: WarningKind;
  status?: number;
  problemType?: string;
}

/** What a test or an unusual environment injects; every default is the platform's. */
export interface Hooks {
  transport?: Transport | undefined;
  /** The wait between attempts; `signal` aborts once a shutdown gives the retry up. */
  sleep?: ((seconds: number, signal: AbortSignal) => Promise<void>) | undefined;
  /** Each warning's line, and what it says as data for a reader to switch on. */
  warn?: ((message: string, warning: Warning) => void) | undefined;
  processor?: ProcessorOptions | undefined;
}
