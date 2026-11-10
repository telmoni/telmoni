/**
 * Scores, live on a handle, recorded, and late through the client's
 * `score`, as `sdk/fixtures/scores.json` holds every SDK to them.
 */

import { randomUUID } from 'node:crypto';

import { trace as otel } from '@opentelemetry/api';
import { describe, expect, it } from 'vitest';

import * as contract from '../src/contract';
import type { ScoreOptions, ScoreTarget, Trace, TraceRecord } from '../src/index';
import {
  type ExportBody,
  type Harness,
  type JsonAttribute,
  type JsonSpan,
  answer,
  attributesOf,
  harness,
  loadFixture,
  one,
} from './helpers';

const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const NOT_SENT = 'telmoni: a score was not sent: ';

type Given = number | boolean | string;

interface ScoreCase {
  case: string;
  name: string;
  type: 'number' | 'boolean' | 'category';
  value: Given;
  range?: [Given, Given];
  version?: string;
  kept?: { name: string; value?: number; label?: string; version: string };
  refused?: true;
}

interface TargetCase {
  case: string;
  trace_id?: string;
  span_id?: string;
  session_id?: string;
  kept?: Record<string, string>;
  refused?: true;
}

interface LateScore {
  target: TargetCase;
  name: string;
  type: ScoreCase['type'];
  value: Given;
  range?: [number, number];
  version?: string;
}

interface Fixture {
  limits: Record<string, number>;
  scores: ScoreCase[];
  targets: TargetCase[];
  late: { scores: LateScore[]; sent: { scores: Array<Record<string, unknown>> } };
  live: {
    scores: LateScore[];
    events: Array<{ timeUnixNano: string; name: string; attributes: JsonAttribute[] }>;
  };
}

const fixture = loadFixture<Fixture>('scores.json');

/** A number score's value as the fixture writes one JSON cannot: `"NaN"`, `"Infinity"`, `"-Infinity"`. */
function valueOf(type: ScoreCase['type'], value: Given): Given {
  return type === 'number' && typeof value === 'string' ? Number(value) : value;
}

function optionsOf(spec: { type: ScoreCase['type']; range?: [Given, Given]; version?: string }): ScoreOptions {
  const options: ScoreOptions = {};
  if (spec.range !== undefined) options.range = [Number(valueOf('number', spec.range[0])), Number(valueOf('number', spec.range[1]))];
  if (spec.version !== undefined) options.version = spec.version;
  return options;
}

function targetOf(spec: TargetCase): ScoreTarget {
  const target: ScoreTarget = {};
  if (spec.trace_id !== undefined) target.traceId = spec.trace_id;
  if (spec.span_id !== undefined) target.spanId = spec.span_id;
  if (spec.session_id !== undefined) target.sessionId = spec.session_id;
  return target;
}

function exportsIn(h: Harness): JsonSpan[] {
  return h.transport.requests
    .filter((request) => request.url.endsWith('/otlp/v1/traces'))
    .flatMap((request) =>
      (JSON.parse(request.body) as ExportBody).resourceSpans.flatMap((r) => r.scopeSpans.flatMap((s) => s.spans)),
    );
}

function scoresIn(h: Harness): Array<Array<Record<string, unknown>>> {
  return h.transport.requests
    .filter((request) => request.url.endsWith('/v1/scores'))
    .map((request) => (JSON.parse(request.body) as { scores: Array<Record<string, unknown>> }).scores);
}

function agentSpan(h: Harness): JsonSpan {
  return one(exportsIn(h).filter((span) => span.name.startsWith('invoke_agent ')));
}

/** The events of the span as the fixture writes them: attributes sorted by code point, each time checked within the span. */
function normalized(span: JsonSpan): Fixture['live']['events'] {
  return (span.events ?? []).map((event) => {
    const time = BigInt(event.timeUnixNano);
    expect(time >= BigInt(span.startTimeUnixNano) && time <= BigInt(span.endTimeUnixNano)).toBe(true);
    const attributes = [...event.attributes].sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
    return { timeUnixNano: '<time>', name: event.name, attributes };
  });
}

async function scored(score: (trace: Trace) => void, config = {}): Promise<Harness> {
  const h = harness(config);
  try {
    h.telmoni.trace('bot', score);
    expect(await h.telmoni.flush()).toBe(true);
  } finally {
    await h.telmoni.shutdown();
  }
  return h;
}

describe('a live score', () => {
  for (const spec of fixture.scores) {
    it(`answers the fixture: ${spec.case}`, async () => {
      const h = await scored((trace) => trace.score(spec.name, valueOf(spec.type, spec.value), optionsOf(spec)));
      const span = agentSpan(h);
      if (spec.refused) {
        expect(span.events).toBeUndefined();
        expect(h.warnings).toHaveLength(1);
        expect(h.warnings[0]).toMatch(new RegExp(`^${NOT_SENT}`));
        expect(h.fields).toStrictEqual([{ kind: 'input' }]);
        return;
      }
      expect(h.warnings).toEqual([]);
      const kept = spec.kept!;
      const event = one(span.events!);
      expect(event.name).toBe('gen_ai.evaluation.result');
      const want: Record<string, Record<string, unknown>> = {
        'gen_ai.evaluation.name': { stringValue: kept.name },
      };
      if (kept.value !== undefined) want['gen_ai.evaluation.score.value'] = { doubleValue: kept.value };
      if (kept.label !== undefined) want['gen_ai.evaluation.score.label'] = { stringValue: kept.label };
      if (spec.type === 'boolean') want['telmoni.score.type'] = { stringValue: 'boolean' };
      if (kept.version !== '') want['telmoni.score.version'] = { stringValue: kept.version };
      if (spec.range !== undefined) {
        want['telmoni.score.range_min'] = { doubleValue: Number(spec.range[0]) };
        want['telmoni.score.range_max'] = { doubleValue: Number(spec.range[1]) };
      }
      expect(Object.fromEntries(event.attributes.map((a) => [a.key, a.value]))).toEqual(want);
    });
  }

  it("answers the fixture's live events, each timed within its span to the nanosecond", async () => {
    const h = await scored((trace) => {
      for (const spec of fixture.live.scores) trace.score(spec.name, spec.value, optionsOf(spec));
    });
    const span = agentSpan(h);
    expect(normalized(span)).toEqual(fixture.live.events);
    const times = span.events!.map((event) => BigInt(event.timeUnixNano));
    // Each its own time: two on one millisecond would be one score to the server.
    expect(times[0]! < times[1]! && times[1]! < times[2]!).toBe(true);
    expect(h.warnings).toEqual([]);
  });

  it('goes on a step and on each call, each its own span', async () => {
    const h = await scored((trace) =>
      trace.step('plan', (step) => {
        step.score('a', 1);
        step.modelCall('m', (call) => call.score('b', true));
        step.toolCall('t', (tool) => tool.score('c', 'ok'));
      }),
    );
    const named = Object.fromEntries(exportsIn(h).map((span) => [span.name, span]));
    for (const [name, score] of [
      ['plan', 'a'],
      ['chat m', 'b'],
      ['execute_tool t', 'c'],
    ] as const) {
      expect(attributesOf(one(named[name]!.events!))['gen_ai.evaluation.name']).toBe(score);
    }
  });

  it("is sent with its score alone where content stays home, an app's explanation and a failure staying home", async () => {
    const h = await scored((trace) => {
      otel.getSpan(trace.context)!.addEvent('gen_ai.evaluation.result', {
        'gen_ai.evaluation.name': 'judge',
        'gen_ai.evaluation.score.value': 0.5,
        'gen_ai.evaluation.explanation': 'its reasoning, which is content',
      });
      trace.score('relevance', 0.5);
      trace.fail(new RangeError('a message, which is content'));
    });
    const span = agentSpan(h);
    expect(span.events!.map((event) => event.name)).toEqual(['gen_ai.evaluation.result', 'gen_ai.evaluation.result']);
    expect(span.events!.map((event) => attributesOf(event))).toEqual([
      { 'gen_ai.evaluation.name': 'judge', 'gen_ai.evaluation.score.value': 0.5 },
      { 'gen_ai.evaluation.name': 'relevance', 'gen_ai.evaluation.score.value': 0.5 },
    ]);
    expect(h.transport.text()).not.toContain('content');
  });

  it("is never the mask's, nor a reason to ask the project's mode", async () => {
    const handed: string[] = [];
    const h = await scored((trace) => trace.score('relevance', 0.5, { version: 'judge-2' }), {
      captureContent: true,
      mask: (key: string, value: string) => {
        handed.push(key);
        return value;
      },
    });
    // The export alone: no ask went first.
    expect(h.transport.requests).toHaveLength(1);
    expect(one(agentSpan(h).events!).attributes).toHaveLength(3);
    expect(handed).toEqual([]);
  });

  it('leaves untouched where content is kept and the mask drops everything', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const handed: string[] = [];
    const mask = (key: string): undefined => {
      handed.push(key);
    };
    const h = harness({ captureContent: true, mask }, {}, Array.from({ length: 4 }, () => on));
    try {
      h.telmoni.trace('warm', { inputMessages: 'asks the mode' }, () => {});
      expect(await h.telmoni.flush()).toBe(true);
      expect(h.telmoni.contentMode).toBe('on');
      handed.length = 0;
      h.telmoni.trace('bot', { inputMessages: 'masked away' }, (trace) => {
        for (const spec of fixture.live.scores) trace.score(spec.name, spec.value, optionsOf(spec));
        otel.getSpan(trace.context)!.addEvent('gen_ai.evaluation.result', {
          'gen_ai.evaluation.name': 'judge',
          'gen_ai.evaluation.explanation': 'masked away too',
        });
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const span = exportsIn(h).find((each) => each.name === 'invoke_agent bot')!;
    expect(attributesOf(span)['gen_ai.input.messages']).toBeUndefined();
    const events = normalized(span);
    expect(events.slice(0, 3)).toEqual(fixture.live.events);
    expect(events[3]!.attributes).toEqual([{ key: 'gen_ai.evaluation.name', value: { stringValue: 'judge' } }]);
    // The score's keys never reach the mask; anything else on its event does.
    expect(handed.sort()).toEqual(['gen_ai.evaluation.explanation', 'gen_ai.input.messages']);
  });

  it("asks the project's mode for an evaluation event holding more than its score", async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const h = harness({ captureContent: true }, {}, Array.from({ length: 4 }, () => on));
    try {
      h.telmoni.trace('bot', (trace) => {
        otel.getSpan(trace.context)!.addEvent('gen_ai.evaluation.result', {
          'gen_ai.evaluation.name': 'judge',
          'gen_ai.evaluation.explanation': 'its reasoning',
        });
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toHaveLength(2);
    expect(h.transport.requests[0]!.body).toBe('{"resourceSpans":[]}');
    expect(attributesOf(one(agentSpan(h).events!))['gen_ai.evaluation.explanation']).toBe('its reasoning');
  });

  it('is kept on a span trimmed to fit an export', async () => {
    const h = await scored((trace) => {
      otel.getSpan(trace.context)!.setAttribute('app.blob', 'x'.repeat(contract.BATCH_MAX_BYTES));
      trace.score('relevance', 0.5);
    });
    const span = agentSpan(h);
    expect(attributesOf(span)['app.blob']).toBeUndefined();
    expect(attributesOf(one(span.events!))).toEqual({
      'gen_ai.evaluation.name': 'relevance',
      'gen_ai.evaluation.score.value': 0.5,
    });
    expect(h.fields).toStrictEqual([{ kind: 'trimmed' }]);
  });

  it(`is refused past ${contract.SCORES_PER_SPAN} on one span, with one warning`, async () => {
    const h = await scored((trace) => {
      for (let n = 0; n <= contract.SCORES_PER_SPAN; n++) trace.score(`s${n}`, n);
    });
    expect(agentSpan(h).events).toHaveLength(contract.SCORES_PER_SPAN);
    expect(h.warnings).toEqual([`${NOT_SENT}its span holds 16 scores already`]);
  });

  it('is refused once its span has ended, with one warning', async () => {
    let ended: Trace | undefined;
    const h = await scored((trace) => {
      ended = trace;
    });
    ended!.score('late', 1);
    ended!.score('later', 1);
    expect(h.warnings).toEqual([
      `${NOT_SENT}its span had ended; send it with the client's score, naming the span`,
    ]);
  });

  it('is refused for a value of another type, or a range on another score, never thrown', async () => {
    const h = await scored((trace) => {
      trace.score('a', null as unknown as number);
      trace.score('b', 1n as unknown as number);
      trace.score('c', true, { range: [0, 1] });
      trace.score('d', 'label', { range: [0, 1] });
    });
    expect(agentSpan(h).events).toBeUndefined();
    expect(h.warnings).toEqual([
      `${NOT_SENT}its value is a number, a yes or no, or a category`,
      `${NOT_SENT}a range is a number score's alone`,
    ]);
  });

  it("names its span by the id the span is sent with", async () => {
    let ids: string[] = [];
    const h = await scored((trace) => {
      ids = [trace.spanId, trace.step('plan', (step) => step.spanId)];
    });
    expect(ids).toEqual([agentSpan(h).spanId, exportsIn(h).find((span) => span.name === 'plan')!.spanId]);
    for (const id of ids) expect(id).toMatch(/^[0-9a-f]{16}$/);
  });
});

describe('a late score', () => {
  for (const spec of fixture.targets) {
    it(`answers the fixture: ${spec.case}`, async () => {
      const h = harness();
      try {
        h.telmoni.score('relevance', 0.5, targetOf(spec));
        expect(await h.telmoni.flush()).toBe(true);
      } finally {
        await h.telmoni.shutdown();
      }
      if (spec.refused) {
        expect(h.transport.requests).toHaveLength(0);
        expect(h.warnings).toEqual([
          `${NOT_SENT}it names a trace by 32 hex digits, a trace and one of its spans by 16, or a session of up to 200 characters`,
        ]);
        expect(h.fields).toStrictEqual([{ kind: 'input' }]);
        return;
      }
      const item = one(one(scoresIn(h)));
      expect(h.transport.requests).toHaveLength(1);
      const ids = Object.fromEntries(['trace_id', 'span_id', 'session_id'].filter((k) => k in item).map((k) => [k, item[k]]));
      expect(ids).toEqual(spec.kept);
      expect(h.warnings).toEqual([]);
    });
  }

  it("answers the fixture's late body: one POST, no span sent", async () => {
    const h = harness();
    try {
      for (const spec of fixture.late.scores) {
        h.telmoni.score(spec.name, spec.value, { ...targetOf(spec.target), ...optionsOf(spec) });
      }
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const request = one(h.transport.requests);
    expect(request.url).toBe('https://api.telmoni.com/v1/scores');
    expect(request.headers['Content-Type']).toBe('application/json');
    expect(request.headers['User-Agent']).toMatch(/^telmoni-typescript\//);
    expect(request.headers['Authorization']).toBe('Bearer telmoni_test_key');
    const body = JSON.parse(request.body) as { scores: Array<Record<string, unknown>> };
    for (const item of body.scores) {
      expect(item['id']).toMatch(UUID_V4);
      item['id'] = '<uuid>';
    }
    expect(body).toEqual(fixture.late.sent);
    expect(request.body).not.toContain('null');
    expect(h.warnings).toEqual([]);
  });

  it('takes a fresh id each call, so a retry replaces itself and a second score does not', async () => {
    const h = harness();
    try {
      h.telmoni.score('thumbs', true, { sessionId: 'conv-1' });
      h.telmoni.score('thumbs', true, { sessionId: 'conv-1' });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const [first, second] = one(scoresIn(h));
    expect(first!['id']).not.toBe(second!['id']);
  });

  it('goes beside the spans of the same export, which carry none of it', async () => {
    const h = harness();
    try {
      h.telmoni.trace('bot', (trace) => h.telmoni.score('thumbs', true, { traceId: trace.traceId }));
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests.map((request) => new URL(request.url).pathname)).toEqual(['/v1/scores', '/otlp/v1/traces']);
    expect(exportsIn(h).map((span) => span.name), 'no carrier among them').toEqual(['telmoni.trace.start', 'invoke_agent bot']);
    expect(h.transport.requests[1]!.body).not.toContain('telmoni.score');
  });

  it(`goes in bodies of up to ${contract.SCORES_SENT_MAX}, in order`, async () => {
    const h = harness({}, { processor: { maxBatchSize: 1000 } });
    try {
      for (let n = 0; n < 250; n++) h.telmoni.score(`s${n}`, n, { sessionId: 'conv-1' });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const bodies = scoresIn(h);
    expect(bodies.map((items) => items.length)).toEqual([100, 100, 50]);
    expect(bodies.flat().map((item) => item['name'])).toEqual(Array.from({ length: 250 }, (_, n) => `s${n}`));
  });

  it('is refused by a 400 with one warning, never retried', async () => {
    const h = harness({}, {}, [answer(400, {}, ''), answer(400, {}, '')]);
    try {
      h.telmoni.score('thumbs', true, { sessionId: 'conv-1' });
      expect(await h.telmoni.flush()).toBe(true);
      h.telmoni.score('thumbs', true, { sessionId: 'conv-1' });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toHaveLength(2);
    expect(h.warnings).toEqual(['telmoni: scores refused with 400; they were dropped']);
    expect(h.fields).toStrictEqual([{ kind: 'refused', status: 400 }]);
    expect(h.sleeps).toEqual([]);
  });

  it('is sent again after a 503, the same body, with no warning', async () => {
    const h = harness({}, {}, [answer(503, {}, ''), answer(204, {}, '')]);
    try {
      h.telmoni.score('thumbs', true, { sessionId: 'conv-1' });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toHaveLength(2);
    expect(h.transport.requests[1]!.body).toBe(h.transport.requests[0]!.body);
    expect(h.sleeps).toEqual([1]);
    expect(h.warnings).toEqual([]);
  });

  it('is dropped once its attempts are spent, with one warning, waiting as an export waits', async () => {
    const h = harness({}, {}, [answer(503, { 'Retry-After': '7' }, ''), answer(503, {}, ''), answer(503, {}, '')]);
    try {
      h.telmoni.score('thumbs', true, { sessionId: 'conv-1' });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toHaveLength(3);
    expect(h.sleeps).toEqual([7, 2]);
    expect(h.warnings).toEqual(['telmoni: scores failed after 3 attempts (503); they were dropped']);
    expect(h.fields).toStrictEqual([{ kind: 'failed', status: 503 }]);
  });

  it('is held to the rules a live score is', async () => {
    const h = harness();
    try {
      h.telmoni.score(' ', 1, { sessionId: 'conv-1' });
      h.telmoni.score('relevance', 2, { sessionId: 'conv-1', range: [0, 1] });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toHaveLength(0);
    expect(h.warnings).toEqual([
      `${NOT_SENT}its name is 1 to 64 characters`,
      `${NOT_SENT}a number score is a finite number within its range`,
    ]);
  });
});

describe('a recorded score', () => {
  const start = Date.UTC(2026, 10, 2, 9, 0, 0);

  function record(): TraceRecord {
    return {
      traceId: randomUUID().replaceAll('-', ''),
      agent: 'bot',
      start,
      end: start + 3000,
      scores: [{ name: 'thumbs', value: true }, { name: ' ', value: 1 }],
      inside: [
        {
          type: 'step',
          name: 'plan',
          start: start + 1000,
          end: start + 2000.5,
          scores: [{ name: 'relevance', value: 0.5, range: [0, 1], version: 'judge-2' }],
        },
      ],
    };
  }

  it("is its span's event at the span's end, a refused one left off with its warning", async () => {
    const h = harness();
    const given = record();
    try {
      h.telmoni.recordTrace(given);
      h.telmoni.recordTrace(given);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const [first, again] = h.transport.requests;
    expect(again!.body).toBe(first!.body);
    const spans = (JSON.parse(first!.body) as ExportBody).resourceSpans[0]!.scopeSpans[0]!.spans;
    const agent = spans.find((span) => span.name === 'invoke_agent bot')!;
    const step = spans.find((span) => span.name === 'plan')!;
    expect(one(agent.events!).timeUnixNano).toBe(agent.endTimeUnixNano);
    expect(attributesOf(one(agent.events!))).toEqual({
      'gen_ai.evaluation.name': 'thumbs',
      'gen_ai.evaluation.score.value': 1,
      'telmoni.score.type': 'boolean',
    });
    expect(one(step.events!).timeUnixNano).toBe(step.endTimeUnixNano);
    expect(step.endTimeUnixNano).toBe(`${BigInt(start + 2000) * 1_000_000n + 500_000n}`);
    expect(attributesOf(one(step.events!))).toEqual({
      'gen_ai.evaluation.name': 'relevance',
      'gen_ai.evaluation.score.value': 0.5,
      'telmoni.score.range_min': 0,
      'telmoni.score.range_max': 1,
      'telmoni.score.version': 'judge-2',
    });
    expect(h.warnings).toEqual([`${NOT_SENT}its name is 1 to 64 characters`]);
  });

  it(`is refused past ${contract.SCORES_PER_SPAN} on one span`, async () => {
    const h = harness();
    try {
      h.telmoni.recordTrace({ ...record(), scores: Array.from({ length: 17 }, (_, n) => ({ name: `s${n}`, value: n })) });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(agentSpan(h).events).toHaveLength(contract.SCORES_PER_SPAN);
    expect(h.warnings).toEqual([`${NOT_SENT}its span holds 16 scores already`]);
  });

  it('throws, sending nothing, for scores that are no array', () => {
    const h = harness();
    const given = { ...record(), scores: { name: 'thumbs', value: true } } as unknown as TraceRecord;
    expect(() => h.telmoni.recordTrace(given)).toThrow("a recorded trace's scores must be an array");
  });
});

describe('the limits', () => {
  it("are the fixture's", () => {
    expect({
      name_max: contract.SCORE_NAME_MAX,
      category_max: contract.SCORE_CATEGORY_MAX,
      version_max: contract.SCORE_VERSION_MAX,
      session_max: contract.SESSION_ID_MAX,
      per_span_max: contract.SCORES_PER_SPAN,
      sent_max: contract.SCORES_SENT_MAX,
    }).toEqual(fixture.limits);
  });
});
