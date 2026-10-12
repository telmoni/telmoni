import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { Run, currentRun } from '../src/index';
import { type Harness, attributesOf, harness, one, runsIn } from './helpers';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$/;

let h: Harness;

beforeEach(() => {
  h = harness();
});

afterEach(async () => {
  vi.restoreAllMocks();
  await h.telmoni.shutdown();
});

describe('a run', () => {
  it('is a span with the contract attributes', async () => {
    const run = h.telmoni.run(
      'support-bot',
      {
        userId: 'user_1',
        sessionId: 'conv_1',
        tags: { env: 'test', team: 'a' },
        bounds: { stallAfterSeconds: 30, maxSteps: 5, maxCostUsd: 0.5 },
      },
      (run) => {
        expect(run).toBeInstanceOf(Run);
        expect(run.runId).toMatch(UUID);
        expect(run.agent).toBe('support-bot');
        return run;
      },
    );
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(runsIn(h.transport));
    expect(span.kind).toBe(1);
    expect(span.parentSpanId).toBeUndefined();
    expect(span.status).toBeUndefined();
    expect(attributesOf(span)).toEqual({
      'gen_ai.operation.name': 'invoke_agent',
      'gen_ai.agent.name': 'support-bot',
      'telmoni.run_id': run.runId,
      'telmoni.agent': 'support-bot',
      'user.id': 'user_1',
      'gen_ai.conversation.id': 'conv_1',
      'telmoni.bound.stall_after_s': 30,
      'telmoni.bound.max_steps': 5,
      'telmoni.bound.max_cost_usd': 0.5,
      'telmoni.sample_rate': 1,
      'telmoni.tag.env': 'test',
      'telmoni.tag.team': 'a',
    });
  });

  it('sends its start at once, with the bounds', async () => {
    await h.telmoni.run('bot', { bounds: { maxSteps: 3 } }, async (run) => {
      expect(await h.transport.waitFor(1)).toBe(true);
      const spans = h.transport.spans();
      expect(spans.map((span) => span.name)).toEqual(['telmoni.run.start']);
      const start = spans[0]!;
      expect(start.startTimeUnixNano).toBe(start.endTimeUnixNano);
      expect(attributesOf(start)).toEqual({
        'telmoni.run_id': run.runId,
        'telmoni.agent': 'bot',
        'telmoni.bound.max_steps': 3,
        'telmoni.sample_rate': 1,
      });
    });
    expect(await h.telmoni.flush()).toBe(true);
    expect(one(runsIn(h.transport)).spanId).toBe(h.transport.spans()[0]!.parentSpanId);
  });

  it('types the bounds and the rate as the contract does, whatever the number', async () => {
    h.telmoni.run('bot', { bounds: { stallAfterSeconds: 30.9, maxSteps: 5, maxCostUsd: 2 }, sampleRate: 1 }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const typed = Object.fromEntries(one(runsIn(h.transport)).attributes.map((a) => [a.key, a.value]));
    expect(typed['telmoni.bound.stall_after_s']).toEqual({ intValue: '30' });
    expect(typed['telmoni.bound.max_steps']).toEqual({ intValue: '5' });
    expect(typed['telmoni.bound.max_cost_usd']).toEqual({ doubleValue: 2 });
    expect(typed['telmoni.sample_rate']).toEqual({ doubleValue: 1 });
  });

  it('fails with the class of an error rejected inside it, and rethrows it', async () => {
    class ProviderDown extends Error {}
    await expect(
      h.telmoni.run('bot', async () => {
        throw new ProviderDown('the message, which is content and never sent');
      }),
    ).rejects.toBeInstanceOf(ProviderDown);
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(runsIn(h.transport));
    expect(span.status).toEqual({ code: 2 });
    expect(attributesOf(span)['error.type']).toBe('ProviderDown');
    expect(h.transport.text()).not.toContain('never sent');
  });

  it('fails with the class of an error thrown inside it, and rethrows it', async () => {
    expect(() =>
      h.telmoni.run('bot', () => {
        throw new RangeError('out of range');
      }),
    ).toThrow(RangeError);
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(runsIn(h.transport));
    expect(span.status).toEqual({ code: 2 });
    expect(attributesOf(span)['error.type']).toBe('RangeError');
    expect(h.transport.text()).not.toContain('out of range');
  });

  it('is marked by fail and cancel', async () => {
    h.telmoni.run('bot', (run) => run.fail('RateLimit'));
    h.telmoni.run('bot', (run) => run.cancel());
    expect(await h.telmoni.flush()).toBe(true);
    const runs = runsIn(h.transport);
    const failed = runs.find((span) => 'error.type' in attributesOf(span))!;
    const cancelled = runs.find((span) => !('error.type' in attributesOf(span)))!;
    expect(attributesOf(failed)['error.type']).toBe('RateLimit');
    expect(failed.status).toEqual({ code: 2 });
    expect(attributesOf(cancelled)['telmoni.cancelled']).toBe(true);
    expect(cancelled.status).toBeUndefined();
  });

  it('is cancelled by an AbortError, not failed', async () => {
    const aborted = new DOMException('stopped', 'AbortError');
    await expect(
      h.telmoni.run('bot', async () => {
        throw aborted;
      }),
    ).rejects.toBe(aborted);
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(runsIn(h.transport));
    expect(attributesOf(span)['telmoni.cancelled']).toBe(true);
    expect(span.status).toBeUndefined();
    expect(attributesOf(span)['error.type']).toBeUndefined();
  });

  it('returns what its callback returns, sync or async', async () => {
    expect(h.telmoni.run('bot', () => 42)).toBe(42);
    expect(await h.telmoni.run('bot', async () => 'later')).toBe('later');
  });

  it('needs an agent and a valid rate', () => {
    expect(() => h.telmoni.run(' ', () => {})).toThrow(TypeError);
    expect(() => h.telmoni.run('bot', { sampleRate: 0 }, () => {})).toThrow(RangeError);
    expect(() => h.telmoni.run('bot', { sampleRate: 1.5 }, () => {})).toThrow(RangeError);
  });

  it('is found from its context; without a context manager, from nothing else', () => {
    h.telmoni.run('bot', (run) => {
      expect(currentRun(run.context)).toBe(run);
      // No context manager is registered in this suite: the active context is the root.
      expect(currentRun()).toBeUndefined();
      run.step('s', (step) => {
        expect(currentRun(step.context)).toBe(run);
      });
    });
  });
});

describe('the wrapper', () => {
  it('opens a run per call, the run its last argument, and keeps the name', async () => {
    const seen: string[] = [];
    function handle(question: string, run: Run): string {
      seen.push(run.runId);
      return question.toUpperCase();
    }
    const wrapped = h.telmoni.wrap('bot', handle);
    expect(wrapped('a')).toBe('A');
    expect(wrapped('b')).toBe('B');
    expect(wrapped.name).toBe('handle');
    expect(await h.telmoni.flush()).toBe(true);
    expect(runsIn(h.transport)).toHaveLength(2);
    expect(new Set(seen).size).toBe(2);
  });

  it('works with async callees and the options', async () => {
    const think = h.telmoni.wrap('bot', { sessionId: 'conv' }, async (depth: number, run: Run) => {
      await run.step('think', async () => {
        await Promise.resolve();
      });
      return `${run.runId}:${depth}`;
    });
    const first = await think(1);
    const second = await h.telmoni.run('bot', async (run) => run.runId);
    expect(await h.telmoni.flush()).toBe(true);
    const spans = h.transport.spans();
    expect(spans.filter((span) => span.name === 'invoke_agent bot')).toHaveLength(2);
    const step = spans.find((span) => span.name === 'think')!;
    expect(attributesOf(step)['telmoni.run_id']).toBe(first.split(':')[0]);
    expect(attributesOf(step)['gen_ai.conversation.id']).toBe('conv');
    expect(first.split(':')[0]).not.toBe(second);
  });
});

describe('outside a run', () => {
  it('nothing can be opened, so nothing is sent', async () => {
    // Steps and calls open on a run's handle, or a step's: the client has no
    // such method, and an idle client sends nothing.
    expect('step' in h.telmoni).toBe(false);
    expect('modelCall' in h.telmoni).toBe(false);
    expect('toolCall' in h.telmoni).toBe(false);
    expect(await h.telmoni.flush()).toBe(true);
    await h.telmoni.shutdown();
    expect(h.transport.requests).toEqual([]);
    expect(h.warnings).toEqual([]);
  });
});

describe('the sample rate', () => {
  it('drops whole runs in the processor', async () => {
    const sampled = harness({ sampleRate: 0.25 });
    try {
      vi.spyOn(Math, 'random').mockReturnValue(0.9);
      sampled.telmoni.run('bot', (run) => run.step('s', () => {}));
      vi.spyOn(Math, 'random').mockReturnValue(0.1);
      const kept = sampled.telmoni.run('bot', (run) => run);
      expect(await sampled.telmoni.flush()).toBe(true);
      const spans = sampled.transport.spans();
      expect(new Set(spans.map((span) => attributesOf(span)['telmoni.run_id']))).toEqual(new Set([kept.runId]));
      expect(new Set(spans.map((span) => span.name))).toEqual(new Set(['telmoni.run.start', 'invoke_agent bot']));
      expect(attributesOf(spans[0]!)['telmoni.sample_rate']).toBe(0.25);
    } finally {
      await sampled.telmoni.shutdown();
    }
  });

  it("is the run's own over the client's", async () => {
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    h.telmoni.run('bot', { sampleRate: 0.4 }, () => {});
    h.telmoni.run('bot', { sampleRate: 0.6 }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const runs = runsIn(h.transport);
    expect(runs).toHaveLength(1);
    expect(attributesOf(runs[0]!)['telmoni.sample_rate']).toBe(0.6);
  });
});
