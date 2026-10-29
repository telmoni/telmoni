/**
 * A run recorded after it ended: `recordRun` and `sdk/fixtures/recorded.json`.
 */

import { SamplingDecision, type Sampler, type SamplingResult } from '@opentelemetry/sdk-trace-base';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { QueueFullError, ShutdownError } from '../src/index';
import type { CallRecord, ModelCallRecord, RunRecord, StepRecord, ToolCallRecord, Usage } from '../src/index';
import { type ExportBody, type Harness, type JsonAttribute, attributesOf, harness, loadFixture } from './helpers';

const VERSIONED = new Set(['telemetry.sdk.version', 'telemetry.distro.version']);

interface Outcome {
  error_type?: string;
  cancelled?: boolean;
}

interface CallSpec {
  model_call?: Outcome & {
    model: string;
    provider?: string;
    operation?: string;
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
}

interface ItemSpec extends CallSpec {
  step?: Outcome & { name: string; start: string; end: string; calls?: CallSpec[] };
}

interface RecordedFixture {
  scenario: {
    record: Outcome & {
      run_id: string;
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

function callFrom(spec: CallSpec): CallRecord {
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

function recordFrom(spec: RecordedFixture['scenario']['record']): RunRecord {
  const inside = spec.inside.map((item): StepRecord | CallRecord => {
    if (item.step) {
      const step = item.step;
      return {
        type: 'step',
        name: step.name,
        start: new Date(step.start),
        end: new Date(step.end),
        calls: (step.calls ?? []).map(callFrom),
        ...outcome(step),
      };
    }
    return callFrom(item);
  });
  const record: RunRecord = {
    runId: spec.run_id,
    agent: spec.agent,
    start: new Date(spec.start),
    end: new Date(spec.end),
    inside,
    ...outcome(spec),
  };
  if (spec.user_id !== undefined) record.userId = spec.user_id;
  if (spec.session_id !== undefined) record.sessionId = spec.session_id;
  if (spec.tags !== undefined) record.tags = spec.tags;
  return record;
}

function theRecord(): RunRecord {
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

function traceOf(record: RunRecord): string {
  return record.runId.replaceAll('-', '');
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

afterEach(() => {
  vi.restoreAllMocks();
});

describe('a recorded run', () => {
  it('is refused once the client is shut down, never taken as sent', async () => {
    const h = harness();
    await h.telmoni.shutdown();
    expect(() => h.telmoni.recordRun(theRecord())).toThrow(ShutdownError);
    expect(h.transport.requests).toHaveLength(0);
  });

  it('is refused whole when the queue has no room for it, and goes whole once there is', async () => {
    // Room for four spans, and a worker that will not wake until flushed.
    const h = harness({}, { processor: { maxQueueSize: 4, maxBatchSize: 100, scheduleDelaySeconds: 60 } });
    const golden = theRecord();
    const call: ToolCallRecord = { type: 'tool_call', name: 't', start: golden.start, end: golden.start };
    const record: RunRecord = { ...golden, inside: [call, call] };
    try {
      // Nothing yields inside the run, so its two steps hold two of the four slots.
      h.telmoni.run('bot', (run) => {
        run.step('s0', () => {});
        run.step('s1', () => {});
        expect(() => h.telmoni.recordRun(record)).toThrow(QueueFullError);
        expect(() => h.telmoni.recordRun(record)).toThrow('3 spans');
      });
      expect(await h.telmoni.flush()).toBe(true);
      h.telmoni.recordRun(record);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const names = h.transport.spans().map((span) => span.name);
    expect(names.filter((name) => name === 'execute_tool t')).toHaveLength(2);
    expect(names).toHaveLength(6);
    expect(h.warnings).toEqual([]);
  });

  it('matches the golden export', async () => {
    const fixture = loadFixture<RecordedFixture>('recorded.json');
    const h = harness();
    try {
      h.telmoni.recordRun(recordFrom(fixture.scenario.record));
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
        h.telmoni.recordRun(record);
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
    const beside: RunRecord = { ...record, runId: '0191a3b4-2222-4aaa-8bbb-000000000002' };
    const h = harness();
    let sent = 0;
    try {
      h.telmoni.run('live', (run) => {
        run.step('live step', () => {});
        h.telmoni.recordRun(record);
        h.telmoni.recordRun(beside);
      });
      expect(await h.telmoni.flush()).toBe(true);
      sent = h.transport.requests.length;
      h.telmoni.recordRun(record);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const bodies = h.transport.requests.map((request) => request.body);
    for (const body of bodies) expect(tracesIn(body).size).toBe(1);
    const alone = bodies.slice(0, sent).filter((body) => tracesIn(body).has(traceOf(record)));
    expect(alone).toEqual(bodies.slice(sent));
    expect(bodies.slice(0, sent).some((body) => tracesIn(body).has(traceOf(beside)))).toBe(true);
  });

  it('larger than a batch is cut from its own start', async () => {
    const h = harness({}, { processor: { maxBatchSize: 3, scheduleDelaySeconds: 60 } });
    const golden = theRecord();
    const call: ToolCallRecord = { type: 'tool_call', name: 't', start: golden.start, end: golden.start };
    try {
      h.telmoni.run('bot', (run) => {
        run.step('s0', () => {});
        run.step('s1', () => {});
        h.telmoni.recordRun({ ...golden, inside: [call, call, call] });
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.bodies().map((body) => body.resourceSpans[0]!.scopeSpans[0]!.spans.map((s) => s.name))).toEqual([
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
      h.telmoni.run('live', (run) => {
        sampler.hook = () => run.step('live step', () => {});
        h.telmoni.recordRun(theRecord());
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
      await h.telmoni.run('live', async (run) => {
        // Room for one step beside the record's three spans.
        sampler.hook = () => {
          run.step('s0', () => {});
          run.step('s1', () => {});
        };
        h.telmoni.recordRun({ ...golden, inside: [call, call] });
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
      await h.telmoni.run('live', async (run) => {
        run.step('live step', () => {});
        h.telmoni.recordRun(theRecord());
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
      expect(() => h.telmoni.recordRun({ ...golden, inside: [call] })).toThrow('the name could not be read');
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(asked, 'the run had opened').toBe(1);
    expect(h.transport.requests).toEqual([]);
  });

  it('is whole beside a run its warning hook records', async () => {
    // The warning is given before the record is built.
    const golden = theRecord();
    const call: ModelCallRecord = {
      type: 'model_call',
      model: 'm',
      start: golden.start,
      end: golden.start,
      usage: { inputTokens: Number.NaN },
    };
    const outer: RunRecord = { ...golden, inside: [call] };
    const inner: RunRecord = { ...golden, runId: '0191a3b4-3333-4aaa-8bbb-000000000003', inside: [] };
    const h: Harness = harness({}, { warn: () => h.telmoni.recordRun(inner) });
    try {
      h.telmoni.recordRun(outer);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const traces = h.transport.requests.map((request) => tracesIn(request.body));
    for (const each of traces) expect(each.size).toBe(1);
    expect(traces.map((each) => [...each][0]).sort()).toEqual([traceOf(outer), traceOf(inner)].sort());
  });

  it('reads milliseconds as the epoch, however early, and a Date the same', async () => {
    const asNumbers = harness();
    const asDates = harness();
    try {
      // Smaller than performance.now(): read as a performance reading by
      // OpenTelemetry's own conversion.
      asNumbers.telmoni.recordRun({ runId: theRecord().runId, agent: 'bot', start: 1000, end: 2500 });
      asDates.telmoni.recordRun({ runId: theRecord().runId, agent: 'bot', start: new Date(1000), end: new Date(2500) });
      expect(await asNumbers.telmoni.flush()).toBe(true);
      expect(await asDates.telmoni.flush()).toBe(true);
    } finally {
      await asNumbers.telmoni.shutdown();
      await asDates.telmoni.shutdown();
    }
    const run = asNumbers.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    expect([run.startTimeUnixNano, run.endTimeUnixNano]).toEqual(['1000000000', '2500000000']);
    expect(asNumbers.transport.requests.map((r) => r.body)).toEqual(asDates.transport.requests.map((r) => r.body));
  });

  it('sends one ending at the last millisecond ingest reads, in July 2554', async () => {
    const h = harness();
    try {
      h.telmoni.recordRun({ runId: theRecord().runId, agent: 'bot', start: 1000, end: 18_446_744_073_709 });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const run = h.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    expect(run.endTimeUnixNano).toBe('18446744073709000000');
  });

  it('takes a trace of its own and leaves the live run where it was', async () => {
    const h = harness();
    const record = theRecord();
    try {
      h.telmoni.run('live', (run) => {
        h.telmoni.recordRun(record);
        run.step('after', () => undefined);
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const spans = h.transport.spans();
    const recorded = spans.filter((span) => attributesOf(span)['telmoni.run_id'] === record.runId);
    expect(new Set(recorded.map((span) => span.traceId))).toEqual(new Set([record.runId.replaceAll('-', '')]));
    expect(recorded.find((span) => span.name === 'invoke_agent claude-code')!.parentSpanId).toBeUndefined();
    const live = spans.find((span) => span.name === 'invoke_agent live')!;
    const after = spans.find((span) => span.name === 'after')!;
    expect(after.parentSpanId).toBe(live.spanId);
    expect(after.traceId).toBe(live.traceId);
  });

  it('marks a failed run on its own span', async () => {
    const h = harness();
    try {
      h.telmoni.recordRun({ ...theRecord(), errorType: 'RateLimitError', cancelled: false, inside: [] });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const run = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    expect(run.status).toEqual({ code: 2 });
    expect(attributesOf(run)['error.type']).toBe('RateLimitError');
    expect(attributesOf(run)['telmoni.cancelled']).toBeUndefined();
  });

  it('is dropped whole by the sample rate', async () => {
    const h = harness({ sampleRate: 0.25 });
    try {
      vi.spyOn(Math, 'random').mockReturnValue(0.9);
      h.telmoni.recordRun(theRecord());
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toEqual([]);
  });

  const start = Date.parse('2026-10-12T09:00:00.000Z');
  const step: StepRecord = { type: 'step', name: 's', start, end: start };
  const tool: ToolCallRecord = { type: 'tool_call', name: 't', start, end: start };
  it.each<[string, Partial<RunRecord> | ((record: RunRecord) => RunRecord), ErrorConstructor, RegExp]>([
    ['a blank agent', { agent: ' ' }, TypeError, /agent's name/],
    ['a run id that is not a UUID', { runId: 'run-1' }, TypeError, /must be a UUID/],
    ['a run id with some of its hyphens', { runId: '3d6f8a1c5b2e-4f7a-9c0d-8e1f2a3b4c5d' }, TypeError, /must be a UUID/],
    ['the nil UUID', { runId: '00000000-0000-0000-0000-000000000000' }, TypeError, /nil UUID/],
    ['a time that is not one', { start: Number.NaN }, TypeError, /Date or milliseconds/],
    ['a time before the epoch', { start: -1 }, RangeError, /after the Unix epoch/],
    ['a time at the epoch itself', { start: 0 }, RangeError, /after the Unix epoch/],
    ['a time past 2554', { end: Date.parse('2600-01-01T00:00:00.000Z') }, RangeError, /2\^64 nanoseconds/],
    ['a time a millisecond past 2^64 nanoseconds', { end: 18_446_744_073_710 }, RangeError, /2\^64 nanoseconds/],
    ["a run's agent past 128 characters", { agent: 'a'.repeat(129) }, RangeError, /run's agent must be up to 128 characters/],
    ["a run's user past 128 characters", { userId: 'u'.repeat(129) }, RangeError, /run's userId must be up to 128/],
    ["a run's session past 200 characters", { sessionId: 's'.repeat(201) }, RangeError, /run's sessionId must be up to 200/],
    ["a run's error type past 128 characters", { errorType: 'E'.repeat(129) }, RangeError, /run's errorType must be up to 128/],
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
    ["a tool call's error type past 128 characters", { inside: [{ ...tool, errorType: 'E'.repeat(129) }] }, RangeError, /tool call's errorType must be up to 128/],
    ["a step's name that is empty", { inside: [{ ...step, name: '' }] }, TypeError, /step's name cannot be empty/],
    [
      "a model call's model that is empty",
      { inside: [{ type: 'model_call', model: '', start, end: start }] },
      TypeError,
      /model call's model cannot be empty/,
    ],
    ['a run that ends before it starts', { end: start - 1000 }, RangeError, /end before it starts/],
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
    [
      'a step inside a step',
      { inside: [{ type: 'step', name: 's', start, end: start, calls: [{ ...step } as unknown as CallRecord] }] },
      TypeError,
      /call's type must be/,
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
    const make = (): RunRecord => (typeof change === 'function' ? change(theRecord()) : { ...theRecord(), ...change });
    try {
      expect(() => h.telmoni.recordRun(make())).toThrow(kind);
      expect(() => h.telmoni.recordRun(make())).toThrow(message);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toEqual([]);
    expect(asked).toBe(0);
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
  it.each<[string, (record: RunRecord) => RunRecord]>([
    ['inside', (record) => Object.defineProperty({ ...record }, 'inside', { get: rebuilt })],
    ["a step's calls", (record) => ({ ...record, inside: [Object.defineProperty({ ...step }, 'calls', { get: rebuilt })] })],
  ])('is read once, each call with its own tokens, when %s is built on each read', async (_what, build) => {
    const h = harness();
    try {
      h.telmoni.recordRun(build(theRecord()));
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
      h.telmoni.recordRun(record);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const run = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    const sent = Object.entries(attributesOf(run)).filter(([key]) => key.startsWith('telmoni.tag.'));
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
      h.telmoni.recordRun({ ...theRecord(), tags: new Labels() });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const run = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    expect(attributesOf(run)['telmoni.tag.team']).toBe('x');
  });

  it('sends a tool call with an empty name, which ingest reads as none', async () => {
    const h = harness();
    try {
      h.telmoni.recordRun({ ...theRecord(), inside: [{ ...tool, name: '' }] });
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
      h.telmoni.recordRun({ ...theRecord(), inside: [{ type: 'model_call', model: 'm', start, end: start, usage: new Counted() }] });
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
      h.telmoni.recordRun({
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
      h.telmoni.recordRun({ ...theRecord(), tags, errorType: 'Boom', cancelled: true });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const run = h.transport.spans().find((span) => span.name === 'invoke_agent claude-code')!;
    const attributes = attributesOf(run);
    const sent = Object.keys(attributes).filter((key) => key.startsWith('telmoni.tag.'));
    expect(sent).toEqual(Array.from({ length: 32 }, (_, n) => `telmoni.tag.t${String(n).padStart(3, '0')}`));
    expect(attributes['error.type']).toBe('Boom');
    expect(attributes['telmoni.cancelled']).toBe(true);
    expect(h.warnings).toContain('telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent');
  });
});
