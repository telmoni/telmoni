import { isIP } from 'node:net';

import { resourceFromAttributes } from '@opentelemetry/resources';
import { BasicTracerProvider } from '@opentelemetry/sdk-trace-base';

import * as contract from './contract.js';
import { Exporter, fetchTransport, realSleep } from './exporter.js';
import { Ids } from './ids.js';
import { Processor } from './processor.js';
import { Opener, Run, split, within } from './runs.js';
import { RecordingSampler } from './sampler.js';
import type { Config, Hooks, ResolvedConfig, RunOptions, RunRecord } from './types.js';
import { VERSION } from './version.js';

/** The machine host; the console's, `https://telmoni.com`, serves people. */
export const DEFAULT_ENDPOINT = 'https://api.telmoni.com';

// Not /\/+$/: that regex takes quadratic time on a long run of slashes that
// something follows, and the endpoint can come from anywhere.
function trimTrailingSlashes(value: string): string {
  let end = value.length;
  while (end > 0 && value[end - 1] === '/') end--;
  return value.slice(0, end);
}

function unsetIfBlank(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

/** Whether `hostname`, as `URL` spells it, is this machine: `localhost`, a name under it, or a loopback address. */
function isLoopbackHost(hostname: string): boolean {
  const bare = hostname.replace(/^\[|\]$/g, '');
  const version = isIP(bare);
  if (version === 4) return bare.startsWith('127.');
  if (version === 6) return bare === '::1';
  return hostname === 'localhost' || hostname.endsWith('.localhost');
}

/** Plain HTTP reaches only this machine: every export carries the key. */
function refuseCleartext(endpoint: string): void {
  let url: URL;
  try {
    url = new URL(endpoint);
  } catch {
    return;
  }
  if (url.protocol === 'http:' && !isLoopbackHost(url.hostname)) {
    throw new TypeError(`refusing to send the API key over unencrypted HTTP to '${url.hostname}'; use HTTPS`);
  }
}

function resolve(config: Config | undefined): ResolvedConfig {
  const endpoint = trimTrailingSlashes(unsetIfBlank(config?.endpoint) ?? DEFAULT_ENDPOINT) || DEFAULT_ENDPOINT;
  refuseCleartext(endpoint);
  const sampleRate = config?.sampleRate ?? 1;
  if (!(sampleRate > 0 && sampleRate <= 1)) {
    throw new RangeError('sampleRate must be above 0 and up to 1');
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
  return resolved as ResolvedConfig;
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
    this.#opener = new Opener(this.#provider.getTracer(contract.SCOPE, VERSION), this.#processor, this.config, ids);
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
   * Send a run that has already ended, from its record, as the live calls
   * would have sent it: for a job that keeps its own log, or a hook that runs
   * after the turn it reports. The same record sent again is the same bytes,
   * which ingest answers once.
   *
   * Throws, sending nothing, a `TypeError` for a blank agent, a run id that
   * is not a UUID, a time that is neither a `Date` nor a finite number, or an
   * item of no known `type`; a `RangeError` for a span that ends before it
   * starts.
   */
  recordRun(record: RunRecord): void {
    this.#opener.record(record);
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
