/**
 * The SDK beside an application's own OpenTelemetry: its spans stay in their
 * traces at their own sampling rate, a run inside a trace it does not sample is
 * still sent, and its `OTEL_*` variables change nothing of ours.
 */

import { TraceFlags, context, trace } from '@opentelemetry/api';
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

import { type Run, currentRun } from '../src/index';
import { AsyncLocalStorageContextManager, type Harness, attributesOf, byName, harness, one, runsIn } from './helpers';

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
// switchable target's, and a span inside a run follows the run's flag.
const appProvider = new BasicTracerProvider({
  sampler: new ParentBasedSampler({ root: appSampler }),
  spanProcessors: [new SimpleSpanProcessor(appExporter)],
});
trace.setGlobalTracerProvider(appProvider);
context.setGlobalContextManager(new AsyncLocalStorageContextManager());
const app = trace.getTracer('the-application');

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
  trace.disable();
});

describe('beside the application', () => {
  it('a run joins the current trace, and the application keeps its spans', async () => {
    let request!: SpanContext;
    await app.startActiveSpan('request', async (span) => {
      request = span.spanContext();
      await h.telmoni.run('bot', async () => {
        app.startActiveSpan('db.query', (query) => query.end());
      });
      span.end();
    });
    expect(await h.telmoni.flush()).toBe(true);
    const run = one(runsIn(h.transport));
    expect(run.traceId).toBe(request.traceId);
    expect(run.parentSpanId).toBe(request.spanId);
    const appSpans = appExporter.getFinishedSpans();
    expect(appSpans.map((span) => span.name).sort()).toEqual(['db.query', 'request']);
    const query = appSpans.find((span) => span.name === 'db.query')!;
    expect(query.parentSpanContext?.spanId, 'nested under the run').toBe(run.spanId);
    expect(query.spanContext().traceId).toBe(run.traceId);
    expect(h.transport.spans().some((span) => span.name === 'request' || span.name === 'db.query')).toBe(false);
  });

  it('the application keeps its own sampling rate inside runs', async () => {
    const everyOther = new EveryOther();
    appSampler.target = everyOther;
    const ours = harness({ sampler: everyOther });
    try {
      for (let i = 0; i < 100; i++) {
        ours.telmoni.run('bot', () => {
          app.startActiveSpan('inside', (span) => span.end());
        });
      }
      expect(await ours.telmoni.flush()).toBe(true);
    } finally {
      await ours.telmoni.shutdown();
    }
    expect(runsIn(ours.transport), 'every run reaches Telmoni').toHaveLength(100);
    expect(appExporter.getFinishedSpans(), 'half, as outside runs').toHaveLength(50);
  });

  it("a root sampler's decision is taken, never its attributes, as in every SDK", async () => {
    const tagging: Sampler = {
      shouldSample: () => ({ decision: SamplingDecision.RECORD_AND_SAMPLED, attributes: { 'sampler.rule': 'all' } }),
      toString: () => 'Tagging',
    };
    const h = harness({ sampler: tagging });
    h.telmoni.run('bot', () => {});
    expect(await h.telmoni.flush()).toBe(true);
    expect(Object.keys(attributesOf(one(runsIn(h.transport))))).not.toContain('sampler.rule');
  });

  it('a run inside a trace the application does not sample is still sent', async () => {
    appSampler.target = new AlwaysOffSampler();
    let request!: SpanContext;
    app.startActiveSpan('request', (span) => {
      request = span.spanContext();
      expect(request.traceFlags & TraceFlags.SAMPLED).toBe(0);
      h.telmoni.run('bot', (run) => run.step('s', () => {}));
      span.end();
    });
    expect(await h.telmoni.flush()).toBe(true);
    const run = one(runsIn(h.transport));
    expect(run.traceId).toBe(request.traceId);
    expect(h.transport.spans().some((span) => span.name === 's')).toBe(true);
    expect(appExporter.getFinishedSpans()).toEqual([]);
  });

  it('a configured sampler decides a root run', async () => {
    const ours = harness({ sampler: new AlwaysOffSampler() });
    try {
      ours.telmoni.run('bot', () => {
        app.startActiveSpan('inside', (inside) => {
          expect(inside.spanContext().traceFlags & TraceFlags.SAMPLED).toBe(0);
          inside.end();
        });
      });
      expect(await ours.telmoni.flush()).toBe(true);
    } finally {
      await ours.telmoni.shutdown();
    }
    expect(runsIn(ours.transport), 'sent regardless of the flag').toHaveLength(1);
    expect(appExporter.getFinishedSpans()).toEqual([]);
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
      ours.telmoni.run('bot', { userId: 'c', sessionId: 'v', tags: { a: 'b' } }, (run) => {
        run.step('s', () => {});
      });
      expect(await ours.telmoni.flush()).toBe(true);
    } finally {
      await ours.telmoni.shutdown();
    }
    const run = one(runsIn(ours.transport));
    expect(Object.keys(attributesOf(run)).sort()).toEqual([
      'gen_ai.agent.name',
      'gen_ai.conversation.id',
      'gen_ai.operation.name',
      'telmoni.agent',
      'telmoni.run_id',
      'telmoni.sample_rate',
      'telmoni.tag.a',
      'user.id',
    ]);
    expect(attributesOf(run)['telmoni.tag.a']).toBe('b');
    expect(attributesOf(run)['user.id']).toBe('c');
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

  it('the run is found from the context in a callback the run started', async () => {
    const found = await h.telmoni.run(
      'bot',
      (run) =>
        new Promise<{ run: Run; current: Run | undefined; inStep: Run | undefined }>((resolve) => {
          setTimeout(() => {
            run.step('s', () => {
              resolve({ run, current: currentRun(), inStep: currentRun() });
            });
          }, 0);
        }),
    );
    expect(found.current).toBe(found.run);
    expect(found.inStep).toBe(found.run);
    expect(currentRun()).toBeUndefined();
  });

  it("an application span nests under ours through a handle's context, given explicitly", async () => {
    const run = h.telmoni.run('bot', (run) => {
      app.startSpan('inside', {}, run.context).end();
      run.step('s', (step) => {
        app.startSpan('under-step', {}, step.context).end();
        expect(currentRun(step.context)).toBe(run);
      });
      return run;
    });
    expect(await h.telmoni.flush()).toBe(true);
    const ours = byName(h.transport);
    const theirs = Object.fromEntries(appExporter.getFinishedSpans().map((span) => [span.name, span]));
    expect(theirs['inside']!.parentSpanContext?.spanId).toBe(ours['invoke_agent bot']!.spanId);
    expect(theirs['under-step']!.parentSpanContext?.spanId).toBe(ours['s']!.spanId);
    expect(theirs['inside']!.spanContext().traceId).toBe(ours['invoke_agent bot']!.traceId);
    expect(currentRun(run.context)).toBe(run);
  });
});
