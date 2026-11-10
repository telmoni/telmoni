/**
 * A sampler that records every span and decides nothing of its own.
 *
 * A trace takes the configured sampler's decision as it opens, a continued
 * one the caller's flag, so an application sampling at a tenth keeps a tenth
 * of its own spans inside traces as outside them; the SDK's processor exports
 * recorded spans whatever the flag.
 */

import { TraceFlags, isSpanContextValid, trace } from '@opentelemetry/api';
import type { Attributes, Context, Link, SpanKind } from '@opentelemetry/api';
import { AlwaysOnSampler, SamplingDecision } from '@opentelemetry/sdk-trace-base';
import type { Sampler, SamplingResult } from '@opentelemetry/sdk-trace-base';

export class RecordingSampler implements Sampler {
  readonly #root: Sampler;

  // The api gives no way to read the application's provider's sampler, as
  // Python's does; without one given, a trace samples, as an application's
  // default ParentBased(AlwaysOn) would.
  constructor(root?: Sampler) {
    this.#root = root ?? new AlwaysOnSampler();
  }

  shouldSample(
    context: Context,
    traceId: string,
    spanName: string,
    spanKind: SpanKind,
    attributes: Attributes,
    links: Link[],
  ): SamplingResult {
    const parent = trace.getSpanContext(context);
    if (parent !== undefined && isSpanContextValid(parent)) {
      const sampled = (parent.traceFlags & TraceFlags.SAMPLED) !== 0;
      const result: SamplingResult = { decision: recording(sampled) };
      if (parent.traceState !== undefined) result.traceState = parent.traceState;
      return result;
    }
    const decision = this.#root.shouldSample(context, traceId, spanName, spanKind, attributes, links);
    // Its decision and trace state, never its attributes, as Python and Go take them.
    const result: SamplingResult = {
      decision: recording(decision.decision === SamplingDecision.RECORD_AND_SAMPLED),
    };
    if (decision.traceState !== undefined) result.traceState = decision.traceState;
    return result;
  }

  toString(): string {
    return 'TelmoniRecordingSampler';
  }
}

function recording(sampled: boolean): SamplingDecision {
  return sampled ? SamplingDecision.RECORD_AND_SAMPLED : SamplingDecision.RECORD;
}
