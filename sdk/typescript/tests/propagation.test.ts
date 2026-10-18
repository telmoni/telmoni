/**
 * A run across services: `inject` writes the W3C `traceparent` and our two
 * `baggage` members as `sdk/fixtures/headers.json` spells them, `extract`
 * reads them back as a run continued here, and nothing of a continued run
 * but its steps and calls is sent.
 */

import { context, trace } from '@opentelemetry/api';
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

import { Run, currentRun } from '../src/index';
import { decodeBaggageValue, encodeBaggageValue, parseTraceparent } from '../src/propagation';
import { AsyncLocalStorageContextManager, type JsonSpan, attributesOf, byName, harness, loadFixture } from './helpers';

interface HeadersFixture {
  inject: {
    agent: string;
    traceparent: string;
    baggage: string;
    beside: { given: string; written: string };
  };
  extract: Array<{
    name: string;
    headers: Record<string, string>;
    run: { run_id: string; agent: string; trace_id: string; parent_span_id: string; sampled: boolean } | null;
  }>;
}

const fixture = loadFixture<HeadersFixture>('headers.json');

afterEach(() => {
  vi.restoreAllMocks();
});

describe('inject', () => {
  it("writes the headers the fixture spells, from the span current inside the run, beside the carrier's own members", async () => {
    const h = harness();
    const headers: Record<string, string> = {};
    const beside: Record<string, string> = { Baggage: fixture.inject.beside.given };
    const fetchHeaders = new Headers({ baggage: fixture.inject.beside.given });
    let runId = '';
    try {
      await h.telmoni.run(fixture.inject.agent, async (run) => {
        runId = run.runId;
        await run.step('plan', (step) => {
          h.telmoni.inject(headers, step);
          h.telmoni.inject(beside, step);
          h.telmoni.inject(fetchHeaders, step);
        });
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const step = byName(h.transport)['plan']!;
    const fill = (template: string) =>
      template.replace('<trace>', step.traceId).replace('<span>', step.spanId).replace('<uuid>', runId);
    expect(headers).toEqual({ traceparent: fill(fixture.inject.traceparent), baggage: fill(fixture.inject.baggage) });
    expect(beside, "the carrier's members kept, ours replaced, the header's key set once").toEqual({
      traceparent: fill(fixture.inject.traceparent),
      baggage: fill(fixture.inject.beside.written),
    });
    expect(fetchHeaders.get('traceparent')).toBe(fill(fixture.inject.traceparent));
    expect(fetchHeaders.get('baggage')).toBe(fill(fixture.inject.beside.written));
  });

  it('writes nothing outside a run, or for a run sampled out', async () => {
    const h = harness({ sampleRate: 0.5 });
    vi.spyOn(Math, 'random').mockReturnValue(0.9);
    const outside: Record<string, string> = { baggage: 'tenant=acme' };
    h.telmoni.inject(outside);
    expect(outside).toEqual({ baggage: 'tenant=acme' });
    try {
      h.telmoni.run('bot', (run) => {
        expect(run.sampledOut).toBe(true);
        const headers: Record<string, string> = {};
        h.telmoni.inject(headers, run);
        expect(headers).toEqual({});
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.spans()).toEqual([]);
  });

  describe('under a context manager', () => {
    beforeAll(() => {
      context.setGlobalContextManager(new AsyncLocalStorageContextManager());
    });

    afterAll(() => {
      context.disable();
    });

    it('writes from the active context, which the callbacks carry', async () => {
      const h = harness();
      const headers: Record<string, string> = {};
      let runId = '';
      try {
        await h.telmoni.run('bot', async (run) => {
          runId = run.runId;
          await run.step('plan', async () => {
            await Promise.resolve();
            h.telmoni.inject(headers);
          });
        });
        expect(await h.telmoni.flush()).toBe(true);
      } finally {
        await h.telmoni.shutdown();
      }
      const step = byName(h.transport)['plan']!;
      expect(headers).toEqual({
        traceparent: `00-${step.traceId}-${step.spanId}-01`,
        baggage: `telmoni.run_id=${runId},telmoni.agent=bot`,
      });
    });
  });
});

describe('extract', () => {
  it('reads the run the fixture names, and nothing from headers that name none', async () => {
    const h = harness();
    try {
      for (const given of fixture.extract) {
        const run = h.telmoni.extract(given.headers);
        if (given.run === null) {
          expect(run, given.name).toBeUndefined();
          continue;
        }
        expect(run, given.name).toBeInstanceOf(Run);
        expect(run?.runId, given.name).toBe(given.run.run_id);
        expect(run?.agent, given.name).toBe(given.run.agent);
        expect(run?.sampledOut, given.name).toBe(false);
        const parent = trace.getSpanContext(run!.context);
        expect(parent?.traceId, given.name).toBe(given.run.trace_id);
        expect(parent?.spanId, given.name).toBe(given.run.parent_span_id);
        expect(parent?.isRemote, given.name).toBe(true);
        expect((parent!.traceFlags & 1) === 1, given.name).toBe(given.run.sampled);
      }
    } finally {
      await h.telmoni.shutdown();
    }
  });

  it("a continued run's steps and calls take the caller's span as their parent and carry its run, and nothing of the run itself is sent", async () => {
    const caller = harness();
    const headers: Record<string, string> = {};
    let callerRun = '';
    let callerSpan = '';
    try {
      await caller.telmoni.run('orchestrator', async (run) => {
        callerRun = run.runId;
        await run.step('hand over', (step) => {
          callerSpan = trace.getSpanContext(step.context)!.spanId;
          caller.telmoni.inject(headers, step);
        });
      });
      expect(await caller.telmoni.flush()).toBe(true);
    } finally {
      await caller.telmoni.shutdown();
    }
    const callerTrace = byName(caller.transport)['hand over']!.traceId;

    const worker = harness();
    try {
      const run = worker.telmoni.extract(headers);
      expect(run).toBeInstanceOf(Run);
      await run!.step('work', async (step) => {
        expect(currentRun(step.context)).toBe(run);
        await step.modelCall('gpt-x', { provider: 'openai' }, () => {});
      });
      run!.toolCall('search', () => {});
      run!.fail('Ignored');
      expect(await worker.telmoni.flush()).toBe(true);
    } finally {
      await worker.telmoni.shutdown();
    }
    const spans = worker.transport.spans();
    expect(spans.map((span) => span.name).sort()).toEqual(['chat gpt-x', 'execute_tool search', 'work']);
    for (const span of spans) {
      expect(span.traceId, span.name).toBe(callerTrace);
      const attributes = attributesOf(span);
      expect(attributes['telmoni.run_id'], span.name).toBe(callerRun);
      expect(attributes['telmoni.agent'], span.name).toBe('orchestrator');
    }
    const named = byName(worker.transport);
    const parentOf = (span: JsonSpan | undefined) => span?.parentSpanId;
    expect(parentOf(named['work'])).toBe(callerSpan);
    expect(parentOf(named['execute_tool search'])).toBe(callerSpan);
    expect(parentOf(named['chat gpt-x'])).toBe(named['work']!.spanId);
  });

  it('a continued run hands itself on again, under the span current here', async () => {
    const h = harness();
    const first = { traceparent: '00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01', baggage: 'telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot' };
    const onwards: Record<string, string> = {};
    try {
      const run = h.telmoni.extract(first)!;
      await run.step('relay', (step) => {
        h.telmoni.inject(onwards, step);
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const relay = byName(h.transport)['relay']!;
    expect(onwards).toEqual({
      traceparent: `00-4bf92f3577b34da6a3ce929d0e0e4736-${relay.spanId}-01`,
      baggage: 'telmoni.run_id=0191a3b4-7c2e-7d3f-8e4a-5b6c7d8e9f01,telmoni.agent=bot',
    });
  });
});

describe('the W3C grammar', () => {
  it('encodes a baggage value as the grammar asks, and reads it back', () => {
    const value = 'support bot, "the" one; é\\%';
    expect(encodeBaggageValue(value)).toBe('support%20bot%2C%20%22the%22%20one%3B%20%C3%A9%5C%25');
    expect(decodeBaggageValue(encodeBaggageValue(value))).toBe(value);
    expect(decodeBaggageValue('a%2')).toBe('a%2');
    expect(decodeBaggageValue('a%zz')).toBe('a%zz');
    expect(decodeBaggageValue('%c3%A9')).toBe('é');
    expect(encodeBaggageValue("plain-bot_1.0~!#$&'()*+/:<=>?@[]^`{|}")).toBe("plain-bot_1.0~!#$&'()*+/:<=>?@[]^`{|}");
  });

  it('parses a traceparent as the grammar asks', () => {
    expect(parseTraceparent('00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01')).toEqual({
      traceId: '4bf92f3577b34da6a3ce929d0e0e4736',
      spanId: '00f067aa0ba902b7',
      traceFlags: 1,
      isRemote: true,
    });
    expect(parseTraceparent(' 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-03 ')?.traceFlags).toBe(1);
    expect(parseTraceparent('01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00-more')?.traceFlags).toBe(0);
    for (const refused of [
      '',
      '00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7',
      '00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra',
      'ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01',
      '00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01',
      '00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b-01',
      '00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-1',
      '00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0g',
    ]) {
      expect(parseTraceparent(refused), refused).toBeUndefined();
    }
  });
});
