/**
 * The golden fixtures in `sdk/fixtures`: the export every SDK must send for
 * the same trace, and what each must do with the server's answers.
 */

import { trace as otel } from '@opentelemetry/api';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { MEDIA_MAX_BYTES } from '../src/contract';
import type { Telmoni } from '../src/index';
import { scrub, scrubbedOf, scrubber } from '../src/scrub';
import {
  ENDPOINT,
  type ExportBody,
  type Harness,
  type JsonAttribute,
  type JsonSpan,
  answer,
  answerNames,
  answers,
  attributesOf,
  fieldsOf,
  harness,
  kept,
  loadFixture,
  warningOf,
} from './helpers';

const HEX32 = /^[0-9a-f]{32}$/;
const HEX16 = /^[0-9a-f]{16}$/;
const VERSIONED = new Set(['telemetry.sdk.version', 'telemetry.distro.version']);

interface ModelCallSpec {
  provider: string;
  model: string;
  prompt: string;
  prompt_version: string;
  response_model: string;
  time_to_first_token: number;
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_read_input_tokens: number;
    cache_write_input_tokens: number;
    reasoning_output_tokens: number;
    cost: number;
  };
  input_messages: unknown;
  output_messages: unknown;
}

interface Scenario {
  trace: {
    agent: string;
    user_id: string;
    session_id: string;
    tags: Record<string, string>;
    inside: [
      { step: string; inside: [{ model_call: ModelCallSpec }] },
      { tool_call: { name: string; fails_with: string } },
    ];
  };
  capture_content: boolean;
  environment: string;
  release: string;
}

interface TraceFixture {
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
  }
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
  const spec = scenario.trace;
  await telmoni.trace(
    spec.agent,
    {
      userId: spec.user_id,
      sessionId: spec.session_id,
      tags: spec.tags,
    },
    async (trace) => {
      const [stepSpec, toolSpec] = spec.inside;
      await trace.step(stepSpec.step, async (step) => {
        const call = stepSpec.inside[0].model_call;
        await step.modelCall(
          call.model,
          {
            provider: call.provider,
            prompt: call.prompt,
            promptVersion: call.prompt_version,
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
                reasoningOutputTokens: call.usage.reasoning_output_tokens,
                cost: call.usage.cost,
              },
              timeToFirstToken: call.time_to_first_token,
              outputMessages: call.output_messages,
            });
          },
        );
      });
      trace.toolCall(toolSpec.tool_call.name, (tool) => {
        tool.fail(toolSpec.tool_call.fails_with);
      });
    },
  );
  expect(await telmoni.flush()).toBe(true);
}

describe('trace.json', () => {
  it('the trace matches the golden export', async () => {
    const fixture = loadFixture<TraceFixture>('trace.json');
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
    expect(bodies.length, 'the trace was sent').toBeGreaterThanOrEqual(1);
    const gathered = structuredClone(bodies[0]!);
    gathered.resourceSpans[0]!.scopeSpans[0]!.spans = bodies.flatMap(
      (body) => body.resourceSpans[0]!.scopeSpans[0]!.spans,
    );
    expect(normalize(gathered)).toEqual(fixture.export);
    for (const request of transport.requests) {
      expect(request.url).toBe(`${ENDPOINT}/otlp/v1/traces`);
      expect(request.headers['Authorization']).toBe('Bearer telmoni_test_key');
      expect(request.headers['Content-Type']).toBe('application/json');
      expect(request.headers['User-Agent']).toMatch(/^telmoni-typescript\/\d/);
      expect(request.headers['Telmoni-Sent-At']).toMatch(/^\d+$/);
    }
  });
});

interface RetrievalSpec {
  source: string;
  query: string;
  top_k: number;
  memory?: boolean;
  documents?: unknown[];
  fails_with?: string;
}

interface RetrievalFixture {
  scenario: {
    trace: { agent: string; user_id: string; session_id: string; inside: Array<{ retrieval: RetrievalSpec }> };
  };
  cases: Array<{ capture_content: boolean; content_mode: string; sends: 'metadata' | 'content' }>;
  metadata: ExportBody;
  content: ExportBody;
}

describe('retrieval.json', () => {
  const fixture = loadFixture<RetrievalFixture>('retrieval.json');
  const spec = fixture.scenario.trace;

  it.each(fixture.cases)('the retrievals match the golden export: %o', async (each) => {
    const mode = (): ReturnType<typeof answer> => answer(200, { 'Telmoni-Content-Mode': each.content_mode }, '{}');
    const { telmoni, transport } = harness({ captureContent: each.capture_content }, {}, [mode(), mode(), mode(), mode()]);
    try {
      telmoni.trace(spec.agent, { userId: spec.user_id, sessionId: spec.session_id }, (trace) => {
        for (const { retrieval } of spec.inside) {
          const options = { query: retrieval.query, topK: retrieval.top_k, memory: retrieval.memory ?? false };
          trace.retrieval(retrieval.source, options, (found) => {
            if (retrieval.documents !== undefined) found.returned(retrieval.documents);
            else found.fail(retrieval.fails_with);
          });
        }
      });
      expect(await telmoni.flush()).toBe(true);
    } finally {
      await telmoni.shutdown();
    }
    // The ask before the first content holds no spans.
    const bodies = transport.bodies().filter((body) => body.resourceSpans.length > 0);
    const gathered = structuredClone(bodies[0]!);
    gathered.resourceSpans[0]!.scopeSpans[0]!.spans = bodies.flatMap(
      (body) => body.resourceSpans[0]!.scopeSpans[0]!.spans,
    );
    expect(normalize(gathered)).toEqual(fixture[each.sends]);
  });
});

interface TraceStartFixture {
  scenario: {
    trace: { agent: string; user_id: string; session_id: string; tags: Record<string, string> };
    environment: string;
    release: string;
    schedule_delay_ms: number;
    sent_within_ms: number;
  };
  first_export: ExportBody;
}

describe('trace-start.json', () => {
  it('a trace is sent from its start', async () => {
    const fixture = loadFixture<TraceStartFixture>('trace-start.json');
    const { scenario } = fixture;
    const spec = scenario.trace;
    const { telmoni, transport } = harness(
      { environment: scenario.environment, release: scenario.release },
      { processor: { scheduleDelaySeconds: scenario.schedule_delay_ms / 1000 } },
    );
    let start: JsonSpan | undefined;
    let parent: string | undefined;
    try {
      await telmoni.trace(spec.agent, { userId: spec.user_id, sessionId: spec.session_id, tags: spec.tags }, async () => {
        expect(await transport.waitFor(1, scenario.sent_within_ms)).toBe(true);
        const first = structuredClone(transport.bodies());
        expect(first, 'one body, before the trace ends').toHaveLength(1);
        const spans = first[0]!.resourceSpans[0]!.scopeSpans[0]!.spans;
        expect(spans).toHaveLength(1);
        start = structuredClone(spans[0]!);
        expect(start.startTimeUnixNano).toBe(start.endTimeUnixNano);
        parent = spans[0]!.parentSpanId;
        delete spans[0]!.parentSpanId;
        const normalized = normalize(first[0]!);
        normalized.resourceSpans[0]!.scopeSpans[0]!.spans[0]!.parentSpanId = '<span:agent>';
        expect(normalized).toEqual(fixture.first_export);
      });
      expect(await telmoni.flush()).toBe(true);
    } finally {
      await telmoni.shutdown();
    }
    const agents = transport.spans().filter((span) => span.name === `invoke_agent ${spec.agent}`);
    expect(agents).toHaveLength(1);
    const agent = agents[0]!;
    expect(parent).toBeDefined();
    expect(agent.spanId).toBe(parent);
    expect(agent.traceId).toBe(start!.traceId);
    expect(agent.startTimeUnixNano).toBe(start!.startTimeUnixNano);
  });
});

/** One empty trace, sent in an export of its own. */
async function aTrace({ telmoni }: Harness): Promise<void> {
  telmoni.trace('bot', () => {});
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

/** The agent's span as `failure.json`'s README normalizes it. */
function normalizeFailure(span: JsonSpan): Pick<JsonSpan, 'status' | 'events'> {
  const errorType = span.attributes.find((a) => a.key === 'error.type')?.value['stringValue'];
  expect(errorType, 'the span names its error.type, withheld or kept').toBeTruthy();
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
      h.telmoni.trace(scenario.agent, () => {});
      expect(await h.telmoni.flush()).toBe(true);
      expect(h.telmoni.contentMode).toBe(each.content_mode);
      expect(() =>
        h.telmoni.trace(scenario.agent, (trace) => {
          otel.getSpan(trace.context)!.addEvent(scenario.event.name, scenario.event.attributes);
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
        h.telmoni.trace(fixture.agent, (trace) => {
          trace.modelCall(fixture.model, each.content ? { inputMessages: fixture.input_messages } : {}, () => {});
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
      await aTrace(h);
      // Twice, in two exports: each file is put once.
      for (let i = 0; i < 2; i++) {
        h.telmoni.trace(scenario.agent, (trace) => {
          trace.modelCall(
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

interface MaskFixture {
  scenario: {
    agent: string;
    user: string;
    session: string;
    tags: Record<string, string>;
    provider: string;
    model: string;
    system_instructions: unknown;
    input_messages: unknown;
    output_messages: unknown;
    event: { name: string; attributes: Record<string, string | number> };
    message: string;
  };
  mask: Record<string, { replace?: string; drop?: boolean; fail?: boolean }>;
  cases: Array<{ capture_content: boolean; content_mode: string; masks: boolean }>;
  handed: string[];
  handed_text: Record<string, string>;
  sent: Record<string, unknown>;
  kept: Pick<JsonSpan, 'status' | 'events'>;
  withheld: Pick<JsonSpan, 'status' | 'events'>;
  warnings: string[];
}

describe('mask.json', () => {
  const fixture = loadFixture<MaskFixture>('mask.json');
  const { scenario } = fixture;
  class Declined extends Error {}
  const MESSAGES = ['gen_ai.input.messages', 'gen_ai.output.messages', 'gen_ai.system_instructions'];

  it.each(fixture.cases)('the mask answers each content field only under both locks: %o', async (each) => {
    const handed: Record<string, string> = {};
    const mask = (key: string, value: string): string | undefined => {
      handed[key] = value;
      const rule = fixture.mask[key];
      if (rule === undefined) return value;
      if (rule.fail) throw new Error(`the mask failed on ${value}`);
      if (rule.drop) return undefined;
      return rule.replace;
    };
    const mode = answer(200, { 'Telmoni-Content-Mode': each.content_mode }, '{}');
    const h = harness({ apiKey: 'k', captureContent: each.capture_content, mask }, {}, [mode, mode]);
    try {
      await aTrace(h);
      expect(() =>
        h.telmoni.trace(
          scenario.agent,
          { userId: scenario.user, sessionId: scenario.session, tags: scenario.tags },
          (trace) => {
            trace.modelCall(
              scenario.model,
              {
                provider: scenario.provider,
                inputMessages: scenario.input_messages,
                systemInstructions: scenario.system_instructions,
              },
              (call) => call.answered({ outputMessages: scenario.output_messages }),
            );
            otel.getSpan(trace.context)!.addEvent(scenario.event.name, scenario.event.attributes);
            throw new Declined(scenario.message);
          },
        ),
      ).toThrow(Declined);
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const spans = h.transport.spans();
    const calls = spans.filter((span) => span.name.startsWith('chat '));
    expect(calls).toHaveLength(1);
    expect(h.transport.uploads).toEqual([]);
    expect(h.transport.text()).not.toContain('4242 ');
    expect(h.warnings.join('\n')).not.toContain('4242 ');
    const sent = Object.fromEntries(
      calls[0]!.attributes
        .filter((a) => MESSAGES.includes(a.key))
        .map((a) => [a.key, JSON.parse(a.value['stringValue'] as string) as unknown]),
    );
    if (!each.masks) {
      expect(handed).toEqual({});
      expect(sent).toEqual({});
      expect(normalizeFailure(spans.at(-1)!)).toEqual(fixture.withheld);
      expect(h.warnings).toEqual([]);
      return;
    }
    expect(sent).toEqual(fixture.sent);
    expect(normalizeFailure(spans.at(-1)!)).toEqual(fixture.kept);
    expect(h.warnings).toEqual(fixture.warnings);
    expect(h.fields.map((field) => field.kind)).toEqual(['mask_failed']);
    expect(Object.keys(handed)).toContain('exception.stacktrace');
    expect(Object.keys(handed).filter((key) => key !== 'exception.stacktrace').sort()).toEqual(fixture.handed);
    for (const [key, value] of Object.entries(fixture.handed_text)) expect(handed[key]).toBe(value);
    // As the code gave them, blob parts and all: before any was put.
    expect(JSON.parse(handed['gen_ai.input.messages']!)).toEqual(scenario.input_messages);
    expect(JSON.parse(handed['gen_ai.output.messages']!)).toEqual(scenario.output_messages);
    expect(JSON.parse(handed['gen_ai.system_instructions']!)).toEqual(scenario.system_instructions);
  });

  it('a mask that throws is warned of once and asked each export', async () => {
    const asked: string[] = [];
    const mask = (key: string, value: string): string => {
      asked.push(key);
      throw new Error(value);
    };
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const h = harness({ apiKey: 'k', captureContent: true, mask }, {}, [on, on, on]);
    try {
      await aTrace(h);
      for (let i = 0; i < 2; i++) {
        h.telmoni.trace('support-bot', (trace) => {
          trace.modelCall('gpt-4o', { inputMessages: 'a secret' }, () => {});
        });
        expect(await h.telmoni.flush()).toBe(true);
      }
    } finally {
      await h.telmoni.shutdown();
    }
    expect(asked).toEqual(['gen_ai.input.messages', 'gen_ai.input.messages']);
    expect(h.warnings).toEqual(['telmoni: the mask failed on gen_ai.input.messages; the span was sent without it']);
    expect(h.transport.text()).not.toContain('a secret');
  });

  it('a mask answering no string sends the span without the field', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const mask = (() => Promise.resolve('masked')) as unknown as (key: string, value: string) => string;
    const h = harness({ apiKey: 'k', captureContent: true, mask }, {}, [on, on]);
    try {
      await aTrace(h);
      h.telmoni.trace('support-bot', (trace) => {
        trace.modelCall('gpt-4o', { inputMessages: 'a secret' }, () => {});
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.text()).not.toContain('a secret');
    expect(h.fields.map((field) => field.kind)).toEqual(['mask_failed']);
  });

  it('a mask answering a value unchanged keeps its type', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const mask = (_key: string, value: string): string => (value === 'true' ? 'yes' : value);
    const h = harness({ apiKey: 'k', captureContent: true, mask }, {}, [on, on]);
    try {
      await aTrace(h);
      h.telmoni.trace('support-bot', (trace) => {
        otel.getSpan(trace.context)!.addEvent('checked', { count: 3, found: true });
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const [event] = h.transport.spans().at(-1)!.events!;
    expect([...event!.attributes].sort(byKey)).toEqual([
      { key: 'count', value: { intValue: '3' } },
      { key: 'found', value: { stringValue: 'yes' } },
    ]);
  });

  it('a mask that is no function is refused', () => {
    expect(() => harness({ mask: 'redact' as unknown as () => string })).toThrow('mask must be a function');
  });
});

interface CanaryFixture {
  planted: Record<string, string>;
  modes: Record<string, { scrub_secrets: boolean; scrub_pii: boolean }>;
  cases: Array<{ name: string; text: string; expect: Record<string, { text: string; scrubbed: Record<string, number> }> }>;
}

describe('canary.json', () => {
  const fixture = loadFixture<CanaryFixture>('canary.json');
  const replays = fixture.cases.flatMap((each) =>
    Object.entries(fixture.modes).map(([mode, switches]) => ({ ...each, mode, switches, expected: each.expect[mode]! })),
  );
  const whole = fixture.cases.find((each) => Object.values(fixture.planted).every((value) => each.text.includes(value)))!;
  const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
  const message = (content: string) => [{ role: 'user', parts: [{ type: 'text', content }] }];
  // Compared as JSON, never as bytes: each SDK's serializer writes a `dropped:<field>` key its own way.
  const scrubbedIn = (sent: Record<string, unknown>): unknown =>
    sent['telmoni.scrubbed'] === undefined ? undefined : JSON.parse(sent['telmoni.scrubbed'] as string);

  // The shell running the suite may set either variable; the switches' defaults are under test.
  beforeEach(() => {
    vi.stubEnv('TELMONI_SCRUB_SECRETS', undefined);
    vi.stubEnv('TELMONI_SCRUB_PII', undefined);
  });
  afterEach(() => vi.unstubAllEnvs());

  it.each(replays)('$mode scrubs "$name" as the fixture says', ({ text, switches, expected }) => {
    const [scrubbed, counts] = scrub(text, switches.scrub_secrets, switches.scrub_pii);
    expect(scrubbed).toBe(expected.text);
    const carried = JSON.parse(scrubbedOf(counts)) as Record<string, number>;
    expect(carried).toEqual(expected.scrubbed);
    expect(Object.keys(carried)).toEqual(Object.keys(carried).sort());
  });

  it("counts a keyword's value in characters, an astral one once", () => {
    // 7 characters in 8 UTF-16 units is no password, and 15 in 19 no token; one more makes each.
    const smile = '\u{1F600}';
    expect(scrub(`password=${smile}abcdef`, true, false)[0]).toBe(`password=${smile}abcdef`);
    expect(scrub(`password=${smile}abcdefg`, true, false)[0]).toBe('password=[secret: password]');
    expect(scrub(`token=${smile.repeat(4)}a1b2c3d4e5f`, true, false)[0]).toBe(`token=${smile.repeat(4)}a1b2c3d4e5f`);
    expect(scrub(`token=${smile.repeat(4)}a1b2c3d4e5fg`, true, false)[0]).toBe('token=[secret: token]');
  });

  it.each(Object.entries(fixture.modes))('a client keeping content sends a message as %s scrubs it, counted', async (mode, switches) => {
    const expected = whole.expect[mode]!;
    // `sdk_default` is a client built with neither switch set.
    const set = mode === 'sdk_default' ? {} : { scrubSecrets: switches.scrub_secrets, scrubPii: switches.scrub_pii };
    const h = harness({ apiKey: 'k', captureContent: true, ...set }, {}, [on, on]);
    try {
      await aTrace(h);
      h.telmoni.trace('support-bot', (trace) => {
        trace.modelCall('gpt-4o', { inputMessages: message(whole.text) }, () => {});
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const sent = attributesOf(h.transport.spans().find((span) => span.name === 'chat gpt-4o')!);
    expect(JSON.parse(sent['gen_ai.input.messages'] as string)).toEqual(message(expected.text));
    expect(scrubbedIn(sent)).toEqual(Object.keys(expected.scrubbed).length > 0 ? expected.scrubbed : undefined);
    for (const value of Object.values(fixture.planted)) {
      if (!expected.text.includes(value)) expect(h.transport.text()).not.toContain(value);
    }
  });

  it("scrubs a recorded trace's content as a live trace's", async () => {
    const start = Date.parse('2026-11-05T10:00:00.000Z');
    const h = harness({ apiKey: 'k', captureContent: true }, {}, [on, on]);
    try {
      await aTrace(h);
      h.telmoni.recordTrace({
        traceId: '5f0c9a7e3b1d4c2a9e8f1a2b3c4d5e6f',
        agent: 'support-bot',
        start,
        end: start + 1000,
        inside: [
          {
            type: 'tool_call',
            name: 'query',
            start,
            end: start + 500,
            arguments: { user: 'app', password: 'Hunter2-canary!' },
            result: fixture.planted.connection_string,
          },
        ],
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const sent = attributesOf(h.transport.spans().find((span) => span.name === 'execute_tool query')!);
    expect(sent).toMatchObject({
      'gen_ai.tool.call.arguments': '{"user":"app","password":"[secret: password]"}',
      'gen_ai.tool.call.result': '[secret: connection_string]',
    });
    expect(scrubbedIn(sent)).toEqual({ 'secret:connection_string': 1, 'secret:password': 1 });
  });

  it("scrubs a message after its images are lifted out, so an image's base64 is never read", async () => {
    // Base64 that holds `/home/`, which read as a home folder would break the image.
    const image = 'AA/home/AAAA';
    const h = harness({ apiKey: 'k', captureContent: true, scrubPii: true }, {}, [on, on]);
    const blob = [{ role: 'user', parts: [{ type: 'blob', modality: 'image', mime_type: 'image/png', content: image }] }];
    try {
      await aTrace(h);
      h.telmoni.trace('support-bot', (trace) => {
        trace.modelCall('gpt-4o', { inputMessages: blob }, () => {});
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.uploads.map((upload) => Buffer.from(upload.body).toString('base64'))).toEqual([image]);
    const sent = attributesOf(h.transport.spans().find((span) => span.name === 'chat gpt-4o')!);
    expect(JSON.parse(sent['gen_ai.input.messages'] as string)).toEqual([
      {
        role: 'user',
        parts: [{ type: 'file', modality: 'image', mime_type: 'image/png', file_id: expect.stringMatching(/^telmoni:[0-9a-f]{64}$/) }],
      },
    ]);
    expect(sent).not.toHaveProperty('telmoni.scrubbed');
  });

  it('a scrubber that fails on a field sends the span without it, counted by its key, the rest scrubbed', async () => {
    const real = scrubber.scrub;
    const failing = vi.spyOn(scrubber, 'scrub').mockImplementation((text, secrets, pii) => {
      if (text.startsWith('fails')) throw new Error(text);
      return real(text, secrets, pii);
    });
    const key = fixture.planted.cloud_key!;
    const h = harness({ apiKey: 'k', captureContent: true }, {}, [on, on]);
    try {
      await aTrace(h);
      h.telmoni.trace('support-bot', (trace) => {
        trace.modelCall('gpt-4o', { inputMessages: `fails on ${key}` }, (call) => call.answered({ outputMessages: `key ${key}` }));
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      failing.mockRestore();
      await h.telmoni.shutdown();
    }
    const sent = attributesOf(h.transport.spans().find((span) => span.name === 'chat gpt-4o')!);
    expect(sent).not.toHaveProperty('gen_ai.input.messages');
    expect(sent['gen_ai.output.messages']).toBe('key [secret: aws_key]');
    expect(scrubbedIn(sent)).toEqual({ 'dropped:gen_ai.input.messages': 1, 'secret:aws_key': 1 });
    expect(h.transport.text()).not.toContain(key);
    expect(h.warnings.join('\n')).not.toContain(key);
  });

  // Each repeated to 256 KiB: shapes a rule could read again from every start, which would take minutes, not milliseconds.
  const hostile = [
    ['eyJ-', ''],
    ['eyJ', ''],
    ['token=', ''],
    ['password=', ''],
    ['atm_', ''],
    ['xsk-ant-', ''],
    ['xoxb-', '_'],
    ['a', ''],
    ['a@', ''],
    ['a@b.', ''],
    ['x://', ''],
    ['a:b', ' '],
    ['1', ''],
    ['1 ', ''],
    ['1.', ''],
    ['a:', ''],
    ['/home/', ''],
    ['AB12', ''],
    ['-----BEGIN PRIVATE KEY-----', ''],
    ['+1 ', ''],
  ].flatMap(([unit, tail]) => [false, true].map((pii) => ({ unit: unit!, tail: tail!, pii })));

  it.each(hostile)('scrubs "$unit" repeated to 256 KiB in under 2 s, PII $pii', ({ unit, tail, pii }) => {
    const text = unit.repeat(Math.ceil(262_144 / unit.length)) + tail;
    const started = performance.now();
    scrub(text, true, pii);
    expect(performance.now() - started).toBeLessThan(2000);
  });
});

describe('answers/', () => {
  it('kept', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('kept'));
    try {
      expect(h.telmoni.contentMode).toBeUndefined();
      await aTrace(h);
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
      await aTrace(h);
      await aTrace(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('dropped'));
  });

  it('unlisted names the tag that would keep it', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('unlisted'));
    try {
      await aTrace(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('unlisted'));
  });

  it('a tag past the cardinality cap is warned with what keeps it', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('cardinality'));
    try {
      await aTrace(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('cardinality'));
  });

  it('refused is warned once and never retried', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('refused', 2));
    try {
      await aTrace(h);
      await aTrace(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('refused'));
    expect(h.transport.requests, 'one export each trace, neither sent again').toHaveLength(2);
  });

  it('rate limited waits Retry-After and sends the same bytes', async () => {
    const h = harness({ apiKey: 'k' }, {}, [...answers('rate-limited'), kept()]);
    try {
      await aTrace(h);
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
      await aTrace(h);
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
      for (let n = 0; n < 3; n++) await aTrace(h);
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
      await aTrace(h);
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
      await aTrace(h);
      await aTrace(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.sleeps).toEqual([1, 1]);
    expect(h.warnings).toEqual(warningOf('unavailable'));
    expect(h.transport.requests, "three for the first trace, then the next trace's own export").toHaveLength(4);
  });

  it('forbidden is not retried and the next batch is still sent', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('forbidden', 2));
    try {
      await aTrace(h);
      await aTrace(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf('forbidden'));
    expect(h.transport.requests).toHaveLength(2);
  });

  it.each(answerNames())("%s met on every attempt logs its fixture's line and fields word for word, as the plugins read them", async (name) => {
    const h = harness({ apiKey: 'k' }, {}, answers(name, 3));
    try {
      await aTrace(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(warningOf(name));
    expect(h.fields).toStrictEqual(fieldsOf(name));
  });

  it('an export whose wait rejects is dropped in the words the plugins read', async () => {
    const h = harness({ apiKey: 'k' }, { sleep: () => Promise.reject(new Error('broke')) }, answers('rate-limited'));
    try {
      await aTrace(h);
      await aTrace(h);
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
      await aTrace(h);
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
      await aTrace(h);
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
      await aTrace(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toEqual(['telmoni: export failed after 3 attempts (Error); the batch was dropped']);
    expect(h.fields).toStrictEqual([{ kind: 'failed' }]);
  });
});
