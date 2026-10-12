/**
 * Trace and span ids: random, as OpenTelemetry's own generator makes them,
 * except for the spans of a recorded run, whose ids are derived from the run's
 * id, so that a record sent twice is the same bytes and ingest's claim on the
 * body answers it once.
 */

import { createHash } from 'node:crypto';

import { RandomIdGenerator } from '@opentelemetry/sdk-trace-base';
import type { IdGenerator } from '@opentelemetry/sdk-trace-base';

export class Ids implements IdGenerator {
  readonly #random = new RandomIdGenerator();
  #given: { traceId: string; spanId: string } | undefined;

  generateTraceId(): string {
    return this.#given?.traceId ?? this.#random.generateTraceId();
  }

  generateSpanId(): string {
    return this.#given?.spanId ?? this.#random.generateSpanId();
  }

  /**
   * `open`, with the span it opens taking these ids. It runs synchronously,
   * so no other span can open in between and take them.
   */
  given<T>(traceId: string, spanId: string, open: () => T): T {
    this.#given = { traceId, spanId };
    try {
      return open();
    } finally {
      this.#given = undefined;
    }
  }
}

/**
 * The id of a recorded run's span at `place`: the first eight bytes of the
 * version 5 UUID (RFC 9562) named by the place, in decimal, under the run's
 * id, given as its 32 hex digits. Its version's bits are among those eight,
 * so it is never zero.
 */
export function spanIdAt(runHex: string, place: number): string {
  const digest = createHash('sha1').update(Buffer.from(runHex, 'hex')).update(String(place)).digest();
  digest[6] = ((digest[6] ?? 0) & 0x0f) | 0x50;
  return digest.subarray(0, 8).toString('hex');
}
