import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { type FakeTransport, type Harness, type JsonSpan, attributesOf, harness, one } from './helpers';

let h: Harness;

beforeEach(() => {
  h = harness();
});

afterEach(async () => {
  await h.telmoni.shutdown();
});

function tools(transport: FakeTransport): JsonSpan[] {
  return transport.spans().filter((span) => span.name.startsWith('execute_tool '));
}

describe('a tool call', () => {
  it("carries the conventions' names", async () => {
    const run = h.telmoni.run('bot', { userId: 'user' }, (run) => {
      run.toolCall('search', { arguments: { q: 'weather' } }, (tool) => {
        tool.returned({ temp: 21 });
      });
      return run;
    });
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(tools(h.transport));
    expect(span.name).toBe('execute_tool search');
    expect(span.kind).toBe(1);
    expect(attributesOf(span)).toEqual({
      'telmoni.run_id': run.runId,
      'telmoni.agent': 'bot',
      'user.id': 'user',
      'gen_ai.operation.name': 'execute_tool',
      'gen_ai.tool.name': 'search',
    });
    expect(h.transport.text()).not.toContain('weather');
    expect(h.transport.text()).not.toContain('temp');
  });

  it('records its error class, from a failure or from fail()', async () => {
    class TimeoutError extends Error {}
    await h.telmoni.run('bot', async (run) => {
      await expect(
        run.toolCall('search', async () => {
          throw new TimeoutError('took 30 s');
        }),
      ).rejects.toBeInstanceOf(TimeoutError);
      run.toolCall('lookup', (tool) => tool.fail('NotFound'));
    });
    expect(await h.telmoni.flush()).toBe(true);
    const byName = Object.fromEntries(tools(h.transport).map((span) => [span.name, span]));
    expect(attributesOf(byName['execute_tool search']!)['error.type']).toBe('TimeoutError');
    expect(byName['execute_tool search']!.status).toEqual({ code: 2 });
    expect(attributesOf(byName['execute_tool lookup']!)['error.type']).toBe('NotFound');
    expect(byName['execute_tool lookup']!.status).toEqual({ code: 2 });
    expect(h.transport.text()).not.toContain('took 30 s');
  });

  it('returns what its callback returns', async () => {
    expect(h.telmoni.run('bot', (run) => run.toolCall('search', () => 'cba'))).toBe('cba');
    expect(await h.telmoni.flush()).toBe(true);
    expect(tools(h.transport)).toHaveLength(1);
  });

  it('is cancelled, not failed, when the tool aborts', async () => {
    await h.telmoni.run('bot', async (run) => {
      await expect(
        run.toolCall('search', async () => {
          throw AbortSignal.abort().reason;
        }),
      ).rejects.toHaveProperty('name', 'AbortError');
    });
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(tools(h.transport));
    expect(attributesOf(span)['telmoni.cancelled']).toBe(true);
    expect(span.status).toBeUndefined();
  });

  it('never throws over content that will not serialize', async () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    h.telmoni.run('bot', (run) => {
      run.toolCall('search', { arguments: cyclic }, (tool) => {
        tool.returned(cyclic);
      });
    });
    expect(await h.telmoni.flush()).toBe(true);
    expect(one(tools(h.transport)).name).toBe('execute_tool search');
  });
});
