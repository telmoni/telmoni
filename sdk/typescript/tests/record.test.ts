/**
 * A run recorded after it ended: `recordRun` and `sdk/fixtures/recorded.json`.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import { QueueFullError } from '../src/index';
import type { CallRecord, ModelCallRecord, RunRecord, StepRecord, ToolCallRecord } from '../src/index';
import { type ExportBody, type JsonAttribute, attributesOf, harness, loadFixture } from './helpers';

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
  it('is refused whole when the queue has no room for it, and goes whole once there is', async () => {
    // Room for five spans, and a worker that will not wake until flushed.
    const h = harness({}, { processor: { maxQueueSize: 5, maxBatchSize: 100, scheduleDelaySeconds: 60 } });
    const golden = theRecord();
    const call: ToolCallRecord = { type: 'tool_call', name: 't', start: golden.start, end: golden.start };
    const record: RunRecord = { ...golden, inside: [call, call] };
    try {
      // Nothing yields inside the run, so its start and two steps hold three of the five slots.
      h.telmoni.run('bot', (run) => {
        run.step('s0', () => {});
        run.step('s1', () => {});
        expect(() => h.telmoni.recordRun(record)).toThrow(QueueFullError);
        expect(() => h.telmoni.recordRun(record)).toThrow('4 spans');
      });
      expect(await h.telmoni.flush()).toBe(true);
      h.telmoni.recordRun(record);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const names = h.transport.spans().map((span) => span.name);
    expect(names.filter((name) => name === 'execute_tool t')).toHaveLength(2);
    expect(names).toHaveLength(8);
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
    expect(h.warnings).toEqual([]);
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
  it.each<[string, Partial<RunRecord>, ErrorConstructor, RegExp]>([
    ['a blank agent', { agent: ' ' }, TypeError, /agent's name/],
    ['a run id that is not a UUID', { runId: 'run-1' }, TypeError, /must be a UUID/],
    ['a run id with some of its hyphens', { runId: '3d6f8a1c5b2e-4f7a-9c0d-8e1f2a3b4c5d' }, TypeError, /must be a UUID/],
    ['the nil UUID', { runId: '00000000-0000-0000-0000-000000000000' }, TypeError, /nil UUID/],
    ['a time that is not one', { start: Number.NaN }, TypeError, /Date or milliseconds/],
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
  ])('is refused, sending nothing, for %s', async (_what, change, kind, message) => {
    const h = harness();
    try {
      expect(() => h.telmoni.recordRun({ ...theRecord(), ...change })).toThrow(kind);
      expect(() => h.telmoni.recordRun({ ...theRecord(), ...change })).toThrow(message);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toEqual([]);
  });
});
