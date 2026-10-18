/**
 * The exporter: OTLP's JSON encoding over the platform's `fetch`.
 *
 * It takes content off every span unless the code asked and the project's mode
 * allows, sends to `POST {endpoint}/otlp/v1/traces`, waits out a
 * `Retry-After`, and reads `Telmoni-Content-Mode` and the partial success
 * from each answer, warning once for each attribute an answer names dropped.
 */

import { SpanKind, SpanStatusCode, isSpanContextValid } from '@opentelemetry/api';
import type { AttributeValue, HrTime } from '@opentelemetry/api';
import { defaultResource } from '@opentelemetry/resources';
import type { ReadableSpan } from '@opentelemetry/sdk-trace-base';

import * as contract from './contract.js';
import { errorName } from './errors.js';
import type { ResolvedConfig, Transport, TransportResponse } from './types.js';
import { VERSION } from './version.js';

/** How many times one batch is sent before it is dropped. */
export const ATTEMPTS = 3;
export const REQUEST_TIMEOUT_MS = 10_000;
export const RETRY_AFTER_MAX_S = 60;
/** How many distinct warnings are remembered before the set starts over. */
export const WARNED_MAX = 1024;
/** Where a dropped content field's warning sends the reader. */
const CONTENT_DOCS = 'https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door';

const DROPPED = /^span (\S+) dropped (\S+): (\w+)(?:; (.*))?$/;
const REFUSED = /^span (\S+) refused: (.*)$/;

// OTLP numbers the kinds from 1; the api's enum from 0.
const KINDS: Readonly<Record<number, number>> = {
  [SpanKind.INTERNAL]: 1,
  [SpanKind.SERVER]: 2,
  [SpanKind.CLIENT]: 3,
  [SpanKind.PRODUCER]: 4,
  [SpanKind.CONSUMER]: 5,
};

// JavaScript has one number type, so the contract says which of ours are
// doubles (1.0 is an integer here) and which are counts.
const DOUBLES: ReadonlySet<string> = new Set([contract.BOUND_MAX_COST_USD, contract.SAMPLE_RATE]);
const INTEGERS: ReadonlySet<string> = new Set([
  contract.BOUND_STALL_AFTER_S,
  contract.BOUND_MAX_STEPS,
  contract.INPUT_TOKENS,
  contract.OUTPUT_TOKENS,
  contract.CACHE_READ_INPUT_TOKENS,
  contract.CACHE_WRITE_INPUT_TOKENS,
]);

export type AnyValue =
  | { stringValue: string }
  | { intValue: string }
  | { doubleValue: number }
  | { boolValue: boolean }
  | { arrayValue: { values: AnyValue[] } };

export interface KeyValue {
  key: string;
  value: AnyValue;
}

export interface OtlpSpan {
  traceId: string;
  spanId: string;
  parentSpanId?: string;
  name: string;
  kind: number;
  startTimeUnixNano: string;
  endTimeUnixNano: string;
  attributes: KeyValue[];
  status?: { code: number };
}

export interface ExportTraceServiceRequest {
  resourceSpans: [
    {
      resource: { attributes: KeyValue[] };
      scopeSpans: [{ scope: { name: string; version: string }; spans: OtlpSpan[] }];
    },
  ];
}

export interface ExporterHooks {
  transport: Transport;
  /** `signal` aborts once the wait is given up; a hook may ignore it. */
  sleep: (seconds: number, signal: AbortSignal) => Promise<void>;
  warn: (message: string) => void;
}

/**
 * The platform's `fetch`, given up on after ten seconds. A redirect is
 * answered as the 3xx it is: followed, it would resend the bearer to wherever
 * it points.
 */
export const fetchTransport: Transport = (url, init) =>
  fetch(url, { ...init, redirect: 'manual', signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) });

/**
 * A real wait between attempts. It holds the process, since a retry is pending
 * work, until `signal` gives the retry up: then it ends at once, and holds
 * nothing.
 */
export const realSleep = (seconds: number, signal: AbortSignal): Promise<void> =>
  new Promise((resolve) => {
    const done = (): void => {
      clearTimeout(timer);
      signal.removeEventListener('abort', done);
      resolve();
    };
    const timer = setTimeout(done, seconds * 1000);
    if (signal.aborted) done();
    else signal.addEventListener('abort', done, { once: true });
  });

function sdkVersion(): string {
  const version = defaultResource().attributes['telemetry.sdk.version'];
  return typeof version === 'string' ? version : 'unknown';
}

export class Exporter {
  readonly #config: ResolvedConfig;
  readonly #transport: Transport;
  readonly #sleep: (seconds: number, signal: AbortSignal) => Promise<void>;
  readonly #warn: (message: string) => void;
  readonly #url: string;
  readonly #warned = new Set<string>();
  readonly #resource: KeyValue[];
  readonly #givenUp = new AbortController();
  #contentMode: string | undefined;

  constructor(config: ResolvedConfig, hooks: ExporterHooks) {
    this.#config = config;
    this.#transport = hooks.transport;
    this.#sleep = hooks.sleep;
    this.#warn = hooks.warn;
    this.#url = `${config.endpoint}/otlp/v1/traces`;
    this.#resource = [
      attribute(contract.DISTRO_NAME, contract.DISTRO),
      attribute('telemetry.distro.version', VERSION),
      attribute('telemetry.sdk.language', 'nodejs'),
      attribute('telemetry.sdk.name', 'opentelemetry'),
      attribute('telemetry.sdk.version', sdkVersion()),
    ];
    // The deployment, where the configuration names one: never from
    // OTEL_RESOURCE_ATTRIBUTES, which the application's exporters read.
    if (config.environment !== undefined) this.#resource.push(attribute(contract.ENVIRONMENT, config.environment));
    if (config.release !== undefined) this.#resource.push(attribute(contract.RELEASE, config.release));
  }

  /** The project's mode, as the last answer named it; undefined before one. */
  get contentMode(): string | undefined {
    return this.#contentMode;
  }

  /**
   * Gives up what is left: the wait before a retry ends now, and no attempt
   * follows it. A shutdown past its deadline calls it, so a process that
   * ends without `process.exit()` is not held for the retries.
   */
  giveUp(): void {
    this.#givenUp.abort();
  }

  async export(spans: readonly ReadableSpan[]): Promise<void> {
    if (spans.length === 0) return;
    const body = JSON.stringify(this.encode(spans));
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      'User-Agent': `telmoni-typescript/${VERSION}`,
    };
    if (this.#config.apiKey !== undefined) {
      headers['Authorization'] = `Bearer ${this.#config.apiKey}`;
    }
    let failure = '';
    for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
      if (this.#givenUp.signal.aborted) return;
      let answer: TransportResponse;
      try {
        answer = await this.#transport(this.#url, { method: 'POST', headers, body });
      } catch (error) {
        failure = errorName(error);
        await this.#wait(null, attempt);
        continue;
      }
      if (answer.status >= 200 && answer.status < 300) {
        await this.#read(answer);
        return;
      }
      const text = await bodyOf(answer);
      if (answer.status === 429 || answer.status === 503) {
        failure = `${answer.status} ${problemType(text)}`;
        await this.#wait(answer.headers.get('Retry-After'), attempt);
        continue;
      }
      this.#warnOnce(
        `status:${answer.status}`,
        `telmoni: export refused with ${answer.status} ${problemType(text)}; the batch was dropped`,
      );
      return;
    }
    this.#warnOnce(
      `failed:${failure}`,
      `telmoni: export failed after ${ATTEMPTS} attempts (${failure}); the batch was dropped`,
    );
  }

  /** OTLP's `ExportTraceServiceRequest`, in its JSON encoding. */
  encode(spans: readonly ReadableSpan[]): ExportTraceServiceRequest {
    return {
      resourceSpans: [
        {
          resource: { attributes: this.#resource },
          scopeSpans: [
            {
              scope: { name: contract.SCOPE, version: VERSION },
              spans: spans.map((span) => this.#span(span)),
            },
          ],
        },
      ],
    };
  }

  #span(span: ReadableSpan): OtlpSpan {
    const keepContent = this.#config.captureContent && this.#contentMode === 'on';
    const attributes: KeyValue[] = [];
    for (const [key, value] of Object.entries(span.attributes)) {
      if (value === undefined) continue;
      if (!keepContent && contract.CONTENT.has(key)) continue;
      attributes.push(attribute(key, value));
    }
    const spanContext = span.spanContext();
    const parent = span.parentSpanContext;
    const start = span.startTime;
    const end = isUnset(span.endTime) ? start : span.endTime;
    const encoded: OtlpSpan = {
      traceId: spanContext.traceId,
      spanId: spanContext.spanId,
      ...(parent !== undefined && isSpanContextValid(parent) ? { parentSpanId: parent.spanId } : {}),
      name: span.name,
      kind: KINDS[span.kind] ?? 1,
      startTimeUnixNano: nanos(start),
      endTimeUnixNano: nanos(end),
      attributes,
    };
    // Events and the status message are where an exception writes its
    // message and stack trace: never sent.
    if (span.status.code === SpanStatusCode.ERROR) {
      encoded.status = { code: 2 };
    }
    return encoded;
  }

  async #read(answer: TransportResponse): Promise<void> {
    const mode = answer.headers.get(contract.CONTENT_MODE_HEADER);
    if (mode) this.#contentMode = mode;
    const decoded = parseJson(await bodyOf(answer));
    const partial = isRecord(decoded) ? decoded['partialSuccess'] : undefined;
    if (!isRecord(partial)) return;
    const message = partial['errorMessage'];
    if (typeof message === 'string' && message !== '') this.#note(message);
  }

  #note(message: string): void {
    for (const line of message.split(/\r?\n/)) {
      const dropped = DROPPED.exec(line);
      if (dropped !== null) {
        const [, , attributeName = '', reason = '', rest] = dropped;
        let text = `telmoni: the server dropped ${attributeName} (${reason}`;
        if (rest) text += `; ${rest}`;
        text += `); ${remedy(attributeName, reason)}`;
        this.#warnOnce(`dropped:${attributeName}:${reason}`, text);
        continue;
      }
      const refused = REFUSED.exec(line);
      if (refused !== null) {
        const why = refused[2] ?? '';
        this.#warnOnce(`refused:${why}`, `telmoni: the server refused a span: ${why}`);
        continue;
      }
      if (line.startsWith('and ') || line === '') continue;
      this.#warnOnce(`line:${line}`, `telmoni: the server said: ${line}`);
    }
  }

  /** Before the next attempt; nothing after the last. */
  async #wait(retryAfter: string | null, attempt: number): Promise<void> {
    if (attempt + 1 >= ATTEMPTS) return;
    let seconds = 2 ** attempt;
    if (retryAfter !== null && /^\s*[+-]?\d+\s*$/.test(retryAfter)) {
      seconds = Number.parseInt(retryAfter, 10);
    }
    await this.#sleep(Math.min(Math.max(seconds, 0), RETRY_AFTER_MAX_S), this.#givenUp.signal);
  }

  #warnOnce(key: string, text: string): void {
    if (this.#warned.has(key)) return;
    // Bounded: a server naming ever-new attributes would otherwise grow it for
    // the life of the process; past the cap, warnings may repeat.
    if (this.#warned.size >= WARNED_MAX) this.#warned.clear();
    this.#warned.add(key);
    this.#warn(text);
  }
}

function remedy(attributeName: string, reason: string): string {
  // The page, not a setting to change: which modes keep content is the
  // platform's to say, and no project can choose one that does yet.
  if (reason === 'content') return `the project's content mode decides whether content is kept: ${CONTENT_DOCS}`;
  if (reason === 'unlisted') return `send it as the tag ${contract.TAG_PREFIX}${attributeName} to keep it`;
  if (reason === 'over_cap') return 'shorten it to keep it';
  return "see the server's answer";
}

/** The problem type a refusal's `message` opens with, or nothing. */
function problemType(body: string): string {
  const decoded = parseJson(body);
  const message = isRecord(decoded) ? decoded['message'] : undefined;
  if (typeof message !== 'string' || !message.startsWith('/errors/')) return '';
  return message.split(':', 1)[0] ?? '';
}

async function bodyOf(answer: TransportResponse): Promise<string> {
  try {
    return await answer.text();
  } catch {
    return '';
  }
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text === '' ? '{}' : text) as unknown;
  } catch {
    return undefined;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isUnset(time: HrTime): boolean {
  return time[0] === 0 && time[1] === 0;
}

/** `[seconds, nanoseconds]` as one count of nanoseconds, past what a number holds. */
function nanos(time: HrTime): string {
  return (BigInt(Math.trunc(time[0])) * 1_000_000_000n + BigInt(Math.trunc(time[1]))).toString();
}

/** One OTLP `KeyValue`: 64-bit integers travel as decimal strings. */
export function attribute(key: string, value: AttributeValue): KeyValue {
  return { key, value: anyValue(key, value) };
}

function anyValue(key: string, value: AttributeValue): AnyValue {
  if (typeof value === 'boolean') return { boolValue: value };
  if (typeof value === 'number') {
    if (DOUBLES.has(key)) return { doubleValue: value };
    if (INTEGERS.has(key) || Number.isSafeInteger(value)) return { intValue: String(Math.trunc(value)) };
    return { doubleValue: value };
  }
  if (typeof value === 'string') return { stringValue: value };
  if (Array.isArray(value)) {
    const values: AnyValue[] = [];
    for (const item of value) {
      if (item !== null && item !== undefined) values.push(anyValue('', item));
    }
    return { arrayValue: { values } };
  }
  return { stringValue: String(value) };
}
