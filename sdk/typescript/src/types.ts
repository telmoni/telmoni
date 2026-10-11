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
   * The API key (`telmoni_…`); `/v1` takes no other bearer.
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
}

/** What a monitor holds a run to; each optional. */
export interface Bounds {
  stallAfterSeconds?: number | undefined;
  maxSteps?: number | undefined;
  maxCostUsd?: number | undefined;
}

/**
 * A model call's tokens, counted as the conventions count them.
 *
 * Input tokens include the cache's reads and writes; output tokens include
 * reasoning. Where a provider reports parts apart, sum them first.
 */
export interface Usage {
  inputTokens?: number | undefined;
  outputTokens?: number | undefined;
  cacheReadInputTokens?: number | undefined;
  cacheWriteInputTokens?: number | undefined;
}

export interface RunOptions {
  /** The end user the run is for, as your own id for them; ingest hashes it. */
  userId?: string | undefined;
  conversationId?: string | undefined;
  tags?: Readonly<Record<string, string>> | undefined;
  bounds?: Bounds | undefined;
  /** This run's share, over the client's `sampleRate`. */
  sampleRate?: number | undefined;
}

export interface ModelCallOptions {
  provider?: string | undefined;
  /** `chat` (default), `generate_content`, `text_completion` or `embeddings`. */
  operation?: string | undefined;
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  inputMessages?: unknown;
  /** Content, as above. */
  systemInstructions?: unknown;
}

/** What the model answered: its name, its tokens, and its messages. */
export interface ModelAnswer {
  responseModel?: string | undefined;
  usage?: Usage | undefined;
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  outputMessages?: unknown;
}

export interface ToolCallOptions {
  /** Content: sent only with `captureContent` and the project's mode `on`. */
  arguments?: unknown;
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

/** What a test or an unusual environment injects; every default is the platform's. */
export interface Hooks {
  transport?: Transport | undefined;
  sleep?: ((seconds: number) => Promise<void>) | undefined;
  warn?: ((message: string) => void) | undefined;
  processor?: ProcessorOptions | undefined;
}
