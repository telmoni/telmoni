/**
 * The exporter: OTLP's JSON encoding over the platform's `fetch`.
 *
 * It takes content off every span unless the code asked and the project's mode
 * allows, sends to `POST {endpoint}/otlp/v1/traces`, waits out a
 * `Retry-After`, and reads `Telmoni-Content-Mode` and the partial success
 * from each answer, warning once for each attribute an answer names dropped.
 */

import { Agent as HttpAgent, ClientRequest, type IncomingMessage } from 'node:http';
import { Agent as HttpsAgent } from 'node:https';
import { urlToHttpOptions } from 'node:url';

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
/** How much of an answer is read: a problem's JSON is small, and an export never pays for more. */
export const ANSWER_MAX = 1 << 20;
export const RETRY_AFTER_MAX_S = 60;
/** How many distinct warnings are remembered before the set starts over. */
export const WARNED_MAX = 1024;
/** Where a dropped content field's warning sends the reader. */
const CONTENT_DOCS = 'https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door';

const LONE_SURROGATE = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g;
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
const DOUBLES: ReadonlySet<string> = new Set([contract.SAMPLE_RATE, contract.COST]);
const INTEGERS: ReadonlySet<string> = new Set([
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
 * The platform's `fetch`, given up on after ten seconds, the answer's body
 * included. A redirect is answered as the 3xx it is: followed, it would
 * resend the bearer to wherever it points.
 */
export const fetchTransport: Transport = async (url, init) => {
  const response = await fetch(url, { ...init, redirect: 'manual', signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) });
  return { status: response.status, headers: response.headers, text: () => textOf(response.body) };
};

/**
 * This machine's transport: Node's own request on an agent of its own, so through no
 * proxy, where `fetch` takes the environment's under `NODE_USE_ENV_PROXY`, and an
 * application may set one on Node's global agent or wrap `http.request` to force its
 * own. As `fetchTransport` in all else: ten seconds, no redirect followed.
 */
export const directTransport: Transport = (url, init) =>
  new Promise((resolve, reject) => {
    const body = Buffer.from(init.body);
    const signal = AbortSignal.timeout(REQUEST_TIMEOUT_MS);
    const request = new ClientRequest(
      {
        ...urlToHttpOptions(new URL(url)),
        method: init.method,
        headers: { ...init.headers, 'Content-Length': String(body.byteLength) },
        agent: /^https:/i.test(url) ? new HttpsAgent() : new HttpAgent(),
        signal,
      },
      (response) => {
        // A body that breaks unread would otherwise throw where nothing catches it.
        response.on('error', () => {});
        const headers = {
          get: (name: string) => {
            const value = response.headers[name.toLowerCase()];
            return value === undefined ? null : Array.isArray(value) ? value.join(', ') : value;
          },
        };
        resolve({ status: response.statusCode ?? 0, headers, text: () => messageText(response) });
      },
    );
    // The timeout's own error, as `fetch` gives it, rather than the abort it causes.
    request.on('error', (error) => reject(signal.aborted ? signal.reason : error));
    // An upgrade nobody asked for closes it unanswered, which would otherwise never settle.
    request.on('close', () => reject(new Error('the connection closed before an answer')));
    request.end(body);
  });

/**
 * At most `ANSWER_MAX` bytes of a body as text, fed a chunk at a time, the rest
 * never read; its status decides, so a body that breaks or outlasts the
 * request's ten seconds is kept as far as it came.
 */
class Capped {
  readonly #decoder = new TextDecoder();
  #text = '';
  #left = ANSWER_MAX;

  /** Takes what fits of `chunk`; false once the cap is reached. */
  take(chunk: Uint8Array): boolean {
    const part = chunk.subarray(0, this.#left);
    this.#text += this.#decoder.decode(part, { stream: true });
    this.#left -= part.byteLength;
    return this.#left > 0;
  }

  end(): string {
    return this.#text + this.#decoder.decode();
  }
}

/** As `textOf`, from Node's own answer. */
async function messageText(response: IncomingMessage): Promise<string> {
  const capped = new Capped();
  try {
    for await (const chunk of response) {
      if (!capped.take(chunk as Buffer)) break;
    }
  } catch {
    // Broken or timed out: kept as far as it came.
  }
  return capped.end();
}

/** A fetch answer's body, read as `Capped` reads it. */
async function textOf(body: ReadableStream<Uint8Array> | null): Promise<string> {
  if (body === null) return '';
  const reader = body.getReader();
  const capped = new Capped();
  try {
    for (let more = true; more; ) {
      const { done, value } = await reader.read();
      more = !done && capped.take(value);
    }
    await reader.cancel();
  } catch {
    // Broken or timed out: a stream that failed needs no cancelling.
  }
  return capped.end();
}

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

  /**
   * Sends the spans, halved past ingest's caps; false once they failed for a
   * reason not their own, which the rest of a batch they are part of would meet.
   */
  async export(spans: readonly ReadableSpan[]): Promise<boolean> {
    if (spans.length === 0) return true;
    // Ingest refuses an export past 1,000 spans whole, as one past 1 MiB below.
    if (spans.length > contract.BATCH_SPANS_MAX) return this.#halve(spans);
    let body = this.#body(spans);
    if (Buffer.byteLength(body) > contract.BATCH_MAX_BYTES) {
      // Ingest refuses it whole, its 413 maybe breaking the upload still
      // going out, which reads as no answer and is sent again: halved.
      if (spans.length > 1) return this.#halve(spans);
      // A span alone past it goes without what ingest drops for its length,
      // and then fits, unless its own name does not.
      body = this.#body(spans, true);
      if (Buffer.byteLength(body) > contract.BATCH_MAX_BYTES) {
        this.warnOnce('too-large', 'telmoni: a span past the 1 MiB an export may hold was dropped');
        return true;
      }
      this.warnOnce(
        'trimmed',
        'telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length',
      );
    }
    return this.#send(body);
  }

  /** Exports each half of the spans, the second only where the first went. */
  async #halve(spans: readonly ReadableSpan[]): Promise<boolean> {
    const half = Math.floor(spans.length / 2);
    return (await this.export(spans.slice(0, half))) && this.export(spans.slice(half));
  }

  #body(spans: readonly ReadableSpan[], trimmed = false): string {
    // A lone surrogate is no text ingest reads, and would lose the batch.
    return JSON.stringify(this.encode(spans, trimmed), (_key, value: unknown) =>
      typeof value === 'string' ? value.replace(LONE_SURROGATE, '\ufffd') : value,
    );
  }

  async #send(body: string): Promise<boolean> {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      'User-Agent': `telmoni-typescript/${VERSION}`,
    };
    if (this.#config.apiKey !== undefined) {
      headers['Authorization'] = `Bearer ${this.#config.apiKey}`;
    }
    let failure = '';
    for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
      if (this.#givenUp.signal.aborted) return false;
      // This clock as the attempt leaves, the spans' own.
      const sentAt = (BigInt(Math.trunc(Date.now())) * 1_000_000n).toString();
      let answer: TransportResponse;
      try {
        answer = await this.#transport(this.#url, {
          method: 'POST',
          headers: { ...headers, [contract.SENT_AT_HEADER]: sentAt },
          body,
        });
      } catch (error) {
        failure = errorName(error);
        await this.#wait(null, attempt);
        continue;
      }
      if (answer.status >= 200 && answer.status < 300) {
        await this.#read(answer);
        return true;
      }
      const text = await bodyOf(answer);
      if (answer.status === 429 || answer.status === 503) {
        failure = `${answer.status} ${problemType(text)}`;
        await this.#wait(answer.headers.get('Retry-After'), attempt);
        continue;
      }
      this.warnOnce(
        `status:${answer.status}`,
        `telmoni: export refused with ${answer.status} ${problemType(text)}; the batch was dropped`,
      );
      // Refused for what it holds, the rest of a batch may still go.
      return answer.status === 400 || answer.status === 413;
    }
    this.warnOnce(
      `failed:${failure}`,
      `telmoni: export failed after ${ATTEMPTS} attempts (${failure}); the batch was dropped`,
    );
    return false;
  }

  /** OTLP's `ExportTraceServiceRequest`, in its JSON encoding; `trimmed`, without the attributes ingest drops for their length. */
  encode(spans: readonly ReadableSpan[], trimmed = false): ExportTraceServiceRequest {
    return {
      resourceSpans: [
        {
          resource: { attributes: this.#resource },
          scopeSpans: [
            {
              scope: { name: contract.SCOPE, version: VERSION },
              spans: spans.map((span) => this.#span(span, trimmed)),
            },
          ],
        },
      ],
    };
  }

  #span(span: ReadableSpan, trimmed = false): OtlpSpan {
    const keepContent = this.#config.captureContent && this.#contentMode === 'on';
    const attributes: KeyValue[] = [];
    for (const [key, value] of Object.entries(span.attributes)) {
      if (value === undefined) continue;
      if (!keepContent && contract.CONTENT.has(key)) continue;
      if (trimmed && pastCaps(key, value)) continue;
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
        this.warnOnce(`dropped:${attributeName}:${reason}`, text);
        continue;
      }
      const refused = REFUSED.exec(line);
      if (refused !== null) {
        const why = refused[2] ?? '';
        this.warnOnce(`refused:${why}`, `telmoni: the server refused a span: ${why}`);
        continue;
      }
      if (line.startsWith('and ') || line === '') continue;
      this.warnOnce(`line:${line}`, `telmoni: the server said: ${line}`);
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

  /** Warns `text` once per client under `key`, as every warning is. */
  warnOnce(key: string, text: string): void {
    if (this.#warned.has(key)) return;
    // Bounded: a server naming ever-new attributes would otherwise grow it for
    // the life of the process; past the cap, warnings may repeat.
    if (this.#warned.size >= WARNED_MAX) this.#warned.clear();
    this.#warned.add(key);
    this.warn(text);
  }

  /** Warns `text` through the hook, which may throw: never into the worker or the agent's own call. */
  warn(text: string): void {
    try {
      this.#warn(text);
    } catch {
      // Nowhere left to say so: the hook is where warnings go.
    }
  }
}

/**
 * Whether ingest drops an attribute for its length alone: a key past 128 characters,
 * or a value past 1,024 as ingest writes it, a list as its JSON.
 */
function pastCaps(key: string, value: AttributeValue): boolean {
  if (Array.from(key).length > contract.ATTRIBUTE_KEY_MAX) return true;
  const text = Array.isArray(value)
    ? JSON.stringify(value.filter((item) => item !== null && item !== undefined))
    : String(value);
  return Array.from(text).length > contract.ATTRIBUTE_VALUE_MAX;
}

function remedy(attributeName: string, reason: string): string {
  // The page, not a setting to change: which modes keep content is the
  // platform's to say, and no project can choose one that does yet.
  if (reason === 'content') return `the project's content mode decides whether content is kept: ${CONTENT_DOCS}`;
  if (reason === 'unlisted') return `send it as the tag ${contract.TAG_PREFIX}${attributeName} to keep it`;
  if (reason === 'over_cap') return 'shorten it to keep it';
  if (reason === 'cardinality') return 'keep tags to a few keys of a few values each: an id goes as the user or the session';
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

/**
 * `[seconds, nanoseconds]` as one count of nanoseconds, past what a number
 * holds; 0 outside 64 bits, a clock set wrong, so ingest refuses that span
 * rather than the whole export.
 */
function nanos(time: HrTime): string {
  const count = BigInt(Math.trunc(time[0])) * 1_000_000_000n + BigInt(Math.trunc(time[1]));
  return count >= 0n && count < 2n ** 64n ? count.toString() : '0';
}

/** One OTLP `KeyValue`: 64-bit integers travel as decimal strings. */
export function attribute(key: string, value: AttributeValue): KeyValue {
  return { key, value: anyValue(key, value) };
}

function anyValue(key: string, value: AttributeValue): AnyValue {
  if (typeof value === 'boolean') return { boolValue: value };
  if (typeof value === 'number') {
    if (DOUBLES.has(key)) return { doubleValue: value };
    // Within 64 bits alone: past them, or NaN, ingest's parser refuses the whole export.
    const whole = Math.trunc(value);
    if ((INTEGERS.has(key) || Number.isSafeInteger(value)) && whole >= -(2 ** 63) && whole < 2 ** 63) {
      return { intValue: String(whole) };
    }
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
