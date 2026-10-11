import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { type Harness, attributesOf, byName, harness } from './helpers';

let h: Harness;

beforeEach(() => {
  h = harness();
});

afterEach(async () => {
  await h.telmoni.shutdown();
});

describe('a step', () => {
  it('nests under its run and carries its ids', async () => {
    const run = h.telmoni.run('bot', { userId: 'c', conversationId: 'v' }, (run) => {
      run.step('plan', (plan) => {
        plan.step('plan.inner', () => {});
      });
      return run;
    });
    expect(await h.telmoni.flush()).toBe(true);
    const spans = byName(h.transport);
    const plan = spans['plan']!;
    const inner = spans['plan.inner']!;
    const agent = spans['invoke_agent bot']!;
    expect(plan.parentSpanId).toBe(agent.spanId);
    expect(inner.parentSpanId).toBe(plan.spanId);
    expect(plan.traceId).toBe(agent.traceId);
    expect(inner.traceId).toBe(agent.traceId);
    expect(attributesOf(plan)).toEqual({
      'telmoni.run_id': run.runId,
      'telmoni.agent': 'bot',
      'user.id': 'c',
      'gen_ai.conversation.id': 'v',
    });
    expect(plan.kind).toBe(1);
    expect(plan.status).toBeUndefined();
  });

  it('opened on the run nests under the run, even from inside another step', async () => {
    h.telmoni.run('bot', (run) => {
      run.step('outer', (outer) => {
        outer.step('under-outer', () => {});
        run.step('under-run', () => {});
      });
    });
    expect(await h.telmoni.flush()).toBe(true);
    const spans = byName(h.transport);
    expect(spans['under-outer']!.parentSpanId).toBe(spans['outer']!.spanId);
    expect(spans['under-run']!.parentSpanId).toBe(spans['invoke_agent bot']!.spanId);
  });

  it('returns what its callback returns, sync or async', async () => {
    const [sync, later] = await h.telmoni.run('bot', async (run) => [
      run.step('a', () => 1),
      await run.step('b', async () => 2),
    ]);
    expect(sync).toBe(1);
    expect(later).toBe(2);
  });

  it('records the error class of a failure and rethrows it, the run going on', async () => {
    class PlanningFailed extends Error {}
    await h.telmoni.run('bot', async (run) => {
      await expect(
        run.step('plan', async () => {
          throw new PlanningFailed('secret details');
        }),
      ).rejects.toBeInstanceOf(PlanningFailed);
    });
    expect(await h.telmoni.flush()).toBe(true);
    const spans = byName(h.transport);
    expect(spans['plan']!.status).toEqual({ code: 2 });
    expect(attributesOf(spans['plan']!)['error.type']).toBe('PlanningFailed');
    expect(spans['invoke_agent bot']!.status, 'the run itself went on').toBeUndefined();
    expect(h.transport.text()).not.toContain('secret details');
  });

  it('ends when its promise settles, so its span covers the await', async () => {
    await h.telmoni.run('bot', async (run) => {
      await run.step('first', async () => {
        await new Promise<void>((resolve) => setTimeout(resolve, 5));
      });
      await run.step('second', async () => {});
    });
    expect(await h.telmoni.flush()).toBe(true);
    const spans = byName(h.transport);
    const first = spans['first']!;
    // OpenTelemetry starts a span on a millisecond clock and ends it on a
    // finer one, so only the duration is a safe comparison, never two spans' edges.
    expect(BigInt(first.endTimeUnixNano) - BigInt(first.startTimeUnixNano) >= 1_000_000n).toBe(true);
    expect(BigInt(spans['second']!.startTimeUnixNano) >= BigInt(first.startTimeUnixNano)).toBe(true);
  });
});
