/**
 * The golden fixtures in `sdk/fixtures`: the export every SDK must send for
 * the same run, and what each must do with the server's answers.
 */

import { trace } from '@opentelemetry/api';
import { describe, expect, it } from 'vitest';

import { MEDIA_MAX_BYTES } from '../src/contract';
import type { Telmoni } from '../src/index';
import {
  type ExportBody,
  type Harness,
  type JsonAttribute,
  type JsonSpan,
  answer,
  answerNames,
  answers,
  fieldsOf,
  harness,
  kept,
  loadFixture,
  warningOf,
} from './helpers';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const HEX32 = /^[0-9a-f]{32}$/;
const HEX16 = /^[0-9a-f]{16}$/;
const VERSIONED = new Set(['telemetry.sdk.version', 'telemetry.distro.version']);

interface ModelCallSpec {
  provider: string;
  model: string;
  response_model: string;
  request: {
    temperature: number;
    top_p: number;
    top_k: number;
    max_tokens: number;
    frequency_penalty: number;
    presence_penalty: number;
    seed: number;
  };
  time_to_first_token: number;
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_read_input_tokens: number;
    cache_write_input_tokens: number;
    cost: number;
  };
  input_messages: unknown;
  output_messages: unknown;
}

interface Scenario {
  run: {
    agent: string;
    user_id: string;
    session_id: string;
    tags: Record<string, string>;
    sample_rate: number;
    inside: [
      { step: string; inside: [{ model_call: ModelCallSpec }] },
      { tool_call: { name: string; fails_with: string } },
    ];
  };
  capture_content: boolean;
  environment: string;
  release: string;
}

interface RunFixture {
  scenario: Scenario;
  export: ExportBody;
}

// Python sorts by code point; the fixture was written in that order.
function byCodePoint(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

function byKey(a: JsonAttribute, b: JsonAttribute): number {
  return byCodePoint(a.key, b.key);
}

/** The fixture's normalization, as its README sets it out. */
function normalize(body: ExportBody): ExportBody {
  const out = structuredClone(body);
  expect(out.resourceSpans).toHaveLength(1);
  const resourceSpans = out.resourceSpans[0]!;
  expect(resourceSpans.scopeSpans).toHaveLength(1);
  const scopeSpans = resourceSpans.scopeSpans[0]!;
  const spans = [...scopeSpans.spans].sort((a, b) => byCodePoint(a.name, b.name));
  const traceIds = new Set(spans.map((span) => span.traceId));
  expect(traceIds.size).toBe(1);
  expect([...traceIds][0]).toMatch(HEX32);
  const names = new Map<string, string>();
  for (const span of spans) {
    expect(span.spanId).toMatch(HEX16);
    names.set(span.spanId, span.name);
  }
  const runIds = new Set<string>();
  for (const span of spans) {
    span.traceId = '<trace>';
    span.spanId = `<span:${names.get(span.spanId)!}>`;
    if (span.parentSpanId !== undefined) {
      expect(names.has(span.parentSpanId), span.name).toBe(true);
      span.parentSpanId = `<span:${names.get(span.parentSpanId)!}>`;
    }
    const start = BigInt(span.startTimeUnixNano);
    const end = BigInt(span.endTimeUnixNano);
    expect(end >= start).toBe(true);
    span.startTimeUnixNano = '<time>';
    span.endTimeUnixNano = '<time>';
    span.attributes = [...span.attributes].sort(byKey);
    for (const attribute of span.attributes) {
      if (attribute.key === 'telmoni.run_id') {
        runIds.add(String(attribute.value['stringValue']));
        attribute.value['stringValue'] = '<uuid>';
      }
    }
  }
  expect(runIds.size).toBe(1);
  expect([...runIds][0]).toMatch(UUID);
  scopeSpans.spans = spans;
  scopeSpans.scope.version = '<version>';
  const resource = resourceSpans.resource;
  resource.attributes = [...resource.attributes].sort(byKey);
  for (const attribute of resource.attributes) {
    if (attribute.key === 'telemetry.sdk.language') attribute.value['stringValue'] = '<language>';
    else if (VERSIONED.has(attribute.key)) attribute.value['stringValue'] = '<version>';
  }
  return out;
}

async function perform(telmoni: Telmoni, scenario: Scenario): Promise<void> {
  const spec = scenario.run;
  await telmoni.run(
    spec.agent,
    {
      userId: spec.user_id,
      sessionId: spec.session_id,
      tags: spec.tags,
      sampleRate: spec.sample_rate,
    },
    async (run) => {
      const [stepSpec, toolSpec] = spec.inside;
      await run.step(stepSpec.step, async (step) => {
        const call = stepSpec.inside[0].model_call;
        await step.modelCall(
          call.model,
          {
            provider: call.provider,
            temperature: call.request.temperature,
            topP: call.request.top_p,
            topK: call.request.top_k,
            maxTokens: call.request.max_tokens,
            frequencyPenalty: call.request.frequency_penalty,
            presencePenalty: call.request.presence_penalty,
            seed: call.request.seed,
            inputMessages: call.input_messages,
          },
          async (modelCall) => {
            modelCall.answered({
              responseModel: call.response_model,
              usage: {
                inputTokens: call.usage.input_tokens,
                outputTokens: call.usage.output_tokens,
                cacheReadInputTokens: call.usage.cache_read_input_tokens,
                cacheWriteInputTokens: call.usage.cache_write_input_tokens,
                cost: call.usage.cost,
              },
              timeToFirstToken: call.time_to_first_token,
              outputMessages: call.output_messages,
            });
          },
        );
      });
      run.toolCall(toolSpec.tool_call.name, (tool) => {
        tool.fail(toolSpec.tool_call.fails_with);
      });
    },
  );
  expect(await telmoni.flush()).toBe(true);
}

describe('run.json', () => {
  it('the run matches the golden export', async () => {
    const fixture = loadFixture<RunFixture>('run.json');
    const { telmoni, transport } = harness({
      captureContent: fixture.scenario.capture_content,
      environment: fixture.scenario.environment,
      release: fixture.scenario.release,
    });
    try {
      await perform(telmoni, fixture.scenario);
    } finally {
      await telmoni.shutdown();
    }
    const bodies = transport.bodies();
    expect(bodies.length, 'the run was sent').toBeGreaterThanOrEqual(1);
    const gathered = structuredClone(bodies[0]!);
    gathered.resourceSpans[0]!.scopeSpans[0]!.spans = bodies.flatMap(
      (body) => body.resourceSpans[0]!.scopeSpans[0]!.spans,
    );
    expect(normalize(gathered)).toEqual(fixture.export);
    for (const request of transport.requests) {
      expect(request.url).toBe('https://api.telmoni.com/otlp/v1/traces');
      expect(request.headers['Authorization']).toBe('Bearer telmoni_test_key');
      expect(request.headers['Content-Type']).toBe('application/json');
      expect(request.headers['User-Agent']).toMatch(/^telmoni-typescript\/\d/);
      expect(request.headers['Telmoni-Sent-At']).toMatch(/^\d+$/);
    }
  });
});

/** One empty run, sent in an export of its own. */
async function aRun({ telmoni }: Harness): Promise<void> {
  telmoni.run('bot', () => {});
  expect(await telmoni.flush()).toBe(true);
}

interface FailureFixture {
  scenario: {
    agent: string;
    event: { name: string; attributes: Record<string, string | number> };
    error: string;
    message: string;
  };
  cases: Array<{ capture_content: boolean; content_mode: string; sends: 'kept' | 'withheld' }>;
  kept: Pick<JsonSpan, 'status' | 'events'>;
  withheld: Pick<JsonSpan, 'status' | 'events'>;
}

/** The run's span as `failure.json`'s README normalizes it. */
function normalizeFailure(span: JsonSpan): Pick<JsonSpan, 'status' | 'events'> {
  const errorType = span.attributes.find((a) => a.key === 'error.type')?.value['stringValue'];
  const sent: Pick<JsonSpan, 'status' | 'events'> = span.status === undefined ? {} : { status: { ...span.status } };
  if (span.events === undefined) return sent;
  sent.events = structuredClone(span.events).map((event) => {
    const at = BigInt(event.timeUnixNano);
    expect(at >= BigInt(span.startTimeUnixNano) && at <= BigInt(span.endTimeUnixNano)).toBe(true);
    const attributes = [...event.attributes].sort(byKey).filter((a) => {
      if (a.key !== 'exception.stacktrace') return true;
      expect(a.value['stringValue']).toBeTruthy();
      return false;
    });
    for (const a of attributes) {
      if (a.key !== 'exception.type') continue;
      expect(a.value['stringValue']).toBe(errorType);
      a.value['stringValue'] = '<error.type>';
    }
    return { ...event, timeUnixNano: '<time>', attributes };
  });
  return sent;
}

describe('failure.json', () => {
  const fixture = loadFixture<FailureFixture>('failure.json');
  const { scenario } = fixture;
  class RateLimited extends Error {}

  it.each(fixture.cases)('a failure leaves in full only under both locks: %o', async (each) => {
    const mode = answer(200, { 'Telmoni-Content-Mode': each.content_mode }, '{}');
    const h = harness({ captureContent: each.capture_content }, {}, [mode]);
    try {
      h.telmoni.run(scenario.agent, () => {});
      expect(await h.telmoni.flush()).toBe(true);
      expect(h.telmoni.contentMode).toBe(each.content_mode);
      expect(() =>
        h.telmoni.run(scenario.agent, (run) => {
          trace.getSpan(run.context)!.addEvent(scenario.event.name, scenario.event.attributes);
          throw new RateLimited(scenario.message);
        }),
      ).toThrow(RateLimited);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const failed = h.transport.spans().at(-1)!;
    expect(normalizeFailure(failed)).toEqual(fixture[each.sends]);
    if (each.sends === 'kept') {
      expect(JSON.stringify(failed.events!.at(-1))).toContain('exception.stacktrace');
    } else {
      expect(h.transport.text()).not.toContain(scenario.message);
      expect(h.transport.text()).not.toContain(scenario.event.name);
    }
  });
});

interface AskFixture {
  body: unknown;
  agent: string;
  model: string;
  input_messages: unknown;
  cases: Array<{
    capture_content: boolean;
    content: boolean;
    answers: Array<string | number>;
    asked: boolean;
    sent: boolean;
    requests: number;
  }>;
}

describe('ask.json', () => {
  const fixture = loadFixture<AskFixture>('ask.json');
  const spansOf = (body: ExportBody): JsonSpan[] =>
    body.resourceSpans.flatMap((resource) => resource.scopeSpans.flatMap((scope) => scope.spans));

  it.each(fixture.cases)('asks the mode before the first content it would send: %o', async (each) => {
    const answered = each.answers.map((given) =>
      typeof given === 'number' ? answer(given, {}, '') : answer(200, { 'Telmoni-Content-Mode': given }, '{}'),
    );
    const h = harness({ apiKey: 'k', captureContent: each.capture_content }, {}, answered);
    try {
      for (let i = 0; i < 2; i++) {
        h.telmoni.run(fixture.agent, (run) => {
          run.modelCall(fixture.model, each.content ? { inputMessages: fixture.input_messages } : {}, () => {});
        });
        expect(await h.telmoni.flush()).toBe(true);
      }
    } finally {
      await h.telmoni.shutdown();
    }
    const bodies = h.transport.bodies();
    expect(bodies).toHaveLength(each.requests);
    const asks = bodies.filter((body) => spansOf(body).length === 0);
    expect(asks).toEqual(each.asked ? [fixture.body] : []);
    if (each.asked) {
      expect(bodies[0]).toEqual(fixture.body);
      expect(h.transport.requests[0]!.headers['Authorization']).toBe('Bearer k');
    }
    const call = spansOf(bodies.find((body) => spansOf(body).length > 0)!).find((span) => span.name.startsWith('chat '))!;
    expect(call.attributes.some((a) => a.key === 'gen_ai.input.messages')).toBe(each.sent);
  });
});

interface MediaFixture {
  scenario: {
    agent: string;
    provider: string;
    model: string;
    system_instructions: unknown;
    input_messages: unknown;
    output_messages: unknown;
  };
  cases: Array<{ capture_content: boolean; content_mode: string; sends: boolean }>;
  max_bytes: number;
  uploads: Array<{ path: string; content_type: string; body: string }>;
  sent: Record<string, unknown>;
}

describe('media.json', () => {
  const fixture = loadFixture<MediaFixture>('media.json');
  const { scenario } = fixture;

  it("puts a file up to the platform's cap", () => {
    expect(MEDIA_MAX_BYTES).toBe(fixture.max_bytes);
  });

  it.each(fixture.cases)('images are lifted out of messages only under both locks: %o', async (each) => {
    const mode = answer(200, { 'Telmoni-Content-Mode': each.content_mode }, '{}');
    const h = harness({ apiKey: 'k', captureContent: each.capture_content }, {}, [mode, mode, mode]);
    try {
      await aRun(h);
      // Twice, in two exports: each file is put once.
      for (let i = 0; i < 2; i++) {
        h.telmoni.run(scenario.agent, (run) => {
          run.modelCall(
            scenario.model,
            {
              provider: scenario.provider,
              inputMessages: scenario.input_messages,
              systemInstructions: scenario.system_instructions,
            },
            (call) => call.answered({ outputMessages: scenario.output_messages }),
          );
        });
        expect(await h.telmoni.flush()).toBe(true);
      }
    } finally {
      await h.telmoni.shutdown();
    }
    const calls = h.transport.spans().filter((span) => span.name.startsWith('chat '));
    expect(calls).toHaveLength(2);
    if (!each.sends) {
      expect(h.transport.uploads).toEqual([]);
      for (const call of calls) {
        expect(call.attributes.filter((a) => a.key in fixture.sent)).toEqual([]);
      }
      return;
    }
    const uploads = [...h.transport.uploads].sort((a, b) => (a.url < b.url ? -1 : 1));
    expect(
      uploads.map((upload) => ({
        path: new URL(upload.url).pathname,
        content_type: upload.headers['Content-Type'],
        body: Buffer.from(upload.body).toString('base64'),
      })),
    ).toEqual(fixture.uploads);
    expect(uploads.every((upload) => upload.headers['Authorization'] === 'Bearer k')).toBe(true);
    for (const call of calls) {
      const sent = Object.fromEntries(
        call.attributes
          .filter((a) => a.key in fixture.sent)
          .map((a) => [a.key, JSON.parse(a.value['stringValue'] as string) as unknown]),
      );
      expect(sent).toEqual(fixture.sent);
    }
  });
});

describe('answers/', () => {
  it('kept', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('kept'));
    try {
      expect(h.telmoni.contentMode).toBeUndefined();
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual([]);
    expect(h.telmoni.contentMode).toBe('off');
    expect(h.sleeps).toEqual([]);
  });

  it('dropped is warned once', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('dropped', 2));
    try {
      await aRun(h);
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('dropped'));
  });

  it('unlisted names the tag that would keep it', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('unlisted'));
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('unlisted'));
  });

  it('a tag past the cardinality cap is warned with what keeps it', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('cardinality'));
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('cardinality'));
  });

  it('refused is warned once and never retried', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('refused', 2));
    try {
      await aRun(h);
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('refused'));
    expect(h.transport.requests, 'one export each run, neither sent again').toHaveLength(2);
  });

  it('rate limited waits Retry-After and sends the same bytes', async () => {
    const h = harness({ apiKey: 'k' }, {}, [...answers('rate-limited'), kept()]);
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.sleeps).toEqual([2]);
    expect(h.transport.requests[0]!.body).toBe(h.transport.requests[1]!.body);
    expect(h.warnings).toEqual([]);
  });

  it('a Retry-After past the cap waits the cap', async () => {
    const h = harness({ apiKey: 'k' }, {}, [...answers('rate-limited-long'), kept()]);
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.sleeps).toEqual([60]);
    expect(h.transport.requests[0]!.body).toBe(h.transport.requests[1]!.body);
    expect(h.warnings).toEqual([]);
  });

  it('reads Retry-After as text: ASCII digits, a sign allowed, below 0 none, past 60 the cap however long', async () => {
    const limited = (retryAfter: string) => answer(429, { 'Retry-After': retryAfter }, '{}');
    const h = harness({ apiKey: 'k' }, {}, [
      ...[limited('99999999999'), limited('1_0'), kept()],
      ...[limited('-5'), limited(' +7\t'), kept()],
      ...[limited('9'.repeat(400)), kept()],
    ]);
    try {
      for (let n = 0; n < 3; n++) await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.sleeps).toEqual([60, 2, 0, 7, 60]);
    expect(h.transport.requests).toHaveLength(8);
    expect(h.warnings).toEqual([]);
  });

  it('each attempt carries this clock as it leaves, in nanoseconds, a retry its own', async () => {
    const h = harness(
      { apiKey: 'k' },
      { sleep: () => new Promise((resolve) => setTimeout(resolve, 5)) },
      answers('unavailable'),
    );
    const before = BigInt(Date.now()) * 1_000_000n;
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    const after = BigInt(Date.now()) * 1_000_000n;
    const sent = h.transport.requests.map((request) => BigInt(request.headers['Telmoni-Sent-At'] ?? '-1'));
    for (const at of sent) expect(at >= before && at <= after, `${before} ${at} ${after}`).toBe(true);
    expect(h.transport.requests[1]!.body, 'the same export, sent again').toBe(h.transport.requests[0]!.body);
    expect(sent[1]! > sent[0]!).toBe(true);
  });

  it('unavailable is retried three times then dropped', async () => {
    const h = harness({ apiKey: 'k' }, {}, [...answers('unavailable', 3), kept()]);
    try {
      await aRun(h);
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.sleeps).toEqual([1, 1]);
    expect(h.warnings).toEqual(warningOf('unavailable'));
    expect(h.transport.requests, "three for the first run, then the next run's own export").toHaveLength(4);
  });

  it('forbidden is not retried and the next batch is still sent', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('forbidden', 2));
    try {
      await aRun(h);
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('forbidden'));
    expect(h.transport.requests).toHaveLength(2);
  });

  it.each(answerNames())("%s met on every attempt logs its fixture's line and fields word for word, as the plugins read them", async (name) => {
    const h = harness({ apiKey: 'k' }, {}, answers(name, 3));
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf(name));
    expect(h.fields).toStrictEqual(fieldsOf(name));
  });

  it('an export whose wait rejects is dropped in the words the plugins read', async () => {
    const h = harness({ apiKey: 'k' }, { sleep: () => Promise.reject(new Error('broke')) }, answers('rate-limited'));
    try {
      await aRun(h);
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(['telmoni: an export failed (Error); the batch was dropped']);
    expect(h.fields).toStrictEqual([{ kind: 'export_error' }]);
    expect(h.transport.requests, 'the next batch still went').toHaveLength(2);
  });

  it('a network failure is retried with backoff', async () => {
    const h = harness({ apiKey: 'k' }, {}, [new Error('refused'), new Error('refused'), kept()]);
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.sleeps).toEqual([1, 2]);
    expect(h.warnings).toEqual([]);
  });

  it('a network failure three times over names the error and drops the batch', async () => {
    class Unreachable extends Error {
      override readonly name = 'Unreachable';
    }
    const h = harness({ apiKey: 'k' }, {}, [new Unreachable('down'), new Unreachable('down'), new Unreachable('down')]);
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.sleeps).toEqual([1, 2]);
    expect(h.warnings).toEqual(['telmoni: export failed after 3 attempts (Unreachable); the batch was dropped']);
    expect(h.fields, 'a transport error has no status').toStrictEqual([{ kind: 'failed' }]);
  });

  it('a failure ending on a transport error hands no status, as its line names none', async () => {
    const h = harness({ apiKey: 'k' }, {}, [...answers('unavailable', 2), new Error('down')]);
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(['telmoni: export failed after 3 attempts (Error); the batch was dropped']);
    expect(h.fields).toStrictEqual([{ kind: 'failed' }]);
  });
});
