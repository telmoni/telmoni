import { describe, expect, it } from 'vitest';

import { harness } from './helpers';

describe('the processor', () => {
  it('bounds the queue and counts the drops once', async () => {
    // A tiny queue, and a worker that will not wake until flushed.
    const h = harness({}, { processor: { maxQueueSize: 2, maxBatchSize: 100, scheduleDelaySeconds: 60 } });
    try {
      // Nothing yields inside the run, so the worker cannot run before the queue fills.
      h.telmoni.run('bot', (run) => {
        for (let n = 0; n < 5; n++) run.step(`s${n}`, () => {});
      });
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    const names = h.transport.spans().map((span) => span.name);
    expect(names, names.join(', ')).toHaveLength(2);
    expect(h.warnings).toHaveLength(1);
    expect(h.warnings[0]).toContain('queue was full');
    expect(h.warnings[0]).toContain('5 spans');
  });

  it('shutdown drains what is queued', async () => {
    const h = harness();
    h.telmoni.run('bot', () => {});
    await h.telmoni.shutdown();
    expect(new Set(h.transport.spans().map((span) => span.name))).toEqual(
      new Set(['telmoni.run.start', 'invoke_agent bot']),
    );
    await h.telmoni.shutdown();
    expect(h.transport.requests).toHaveLength(1);
  });

  it('a batch fills and goes without a flush', async () => {
    const h = harness({}, { processor: { maxBatchSize: 3, scheduleDelaySeconds: 60 } });
    try {
      await h.telmoni.run('bot', async (run) => {
        expect(await h.transport.waitFor(1)).toBe(true);
        for (let n = 0; n < 3; n++) run.step(`s${n}`, () => {});
        expect(await h.transport.waitFor(2), 'three steps fill a batch of three').toBe(true);
        expect(h.transport.spans().map((span) => span.name)).toEqual(['telmoni.run.start', 's0', 's1', 's2']);
      });
    } finally {
      await h.telmoni.shutdown();
    }
  });

  it('exports one batch at a time, and flush waits for the one in flight', async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const h = harness({}, {
      transport: async (url, init) => {
        await gate;
        return h.transport.send(url, init);
      },
    });
    try {
      h.telmoni.run('bot', () => {});
      const flushed = h.telmoni.flush(1);
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      expect(h.transport.requests).toHaveLength(0);
      release();
      expect(await flushed).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.requests).toHaveLength(1);
  });

  it('flush gives up past its timeout', async () => {
    const h = harness({}, { transport: () => new Promise(() => {}) });
    h.telmoni.run('bot', () => {});
    expect(await h.telmoni.flush(0.01)).toBe(false);
  });
});
