import { BlockList, isIPv4, isIPv6 } from 'node:net';

import { context, isSpanContextValid, trace } from '@opentelemetry/api';
import { resourceFromAttributes } from '@opentelemetry/resources';
import { BasicTracerProvider } from '@opentelemetry/sdk-trace-base';

import * as contract from './contract.js';
import { Exporter, fetchTransport, realSleep } from './exporter.js';
import { Ids } from './ids.js';
import { Processor } from './processor.js';
import { readHeaders, writeHeaders } from './propagation.js';
import { type Handle, Opener, Run, currentRun, split, within } from './runs.js';
import { RecordingSampler } from './sampler.js';
import type { Config, HeaderCarrier, Hooks, ResolvedConfig, RunOptions, RunRecord } from './types.js';
import { VERSION } from './version.js';

/** The machine host; the console's, `https://telmoni.com`, serves people. */
export const DEFAULT_ENDPOINT = 'https://api.telmoni.com';

/** Unicode's White_Space, which an endpoint's ends are trimmed of, as Go and Rust trim them: `trim` differs. */
const WHITE_SPACE = new Set('\t\n\v\f\r \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000');

// Loops, not /[\s/]+$/, which takes quadratic time on a long run of slashes something follows.
function trimTail(value: string): string {
  let end = value.length;
  while (end > 0 && (value.charAt(end - 1) === '/' || WHITE_SPACE.has(value.charAt(end - 1)))) end--;
  return value.slice(0, end);
}

function trimEnds(value: string): string {
  let start = 0;
  while (start < value.length && WHITE_SPACE.has(value.charAt(start))) start++;
  let end = value.length;
  while (end > start && WHITE_SPACE.has(value.charAt(end - 1))) end--;
  return value.slice(start, end);
}

function unsetIfBlank(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

const PRINTABLE = /^[\x21-\x7e]*$/;
const NAME = /^[A-Za-z0-9._-]+$/;
const PATH = /^[A-Za-z0-9\-._~!$&'()*+,;=:@/]*$/;
const IPV6 = /^[0-9A-Fa-f:.]+$/;
const PORT = /^(:\d{0,5})?$/;

/** The loopback addresses, which a `BlockList` also matches IPv4-mapped. */
const LOOPBACK = new BlockList();
LOOPBACK.addSubnet('127.0.0.0', 8, 'ipv4');
LOOPBACK.addAddress('::1', 'ipv6');

/**
 * The host `endpoint` names, as written, or `undefined` where it is no URL every SDK
 * reads alike (`sdk/fixtures/endpoints.json`): nothing a URL parser fills in or rewrites.
 */
function hostOf(endpoint: string): string | undefined {
  const scheme = endpoint.indexOf('://');
  if (!PRINTABLE.test(endpoint) || scheme < 0 || !/^https?$/i.test(endpoint.slice(0, scheme))) return undefined;
  const rest = endpoint.slice(scheme + 3);
  const slash = rest.indexOf('/');
  const authority = slash < 0 ? rest : rest.slice(0, slash);
  const path = slash < 0 ? '' : rest.slice(slash + 1);
  if (!PATH.test(path) || path.split('/').some((segment) => segment === '.' || segment === '..')) return undefined;
  let host: string;
  let port: string;
  if (authority.startsWith('[')) {
    const end = authority.indexOf(']') + 1;
    host = authority.slice(0, end);
    port = authority.slice(end);
    const inner = host.slice(1, -1);
    if (end === 0 || !IPV6.test(inner) || !isIPv6(inner)) return undefined;
  } else {
    const colon = authority.indexOf(':');
    host = colon < 0 ? authority : authority.slice(0, colon);
    port = colon < 0 ? '' : authority.slice(colon);
    // WHATWG's parser reads a name ending in a number as an IPv4 address.
    const labels = host.replace(/\.$/, '').split('.');
    if (!NAME.test(host) || !labels.every(labelOk) || (numbered(host) && !isIPv4(host))) return undefined;
  }
  return portOk(port) ? host : undefined;
}

/** A name's label as DNS and TLS take it: 1 to 63 characters, no hyphen at either end. */
function labelOk(label: string): boolean {
  return label.length > 0 && label.length <= 63 && !label.startsWith('-') && !label.endsWith('-');
}

/** No port, an empty one, or up to five digits from 1 to 65535. */
function portOk(port: string): boolean {
  if (!PORT.test(port)) return false;
  if (port.length <= 1) return true;
  const number = Number(port.slice(1));
  return number > 0 && number <= 65535;
}

/** Whether `host`'s last label, a trailing dot aside, is a number. */
function numbered(host: string): boolean {
  const last = host.replace(/\.$/, '').split('.').pop() ?? '';
  return /^\d+$/.test(last) || /^0x[0-9a-f]*$/i.test(last);
}

/**
 * Whether `host`, as `hostOf` gives it, is this machine: `localhost`, a name
 * under it, 127.0.0.0/8 or ::1, an IPv4-mapped address read as its IPv4.
 */
function isThisMachine(host: string): boolean {
  if (host.startsWith('[')) return LOOPBACK.check(host.slice(1, -1), 'ipv6');
  if (isIPv4(host)) return LOOPBACK.check(host, 'ipv4');
  const name = host.toLowerCase();
  return name === 'localhost' || name.endsWith('.localhost');
}

/** The endpoint as the CLI reads one: a host without a scheme gets `https://`, or `http://` for this machine. */
function endpointOf(given: string | undefined): string {
  const text = trimEnds(given ?? '');
  // Only what follows the `://` is trimmed, so a second reading changes nothing.
  const scheme = text.indexOf('://');
  if (scheme >= 0) return text.slice(0, scheme + 3) + trimTail(text.slice(scheme + 3));
  const endpoint = trimTail(text);
  if (endpoint === '') return DEFAULT_ENDPOINT;
  // `https:/host` or `https//host` is a scheme mistyped, not a host: the check refuses it.
  if (/^https?$/i.test(endpoint.split(/[:/]/, 1)[0] ?? '')) return endpoint;
  const host = hostOf(`http://${endpoint}`);
  const local = host !== undefined && isThisMachine(host);
  return `${local ? 'http' : 'https'}://${endpoint}`;
}

/**
 * An endpoint the four SDKs could read two ways fails some export or reaches
 * another host, and plain HTTP to another machine would carry the key bare.
 */
function checkEndpoint(endpoint: string): void {
  const host = hostOf(endpoint);
  if (host === undefined) throw new TypeError('the endpoint must be an http:// or https:// URL');
  if (endpoint.slice(0, 7).toLowerCase() === 'http://' && !isThisMachine(host)) {
    throw new TypeError(`refusing to send the API key over unencrypted HTTP to '${host}'; use HTTPS`);
  }
}

function resolve(config: Config | undefined): ResolvedConfig {
  const endpoint = endpointOf(config?.endpoint);
  checkEndpoint(endpoint);
  const sampleRate: unknown = config?.sampleRate ?? 1;
  // '0.5' from an environment variable compares as a number, and goes out as text ingest refuses.
  if (typeof sampleRate !== 'number' || !(sampleRate > 0 && sampleRate <= 1)) {
    throw new RangeError('sampleRate must be a number above 0 and up to 1');
  }
  const resolved = {
    endpoint,
    organizationId: unsetIfBlank(config?.organizationId),
    captureContent: config?.captureContent ?? false,
    sampleRate,
    sampler: config?.sampler,
    environment: unsetIfBlank(config?.environment),
    release: unsetIfBlank(config?.release),
  };
  // Not enumerable: console.log, util.inspect and JSON.stringify skip it, so
  // a client or a configuration logged or serialized whole leaves the key out.
  Object.defineProperty(resolved, 'apiKey', { value: unsetIfBlank(config?.apiKey), enumerable: false });
  // Frozen, so plain JavaScript cannot set what was checked here to what was not.
  return Object.freeze(resolved) as ResolvedConfig;
}

/**
 * The client: a tracer provider of its own, and the runs it records.
 *
 * The provider is built part by part — resource, sampler, limits, ids,
 * processor — so nothing an `OTEL_*` variable says reaches it, and an
 * application's own OpenTelemetry is neither replaced, sent to Telmoni, nor
 * changed.
 */
export class Telmoni {
  public readonly config: ResolvedConfig;
  readonly #exporter: Exporter;
  readonly #processor: Processor;
  readonly #provider: BasicTracerProvider;
  readonly #opener: Opener;

  constructor(config?: Config, hooks?: Hooks) {
    this.config = resolve(config);
    const warn = hooks?.warn ?? ((message: string) => console.warn(message));
    this.#exporter = new Exporter(this.config, {
      transport: hooks?.transport ?? fetchTransport,
      sleep: hooks?.sleep ?? realSleep,
      warn,
    });
    this.#processor = new Processor(this.#exporter, warn, hooks?.processor);
    const ids = new Ids();
    // sdk-trace-base's provider merges what it reads from OTEL_* under the
    // options given; with every option given, nothing of the environment's
    // survives the merge.
    this.#provider = new BasicTracerProvider({
      resource: resourceFromAttributes({
        [contract.DISTRO_NAME]: contract.DISTRO,
        'telemetry.distro.version': VERSION,
      }),
      sampler: new RecordingSampler(this.config.sampler),
      spanLimits: {
        attributeCountLimit: 128,
        attributeValueLengthLimit: Infinity,
        eventCountLimit: 0,
        linkCountLimit: 0,
        attributePerEventCountLimit: 0,
        attributePerLinkCountLimit: 0,
      },
      generalLimits: { attributeCountLimit: 128, attributeValueLengthLimit: Infinity },
      idGenerator: ids,
      spanProcessors: [this.#processor],
    });
    this.#opener = new Opener(
      this.#provider.getTracer(contract.SCOPE, VERSION),
      this.#processor,
      this.config,
      ids,
      (key, text) => this.#exporter.warnOnce(key, text),
    );
  }

  /**
   * Initializes a Telmoni client automatically from environment variables:
   * - `TELMONI_ENDPOINT`
   * - `TELMONI_API_KEY`
   * - `TELMONI_ORG`
   * - `TELMONI_ENVIRONMENT` and `TELMONI_RELEASE`, the deployment it names
   *
   * A blank value is unset, as the CLI reads them. No `OTEL_*` variable is
   * read: the SDK's tracer provider is built part by part.
   */
  public static fromEnv(hooks?: Hooks): Telmoni {
    const env = typeof process !== 'undefined' ? process.env : undefined;
    return new Telmoni(
      {
        endpoint: env?.['TELMONI_ENDPOINT'],
        apiKey: env?.['TELMONI_API_KEY'],
        organizationId: env?.['TELMONI_ORG'],
        environment: env?.['TELMONI_ENVIRONMENT'],
        release: env?.['TELMONI_RELEASE'],
      },
      hooks,
    );
  }

  /** The project's content mode, learned from the first answer. */
  get contentMode(): string | undefined {
    return this.#exporter.contentMode;
  }

  /**
   * A run of `agent`: `fn` runs with the run's span current and gets the
   * run's handle, which opens its steps and calls; the span ends when `fn`
   * settles, and `fn`'s result is returned.
   */
  run<R>(agent: string, fn: (run: Run) => R): R;
  run<R>(agent: string, options: RunOptions, fn: (run: Run) => R): R;
  run<R>(agent: string, a: RunOptions | ((run: Run) => R), b?: (run: Run) => R): R {
    const [options, fn] = split<RunOptions, (run: Run) => R>(a, b);
    return within(this.#opener.run(agent, options ?? {}), fn);
  }

  /** `fn`, opening a run of `agent` per call; the run's handle is `fn`'s last argument. */
  wrap<A extends unknown[], R>(agent: string, fn: (...args: [...A, Run]) => R): (...args: A) => R;
  wrap<A extends unknown[], R>(
    agent: string,
    options: RunOptions,
    fn: (...args: [...A, Run]) => R,
  ): (...args: A) => R;
  wrap<A extends unknown[], R>(
    agent: string,
    a: RunOptions | ((...args: [...A, Run]) => R),
    b?: (...args: [...A, Run]) => R,
  ): (...args: A) => R {
    const [options, fn] = split<RunOptions, (...args: [...A, Run]) => R>(a, b);
    const wrapped = (...args: A): R => this.run(agent, options ?? {}, (run) => fn(...args, run));
    // Logs and stack traces still name the callee, as under Python's functools.wraps.
    Object.defineProperty(wrapped, 'name', { value: fn.name, configurable: true });
    return wrapped;
  }

  /**
   * Send a run that has already ended, as the live calls would have; a record
   * ingest would refuse, or the queue has no room for, throws, sending nothing.
   */
  recordRun(record: RunRecord): void {
    this.#opener.record(record);
  }

  /**
   * Writes into `headers` what the next service continues the run from: the
   * W3C `traceparent` of the span current inside the run, and the run's id
   * and agent as the W3C `baggage` members `telmoni.run_id` and
   * `telmoni.agent`, beside whatever members the headers hold. The span is
   * `from`'s — a run, a step or a call — or, without one, the active
   * context's, which an application running OpenTelemetry's context manager
   * carries into callbacks. Outside a run, or for a run sampled out, it
   * writes nothing.
   */
  inject(headers: HeaderCarrier, from?: Handle): void {
    const ctx = from?.context ?? context.active();
    const run = currentRun(ctx);
    if (run === undefined || run.sampledOut) return;
    const span = trace.getSpanContext(ctx);
    if (span === undefined || !isSpanContextValid(span)) return;
    writeHeaders(headers, span, run.runId, run.agent);
  }

  /**
   * The run `headers` name, continued here: its steps and calls take the
   * caller's span as their parent and carry the caller's run, so the trace
   * shows the work where it was asked for. Nothing ends a continued run
   * here; the caller's own span does. Undefined for headers that name no
   * run.
   */
  extract(headers: HeaderCarrier): Run | undefined {
    const read = readHeaders(headers);
    if (read === undefined) return undefined;
    return this.#opener.continued(read.spanContext, read.runId, read.agent);
  }

  /** Send everything queued; true once the platform has it, false past the timeout. */
  async flush(timeoutSeconds = 30): Promise<boolean> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    // Held, unlike the worker's timer: a flush is what a short-lived process
    // awaits last, and with nothing else holding it the process would exit
    // before the worker ran, its spans unsent. Cleared as the flush settles.
    const expired = new Promise<boolean>((resolve) => {
      timer = setTimeout(() => resolve(false), timeoutSeconds * 1000);
    });
    try {
      return await Promise.race([this.#processor.forceFlush().then(() => true), expired]);
    } finally {
      clearTimeout(timer);
    }
  }

  /** Flush, then stop the exporter; call it before the process exits. */
  shutdown(): Promise<void> {
    return this.#provider.shutdown();
  }
}
