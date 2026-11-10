/**
 * The SDK beside an application's own OpenTelemetry: each trace is its own,
 * linked to the application's span rather than inside its trace, the
 * application's spans stay in their traces at their own sampling rate, a
 * trace beside a span the application does not sample is still sent, and its
 * `OTEL_*` variables change nothing of ours.
 */

import { TraceFlags, context, trace as otel } from '@opentelemetry/api';
import type { SpanContext } from '@opentelemetry/api';
import {
  AlwaysOffSampler,
  AlwaysOnSampler,
  BasicTracerProvider,
  InMemorySpanExporter,
  ParentBasedSampler,
  SamplingDecision,
  SimpleSpanProcessor,
} from '@opentelemetry/sdk-trace-base';
import type { Sampler, SamplingResult } from '@opentelemetry/sdk-trace-base';
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { type Trace, currentTrace } from '../src/index';
import { AsyncLocalStorageContextManager, type Harness, attributesOf, byName, harness, one, tracesIn } from './helpers';

/** The application's sampler in the tests: its target is swapped per test. */
class SwitchableSampler implements Sampler {
  target: Sampler = new AlwaysOnSampler();

  shouldSample(...args: Parameters<Sampler['shouldSample']>): SamplingResult {
    return this.target.shouldSample(...args);
  }

  toString(): string {
    return `Switchable(${this.target.toString()})`;
  }
}

/** Samples every second root span: a rate of one half, deterministic. */
class EveryOther implements Sampler {
  calls = 0;

  shouldSample(): SamplingResult {
    this.calls += 1;
    return { decision: this.calls % 2 === 0 ? SamplingDecision.RECORD_AND_SAMPLED : SamplingDecision.NOT_RECORD };
  }

  toString(): string {
    return 'EveryOther';
  }
}

const appSampler = new SwitchableSampler();
const appExporter = new InMemorySpanExporter();
// The application's own OpenTelemetry, which every test here runs beside.
// ParentBased, as an application's default sampler is: its root decision is the
// switchable target's, and a span inside a trace of ours follows our flag.
const appProvider = new BasicTracerProvider({
  sampler: new ParentBasedSampler({ root: appSampler }),
  spanProcessors: [new SimpleSpanProcessor(appExporter)],
});
otel.setGlobalTracerProvider(appProvider);
context.setGlobalContextManager(new AsyncLocalStorageContextManager());
const app = otel.getTracer('the-application');

let h: Harness;

beforeEach(() => {
  h = harness();
  appExporter.reset();
  appSampler.target = new AlwaysOnSampler();
});

afterEach(async () => {
  vi.unstubAllEnvs();
  await h.telmoni.shutdown();
});

afterAll(async () => {
  await appProvider.shutdown();
  context.disable();
  otel.disable();
});

describe('beside the application', () => {
  it("a trace is its own, linked to the application's span, and the application keeps its spans", async () => {
    let request!: SpanContext;
    await app.startActiveSpan('request', async (span) => {
      request = span.spanContext();
      await h.telmoni.trace('bot', async () => {
        app.startActiveSpan('db.query', (query) => query.end());
      });
      span.end();
    });
    expect(await h.telmoni.flush()).toBe(true);
    const agent = one(tracesIn(h.transport));
    expect(agent.traceId).not.toBe(request.traceId);
    expect(agent.parentSpanId).toBeUndefined();
    expect(agent.links).toEqual([{ traceId: request.traceId, spanId: request.spanId }]);
    const appSpans = appExporter.getFinishedSpans();
    expect(appSpans.map((span) => span.name).sort()).toEqual(['db.query', 'request']);
    const query = appSpans.find((span) => span.name === 'db.query')!;
    expect(query.parentSpanContext?.spanId, "nested under the agent's span").toBe(agent.spanId);
    expect(query.spanContext().traceId).toBe(agent.traceId);
    expect(h.transport.spans().some((span) => span.name === 'request' || span.name === 'db.query')).toBe(false);
  });

  it('a trace opened outside any application span links to none', async () => {
    h.telmoni.trace('bot', () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const agent = one(tracesIn(h.transport));
    expect(agent.parentSpanId).toBeUndefined();
    expect(agent.links).toBeUndefined();
  });

  it('the application keeps its own sampling rate inside traces', async () => {
    const everyOther = new EveryOther();
    appSampler.target = everyOther;
    const ours = harness({ sampler: everyOther });
    try {
      for (let i = 0; i < 100; i++) {
        ours.telmoni.trace('bot', () => {
          app.startActiveSpan('inside', (span) => span.end());
        });
      }
      expect(await ours.telmoni.flush()).toBe(true);
    } finally {
      await ours.telmoni.shutdown();
    }
    expect(tracesIn(ours.transport), 'every trace reaches Telmoni').toHaveLength(100);
    expect(appExporter.getFinishedSpans(), 'half, as outside traces').toHaveLength(50);
  });

  it("a root sampler's decision is taken, never its attributes, as in every SDK", async () => {
    const tagging: Sampler = {
      shouldSample: () => ({ decision: SamplingDecision.RECORD_AND_SAMPLED, attributes: { 'sampler.rule': 'all' } }),
      toString: () => 'Tagging',
    };
    const h = harness({ sampler: tagging });
    h.telmoni.trace('bot', () => {});
    expect(await h.telmoni.flush()).toBe(true);
    expect(Object.keys(attributesOf(one(tracesIn(h.transport))))).not.toContain('sampler.rule');
  });

  it('a trace beside a span the application does not sample is still sent, linked to it', async () => {
    appSampler.target = new AlwaysOffSampler();
    let request!: SpanContext;
    app.startActiveSpan('request', (span) => {
      request = span.spanContext();
      expect(request.traceFlags & TraceFlags.SAMPLED).toBe(0);
      h.telmoni.trace('bot', (trace) => trace.step('s', () => {}));
      span.end();
    });
    expect(await h.telmoni.flush()).toBe(true);
    const agent = one(tracesIn(h.transport));
    expect(agent.traceId).not.toBe(request.traceId);
    expect(agent.links).toEqual([{ traceId: request.traceId, spanId: request.spanId }]);
    expect(h.transport.spans().some((span) => span.name === 's')).toBe(true);
    expect(appExporter.getFinishedSpans()).toEqual([]);
  });

  it("a configured sampler decides each trace, whatever the application's span says", async () => {
    const ours = harness({ sampler: new AlwaysOffSampler() });
    try {
      app.startActiveSpan('request', (request) => {
        expect(request.spanContext().traceFlags & TraceFlags.SAMPLED).toBe(1);
        ours.telmoni.trace('bot', () => {
          app.startActiveSpan('inside', (inside) => {
            expect(inside.spanContext().traceFlags & TraceFlags.SAMPLED).toBe(0);
            inside.end();
          });
        });
        request.end();
      });
      expect(await ours.telmoni.flush()).toBe(true);
    } finally {
      await ours.telmoni.shutdown();
    }
    expect(tracesIn(ours.transport), 'sent regardless of the flag').toHaveLength(1);
    expect(appExporter.getFinishedSpans().map((span) => span.name)).toEqual(['request']);
  });

  it('OTEL_* variables change nothing of ours', async () => {
    vi.stubEnv('OTEL_SDK_DISABLED', 'true');
    vi.stubEnv('OTEL_TRACES_SAMPLER', 'always_off');
    vi.stubEnv('OTEL_RESOURCE_ATTRIBUTES', 'leaked=yes');
    vi.stubEnv('OTEL_SERVICE_NAME', 'leaked');
    vi.stubEnv('OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT', '1');
    vi.stubEnv('OTEL_BSP_MAX_QUEUE_SIZE', '1');
    const ours = harness();
    try {
      ours.telmoni.trace('bot', { userId: 'c', sessionId: 'v', tags: { a: 'b' } }, (trace) => {
        trace.step('s', () => {});
      });
      expect(await ours.telmoni.flush()).toBe(true);
    } finally {
      await ours.telmoni.shutdown();
    }
    const agent = one(tracesIn(ours.transport));
    expect(Object.keys(attributesOf(agent)).sort()).toEqual([
      'gen_ai.agent.name',
      'gen_ai.conversation.id',
      'gen_ai.operation.name',
      'telmoni.agent',
      'telmoni.tag.a',
      'user.id',
    ]);
    expect(attributesOf(agent)['telmoni.tag.a']).toBe('b');
    expect(attributesOf(agent)['user.id']).toBe('c');
    expect(ours.transport.spans().map((span) => span.name).sort()).toEqual(['invoke_agent bot', 's']);
    const resource = ours.transport.bodies()[0]!.resourceSpans[0]!.resource.attributes;
    expect(new Set(resource.map((attribute) => attribute.key))).toEqual(
      new Set([
        'telemetry.distro.name',
        'telemetry.distro.version',
        'telemetry.sdk.language',
        'telemetry.sdk.name',
        'telemetry.sdk.version',
      ]),
    );
    expect(ours.transport.text()).not.toContain('leaked');
  });

  it('the trace is found from the context in a callback the trace started', async () => {
    const found = await h.telmoni.trace(
      'bot',
      (trace) =>
        new Promise<{ trace: Trace; current: Trace | undefined; inStep: Trace | undefined }>((resolve) => {
          setTimeout(() => {
            trace.step('s', () => {
              resolve({ trace, current: currentTrace(), inStep: currentTrace() });
            });
          }, 0);
        }),
    );
    expect(found.current).toBe(found.trace);
    expect(found.inStep).toBe(found.trace);
    expect(currentTrace()).toBeUndefined();
  });

  it("an application span nests under ours through a handle's context, given explicitly", async () => {
    const opened = h.telmoni.trace('bot', (trace) => {
      app.startSpan('inside', {}, trace.context).end();
      trace.step('s', (step) => {
        app.startSpan('under-step', {}, step.context).end();
        expect(currentTrace(step.context)).toBe(trace);
      });
      return trace;
    });
    expect(await h.telmoni.flush()).toBe(true);
    const ours = byName(h.transport);
    const theirs = Object.fromEntries(appExporter.getFinishedSpans().map((span) => [span.name, span]));
    expect(theirs['inside']!.parentSpanContext?.spanId).toBe(ours['invoke_agent bot']!.spanId);
    expect(theirs['under-step']!.parentSpanContext?.spanId).toBe(ours['s']!.spanId);
    expect(theirs['inside']!.spanContext().traceId).toBe(ours['invoke_agent bot']!.traceId);
    expect(currentTrace(opened.context)).toBe(opened);
  });
});
