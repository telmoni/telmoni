/**
 * The SDK's own batch processor: OpenTelemetry's batching and bounded queue,
 * without the check that skips a span whose trace is not sampled.
 *
 * A run inside a trace the application does not sample is still sent; the run's
 * own sample rate is applied here, by run, never through the trace's flag. The
 * queue is bounded, so a slow Telmoni never slows an agent: past its size, spans
 * are dropped and counted. `telmoni.run.start` wakes the worker at once.
 */

import type { Context } from '@opentelemetry/api';
import type { ReadableSpan, Span, SpanProcessor } from '@opentelemetry/sdk-trace-base';

import * as contract from './contract.js';
import { errorName } from './errors.js';
import type { Exporter } from './exporter.js';
import type { ProcessorOptions } from './types.js';

export const MAX_QUEUE_SIZE = 2048;
export const MAX_BATCH_SIZE = 512;
export const SCHEDULE_DELAY_S = 5;
/** How long `shutdown` waits for the export in flight, as the Python and Rust SDKs wait. */
export const SHUTDOWN_TIMEOUT_S = 30;

export class Processor implements SpanProcessor {
  readonly #exporter: Exporter;
  readonly #warn: (message: string) => void;
  readonly #maxQueueSize: number;
  readonly #maxBatchSize: number;
  readonly #delayMs: number;
  readonly #queue: ReadableSpan[] = [];
  readonly #sampledOut = new Set<string>();
  #dropped = 0;
  #urgent = false;
  #flushers: Array<() => void> = [];
  #timer: ReturnType<typeof setTimeout> | undefined;
  #inFlight: Promise<void> | undefined;
  #down = false;
  /** Set once a shutdown's deadline has passed: nothing more is started. */
  #givenUp = false;

  constructor(exporter: Exporter, warn: (message: string) => void, options: ProcessorOptions = {}) {
    this.#exporter = exporter;
    // A hook of the application's may throw; the worker must not stop on it.
    this.#warn = (message) => {
      try {
        warn(message);
      } catch {
        // Nowhere left to say so: the hook is where warnings go.
      }
    };
    this.#maxQueueSize = options.maxQueueSize ?? MAX_QUEUE_SIZE;
    this.#maxBatchSize = options.maxBatchSize ?? MAX_BATCH_SIZE;
    this.#delayMs = (options.scheduleDelaySeconds ?? SCHEDULE_DELAY_S) * 1000;
  }

  /** Whether the queue has room for `need` more spans: a record is queued whole or not at all. */
  hasRoom(need: number): boolean {
    return this.#queue.length + need <= this.#maxQueueSize;
  }

  /** Drop every span of this run, the run's own among them. */
  sampleOut(runId: string): void {
    this.#sampledOut.add(runId);
  }

  onStart(_span: Span, _parentContext: Context): void {}

  onEnd(span: ReadableSpan): void {
    if (this.#down) return;
    const runId = span.attributes[contract.RUN_ID];
    if (typeof runId === 'string' && this.#sampledOut.has(runId)) {
      const operation = span.attributes[contract.OPERATION];
      if (typeof operation === 'string' && contract.RUN_OPERATIONS.has(operation)) {
        this.#sampledOut.delete(runId);
      }
      return;
    }
    if (this.#queue.length >= this.#maxQueueSize) {
      this.#dropped += 1;
      return;
    }
    this.#queue.push(span);
    if (span.name === contract.RUN_START) this.#urgent = true;
    this.#wake(this.#due() ? 0 : this.#delayMs);
  }

  /** Resolves once the queue is empty and the export in flight has settled. */
  forceFlush(): Promise<void> {
    // Given up, nothing will drain: as the other SDKs' flush after a
    // shutdown, it returns at once rather than wait out its timeout.
    if (this.#idle() || this.#givenUp) return Promise.resolve();
    return new Promise((resolve) => {
      this.#flushers.push(resolve);
      this.#wake(0);
    });
  }

  /**
   * Drains, then takes nothing more. Past the timeout what is left is given
   * up: the wait for a retry ends there and no batch or attempt follows, so
   * nothing of the SDK's but a request already in flight, which its own
   * timeout bounds, holds the process the caller is ending, as the other
   * SDKs' workers hold none.
   */
  async shutdown(): Promise<void> {
    if (this.#down) return;
    this.#down = true;
    const deadline = Date.now() + SHUTDOWN_TIMEOUT_S * 1000;
    while (!this.#idle()) {
      if (this.#inFlight === undefined) this.#tick();
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        this.#givenUp = true;
        this.#exporter.giveUp();
        break;
      }
      await Promise.race([this.#inFlight, pause(remaining)]);
    }
    if (this.#timer !== undefined) {
      clearTimeout(this.#timer);
      this.#timer = undefined;
    }
  }

  #idle(): boolean {
    return this.#queue.length === 0 && this.#inFlight === undefined;
  }

  /** Whether the worker should export without waiting. */
  #due(): boolean {
    return (
      this.#down || this.#flushers.length > 0 || this.#urgent || this.#queue.length >= this.#maxBatchSize
    );
  }

  // The worker is a timer, never referenced: an idle SDK never keeps the
  // process alive, and a pending one fires no later than the delay.
  #wake(ms: number): void {
    if (this.#givenUp) return;
    if (this.#timer !== undefined) {
      if (ms > 0) return;
      clearTimeout(this.#timer);
    }
    const timer = setTimeout(() => {
      this.#timer = undefined;
      this.#tick();
    }, ms);
    timer.unref();
    this.#timer = timer;
  }

  /** One batch, one at a time; the export's end looks again. */
  #tick(): void {
    if (this.#inFlight !== undefined || this.#givenUp) return;
    const batch = this.#queue.splice(0, this.#maxBatchSize);
    const dropped = this.#dropped;
    this.#dropped = 0;
    this.#urgent = false;
    if (dropped > 0) {
      this.#warn(`telmoni: the export queue was full; ${dropped} spans were dropped`);
    }
    if (batch.length === 0) {
      this.#settle();
      return;
    }
    this.#inFlight = this.#send(batch).then(() => {
      this.#inFlight = undefined;
      this.#settle();
    });
  }

  async #send(batch: ReadableSpan[]): Promise<void> {
    try {
      await this.#exporter.export(batch);
    } catch (error) {
      this.#warn(`telmoni: an export failed (${errorName(error)}); the batch was dropped`);
    }
  }

  #settle(): void {
    if (this.#queue.length > 0) {
      this.#wake(this.#due() ? 0 : this.#delayMs);
      return;
    }
    const flushers = this.#flushers;
    this.#flushers = [];
    for (const resolve of flushers) resolve();
  }
}

/** A wait that holds nothing: unref'd, so a shutdown past its deadline lets the process go. */
function pause(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    timer.unref();
  });
}
