/**
 * The SDK's own batch processor: OpenTelemetry's batching and bounded queue,
 * without the check that skips a span whose trace is not sampled.
 *
 * A run inside a trace the application does not sample is still sent; the run's
 * own sample rate is applied here, by run, never through the trace's flag. The
 * queue is bounded, so a slow Telmoni never slows an agent: past its size, spans
 * are dropped and counted. A recorded run goes at once, in exports of its own.
 */

import type { Context } from '@opentelemetry/api';
import type { ReadableSpan, Span, SpanProcessor } from '@opentelemetry/sdk-trace-base';

import * as contract from './contract.js';
import { QueueFullError, ShutdownError, errorName } from './errors.js';
import type { Exporter } from './exporter.js';
import type { ProcessorOptions } from './types.js';

export const MAX_QUEUE_SIZE = 2048;
export const MAX_BATCH_SIZE = 512;
export const SCHEDULE_DELAY_S = 5;
/** How long `shutdown` waits for the export in flight, as the Python and Rust SDKs wait. */
export const SHUTDOWN_TIMEOUT_S = 30;

/** Every client's processor, held weakly under one listener: a process that ends of itself drains each, as Python's `atexit` does. */
const live = new Set<WeakRef<Processor>>();
let hooked = false;

function beforeExit(): void {
  for (const held of live) {
    const processor = held.deref();
    if (processor === undefined) live.delete(held);
    else processor.drainBeforeExit();
  }
}

export class Processor implements SpanProcessor {
  readonly #held = new WeakRef<Processor>(this);
  readonly #exporter: Exporter;
  readonly #maxQueueSize: number;
  readonly #maxBatchSize: number;
  readonly #delayMs: number;
  readonly #queue: ReadableSpan[] = [];
  /**
   * The queue's first span is the `#taken`th ever queued; a batch stops at
   * each fence, a record's first span and one past its last.
   */
  #taken = 0;
  readonly #fences: number[] = [];
  /** The record being built: its trace, its spans as they end, and its room. */
  #stage: { traceId: string; spans: ReadableSpan[] } | undefined;
  #reserved = 0;
  #dropped = 0;
  #flushers: Array<() => void> = [];
  #timer: ReturnType<typeof setTimeout> | undefined;
  #inFlight: Promise<void> | undefined;
  #down = false;
  /** Set once a shutdown's deadline has passed: nothing more is started. */
  #givenUp = false;

  constructor(exporter: Exporter, options: ProcessorOptions = {}) {
    this.#exporter = exporter;
    this.#maxQueueSize = bound(options.maxQueueSize ?? MAX_QUEUE_SIZE, 'maxQueueSize');
    this.#maxBatchSize = bound(options.maxBatchSize ?? MAX_BATCH_SIZE, 'maxBatchSize');
    const delay: unknown = options.scheduleDelaySeconds ?? SCHEDULE_DELAY_S;
    // Node fires a timer past 2^31 - 1 ms at once.
    if (typeof delay !== 'number' || !(delay >= 0 && delay * 1000 <= 2 ** 31 - 1)) {
      throw new RangeError('scheduleDelaySeconds must be a number of seconds from 0, under 25 days');
    }
    this.#delayMs = delay * 1000;
    live.add(this.#held);
    if (!hooked) {
      hooked = true;
      process.on('beforeExit', beforeExit);
    }
  }

  /**
   * Runs `build`, staging the spans of trace `traceId` as they end, then
   * queues them whole, to travel in exports of their own: so the same record
   * sent again is the same bytes, whatever else ends meanwhile. Throws
   * `ShutdownError` once shut down, and `QueueFullError` when the queue has no
   * room for the record's `need` spans, running nothing; the room is kept for
   * them meanwhile. A `build` that throws queues nothing. Of the application's,
   * it runs the root sampler and the reads of the caller's record alone, a
   * record's warnings given before it.
   */
  record(traceId: string, need: number, build: () => void): void {
    if (this.#down) throw new ShutdownError();
    if (this.#queue.length + need > this.#maxQueueSize) throw new QueueFullError(need, this.#maxQueueSize);
    const stage = { traceId, spans: [] as ReadableSpan[] };
    this.#stage = stage;
    this.#reserved = need;
    try {
      build();
    } finally {
      this.#stage = undefined;
      this.#reserved = 0;
    }
    if (this.#down || stage.spans.length === 0) return;
    const first = this.#taken + this.#queue.length;
    if (first > this.#taken) this.#fences.push(first);
    this.#fences.push(first + stage.spans.length);
    for (const span of stage.spans) this.#queue.push(span);
    this.#wake(0);
  }

  onStart(_span: Span, _parentContext: Context): void {}

  onEnd(span: ReadableSpan): void {
    if (this.#down || contract.SAMPLED_OUT in span.attributes) return;
    if (this.#stage !== undefined && span.spanContext().traceId === this.#stage.traceId) {
      this.#stage.spans.push(span);
      return;
    }
    if (this.#queue.length + this.#reserved >= this.#maxQueueSize) {
      this.#dropped += 1;
      return;
    }
    this.#queue.push(span);
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

  /** Spans still queued as Node's loop empties: sent within a shutdown's 30 s, the export holding the loop. */
  drainBeforeExit(): void {
    if (!this.#down && !this.#idle()) void this.shutdown();
  }

  /**
   * Drains, then takes nothing more. Past the timeout what is left is given
   * up: the wait for a retry ends there and no batch or attempt follows, so
   * nothing of the SDK's but a request already in flight, which its own
   * timeout bounds, holds the process the caller is ending, as the other
   * SDKs' workers hold none.
   */
  async shutdown(): Promise<void> {
    live.delete(this.#held);
    if (this.#down) return;
    this.#down = true;
    const deadline = Date.now() + SHUTDOWN_TIMEOUT_S * 1000;
    while (!this.#idle()) {
      if (this.#inFlight === undefined) this.#tick();
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        this.#givenUp = true;
        this.#exporter.giveUp();
        // A flush still waiting would hold the process on its own timer; nothing more will drain.
        for (const resolve of this.#flushers.splice(0)) resolve();
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

  /** Whether the worker should export without waiting; a record queued goes at once. */
  #due(): boolean {
    return (
      this.#down ||
      this.#flushers.length > 0 ||
      this.#fences.length > 0 ||
      this.#queue.length >= this.#maxBatchSize
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
    const fence = this.#fences[0];
    const batch = this.#queue.splice(0, Math.min(this.#maxBatchSize, (fence ?? Infinity) - this.#taken));
    this.#taken += batch.length;
    while ((this.#fences[0] ?? Infinity) <= this.#taken) this.#fences.shift();
    const dropped = this.#dropped;
    this.#dropped = 0;
    if (dropped > 0) {
      this.#exporter.warn(`telmoni: the export queue was full; ${dropped} spans were dropped`);
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
      this.#exporter.warn(`telmoni: an export failed (${errorName(error)}); the batch was dropped`);
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

/** A queue's bound as given: a whole number from 1, or `Infinity` for none; a batch of nothing would never export. */
function bound(given: unknown, name: string): number {
  if (typeof given === 'number' && (given === Number.POSITIVE_INFINITY || (Number.isInteger(given) && given >= 1))) {
    return given;
  }
  throw new RangeError(`${name} must be a whole number from 1, or Infinity`);
}

/** A wait that holds nothing: unref'd, so a shutdown past its deadline lets the process go. */
function pause(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    timer.unref();
  });
}
