/**
 * A trace recorded after it ended: `recordTrace` and `sdk/fixtures/recorded.json`.
 */

import { SamplingDecision, type Sampler, type SamplingResult } from '@opentelemetry/sdk-trace-base';
import { describe, expect, it } from 'vitest';

import { spanIdAt } from '../src/ids';
import { QueueFullError, ShutdownError } from '../src/index';
import type { CallRecord, ModelCallRecord, StepRecord, ToolCallRecord, TraceRecord, Usage } from '../src/index';
import {
  type Answer,
  type ExportBody,
  type Harness,
  type JsonAttribute,
  answer,
  attributesOf,
  harness,
  loadFixture,
} from './helpers';

const VERSIONED = new Set(['telemetry.sdk.version', 'telemetry.distro.version']);

interface Outcome {
  error_type?: string;
  cancelled?: boolean;
}

interface ItemSpec {
  model_call?: Outcome & {
    model: string;
    provider?: string;
    operation?: string;
    reasoning_effort?: string;
    input_messages?: unknown;
    start: string;
    end: string;
    usage?: {
      input_tokens: number;
      output_tokens: number;
      cache_read_input_tokens: number;
      cache_write_input_tokens: number;
      cost?: number;
    };
  };
  tool_call?: Outcome & { name: string; start: string; end: string };
  step?: Outcome & { name: string; start: string; end: string; calls?: ItemSpec[] };
}

interface RecordedFixture {
  scenario: {
    record: Outcome & {
      trace_id: string;
      agent: string;
      user_id?: string;
      session_id?: string;
      tags?: Record<string, string>;
      start: string;
      end: string;
      inside: ItemSpec[];
    };
    capture_content: boolean;
  };
  export: ExportBody;
}

function outcome(spec: Outcome): { errorType?: string; cancelled?: boolean } {
  const out: { errorType?: string; cancelled?: boolean } = {};
  if (spec.error_type !== undefined) out.errorType = spec.error_type;
  if (spec.cancelled !== undefined) out.cancelled = spec.cancelled;
  return out;
}

function itemFrom(spec: ItemSpec): StepRecord | CallRecord {
  if (spec.step) {
    const step = spec.step;
    return {
      type: 'step',
      name: step.name,
      start: new Date(step.start),
      end: new Date(step.end),
      calls: (step.calls ?? []).map(itemFrom),
      ...outcome(step),
    };
  }
  if (spec.model_call) {
    const call = spec.model_call;
    const record: ModelCallRecord = {
      type: 'model_call',
      model: call.model,
      start: new Date(call.start),
      end: new Date(call.end),
      ...outcome(call),
    };
    if (call.provider !== undefined) record.provider = call.provider;
    if (call.operation !== undefined) record.operation = call.operation;
    if (call.reasoning_effort !== undefined) record.reasoningEffort = call.reasoning_effort;
    if (call.input_messages !== undefined) record.inputMessages = call.input_messages;
    if (call.usage !== undefined) {
      record.usage = {
        inputTokens: call.usage.input_tokens,
        outputTokens: call.usage.output_tokens,
        cacheReadInputTokens: call.usage.cache_read_input_tokens,
        cacheWriteInputTokens: call.usage.cache_write_input_tokens,
        cost: call.usage.cost,
      };
    }
    return record;
  }
  const tool = spec.tool_call!;
  const record: ToolCallRecord = {
    type: 'tool_call',
    name: tool.name,
    start: new Date(tool.start),
    end: new Date(tool.end),
    ...outcome(tool),
  };
  return record;
}

function recordFrom(spec: RecordedFixture['scenario']['record']): TraceRecord {
  const record: TraceRecord = {
    traceId: spec.trace_id,
    agent: spec.agent,
    start: new Date(spec.start),
    end: new Date(spec.end),
    inside: spec.inside.map(itemFrom),
    ...outcome(spec),
  };
  if (spec.user_id !== undefined) record.userId = spec.user_id;
  if (spec.session_id !== undefined) record.sessionId = spec.session_id;
  if (spec.tags !== undefined) record.tags = spec.tags;
  return record;
}

function theRecord(): TraceRecord {
  return recordFrom(loadFixture<RecordedFixture>('recorded.json').scenario.record);
}

/** Calls its hook once, as the next span with no parent starts: a record's own span, mid-build. */
class Hooked implements Sampler {
  hook: (() => void) | undefined;

  shouldSample(): SamplingResult {
    const hook = this.hook;
    this.hook = undefined;
    hook?.();
    return { decision: SamplingDecision.RECORD_AND_SAMPLED };
  }

  toString(): string {
    return 'Hooked';
  }
}

/** Its items as an iterator, which a first read spends. */
function* once<T>(...items: T[]): Generator<T> {
  yield* items;
}

function tracesIn(body: string): Set<string> {
  const parsed = JSON.parse(body) as ExportBody;
  return new Set(parsed.resourceSpans.flatMap((r) => r.scopeSpans.flatMap((s) => s.spans.map((span) => span.traceId))));
}

// Python sorts by code point; the fixture was written in that order.
function byKey(a: JsonAttribute, b: JsonAttribute): number {
  return a.key < b.key ? -1 : a.key > b.key ? 1 : 0;
}

/** The fixture's normalization, as its README sets it out. */
function normalize(body: ExportBody): ExportBody {
  const out = structuredClone(body);
  expect(out.resourceSpans).toHaveLength(1);
  const resourceSpans = out.resourceSpans[0]!;
  expect(resourceSpans.scopeSpans).toHaveLength(1);
  const scopeSpans = resourceSpans.scopeSpans[0]!;
  scopeSpans.scope.version = '<version>';
  for (const span of scopeSpans.spans) span.attributes = [...span.attributes].sort(byKey);
  resourceSpans.resource.attributes = [...resourceSpans.resource.attributes].sort(byKey);
  for (const attribute of resourceSpans.resource.attributes) {
    if (attribute.key === 'telemetry.sdk.language') attribute.value = { stringValue: '<language>' };
    else if (VERSIONED.has(attribute.key)) attribute.value = { stringValue: '<version>' };
  }
  return out;
}

describe('a recorded trace', () => {
  it('is refused once the client is shut down, never taken as sent', async () => {
    const h = harness();
    await h.telmoni.shutdown();
    expect(() => h.telmoni.recordTrace(theRecord())).toThrow(ShutdownError);
    expect(h.transport.requests).toHaveLength(0);
  });

  it('is refused whole when the queue has no room for it, and goes whole once there is', async () => {
    // Room for four spans, and a worker that will not wake until flushed.
    const h = harness({}, { processor: { maxQueueSize: 4, maxBatchSize: 100, scheduleDelaySeconds: 60 } });
    const golden = theRecord();
    const call: ToolCallRecord = { type: 'tool_call', name: 't', start: golden.start, end: golden.start };
    const record: TraceRecord = { ...golden, inside: [call, call] };
    try {
      // Nothing yields inside the trace, so its two steps hold two of the four slots.
      h.telmoni.trace('bot', (trace) => {
        trace.step('s0', () => {});
        trace.step('s1', () => {});
        expect(() => h.telmoni.recordTrace(record)).toThrow(QueueFullError);
        expect(() => h.telmoni.recordTrace(record)).toThrow('3 spans; flush or wait, then record it again');
      });
      expect(await h.telmoni.flush()).toBe(true);
      h.telmoni.recordTrace(record);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const names = h.transport.spans().map((span) => span.name);
    expect(names.filter((name) => name === 'execute_tool t')).toHaveLength(2);
    expect(names).toHaveLength(6);
    expect(h.warnings).toEqual([]);
  });

  it('names the bound to raise for a record of more spans than the queue holds, which no flush makes room for', async () => {
    const h = harness({}, { processor: { maxQueueSize: 2 } });
    const golden = theRecord();
    const call: ToolCallRecord = { type: 'tool_call', name: 't', start: golden.start, end: golden.start };
    try {
      expect(() => h.telmoni.recordTrace({ ...golden, inside: [call, call] })).toThrow(
        new QueueFullError(3, 2),
      );
      expect(new QueueFullError(3, 2).message).toBe(
        "the export queue has no room for the record's 3 spans; the queue holds 2, so raise its size to record it",
      );
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toHaveLength(0);
  });

  it('matches the golden export', async () => {
    const fixture = loadFixture<RecordedFixture>('recorded.json');
    const h = harness();
    try {
      h.telmoni.recordTrace(recordFrom(fixture.scenario.record));
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const bodies = h.transport.bodies();
    expect(bodies, 'a record travels in one export').toHaveLength(1);
    expect(normalize(bodies[0]!)).toEqual(fixture.export);
    for (const request of h.transport.requests) {
      expect(request.headers['Authorization']).toBe('Bearer telmoni_test_key');
      expect(request.headers['User-Agent']).toMatch(/^telmoni-typescript\//);
    }
    // The record's one call of an operation ingest reads as no model call goes as chat.
    expect(h.warnings).toEqual([
      "telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat",
    ]);
  });

  it('is the same bytes when sent again', async () => {
    const record = theRecord();
    const sent: string[][] = [];
    for (let n = 0; n < 20; n++) {
      const h = harness();
      try {
        h.telmoni.recordTrace(record);
        expect(await h.telmoni.flush()).toBe(true);
      } finally {
        await h.telmoni.shutdown();
      }
      sent.push(h.transport.requests.map((request) => request.body));
    }
    expect(sent[0]).toHaveLength(1);
    for (const bodies of sent) expect(bodies).toEqual(sent[0]);
  });

  it('travels in exports of its own, so it is the same bytes whatever is queued beside it', async () => {
    const record = theRecord();
    const beside: TraceRecord = { ...record, traceId: '0191a3b422224aaa8bbb000000000002' };
    const h = harness();
    let sent = 0;
    try {
      h.telmoni.trace('live', (trace) => {
        trace.step('live step', () => {});
        h.telmoni.recordTrace(record);
        h.telmoni.recordTrace(beside);
      });
      expect(await h.telmoni.flush()).toBe(true);
      sent = h.transport.requests.length;
      h.telmoni.recordTrace(record);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const bodies = h.transport.requests.map((request) => request.body);
    for (const body of bodies) expect(tracesIn(body).size).toBe(1);
    const alone = bodies.slice(0, sent).filter((body) => tracesIn(body).has(record.traceId));
    expect(alone).toEqual(bodies.slice(sent));
    expect(bodies.slice(0, sent).some((body) => tracesIn(body).has(beside.traceId))).toBe(true);
  });

  it('larger than a batch is cut from its own start', async () => {
    const h = harness({}, { processor: { maxBatchSize: 3, scheduleDelaySeconds: 60 } });
    const golden = theRecord();
    const call: ToolCallRecord = { type: 'tool_call', name: 't', start: golden.start, end: golden.start };
    try {
      await h.telmoni.trace('bot', async (trace) => {
        expect(await h.transport.waitFor(1), "the trace's opening, at once").toBe(true);
        trace.step('s0', () => {});
        trace.step('s1', () => {});
        h.telmoni.recordTrace({ ...golden, inside: [call, call, call] });
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.bodies().map((body) => body.resourceSpans[0]!.scopeSpans[0]!.spans.map((s) => s.name))).toEqual([
      ['telmoni.trace.start'],
      ['s0', 's1'],
      ['execute_tool t', 'execute_tool t', 'execute_tool t'],
      ['invoke_agent claude-code'],
      ['invoke_agent bot'],
    ]);
  });

  it('travels apart from a span ending while it is built', async () => {
    const sampler = new Hooked();
    const h = harness({ sampler });
    try {
      h.telmoni.trace('live', (trace) => {
        sampler.hook = () => trace.step('live step', () => {});
        h.telmoni.recordTrace(theRecord());
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    for (const request of h.transport.requests) expect(tracesIn(request.body).size).toBe(1);
    expect(h.transport.spans().map((span) => span.name)).toContain('live step');
  });

  it('keeps its room while it is built, a live span past it dropped', async () => {
    const sampler = new Hooked();
    // Room for four spans, and a worker that will not wake until flushed.
    const h = harness({ sampler }, { processor: { maxQueueSize: 4, maxBatchSize: 100, scheduleDelaySeconds: 60 } });
    const golden = theRecord();
    const call: ToolCallRecord = { type: 'tool_call', name: 't', start: golden.start, end: golden.start };
    try {
      await h.telmoni.trace('live', async (trace) => {
        expect(await h.transport.waitFor(1), "the trace's opening, out of the queue").toBe(true);
        // Room for one step beside the record's three spans.
        sampler.hook = () => {
          trace.step('s0', () => {});
          trace.step('s1', () => {});
        };
        h.telmoni.recordTrace({ ...golden, inside: [call, call] });
        expect(await h.telmoni.flush()).toBe(true);
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(
      h.transport
        .spans()
        .map((span) => span.name)
        .sort(),
    ).toEqual([
      'execute_tool t',
      'execute_tool t',
      'invoke_agent claude-code',
      'invoke_agent live',
      's0',
    ]);
    expect(h.warnings).toEqual(['telmoni: the export queue was full; 1 spans were dropped']);
  });

  it('goes at once', async () => {
    const h = harness({}, { processor: { scheduleDelaySeconds: 60 } });
    try {
      await h.telmoni.trace('live', async (trace) => {
        trace.step('live step', () => {});
        h.telmoni.recordTrace(theRecord());
        expect(await h.transport.waitFor(2), 'the record waited for the delay').toBe(true);
      });
    } finally {
      await h.telmoni.shutdown();
    }
  });

  it('sends nothing when an item of its throws as it is read, part-way through', async () => {
    const sampler = new Hooked();
    let asked = 0;
    sampler.hook = () => {
      asked += 1;
    };
    const h = harness({ sampler });
    const golden = theRecord();
    const call = { type: 'tool_call', start: golden.start, end: golden.start } as ToolCallRecord;
    let reads = 0;
    // Its second read, the build's, throws.
    Object.defineProperty(call, 'name', {
      get(): string {
        reads += 1;
        if (reads > 1) throw new Error('the name could not be read');
        return 't';
      },
    });
    try {
      expect(() => h.telmoni.recordTrace({ ...golden, inside: [call] })).toThrow('the name could not be read');
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(asked, 'the trace had opened').toBe(1);
    expect(h.transport.requests).toEqual([]);
  });

  it('is whole beside a trace its warning hook records', async () => {
    // The warning is given before the record is built.
    const golden = theRecord();
    const call: ModelCallRecord = {
      type: 'model_call',
      model: 'm',
      start: golden.start,
      end: golden.start,
      usage: { inputTokens: Number.NaN },
    };
    const outer: TraceRecord = { ...golden, inside: [call] };
    const inner: TraceRecord = { ...golden, traceId: '0191a3b433334aaa8bbb000000000003', inside: [] };
    const h: Harness = harness({}, { warn: () => h.telmoni.recordTrace(inner) });
    try {
      h.telmoni.recordTrace(outer);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const traces = h.transport.requests.map((request) => tracesIn(request.body));
    for (const each of traces) expect(each.size).toBe(1);
    expect(traces.map((each) => [...each][0]).sort()).toEqual([outer.traceId, inner.traceId].sort());
  });

  it('reads milliseconds as the epoch, however early, and a Date the same', async () => {
    const asNumbers = harness();
    const asDates = harness();
    try {
      // Smaller than performance.now(): read as a performance reading by
      // OpenTelemetry's own conversion.
      asNumbers.telmoni.recordTrace({ traceId: theRecord().traceId, agent: 'bot', start: 1000, end: 2500 });
      asDates.telmoni.recordTrace({ traceId: theRecord().traceId, agent: 'bot', start: new Date(1000), end: new Date(2500) });
      expect(await asNumbers.telmoni.flush()).toBe(true);
      expect(await asDates.telmoni.flush()).toBe(true);
    } finally {
      await asNumbers.telmoni.shutdown();
      await asDates.telmoni.shutdown();
    }
    const agent = asNumbers.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    expect([agent.startTimeUnixNano, agent.endTimeUnixNano]).toEqual(['1000000000', '2500000000']);
    expect(asNumbers.transport.requests.map((r) => r.body)).toEqual(asDates.transport.requests.map((r) => r.body));
  });

  it('sends one ending at the last millisecond ingest reads, in July 2554', async () => {
    const h = harness();
    try {
      h.telmoni.recordTrace({ traceId: theRecord().traceId, agent: 'bot', start: 1000, end: 18_446_744_073_709 });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    expect(agent.endTimeUnixNano).toBe('18446744073709000000');
  });

  it('goes under the id it gives, linked to nothing, and leaves the live trace where it was', async () => {
    const h = harness();
    const record = theRecord();
    try {
      h.telmoni.trace('live', (trace) => {
        h.telmoni.recordTrace(record);
        trace.step('after', () => undefined);
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const spans = h.transport.spans();
    const recorded = spans.filter((span) => span.traceId === record.traceId);
    expect(recorded.map((span) => span.name)).toContain('invoke_agent claude-code');
    const root = recorded.find((span) => span.name === 'invoke_agent claude-code')!;
    expect(root.parentSpanId).toBeUndefined();
    expect(root.links).toBeUndefined();
    const live = spans.find((span) => span.name === 'invoke_agent live')!;
    const after = spans.find((span) => span.name === 'after')!;
    expect(live.traceId).not.toBe(record.traceId);
    expect(after.parentSpanId).toBe(live.spanId);
    expect(after.traceId).toBe(live.traceId);
  });

  it('sends a trace id given in capitals lowercase, leading zeros kept', async () => {
    const h = harness();
    try {
      h.telmoni.recordTrace({ ...theRecord(), traceId: '0000A3B47C2E7D3F8E4A5B6C7D8E9F01', inside: [] });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    expect(agent.traceId).toBe('0000a3b47c2e7d3f8e4a5b6c7d8e9f01');
  });

  it("marks a failed trace on the agent's span", async () => {
    const h = harness();
    try {
      h.telmoni.recordTrace({ ...theRecord(), errorType: 'RateLimitError', cancelled: false, inside: [] });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    expect(agent.status).toEqual({ code: 2 });
    expect(attributesOf(agent)['error.type']).toBe('RateLimitError');
    expect(attributesOf(agent)['telmoni.cancelled']).toBeUndefined();
  });

  it("sends its content only when asked and the project allows, and a call's reasoning effort in every mode", async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const at = theRecord().start;
    const sendWith = async (captureContent: boolean, answers: Answer[]): Promise<Record<string, Record<string, unknown>>> => {
      const h = harness({ captureContent }, {}, answers);
      try {
        h.telmoni.trace('warm', () => {});
        expect(await h.telmoni.flush()).toBe(true);
        h.telmoni.recordTrace({
          ...theRecord(),
          inputMessages: ['asked'],
          outputMessages: { said: 'answered' },
          inside: [
            {
              type: 'step',
              name: 'subagent',
              start: at,
              end: at,
              inputMessages: ['the task'],
              outputMessages: ['the answer'],
              calls: [
                {
                  type: 'step',
                  name: 'its subagent',
                  start: at,
                  end: at,
                  inputMessages: ['its task'],
                  outputMessages: ['its answer'],
                  calls: [
                    {
                      type: 'model_call',
                      model: 'm',
                      reasoningEffort: 'high',
                      start: at,
                      end: at,
                      inputMessages: ['the question'],
                      outputMessages: { said: 'it' },
                    },
                  ],
                },
              ],
            },
            { type: 'tool_call', name: 't', start: at, end: at, arguments: { q: 1 }, result: 'found' },
          ],
        });
        expect(await h.telmoni.flush()).toBe(true);
      } finally {
        await h.telmoni.shutdown();
      }
      return Object.fromEntries(h.transport.spans().map((span) => [span.name, attributesOf(span)]));
    };
    const content = {
      'invoke_agent claude-code': { 'gen_ai.input.messages': '["asked"]', 'gen_ai.output.messages': '{"said":"answered"}' },
      subagent: { 'gen_ai.input.messages': '["the task"]', 'gen_ai.output.messages': '["the answer"]' },
      'its subagent': { 'gen_ai.input.messages': '["its task"]', 'gen_ai.output.messages': '["its answer"]' },
      'chat m': { 'gen_ai.input.messages': '["the question"]', 'gen_ai.output.messages': '{"said":"it"}' },
      'execute_tool t': { 'gen_ai.tool.call.arguments': '{"q":1}', 'gen_ai.tool.call.result': 'found' },
    };
    const asked = await sendWith(true, [on, on]);
    const quiet = await sendWith(false, [on, on]);
    // The project's mode off, as the default answer names it.
    const barred = await sendWith(true, []);
    for (const [name, fields] of Object.entries(content)) {
      for (const [key, value] of Object.entries(fields)) {
        expect(asked[name]?.[key], `${name} ${key}`).toBe(value);
        expect(quiet[name], name).not.toHaveProperty(key);
        expect(barred[name], name).not.toHaveProperty(key);
      }
    }
    for (const sent of [asked, quiet, barred]) expect(sent['chat m']?.['gen_ai.request.reasoning.level']).toBe('high');
  });

  it('nests a step inside a step under its span, each span at its place depth first, each step sent after what it holds', async () => {
    const record = theRecord();
    const at = record.start;
    const call = (name: string): ToolCallRecord => ({ type: 'tool_call', name, start: at, end: at });
    const h = harness();
    try {
      h.telmoni.recordTrace({
        ...record,
        agent: 'bot',
        inside: [
          {
            type: 'step',
            name: 'outer',
            start: at,
            end: at,
            calls: [
              call('a'),
              { type: 'step', name: 'middle', start: at, end: at, calls: [{ type: 'step', name: 'inner', start: at, end: at, calls: [call('b')] }] },
              call('c'),
            ],
          },
          call('d'),
        ],
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const spans = h.transport.spans();
    expect(spans.map((span) => span.name)).toEqual([
      'execute_tool a',
      'execute_tool b',
      'inner',
      'middle',
      'execute_tool c',
      'outer',
      'execute_tool d',
      'invoke_agent bot',
    ]);
    const places = ['invoke_agent bot', 'outer', 'execute_tool a', 'middle', 'inner', 'execute_tool b', 'execute_tool c', 'execute_tool d'];
    const idOf = (name: string): string => spanIdAt(record.traceId, places.indexOf(name));
    const parents: Record<string, string> = {
      outer: 'invoke_agent bot',
      'execute_tool a': 'outer',
      middle: 'outer',
      inner: 'middle',
      'execute_tool b': 'inner',
      'execute_tool c': 'outer',
      'execute_tool d': 'invoke_agent bot',
    };
    for (const span of spans) {
      expect(span.spanId, span.name).toBe(idOf(span.name));
      const parent = parents[span.name];
      expect(span.parentSpanId, span.name).toBe(parent === undefined ? undefined : idOf(parent));
    }
  });

  const start = Date.parse('2026-10-12T09:00:00.000Z');
  const step: StepRecord = { type: 'step', name: 's', start, end: start };
  const tool: ToolCallRecord = { type: 'tool_call', name: 't', start, end: start };
  /** `depth` steps, each inside the one before. */
  const nested = (depth: number): StepRecord =>
    depth === 1 ? step : { ...step, name: `s${depth}`, calls: [nested(depth - 1)] };
  it.each<[string, Partial<TraceRecord> | ((record: TraceRecord) => TraceRecord), ErrorConstructor, RegExp]>([
    ['a blank agent', { agent: ' ' }, TypeError, /agent's name/],
    ['a trace id that is no hex', { traceId: 'trace-1' }, TypeError, /must be 32 hex digits/],
    ['a trace id spelled as a UUID, with hyphens', { traceId: '3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d' }, TypeError, /must be 32 hex digits/],
    ['a trace id of 31 digits', { traceId: '3d6f8a1c5b2e4f7a9c0d8e1f2a3b4c5' }, TypeError, /must be 32 hex digits/],
    ['a trace id of zeros', { traceId: '0'.repeat(32) }, TypeError, /not all zeros/],
    ['a time that is not one', { start: Number.NaN }, TypeError, /Date or milliseconds/],
    ['a time before the epoch', { start: -1 }, RangeError, /after the Unix epoch/],
    ['a time at the epoch itself', { start: 0 }, RangeError, /after the Unix epoch/],
    ['a time past 2554', { end: Date.parse('2600-01-01T00:00:00.000Z') }, RangeError, /2\^64 nanoseconds/],
    ['a time a millisecond past 2^64 nanoseconds', { end: 18_446_744_073_710 }, RangeError, /2\^64 nanoseconds/],
    ["a trace's agent past 128 characters", { agent: 'a'.repeat(129) }, RangeError, /trace's agent must be up to 128 characters/],
    ["a trace's user past 128 characters", { userId: 'u'.repeat(129) }, RangeError, /trace's userId must be up to 128/],
    ["a trace's session past 200 characters", { sessionId: 's'.repeat(201) }, RangeError, /trace's sessionId must be up to 200/],
    ["a trace's error type past 128 characters", { errorType: 'E'.repeat(129) }, RangeError, /trace's errorType must be up to 128/],
    ["a step's name past 145 characters", { inside: [{ ...step, name: 'n'.repeat(146) }] }, RangeError, /step's name must be up to 145/],
    [
      "a tool call's name past 145 characters, in a step",
      { inside: [{ ...step, calls: [{ ...tool, name: 't'.repeat(146) }] }] },
      RangeError,
      /tool call's name must be up to 145/,
    ],
    [
      "a model call's model past 128 characters",
      { inside: [{ type: 'model_call', model: 'm'.repeat(129), start, end: start }] },
      RangeError,
      /model call's model must be up to 128/,
    ],
    [
      "a model call's provider past 128 characters",
      { inside: [{ type: 'model_call', model: 'm', provider: 'p'.repeat(129), start, end: start }] },
      RangeError,
      /model call's provider must be up to 128/,
    ],
    [
      "a model call's reasoning effort past 1024 characters, in a step",
      { inside: [{ ...step, calls: [{ type: 'model_call', model: 'm', reasoningEffort: 'r'.repeat(1025), start, end: start }] }] },
      RangeError,
      /model call's reasoningEffort must be up to 1024 characters/,
    ],
    ["a tool call's error type past 128 characters", { inside: [{ ...tool, errorType: 'E'.repeat(129) }] }, RangeError, /tool call's errorType must be up to 128/],
    ["a step's name that is empty", { inside: [{ ...step, name: '' }] }, TypeError, /step's name cannot be empty/],
    [
      "a model call's model that is empty",
      { inside: [{ type: 'model_call', model: '', start, end: start }] },
      TypeError,
      /model call's model cannot be empty/,
    ],
    ['a trace that ends before it starts', { end: start - 1000 }, RangeError, /end before it starts/],
    [
      'a call that ends before it starts',
      {
        inside: [
          {
            type: 'step',
            name: 's',
            start,
            end: start + 2000,
            calls: [{ type: 'tool_call', name: 't', start: start + 1000, end: start }],
          },
        ],
      },
      RangeError,
      /end before it starts/,
    ],
    [
      'an item of no known type',
      { inside: [{ type: 'span', name: 's', start, end: start } as unknown as StepRecord] },
      TypeError,
      /type must be/,
    ],
    ['an item that is null', { inside: [null as unknown as StepRecord] }, TypeError, /item's type must be/],
    ['a user id that is no string', { userId: 42 as unknown as string }, TypeError, /userId must be a string/],
    ['a session id that is no string', { sessionId: 7 as unknown as string }, TypeError, /sessionId must be a string/],
    ['tags that are a string', { tags: 'ab' as unknown as Record<string, string> }, TypeError, /an object of strings/],
    ['tags boxed as a string', { tags: Object('ab') as Record<string, string> }, TypeError, /an object of strings/],
    ['tags that are an array', { tags: ['ab'] as unknown as Record<string, string> }, TypeError, /an object of strings/],
    ['tags that are a Map', { tags: new Map([['team', 'x']]) as unknown as Record<string, string> }, TypeError, /an object/],
    ['a tag that is no string', { tags: { team: 1 } as unknown as Record<string, string> }, TypeError, /of strings/],
    ['an error type that is no string', { errorType: {} as unknown as string }, TypeError, /errorType must be/],
    ['cancelled that is no boolean', { cancelled: 'false' as unknown as boolean }, TypeError, /be a boolean/],
    ["a step's name that is no string", { inside: [{ ...step, name: 5 as unknown as string }] }, TypeError, /name must/],
    [
      "a model call's model that is no string",
      { inside: [{ type: 'model_call', model: null as unknown as string, start, end: start }] },
      TypeError,
      /model call's model must be a string/,
    ],
    [
      "a model call's provider that is no string",
      { inside: [{ type: 'model_call', model: 'm', start, end: start, provider: 5 as unknown as string }] },
      TypeError,
      /provider must be a string/,
    ],
    [
      "a model call's operation that is no string",
      { inside: [{ type: 'model_call', model: 'm', start, end: start, operation: 5 as unknown as string }] },
      TypeError,
      /operation must be a string/,
    ],
    [
      "a model call's reasoning effort that is no string",
      { inside: [{ type: 'model_call', model: 'm', start, end: start, reasoningEffort: { level: 'high' } as unknown as string }] },
      TypeError,
      /reasoningEffort must be a string/,
    ],
    [
      "a model call's usage that is no object",
      { inside: [{ type: 'model_call', model: 'm', start, end: start, usage: 5 as unknown as Usage }] },
      TypeError,
      /usage must be an object/,
    ],
    [
      "a model call's usage that is an array",
      { inside: [{ type: 'model_call', model: 'm', start, end: start, usage: [1, 2] as unknown as Usage }] },
      TypeError,
      /usage must be an object/,
    ],
    [
      'items that change between reads, refused for those it holds first',
      (record) =>
        Object.defineProperty({ ...record }, 'inside', {
          get: ((reads) => () => (reads++ === 0 ? [{ ...step, name: 5 }] : [tool]))(0),
        }),
      TypeError,
      /step's name must be a string/,
    ],
    [
      "a tool call's name in a step that is no string",
      { inside: [{ ...step, calls: [{ ...tool, name: null as unknown as string }] }] },
      TypeError,
      /tool call's name must be a string/,
    ],
    ['steps nested 17 deep', { inside: [nested(17)] }, RangeError, /a recorded step must be nested at most 16 deep/],
    [
      'a step that holds itself',
      (record) => {
        const calls: StepRecord[] = [];
        const looped: StepRecord = { ...step, calls };
        calls.push(looped);
        return { ...record, inside: [looped] };
      },
      RangeError,
      /nested at most 16 deep/,
    ],
    [
      'an item of no known type in a step inside a step',
      { inside: [{ ...step, calls: [{ ...step, calls: [{ type: 'span' } as unknown as CallRecord] }] }] },
      TypeError,
      /item's type must be/,
    ],
    ['inside that is an iterator', { inside: once(tool) as unknown as CallRecord[] }, TypeError, /inside must be an array/],
    [
      "a step's calls that are an iterator",
      { inside: [{ ...step, calls: once(tool) as unknown as CallRecord[] }] },
      TypeError,
      /calls must be an array/,
    ],
  ])('is refused, opening and sending nothing, for %s', async (_what, change, kind, message) => {
    // The record's own span asks the root sampler as it opens.
    const sampler = new Hooked();
    let asked = 0;
    sampler.hook = () => {
      asked += 1;
    };
    const h = harness({ sampler });
    // Made anew for each try, since a getter may count its reads.
    const make = (): TraceRecord => (typeof change === 'function' ? change(theRecord()) : { ...theRecord(), ...change });
    try {
      expect(() => h.telmoni.recordTrace(make())).toThrow(kind);
      expect(() => h.telmoni.recordTrace(make())).toThrow(message);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toEqual([]);
    expect(asked).toBe(0);
  });

  it('sends steps nested 16 deep, the deepest it takes', async () => {
    const h = harness();
    try {
      h.telmoni.recordTrace({ ...theRecord(), inside: [nested(16)] });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const spans = h.transport.spans();
    expect(spans).toHaveLength(17);
    // The deepest first, each under the next.
    for (const [inner, outer] of spans.slice(0, 16).map((span, n) => [span, spans[n + 1]] as const)) {
      expect(inner.parentSpanId).toBe(outer?.spanId);
    }
  });

  /** Its calls built afresh on each read, as a getter over rows builds them. */
  function rebuilt(): ModelCallRecord[] {
    return [1, 2, 3].map((n) => ({
      type: 'model_call',
      model: `m${n}`,
      start,
      end: start,
      usage: { inputTokens: n },
    }));
  }

  // A getter, since a spread would read it into a plain array.
  it.each<[string, (record: TraceRecord) => TraceRecord]>([
    ['inside', (record) => Object.defineProperty({ ...record }, 'inside', { get: rebuilt })],
    ["a step's calls", (record) => ({ ...record, inside: [Object.defineProperty({ ...step }, 'calls', { get: rebuilt })] })],
  ])('is read once, each call with its own tokens, when %s is built on each read', async (_what, build) => {
    const h = harness();
    try {
      h.telmoni.recordTrace(build(theRecord()));
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const tokens = h.transport
      .spans()
      .filter((span) => span.name.startsWith('chat'))
      .map((span) => [span.name, attributesOf(span)['gen_ai.usage.input_tokens']]);
    expect(tokens).toEqual([
      ['chat m1', 1],
      ['chat m2', 2],
      ['chat m3', 3],
    ]);
  });

  it('sends the tags it checked, read before the items whose getter changes them', async () => {
    const tags: Record<string, string> = { team: 'x' };
    const record = Object.defineProperty({ ...theRecord(), tags }, 'inside', {
      get: () => {
        tags['rows'] = '1';
        return [tool];
      },
    });
    const h = harness();
    try {
      h.telmoni.recordTrace(record);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    const sent = Object.entries(attributesOf(agent)).filter(([key]) => key.startsWith('telmoni.tag.'));
    expect(sent).toEqual([['telmoni.tag.team', 'x']]);
  });

  it("takes tags of a class of the caller's own, its fields as the tags", async () => {
    class Labels {
      [key: string]: string;
      constructor() {
        this['team'] = 'x';
      }
    }
    const h = harness();
    try {
      h.telmoni.recordTrace({ ...theRecord(), tags: new Labels() });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    expect(attributesOf(agent)['telmoni.tag.team']).toBe('x');
  });

  it('sends a tool call with an empty name, which ingest reads as none', async () => {
    const h = harness();
    try {
      h.telmoni.recordTrace({ ...theRecord(), inside: [{ ...tool, name: '' }] });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.spans().map((span) => span.name)).toContain('execute_tool ');
  });

  it("takes a usage of a class of the caller's own, its counts read as fields", async () => {
    class Counted {
      get inputTokens(): number {
        return 9;
      }
    }
    const h = harness();
    try {
      h.telmoni.recordTrace({ ...theRecord(), inside: [{ type: 'model_call', model: 'm', start, end: start, usage: new Counted() }] });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const call = h.transport.spans().find((span) => span.name === 'chat m')!;
    expect(attributesOf(call)['gen_ai.usage.input_tokens']).toBe(9);
  });

  it("sends a record at ingest's text caps whole, counted in characters rather than UTF-16's units", async () => {
    const start = Date.parse('2026-10-12T09:00:00.000Z');
    // Two of UTF-16's units each.
    const smile = String.fromCodePoint(0x1f600);
    const h = harness();
    try {
      h.telmoni.recordTrace({
        ...theRecord(),
        agent: smile.repeat(128),
        userId: 'u'.repeat(128),
        sessionId: '\ud83d\ude00'.repeat(200),
        inside: [
          {
            type: 'step',
            name: 'n'.repeat(145),
            start,
            end: start,
            calls: [{ type: 'tool_call', name: 't'.repeat(145), errorType: 'E'.repeat(128), start, end: start }],
          },
          {
            type: 'model_call',
            operation: 'generate_content',
            model: smile.repeat(128),
            provider: 'p'.repeat(128),
            start,
            end: start,
          },
        ],
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const names = h.transport.spans().map((span) => span.name);
    expect(names).toEqual(
      expect.arrayContaining(['n'.repeat(145), `generate_content ${smile.repeat(128)}`, `execute_tool ${'t'.repeat(145)}`]),
    );
  });

  it('sends its first 32 tags and its own attributes, its error and its cancelling among them', async () => {
    const tags = Object.fromEntries(Array.from({ length: 130 }, (_, n) => [`t${String(n).padStart(3, '0')}`, 'v']));
    const h = harness();
    try {
      h.telmoni.recordTrace({ ...theRecord(), tags, errorType: 'Boom', cancelled: true });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    const attributes = attributesOf(agent);
    const sent = Object.keys(attributes).filter((key) => key.startsWith('telmoni.tag.'));
    expect(sent).toEqual(Array.from({ length: 32 }, (_, n) => `telmoni.tag.t${String(n).padStart(3, '0')}`));
    expect(attributes['error.type']).toBe('Boom');
    expect(attributes['telmoni.cancelled']).toBe(true);
    expect(h.warnings).toContain('telmoni: a trace carries up to 32 tags, the most ingest keeps; the rest are not sent');
  });
});
