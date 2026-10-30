import { describe, expect, it, vi } from 'vitest';

import { FakeTransport, answer, harness } from './helpers';

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
    expect(names).toEqual(['s0', 's1']);
    expect(h.warnings).toHaveLength(1);
    expect(h.warnings[0]).toContain('queue was full');
    expect(h.warnings[0]).toContain('4 spans');
    expect(h.fields).toStrictEqual([{ kind: 'queue_full' }]);
  });

  it('keeps going past a warning whose hook throws, the next flush still sending', async () => {
    const h = harness(
      {},
      {
        warn: () => {
          throw new Error("the hook's own");
        },
        processor: { maxQueueSize: 2, maxBatchSize: 100, scheduleDelaySeconds: 60 },
      },
    );
    try {
      h.telmoni.run('bot', (run) => {
        for (let n = 0; n < 5; n++) run.step(`s${n}`, () => {});
      });
      expect(await h.telmoni.flush()).toBe(true);
      h.telmoni.run('later', () => {});
      expect(await h.telmoni.flush()).toBe(true);
    } finally {
      await h.telmoni.shutdown();
    }
    expect(h.transport.spans().map((span) => span.name)).toContain('invoke_agent later');
  });

  it('shutdown drains what is queued', async () => {
    const h = harness();
    h.telmoni.run('bot', () => {});
    await h.telmoni.shutdown();
    expect(h.transport.spans().map((span) => span.name)).toEqual(['invoke_agent bot']);
    await h.telmoni.shutdown();
    expect(h.transport.requests).toHaveLength(1);
  });

  it('a batch fills and goes without a flush', async () => {
    const h = harness({}, { processor: { maxBatchSize: 3, scheduleDelaySeconds: 60 } });
    try {
      await h.telmoni.run('bot', async (run) => {
        for (let n = 0; n < 3; n++) run.step(`s${n}`, () => {});
        expect(await h.transport.waitFor(1), 'three steps fill a batch of three').toBe(true);
        expect(h.transport.spans().map((span) => span.name)).toEqual(['s0', 's1', 's2']);
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

  it('refuses a bound that is none at construction, as a batch of nothing would never export', () => {
    for (const processor of [{ maxBatchSize: 0 }, { maxBatchSize: Number.NaN }, { maxQueueSize: 0.5 }]) {
      expect(() => harness({}, { processor })).toThrow(RangeError);
    }
    for (const scheduleDelaySeconds of [-1, Number.NaN, Infinity]) {
      expect(() => harness({}, { processor: { scheduleDelaySeconds } })).toThrow(RangeError);
    }
    // No bound at all, as the coding agents' plugins ask.
    expect(() => harness({}, { processor: { maxQueueSize: Infinity, scheduleDelaySeconds: 0 } })).not.toThrow();
  });

  it('flush gives up past its timeout', async () => {
    const h = harness({}, { transport: () => new Promise(() => {}) });
    h.telmoni.run('bot', () => {});
    expect(await h.telmoni.flush(0.01)).toBe(false);
  });

  it('a flush given Infinity waits up to its cap, where Node would fire its timer at once', async () => {
    const sent = new FakeTransport();
    const h = harness(
      {},
      {
        transport: async (url, init) => {
          await new Promise((resolve) => setTimeout(resolve, 50));
          return sent.send(url, init);
        },
      },
    );
    try {
      h.telmoni.run('bot', () => {});
      expect(await h.telmoni.flush(Infinity)).toBe(true);
      expect(sent.requests).toHaveLength(1);
      // NaN waits not at all.
      h.telmoni.run('bot', () => {});
      expect(await h.telmoni.flush(Number.NaN)).toBe(false);
    } finally {
      await h.telmoni.shutdown();
    }
  });

  it('shutdown drains until its deadline, then holds no timer and sends nothing more', async () => {
    vi.useFakeTimers();
    try {
      // The real sleep, on the fake clock: every answer asks for 20 s, and a
      // batch of one leaves the run's own span queued behind its step.
      const h = harness(
        {},
        { sleep: undefined, processor: { maxBatchSize: 1 } },
        Array.from({ length: 10 }, () => answer(429, { 'Retry-After': '20' }, '{}')),
      );
      h.telmoni.run('bot', (run) => run.step('s', () => {}));
      const done = h.telmoni.shutdown();
      // Before the deadline the drain goes on: the step's retry, 20 s in.
      await vi.advanceTimersByTimeAsync(30_000);
      await done;
      expect(h.transport.requests).toHaveLength(2);
      // Past it nothing is pending to hold the process, and nothing more is
      // sent however long the process lives.
      expect(vi.getTimerCount()).toBe(0);
      await vi.advanceTimersByTimeAsync(600_000);
      expect(h.transport.requests).toHaveLength(2);
      // A flush after it has nothing left to wait for, so it waits for nothing.
      expect(await h.telmoni.flush()).toBe(true);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it('settles a flush still waiting when a shutdown gives up, its own timer let go', async () => {
    vi.useFakeTimers();
    try {
      const h = harness(
        {},
        { sleep: undefined, processor: { maxBatchSize: 1 } },
        Array.from({ length: 10 }, () => answer(429, { 'Retry-After': '20' }, '{}')),
      );
      h.telmoni.run('bot', (run) => run.step('s', () => {}));
      let flushed: boolean | undefined;
      void h.telmoni.flush(60).then((value) => {
        flushed = value;
      });
      const done = h.telmoni.shutdown();
      await vi.advanceTimersByTimeAsync(30_000);
      await done;
      await vi.advanceTimersByTimeAsync(0);
      expect(flushed).toBe(true);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("hands a sleep of the application's the signal a shutdown's give-up aborts", async () => {
    vi.useFakeTimers();
    try {
      // On a timer of its own, which would hold the process until it ends.
      const sleep = (seconds: number, signal: AbortSignal) =>
        new Promise<void>((resolve) => {
          const timer = setTimeout(resolve, seconds * 1000);
          signal.addEventListener('abort', () => {
            clearTimeout(timer);
            resolve();
          });
        });
      const h = harness(
        {},
        { sleep, processor: { maxBatchSize: 1 } },
        Array.from({ length: 10 }, () => answer(429, { 'Retry-After': '20' }, '{}')),
      );
      h.telmoni.run('bot', (run) => run.step('s', () => {}));
      const done = h.telmoni.shutdown();
      await vi.advanceTimersByTimeAsync(30_000);
      await done;
      expect(h.transport.requests).toHaveLength(2);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});
