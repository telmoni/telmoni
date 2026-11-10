/**
 * A trace across services: `inject` writes the W3C `traceparent` and our
 * `baggage` member as `sdk/fixtures/headers.json` spells them, `extract`
 * reads them back as a trace continued here, and nothing of a continued trace
 * but its steps and calls is sent.
 */

import { context, trace as otel } from '@opentelemetry/api';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import { Trace, currentTrace } from '../src/index';
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
    trace: {
      agent: string;
      trace_id: string;
      parent_span_id: string;
      sampled: boolean;
    } | null;
  }>;
}

const fixture = loadFixture<HeadersFixture>('headers.json');

describe('inject', () => {
  it("writes the headers the fixture spells, from the span current inside the trace, beside the carrier's own members", async () => {
    const h = harness();
    const headers: Record<string, string> = {};
    const beside: Record<string, string> = { Baggage: fixture.inject.beside.given };
    const fetchHeaders = new Headers({ baggage: fixture.inject.beside.given });
    try {
      await h.telmoni.trace(fixture.inject.agent, async (trace) => {
        await trace.step('plan', (step) => {
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
    const fill = (template: string) => template.replace('<trace>', step.traceId).replace('<span>', step.spanId);
    expect(headers).toEqual({ traceparent: fill(fixture.inject.traceparent), baggage: fill(fixture.inject.baggage) });
    expect(beside, "the carrier's members kept, ours replaced, the header's key set once").toEqual({
      traceparent: fill(fixture.inject.traceparent),
      baggage: fill(fixture.inject.beside.written),
    });
    expect(fetchHeaders.get('traceparent')).toBe(fill(fixture.inject.traceparent));
    expect(fetchHeaders.get('baggage')).toBe(fill(fixture.inject.beside.written));
  });

  it('writes nothing outside a trace', async () => {
    const h = harness();
    const outside: Record<string, string> = { baggage: 'tenant=acme' };
    try {
      h.telmoni.inject(outside);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(outside).toEqual({ baggage: 'tenant=acme' });
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
      try {
        await h.telmoni.trace('bot', async (trace) => {
          await trace.step('plan', async () => {
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
        baggage: 'telmoni.agent=bot',
      });
    });
  });
});

describe('extract', () => {
  it('reads the trace the fixture names, and nothing from headers that name none', async () => {
    const h = harness();
    try {
      for (const given of fixture.extract) {
        const continued = h.telmoni.extract(given.headers);
        if (given.trace === null) {
          expect(continued, given.name).toBeUndefined();
          continue;
        }
        expect(continued, given.name).toBeInstanceOf(Trace);
        expect(continued?.traceId, given.name).toBe(given.trace.trace_id);
        expect(continued?.agent, given.name).toBe(given.trace.agent);
        const parent = otel.getSpanContext(continued!.context);
        expect(parent?.traceId, given.name).toBe(given.trace.trace_id);
        expect(parent?.spanId, given.name).toBe(given.trace.parent_span_id);
        expect(parent?.isRemote, given.name).toBe(true);
        expect((parent!.traceFlags & 1) === 1, given.name).toBe(given.trace.sampled);
      }
    } finally {
      await h.telmoni.shutdown();
    }
  });

  it("a continued trace's steps and calls take the caller's span as their parent and carry its agent, and nothing of the trace itself is sent", async () => {
    const caller = harness();
    const headers: Record<string, string> = {};
    let callerSpan = '';
    try {
      await caller.telmoni.trace('orchestrator', async (trace) => {
        await trace.step('hand over', (step) => {
          callerSpan = otel.getSpanContext(step.context)!.spanId;
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
      const continued = worker.telmoni.extract(headers);
      expect(continued).toBeInstanceOf(Trace);
      expect(continued!.traceId).toBe(callerTrace);
      await continued!.step('work', async (step) => {
        expect(currentTrace(step.context)).toBe(continued);
        await step.modelCall('gpt-x', { provider: 'openai' }, () => {});
      });
      continued!.toolCall('search', () => {});
      continued!.fail('Ignored');
      expect(await worker.telmoni.flush()).toBe(true);
    } finally {
      await worker.telmoni.shutdown();
    }
    const spans = worker.transport.spans();
    expect(spans.map((span) => span.name).sort()).toEqual(['chat gpt-x', 'execute_tool search', 'work']);
    for (const span of spans) {
      expect(span.traceId, span.name).toBe(callerTrace);
      expect(span.links, span.name).toBeUndefined();
      expect(attributesOf(span)['telmoni.agent'], span.name).toBe('orchestrator');
    }
    const named = byName(worker.transport);
    const parentOf = (span: JsonSpan | undefined) => span?.parentSpanId;
    expect(parentOf(named['work'])).toBe(callerSpan);
    expect(parentOf(named['execute_tool search'])).toBe(callerSpan);
    expect(parentOf(named['chat gpt-x'])).toBe(named['work']!.spanId);
  });

  it("a continued trace's spans carry its agent cut to its cap, where uncut ingest refused every one", async () => {
    const h = harness();
    const given = fixture.extract.find((each) => each.name.startsWith('an agent past'))!;
    try {
      h.telmoni.extract(given.headers)!.step('work', () => {});
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(attributesOf(byName(h.transport)['work']!)['telmoni.agent']).toBe(given.trace!.agent);
  });

  it('a continued trace hands itself on again, under the span current here', async () => {
    const h = harness();
    const first = { traceparent: '00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01', baggage: 'telmoni.agent=bot' };
    const onwards: Record<string, string> = {};
    try {
      const continued = h.telmoni.extract(first)!;
      await continued.step('relay', (step) => {
        h.telmoni.inject(onwards, step);
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const relay = byName(h.transport)['relay']!;
    expect(onwards).toEqual({
      traceparent: `00-4bf92f3577b34da6a3ce929d0e0e4736-${relay.spanId}-01`,
      baggage: 'telmoni.agent=bot',
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
