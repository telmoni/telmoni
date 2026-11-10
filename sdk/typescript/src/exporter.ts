/**
 * The exporter: OTLP's JSON encoding over the platform's `fetch`.
 *
 * It takes content off every span unless the code asked and the project's mode
 * allows, puts each image or file a message carries to
 * `PUT {endpoint}/v1/media/{sha256}` where it sends content, scrubs it, sends to
 * `POST {endpoint}/otlp/v1/traces`, and each score carried to it to
 * `POST {endpoint}/v1/scores`, waits out a
 * `Retry-After`, and reads `Telmoni-Content-Mode` and the partial success
 * from each answer, warning once for each attribute an answer names dropped.
 */

import { createHash } from 'node:crypto';
import { Agent as HttpAgent, ClientRequest, type IncomingMessage } from 'node:http';
import { Agent as HttpsAgent } from 'node:https';
import { urlToHttpOptions } from 'node:url';

import { SpanKind, SpanStatusCode, isSpanContextValid } from '@opentelemetry/api';
import type { AttributeValue, Attributes, HrTime, SpanStatus } from '@opentelemetry/api';
import { defaultResource } from '@opentelemetry/resources';
import type { ReadableSpan, TimedEvent } from '@opentelemetry/sdk-trace-base';

import * as contract from './contract.js';
import { errorName } from './errors.js';
import { scrubbedOf, scrubber } from './scrub.js';
import type { ResolvedConfig, Transport, TransportResponse, Warning } from './types.js';
import { VERSION } from './version.js';
import { trimEnds } from './whitespace.js';

/** How many times one batch is sent before it is dropped. */
export const ATTEMPTS = 3;
/** OTLP's retryable statuses: a 502 or a 504 is a load balancer's, a server's rollout among its causes. */
/** What a mask that threw answered: its field goes, warned of. */
const MASK_FAILED = Symbol('mask failed');
const RETRIED: ReadonlySet<number> = new Set([429, 502, 503, 504]);
export const REQUEST_TIMEOUT_MS = 10_000;
/** How much of an answer is read: a problem's JSON is small, and an export never pays for more. */
export const ANSWER_MAX = 1 << 20;
export const RETRY_AFTER_MAX_S = 60;
/** How many distinct warnings are remembered before the set starts over. */
export const WARNED_MAX = 1024;
/** How many files put are remembered before the set starts over: an image a conversation repeats in every call is put once. */
export const UPLOADED_MAX = 4096;
/** Where a dropped content field's warning sends the reader. */
const CONTENT_DOCS = 'https://telmoni.com/docs/api/ingest#what-is-dropped-at-the-door';

export const LONE_SURROGATE = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g;
const DROPPED = /^span (\S+) dropped (\S+): (\w+)(?:; (.*))?$/;
const REFUSED = /^span (\S+) refused: (.*)$/;
const SECONDS = /^[ \t]*([+-]?)([0-9]+)[ \t]*$/;
// Padded to a multiple of 4, as Python's `validate=True` reads it; `Buffer` would skip any other character.
const BASE64 = /^[A-Za-z0-9+/]*={0,2}$/;
// Printable ASCII, not blank: a part's type that can go as a header.
const HEADER_SAFE = /^ *[\x21-\x7e][\x20-\x7e]*$/;

/** Content fields with their blobs lifted out, by their span. */
type Lifted = ReadonlyMap<ReadableSpan, Readonly<Record<string, string>>>;

// OTLP numbers the kinds from 1; the api's enum from 0.
const KINDS = {
  [SpanKind.INTERNAL]: 1,
  [SpanKind.SERVER]: 2,
  [SpanKind.CLIENT]: 3,
  [SpanKind.PRODUCER]: 4,
  [SpanKind.CONSUMER]: 5,
} as const satisfies Record<SpanKind, number>;

// JavaScript has one number type, so the contract says which of ours are
// doubles (1.0 is an integer here) and which are counts.
const DOUBLES: ReadonlySet<string> = new Set([
  contract.COST,
  contract.TIME_TO_FIRST_CHUNK,
  contract.EVALUATION_VALUE,
  contract.SCORE_RANGE_MIN,
  contract.SCORE_RANGE_MAX,
]);
/** What a score's event carries: no content, so sent whatever the lock says, and never masked. */
const SCORE_KEYS: ReadonlySet<string> = new Set([
  contract.EVALUATION_NAME,
  contract.EVALUATION_VALUE,
  contract.EVALUATION_LABEL,
  contract.SCORE_TYPE,
  contract.SCORE_RANGE_MIN,
  contract.SCORE_RANGE_MAX,
  contract.SCORE_VERSION,
]);
const INTEGERS: ReadonlySet<string> = new Set([
  contract.INPUT_TOKENS,
  contract.OUTPUT_TOKENS,
  contract.REASONING_OUTPUT_TOKENS,
  contract.CACHE_READ_INPUT_TOKENS,
  contract.CACHE_CREATION_INPUT_TOKENS,
  contract.RETRIEVAL_TOP_K,
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
  events?: { timeUnixNano: string; name: string; attributes: KeyValue[] }[];
  /** The application's span a trace was opened beside: its ids alone. */
  links?: { traceId: string; spanId: string }[];
  status?: { code: number; message?: string };
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
  warn: (message: string, warning: Warning) => void;
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
    const body = typeof init.body === 'string' ? Buffer.from(init.body) : init.body;
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
  readonly #warn: (message: string, warning: Warning) => void;
  readonly #url: string;
  readonly #mediaUrl: string;
  readonly #scoresUrl: string;
  readonly #uploaded = new Set<string>();
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
    this.#mediaUrl = `${config.endpoint}${contract.MEDIA_PATH}`;
    this.#scoresUrl = `${config.endpoint}${contract.SCORES_PATH}`;
    this.#resource = [
      attribute(contract.DISTRO_NAME, contract.DISTRO),
      attribute('telemetry.distro.version', VERSION),
      attribute('telemetry.sdk.language', 'nodejs'),
      attribute('telemetry.sdk.name', 'opentelemetry'),
      attribute('telemetry.sdk.version', sdkVersion()),
    ];
    // The deployment, where the configuration names one: never from
    // OTEL_RESOURCE_ATTRIBUTES, which the application's exporters read.
    const deployment = [
      [contract.ENVIRONMENT, config.environment, contract.ENVIRONMENT_MAX, 'environment'],
      [contract.RELEASE, config.release, contract.RELEASE_MAX, 'release'],
    ] as const;
    for (const [key, value, most, what] of deployment) {
      if (value === undefined) continue;
      // Counted in characters, as ingest counts them, not UTF-16's units.
      if ([...value].length > most) {
        this.warnOnce(`long:${key}`, `telmoni: the ${what} must be up to ${most} characters; it is not sent`, {
          kind: 'input',
        });
        continue;
      }
      this.#resource.push(attribute(key, value));
    }
  }

  /** The project's mode, as the last answer named it; undefined while unheard. */
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
    // Scores first, posted apart: never sent, masked or lifted as spans, nor a reason to ask.
    const scores = spans.filter(carriesScore);
    if (scores.length > 0) {
      await this.#sendScores(scores.map((span) => span.attributes[contract.SCORE_SENT] as string));
      spans = spans.filter((span) => !carriesScore(span));
      if (spans.length === 0) return true;
    }
    // Yet to hear the project's mode, a client asks it first, so a process
    // that sends once, as a hook does, still sends its content.
    if (this.#config.captureContent && this.#contentMode === undefined && spans.some(holdsContent)) {
      await this.#ask();
    }
    // Decided once, so no answer met halfway sends content the mask never saw.
    const keep = this.#keepsContent();
    if (keep) spans = this.#mask(spans);
    let lifted: Lifted = keep ? await this.#lift(spans) : new Map<ReadableSpan, Record<string, string>>();
    // As each field is sent, its images lifted out: a false match in an image's base64 would break the image.
    if (keep && (this.#config.scrubSecrets || this.#config.scrubPii)) {
      spans = this.#scrub(spans, lifted);
      lifted = new Map();
    }
    return this.#export(spans, lifted, keep);
  }

  async #export(spans: readonly ReadableSpan[], lifted: Lifted, keep: boolean): Promise<boolean> {
    // Ingest refuses an export past 1,000 spans whole, as one past 1 MiB below.
    if (spans.length > contract.BATCH_SPANS_MAX) return this.#halve(spans, lifted, keep);
    let body = this.#body(spans, lifted, keep);
    if (Buffer.byteLength(body) > contract.BATCH_MAX_BYTES) {
      // Ingest refuses it whole, its 413 maybe breaking the upload still
      // going out, which reads as no answer and is sent again: halved.
      if (spans.length > 1) return this.#halve(spans, lifted, keep);
      // Content sent goes first, the longest field first, each named.
      const cut = this.#cut(spans[0]!, lifted, keep);
      if (cut !== undefined) {
        this.warnOnce(
          'content-cut',
          'telmoni: a span past the 1 MiB an export may hold was sent without its longest content, named in telmoni.content_cut',
          { kind: 'trimmed' },
        );
        return this.#send(cut);
      }
      // A span alone past it goes without what ingest drops for its length,
      // and then fits, unless its own name does not.
      body = this.#body(spans, lifted, keep, true);
      if (Buffer.byteLength(body) > contract.BATCH_MAX_BYTES) {
        this.warnOnce('too-large', 'telmoni: a span past the 1 MiB an export may hold was dropped', {
          kind: 'too_large',
        });
        return true;
      }
      this.warnOnce(
        'trimmed',
        'telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length',
        { kind: 'trimmed' },
      );
    }
    return this.#send(body);
  }

  /** Exports each half of the spans, the second only where the first went. */
  async #halve(spans: readonly ReadableSpan[], lifted: Lifted, keep: boolean): Promise<boolean> {
    const half = Math.floor(spans.length / 2);
    return (await this.#export(spans.slice(0, half), lifted, keep)) && this.#export(spans.slice(half), lifted, keep);
  }

  /** The spans with every field the mask is handed as it answers it, before a part it takes out could be put. */
  #mask(spans: readonly ReadableSpan[]): readonly ReadableSpan[] {
    if (this.#config.mask === undefined) return spans;
    return spans.map((span) => fieldsAs(span, span.attributes, (key, value) => this.#masked(key, value)));
  }

  /** One field as the mask answers it, handed as a string, any other value as its JSON; undefined to send the span without it. An answer unchanged keeps the value as it was. */
  #masked(key: string, value: AttributeValue): AttributeValue | undefined {
    const given = typeof value === 'string' ? value : JSON.stringify(value);
    let answer: unknown;
    try {
      answer = this.#config.mask!(key, given);
    } catch {
      answer = MASK_FAILED;
    }
    if (answer === undefined || answer === null) return undefined;
    if (typeof answer !== 'string') {
      // The key alone: an error's words may hold the content.
      this.warnOnce(`mask:${key}`, `telmoni: the mask failed on ${key}; the span was sent without it`, {
        kind: 'mask_failed',
      });
      return undefined;
    }
    return answer === given ? value : answer;
  }

  /**
   * The spans with every field the mask is handed as the scrubbers leave it, each as it is sent, `lifted`'s in place
   * of their own; a span they changed counts what in `telmoni.scrubbed`.
   */
  #scrub(spans: readonly ReadableSpan[], lifted: Lifted): readonly ReadableSpan[] {
    return spans.map((span) => {
      const counts = new Map<string, number>();
      const scrubbed = fieldsAs(span, attributesOf(span, lifted), (key, value) => this.#scrubbed(key, value, counts));
      if (counts.size > 0) scrubbed.attributes[contract.SCRUBBED] = scrubbedOf(counts);
      return scrubbed;
    });
  }

  /** One field as the scrubbers switched on leave it, handed as the mask is, what they replaced counted; undefined where one fails, counted as dropped. */
  #scrubbed(key: string, value: AttributeValue, counts: Map<string, number>): AttributeValue | undefined {
    const given = typeof value === 'string' ? value : JSON.stringify(value);
    try {
      const [text, found] = scrubber.scrub(given, this.#config.scrubSecrets, this.#config.scrubPii);
      for (const [kind, count] of found) counts.set(kind, (counts.get(kind) ?? 0) + count);
      return text === given ? value : text;
    } catch {
      // Never sent as it was: the field goes, named.
      counts.set(`dropped:${key}`, (counts.get(`dropped:${key}`) ?? 0) + 1);
      return undefined;
    }
  }

  /** Each span's messages with every `blob` part lifted out: a blob repeated in every span would bloat each, past 1 MiB maybe. */
  async #lift(spans: readonly ReadableSpan[]): Promise<Lifted> {
    const lifted = new Map<ReadableSpan, Record<string, string>>();
    for (const span of spans) {
      for (const key of contract.MEDIA_FIELDS) {
        const value = span.attributes[key];
        if (typeof value !== 'string') continue;
        const decoded = parseJson(value);
        if (!Array.isArray(decoded) || !(await this.#liftParts(decoded))) continue;
        try {
          lifted.set(span, { ...lifted.get(span), [key]: JSON.stringify(decoded) });
        } catch {
          // Nesting past the stack is left as it is, never the batch's loss.
        }
      }
    }
    return lifted;
  }

  /** Replaces each `blob` part in place, a message's parts too; whether any was. */
  async #liftParts(parts: unknown[]): Promise<boolean> {
    let lifted = false;
    for (const [at, part] of parts.entries()) {
      if (isRecord(part) && Array.isArray(part['parts'])) {
        lifted = (await this.#liftParts(part['parts'] as unknown[])) || lifted;
        continue;
      }
      const file = await this.#filePart(part);
      if (file !== undefined) {
        parts[at] = file;
        lifted = true;
      }
    }
    return lifted;
  }

  /** The `file` part naming a `blob` part's bytes, put first unless past the platform's cap or put already. */
  async #filePart(part: unknown): Promise<Record<string, unknown> | undefined> {
    if (!isRecord(part) || part['type'] !== 'blob') return undefined;
    const content = part['content'];
    if (typeof content !== 'string' || content.length % 4 !== 0 || !BASE64.test(content)) return undefined;
    const data = Buffer.from(content, 'base64');
    const sha256 = createHash('sha256').update(data).digest('hex');
    if (data.byteLength <= contract.MEDIA_MAX_BYTES && !this.#uploaded.has(sha256)) {
      await this.#upload(sha256, data, part['mime_type']);
    }
    // Named whatever became of it: the platform says itself what it holds.
    const file: Record<string, unknown> = { type: 'file' };
    for (const name of ['modality', 'mime_type']) {
      if (name in part) file[name] = part[name];
    }
    file['file_id'] = `${contract.FILE_ID_PREFIX}${sha256}`;
    return file;
  }

  /** Puts one file, once: a failure is warned of, never the export's. */
  async #upload(sha256: string, data: Uint8Array, mimeType: unknown): Promise<void> {
    if (this.#givenUp.signal.aborted) return;
    const headers = this.#headers(
      typeof mimeType === 'string' && HEADER_SAFE.test(mimeType) ? mimeType : contract.MEDIA_DEFAULT_TYPE,
    );
    let failure: string;
    let warning: Warning = { kind: 'upload_failed' };
    try {
      const answer = await this.#transport(`${this.#mediaUrl}${sha256}`, { method: 'PUT', headers, body: data });
      await bodyOf(answer);
      if (answer.status >= 200 && answer.status < 300) {
        if (this.#uploaded.size >= UPLOADED_MAX) this.#uploaded.clear();
        this.#uploaded.add(sha256);
        return;
      }
      failure = String(answer.status);
      warning = { kind: 'upload_failed', status: answer.status };
    } catch (error) {
      failure = errorName(error);
    }
    this.warnOnce(
      `upload:${failure}`,
      `telmoni: an image or file in a message could not be uploaded (${failure}); its part was sent naming it, which the platform may not hold`,
      warning,
    );
  }

  /**
   * The span's body without its longest content, as few fields as fit it in an export: a content field, or any
   * past ingest's caps, which on a project that keeps content is content too; undefined where it sends no
   * content, or fits without all of it.
   */
  #cut(span: ReadableSpan, lifted: Lifted, keep: boolean): string | undefined {
    if (!keep) return undefined;
    const content = Object.entries(attributesOf(span, lifted))
      .filter(
        ([key, value]) =>
          value !== undefined && key !== contract.CONTENT_CUT && (contract.CONTENT.has(key) || pastCaps(key, value)),
      )
      .map(([key, value]) => [key, Array.from(asIngestWrites(value!)).length] as const)
      .sort((a, b) => b[1] - a[1]);
    const cut: string[] = [];
    for (const [key] of content) {
      cut.push(key);
      const body = this.#body([span], lifted, keep, false, cut);
      if (Buffer.byteLength(body) <= contract.BATCH_MAX_BYTES) return body;
    }
    return undefined;
  }

  #keepsContent(): boolean {
    return this.#config.captureContent && this.#contentMode === 'on';
  }

  #body(spans: readonly ReadableSpan[], lifted: Lifted, keep: boolean, trimmed = false, cut: readonly string[] = []): string {
    // A lone surrogate is no text ingest reads, and would lose the batch.
    return JSON.stringify(this.encode(spans, lifted, keep, trimmed, cut), (_key, value: unknown) =>
      typeof value === 'string' ? value.replace(LONE_SURROGATE, '\ufffd') : value,
    );
  }

  #headers(contentType = 'application/json'): Record<string, string> {
    const headers: Record<string, string> = {
      'Content-Type': contentType,
      'User-Agent': `telmoni-typescript/${VERSION}`,
    };
    if (this.#config.apiKey !== undefined) {
      headers['Authorization'] = `Bearer ${this.#config.apiKey}`;
    }
    return headers;
  }

  /**
   * Asks the project's content mode with an export of no spans, which ingest
   * answers as the mode is now: once and silently, since the export after it
   * retries and warns of what it meets, its content held back meanwhile.
   */
  async #ask(): Promise<void> {
    if (this.#givenUp.signal.aborted) return;
    try {
      const answer = await this.#transport(this.#url, {
        method: 'POST',
        headers: { ...this.#headers(), [contract.SENT_AT_HEADER]: sentAtNow() },
        body: ASK,
      });
      if (answer.status >= 200 && answer.status < 300) await this.#read(answer);
    } catch {
      // Unheard, the mode keeps the content back, as no answer does.
    }
  }

  async #send(body: string): Promise<boolean> {
    const headers = this.#headers();
    // This clock as each attempt leaves, the spans' own.
    const posted = await this.#post(this.#url, body, () => ({ ...headers, [contract.SENT_AT_HEADER]: sentAtNow() }));
    switch (posted.kind) {
      case 'answered':
        await this.#read(posted.answer);
        return true;
      case 'refused':
        this.warnOnce(
          `status:${posted.status}`,
          `telmoni: export refused with ${nameOf(posted.status, posted.named)}; the batch was dropped`,
          answered('refused', posted.status, posted.named),
        );
        // Refused for what it holds, the rest of a batch may still go.
        return posted.status === 400 || posted.status === 413;
      case 'failed':
        this.warnOnce(
          `failed:${posted.failure}`,
          `telmoni: export failed after ${ATTEMPTS} attempts (${posted.failure}); the batch was dropped`,
          posted.warning,
        );
        return false;
      case 'given_up':
        return false;
    }
  }

  /** Posts the scores' items, in order, `SCORES_SENT_MAX` a body; a failure is warned of, never the spans'. */
  async #sendScores(items: readonly string[]): Promise<void> {
    const headers = this.#headers();
    for (let at = 0; at < items.length; at += contract.SCORES_SENT_MAX) {
      const body = `{"scores":[${items.slice(at, at + contract.SCORES_SENT_MAX).join(',')}]}`;
      const posted = await this.#post(this.#scoresUrl, body, () => headers);
      if (posted.kind === 'given_up') return;
      if (posted.kind === 'answered') {
        await bodyOf(posted.answer);
      } else if (posted.kind === 'refused') {
        this.warnOnce(
          `scores:status:${posted.status}`,
          `telmoni: scores refused with ${nameOf(posted.status, posted.named)}; they were dropped`,
          answered('refused', posted.status, posted.named),
        );
      } else {
        this.warnOnce(
          `scores:failed:${posted.failure}`,
          `telmoni: scores failed after ${ATTEMPTS} attempts (${posted.failure}); they were dropped`,
          posted.warning,
        );
      }
    }
  }

  /** Posts `body`, sent again on no answer or a retried status, `ATTEMPTS` in all, as long as nothing gave it up. */
  async #post(url: string, body: string, headers: () => Record<string, string>): Promise<Posted> {
    let failure = '';
    // The last attempt's, for the warning's fields, as its line names them; a transport error clears it.
    let met: Warning = { kind: 'failed' };
    for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
      if (this.#givenUp.signal.aborted) return { kind: 'given_up' };
      let answer: TransportResponse;
      try {
        answer = await this.#transport(url, { method: 'POST', headers: headers(), body });
      } catch (error) {
        failure = errorName(error);
        met = { kind: 'failed' };
        await this.#wait(null, attempt);
        continue;
      }
      if (answer.status >= 200 && answer.status < 300) return { kind: 'answered', answer };
      const named = problemType(await bodyOf(answer));
      if (RETRIED.has(answer.status)) {
        failure = nameOf(answer.status, named);
        met = answered('failed', answer.status, named);
        await this.#wait(answer.headers.get('Retry-After'), attempt);
        continue;
      }
      return { kind: 'refused', status: answer.status, named };
    }
    return { kind: 'failed', failure, warning: met };
  }

  /**
   * OTLP's `ExportTraceServiceRequest`, in its JSON encoding, `lifted`'s fields in place of the spans' own; `keep`,
   * with their content; `trimmed`, without the attributes ingest drops for their length; `cut`, without those
   * content fields, named in `telmoni.content_cut`.
   */
  encode(
    spans: readonly ReadableSpan[],
    lifted: Lifted,
    keep: boolean,
    trimmed = false,
    cut: readonly string[] = [],
  ): ExportTraceServiceRequest {
    return {
      resourceSpans: [
        {
          resource: { attributes: this.#resource },
          scopeSpans: [
            {
              scope: { name: contract.SCOPE, version: VERSION },
              spans: spans.map((span) => this.#span(span, lifted, keep, trimmed, cut)),
            },
          ],
        },
      ],
    };
  }

  #span(span: ReadableSpan, lifted: Lifted, keepContent: boolean, trimmed = false, cut: readonly string[] = []): OtlpSpan {
    const attributes: KeyValue[] = [];
    for (const [key, value] of Object.entries(attributesOf(span, lifted))) {
      if (value === undefined) continue;
      if (!keepContent && contract.CONTENT.has(key)) continue;
      if (cut.includes(key) || (cut.length > 0 && key === contract.CONTENT_CUT)) continue;
      if (trimmed && pastCaps(key, value)) continue;
      attributes.push(attribute(key, value));
    }
    if (cut.length > 0) attributes.push(attribute(contract.CONTENT_CUT, [...cut]));
    const spanContext = span.spanContext();
    const parent = span.parentSpanContext;
    const start = span.startTime;
    const end = isUnset(span.endTime) ? start : span.endTime;
    const encoded: OtlpSpan = {
      traceId: spanContext.traceId,
      spanId: spanContext.spanId,
      ...(parent !== undefined && isSpanContextValid(parent) ? { parentSpanId: parent.spanId } : {}),
      name: span.name,
      kind: KINDS[span.kind],
      startTimeUnixNano: nanos(start),
      endTimeUnixNano: nanos(end),
      attributes,
    };
    if (span.links.length > 0) {
      encoded.links = span.links.map((link) => ({ traceId: link.context.traceId, spanId: link.context.spanId }));
    }
    // Events and the status message carry an exception's message and stack
    // trace: content, and the first to go from a span trimmed to fit. A
    // score's event goes whatever, with its score alone where content stays home.
    const events = keepContent && !trimmed;
    const sent = span.events.filter((event) => events || event.name === contract.EVALUATION_EVENT);
    if (sent.length > 0) {
      encoded.events = sent.map((event) => ({
        timeUnixNano: nanos(event.time),
        name: event.name,
        attributes: Object.entries(event.attributes ?? {})
          .filter((entry): entry is [string, AttributeValue] => entry[1] !== undefined && (events || SCORE_KEYS.has(entry[0])))
          .map(([key, value]) => attribute(key, value)),
      }));
    }
    if (span.status.code === SpanStatusCode.ERROR) {
      encoded.status = { code: 2 };
      if (events && span.status.message) encoded.status.message = span.status.message;
    }
    return encoded;
  }

  async #read(answer: TransportResponse): Promise<void> {
    // Trimmed of White_Space, as every SDK trims it; a word ingest does not
    // send reads as none, which keeps content home.
    const mode = trimEnds(answer.headers.get(contract.CONTENT_MODE_HEADER) ?? '');
    if (mode) this.#contentMode = mode === 'off' || mode === 'on' ? mode : undefined;
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
        this.warnOnce(`dropped:${attributeName}:${reason}`, text, { kind: 'dropped' });
        continue;
      }
      const refused = REFUSED.exec(line);
      if (refused !== null) {
        const why = refused[2] ?? '';
        this.warnOnce(`refused:${why}`, `telmoni: the server refused a span: ${why}`, { kind: 'span_refused' });
        continue;
      }
      if (line.startsWith('and ') || line === '') continue;
      this.warnOnce(`line:${line}`, `telmoni: the server said: ${line}`, { kind: 'said' });
    }
  }

  /** Before the next attempt; nothing after the last. */
  async #wait(retryAfter: string | null, attempt: number): Promise<void> {
    if (attempt + 1 >= ATTEMPTS) return;
    const seconds = retrySeconds(retryAfter) ?? 2 ** attempt;
    await this.#sleep(Math.min(seconds, RETRY_AFTER_MAX_S), this.#givenUp.signal);
  }

  /** Warns `text` once per client under `key`, as every warning is. */
  warnOnce(key: string, text: string, warning: Warning): void {
    if (this.#warned.has(key)) return;
    // Bounded: a server naming ever-new attributes would otherwise grow it for
    // the life of the process; past the cap, warnings may repeat.
    if (this.#warned.size >= WARNED_MAX) this.#warned.clear();
    this.#warned.add(key);
    this.warn(text, warning);
  }

  /** Warns `text` through the hook, which may throw: never into the worker or the agent's own call. */
  warn(text: string, warning: Warning): void {
    try {
      this.#warn(text, warning);
    } catch {
      // Nowhere left to say so: the hook is where warnings go.
    }
  }
}

/** An attribute's value as ingest writes it, a list as its JSON. */
function asIngestWrites(value: AttributeValue): string {
  return Array.isArray(value)
    ? JSON.stringify(value.filter((item) => item !== null && item !== undefined))
    : String(value);
}

/**
 * Whether ingest drops an attribute for its length alone: a key past 128 characters,
 * or a value past 1,024 as ingest writes it.
 */
function pastCaps(key: string, value: AttributeValue): boolean {
  if (Array.from(key).length > contract.ATTRIBUTE_KEY_MAX) return true;
  return Array.from(asIngestWrites(value)).length > contract.ATTRIBUTE_VALUE_MAX;
}

function remedy(attributeName: string, reason: string): string {
  // The page, not a setting to change: which modes keep content is the
  // platform's to say, and only the organization's owner switches one.
  if (reason === 'content') return `the project's content mode decides whether content is kept: ${CONTENT_DOCS}`;
  if (reason === 'unlisted') return `send it as the tag ${contract.TAG_PREFIX}${attributeName} to keep it`;
  if (reason === 'over_cap') return 'shorten it to keep it';
  if (reason === 'cardinality') return 'keep tags to a few keys of a few values each: an id goes as the user or the session';
  return "see the server's answer";
}

/** An export's own failure as data: the status met, and the problem type where the body named one. */
function answered(kind: 'refused' | 'failed', status: number, type: string): Warning {
  return type === '' ? { kind, status } : { kind, status, problemType: type };
}

/** What became of a body posted: its answer, a status not retried, its attempts spent, or given up. */
type Posted =
  | { kind: 'answered'; answer: TransportResponse }
  | { kind: 'refused'; status: number; named: string }
  | { kind: 'failed'; failure: string; warning: Warning }
  | { kind: 'given_up' };

/** Whether the span is the SDK's carrier of a score bound for `/v1/scores`, opened under no parent. */
function carriesScore(span: ReadableSpan): boolean {
  return (
    span.name === contract.SCORE_SPAN &&
    span.parentSpanContext === undefined &&
    typeof span.attributes[contract.SCORE_SENT] === 'string'
  );
}

/** An export of no spans: how a client asks the project's content mode. */
const ASK = '{"resourceSpans":[]}';

/** Whether a span holds anything the content lock decides on: a content field, an event beyond a score's keys, or a status message. */
function holdsContent(span: ReadableSpan): boolean {
  return (
    Object.keys(span.attributes).some((key) => contract.CONTENT.has(key)) ||
    span.events.some(
      (event) =>
        event.name !== contract.EVALUATION_EVENT || Object.keys(event.attributes ?? {}).some((key) => !SCORE_KEYS.has(key)),
    ) ||
    Boolean(span.status.message)
  );
}

/** This clock now, in decimal nanoseconds since the Unix epoch. */
function sentAtNow(): string {
  return (BigInt(Math.trunc(Date.now())) * 1_000_000n).toString();
}

/** An answer as a warning names it: its status, then the problem type its body named, where it named one. */
function nameOf(status: number, problem: string): string {
  return problem === '' ? String(status) : `${status} ${problem}`;
}

/** The problem type a refusal's `message` opens with, or nothing. */
function problemType(body: string): string {
  const decoded = parseJson(body);
  const message = isRecord(decoded) ? decoded['message'] : undefined;
  if (typeof message !== 'string' || !message.startsWith('/errors/')) return '';
  return message.split(':', 1)[0] ?? '';
}

/**
 * `Retry-After`'s whole seconds, read as text: a number of any length reaches
 * the cap without converting, and one below zero is none.
 */
function retrySeconds(header: string | null): number | undefined {
  const given = SECONDS.exec(header ?? '');
  if (given === null) return undefined;
  const whole = (given[2] ?? '').replace(/^0+/, '');
  if (given[1] === '-') return 0;
  return whole.length > 2 ? RETRY_AFTER_MAX_S : Number(whole);
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

/**
 * The span with every field the mask is handed — each content field of `attributes`, each event's attribute but a
 * score's own, the status message — as `answer` gives it, undefined sending the span without it.
 */
function fieldsAs(
  span: ReadableSpan,
  attributes: ReadableSpan['attributes'],
  answer: (key: string, value: AttributeValue) => AttributeValue | undefined,
): ReadableSpan {
  const kept: Attributes = {};
  for (const [key, value] of Object.entries(attributes)) {
    if (value === undefined) continue;
    const given = contract.CONTENT.has(key) && key !== contract.CONTENT_CUT ? answer(key, value) : value;
    if (given !== undefined) kept[key] = given;
  }
  const events = span.events.map((event) => {
    const eventKept: Attributes = {};
    for (const [key, value] of Object.entries(event.attributes ?? {})) {
      const score = event.name === contract.EVALUATION_EVENT && SCORE_KEYS.has(key);
      const given = value === undefined || score ? value : answer(key, value);
      if (given !== undefined) eventKept[key] = given;
    }
    return { ...event, attributes: eventKept };
  });
  let status = span.status;
  if (status.message) {
    const message = answer(contract.STATUS_DESCRIPTION, status.message);
    status = message === undefined ? { code: status.code } : { ...status, message: message as string };
  }
  return maskedSpan(span, kept, events, status);
}

/** The span as the mask or the scrubbers left it. */
function maskedSpan(span: ReadableSpan, attributes: Attributes, events: TimedEvent[], status: SpanStatus): ReadableSpan {
  return {
    name: span.name,
    kind: span.kind,
    spanContext: () => span.spanContext(),
    ...(span.parentSpanContext === undefined ? {} : { parentSpanContext: span.parentSpanContext }),
    startTime: span.startTime,
    endTime: span.endTime,
    status,
    attributes,
    links: span.links,
    events,
    duration: span.duration,
    ended: span.ended,
    resource: span.resource,
    instrumentationScope: span.instrumentationScope,
    droppedAttributesCount: span.droppedAttributesCount,
    droppedEventsCount: span.droppedEventsCount,
    droppedLinksCount: span.droppedLinksCount,
  };
}

/** The span's attributes, any of its fields lifted in their place. */
function attributesOf(span: ReadableSpan, lifted: Lifted): ReadableSpan['attributes'] {
  const fields = lifted.get(span);
  return fields === undefined ? span.attributes : { ...span.attributes, ...fields };
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
