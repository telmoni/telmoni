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
   * The share of runs sent, above 0 and up to 1 (default: 1). Decided per run,
   * never through the trace's sampled flag, so an application's own sampling
   * is untouched.
   */
  sampleRate?: number | undefined;

  /**
   * The sampler whose root decision a run takes when it starts a trace of its
   * own; without one, every run samples.
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
   * The project's own prices, which a run's spend cap counts a call by before
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
  readonly sampleRate: number;
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
 * warning, and a call without one is priced by its tokens.
 */
export interface Usage {
  inputTokens?: number | undefined;
  outputTokens?: number | undefined;
  cacheReadInputTokens?: number | undefined;
  cacheWriteInputTokens?: number | undefined;
  cost?: number | undefined;
}

export interface RunOptions {
  /** The end user the run is for, as your own id for them; ingest hashes it. */
  userId?: string | undefined;
  sessionId?: string | undefined;
  tags?: Readonly<Record<string, string | number | boolean>> | undefined;
  /** This run's share, over the client's `sampleRate`. */
  sampleRate?: number | undefined;
  /** The guard's cap on the run's steps: its next step past it throws `CapExceededError`. */
  maxSteps?: number | undefined;
  /** The guard's cap on the run's spend, in US dollars: reached, what it opens next throws. */
  maxSpend?: number | undefined;
}

export interface ModelCallOptions {
  provider?: string | undefined;
  /** `chat` (default, and what a blank one means), `generate_content`, `text_completion` or `embeddings`; any other goes as `chat`, with one warning. */
  operation?: string | undefined;
  /** What the model was asked with, each sent under the conventions' `gen_ai.request.*` name. */
  temperature?: number | undefined;
  topP?: number | undefined;
  topK?: number | undefined;
  /** A whole number within 64 bits, as is `seed`; another is left off the call with one warning. */
  maxTokens?: number | undefined;
  frequencyPenalty?: number | undefined;
  presencePenalty?: number | undefined;
  seed?: number | undefined;
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

/** When something in a record happened: a `Date`, or milliseconds since the Unix epoch. */
export type RecordTime = Date | number;

/** What a recorded span ended as; neither field set is ok. */
export interface RecordOutcome {
  /** The error's class, never its message; marks the span failed. */
  errorType?: string | undefined;
  /** Cancelled by the agent, which is neither ok nor an error. */
  cancelled?: boolean | undefined;
}

/** A model call that has ended: its model, its times and its tokens. */
export interface ModelCallRecord extends RecordOutcome {
  type: 'model_call';
  model: string;
  provider?: string | undefined;
  /** `chat` (default, and what a blank one means), `generate_content`, `text_completion` or `embeddings`; any other goes as `chat`, with one warning. */
  operation?: string | undefined;
  start: RecordTime;
  end: RecordTime;
  usage?: Usage | undefined;
}

/** A tool call that has ended: its name and its times. */
export interface ToolCallRecord extends RecordOutcome {
  type: 'tool_call';
  name: string;
  start: RecordTime;
  end: RecordTime;
}

export type CallRecord = ModelCallRecord | ToolCallRecord;

/** A step that has ended, with the calls made inside it. */
export interface StepRecord extends RecordOutcome {
  type: 'step';
  name: string;
  start: RecordTime;
  end: RecordTime;
  calls?: readonly CallRecord[] | undefined;
}

/**
 * A run that has ended, as `recordRun` sends it: its id, a UUID the caller
 * gives and gives again whenever the same run is sent, and everything in it,
 * each with its own times.
 */
export interface RunRecord extends RecordOutcome {
  runId: string;
  agent: string;
  /** The end user the run was for, as your own id for them; ingest hashes it. */
  userId?: string | undefined;
  sessionId?: string | undefined;
  tags?: Readonly<Record<string, string>> | undefined;
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

export interface TransportRequest {
  method: 'POST';
  headers: Record<string, string>;
  body: string;
}

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
  | 'input';

/** A warning as data, beside its line; `status` and `problemType` only for an export's own failure. */
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
