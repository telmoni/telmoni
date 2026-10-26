/**
 * The golden fixtures in `sdk/fixtures`: the export every SDK must send for
 * the same run, and what each must do with the server's answers.
 */

import { describe, expect, it } from 'vitest';

import type { Telmoni } from '../src/index';
import {
  type ExportBody,
  type Harness,
  type JsonAttribute,
  answers,
  harness,
  kept,
  loadFixture,
} from './helpers';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const HEX32 = /^[0-9a-f]{32}$/;
const HEX16 = /^[0-9a-f]{16}$/;
const VERSIONED = new Set(['telemetry.sdk.version', 'telemetry.distro.version']);

interface ModelCallSpec {
  provider: string;
  model: string;
  response_model: string;
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_read_input_tokens: number;
    cache_write_input_tokens: number;
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
          { provider: call.provider, inputMessages: call.input_messages },
          async (modelCall) => {
            modelCall.answered({
              responseModel: call.response_model,
              usage: {
                inputTokens: call.usage.input_tokens,
                outputTokens: call.usage.output_tokens,
                cacheReadInputTokens: call.usage.cache_read_input_tokens,
                cacheWriteInputTokens: call.usage.cache_write_input_tokens,
              },
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
    expect(h.warnings).toHaveLength(1);
    expect(h.warnings[0]).toContain('gen_ai.input.messages');
    expect(h.warnings[0]).toContain("content; the project's content mode is off");
    expect(h.warnings[0]).toContain('content mode decides whether content is kept');
    expect(h.warnings[0]).toContain('docs/api/ingest#what-is-dropped-at-the-door');
  });

  it('unlisted names the tag that would keep it', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('unlisted'));
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toHaveLength(1);
    expect(h.warnings[0]).toContain('ai.prompt');
    expect(h.warnings[0]).toContain('telmoni.tag.ai.prompt');
  });

  it('a tag past the cardinality cap is warned with what keeps it', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('cardinality'));
    try {
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toHaveLength(1);
    expect(h.warnings[0]).toContain('telmoni.tag.request');
    expect(h.warnings[0]).toContain('a few keys of a few values each');
  });

  it('refused is warned once and never retried', async () => {
    const h = harness({ apiKey: 'k' }, {}, answers('refused', 2));
    try {
      await aRun(h);
      await aRun(h);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.warnings).toHaveLength(1);
    expect(h.warnings[0]).toContain('a span outside a Telmoni run is refused');
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
    expect(h.warnings).toHaveLength(1);
    expect(h.warnings[0]).toContain('3 attempts');
    expect(h.warnings[0]).toContain('/errors/telemetry/store-unavailable');
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
    expect(h.warnings).toHaveLength(1);
    expect(h.warnings[0]).toContain('403');
    expect(h.warnings[0]).toContain('/errors/authz/forbidden');
    expect(h.transport.requests).toHaveLength(2);
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
  });
});
