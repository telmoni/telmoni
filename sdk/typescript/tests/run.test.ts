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
      'telmoni.sample_rate': 1,
      'telmoni.tag.env': 'test',
      'telmoni.tag.team': 'a',
    });
  });

  it('types the rate as the contract does, whatever the number', async () => {
    h.telmoni.run('bot', { sampleRate: 1 }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const typed = Object.fromEntries(one(runsIn(h.transport)).attributes.map((a) => [a.key, a.value]));
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

  it('is failed with no type for an empty one, which ingest refuses the run over', async () => {
    h.telmoni.run('bot', (run) => run.fail(''));
    expect(await h.telmoni.flush()).toBe(true);
    const [failed] = runsIn(h.transport);
    expect(attributesOf(failed!)['error.type']).toBeUndefined();
    expect(failed!.status).toEqual({ code: 2 });
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
    expect(() => h.telmoni.run(null as unknown as string, () => {})).toThrow("a run needs its agent's name");
    // Its text is blank: `String([])` is `''`.
    expect(() => h.telmoni.run([] as unknown as string, () => {})).toThrow("a run needs its agent's name");
    expect(() => h.telmoni.run('bot', { sampleRate: 0 }, () => {})).toThrow(RangeError);
    expect(() => h.telmoni.run('bot', { sampleRate: 1.5 }, () => {})).toThrow(RangeError);
    expect(() => h.telmoni.run('bot', { sampleRate: '0.5' as unknown as number }, () => {})).toThrow(RangeError);
  });

  it('sends its first 32 tags, warned once of the rest, so none crowds out its own', async () => {
    const tags = Object.fromEntries(Array.from({ length: 40 }, (_, n) => [`t${String(n).padStart(2, '0')}`, 'v']));
    for (let i = 0; i < 2; i += 1) h.telmoni.run('bot', { tags }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    for (const run of h.transport.spans().filter((span) => span.name === 'invoke_agent bot')) {
      const sent = Object.keys(attributesOf(run)).filter((key) => key.startsWith('telmoni.tag.'));
      expect(sent).toEqual(Array.from({ length: 32 }, (_, n) => `telmoni.tag.t${String(n).padStart(2, '0')}`));
    }
    expect(h.warnings).toEqual(['telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent']);
  });

  it('sends a tag of a number as it is, another as its text or its type where that throws, and leaves an unset one off', async () => {
    const tags = {
      team: null,
      n: 3,
      on: true,
      ratio: 2.5,
      odd: Number.NaN,
      kind: { a: 1 },
      bare: Object.create(null) as unknown,
    } as unknown as Record<string, string>;
    h.telmoni.run('bot', { tags }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const run = h.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    const typed = Object.fromEntries(run.attributes.map((a) => [a.key, a.value]));
    expect(typed['telmoni.tag.n']).toEqual({ intValue: '3' });
    expect(typed['telmoni.tag.on']).toEqual({ boolValue: true });
    expect(typed['telmoni.tag.ratio']).toEqual({ doubleValue: 2.5 });
    expect(typed['telmoni.tag.odd']).toEqual({ stringValue: 'NaN' });
    expect(typed['telmoni.tag.kind']).toEqual({ stringValue: '[object Object]' });
    expect(typed['telmoni.tag.bare']).toEqual({ stringValue: 'object' });
    expect(typed['telmoni.tag.team']).toBeUndefined();
    expect(h.warnings.filter((warning) => warning.includes('is sent as its text'))).toHaveLength(1);
  });

  it('leaves off a tag ingest would drop, which takes no place among the 32', async () => {
    const tags: Record<string, string> = { '': 'empty', ['k'.repeat(65)]: 'long', v: 'x'.repeat(257) };
    for (let n = 0; n < 31; n += 1) tags[`t${String(n).padStart(2, '0')}`] = '1';
    tags['\ud83d\ude00'.repeat(64)] = '\ud83d\ude01'.repeat(256);
    // Past the 32nd, a tag ingest drops is still warned of as dropped.
    tags['w'] = 'y'.repeat(257);
    h.telmoni.run('bot', { tags }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const run = h.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    const sent = Object.keys(attributesOf(run)).filter((key) => key.startsWith('telmoni.tag.'));
    const kept = Array.from({ length: 31 }, (_, n) => `telmoni.tag.t${String(n).padStart(2, '0')}`);
    expect(new Set(sent)).toEqual(new Set([...kept, `telmoni.tag.${'\ud83d\ude00'.repeat(64)}`]));
    expect(h.warnings).toEqual(["telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent"]);
  });

  it('leaves off a tag of a number ingest writes past 256 characters, which takes no place either', async () => {
    // Ingest writes every digit and never an exponent: 1e256 in 257 of them.
    const tags: Record<string, unknown> = { at: 1e255, small: 1.5e-253, past: 1e256, below: -1e255, tiny: 1e-255 };
    for (let n = 0; n < 30; n += 1) tags[`t${String(n).padStart(2, '0')}`] = '1';
    h.telmoni.run('bot', { tags: tags as Record<string, string> }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const run = h.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    const sent = Object.keys(attributesOf(run)).filter((key) => key.startsWith('telmoni.tag.'));
    const kept = Array.from({ length: 30 }, (_, n) => `telmoni.tag.t${String(n).padStart(2, '0')}`);
    expect(new Set(sent)).toEqual(new Set(['telmoni.tag.at', 'telmoni.tag.small', ...kept]));
    expect(h.warnings).toEqual(["telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent"]);
  });

  it('sends a user or session that is no string as its text', async () => {
    h.telmoni.run('bot', { userId: [7, 8] as unknown as string, sessionId: 42 as unknown as string }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const typed = Object.fromEntries(one(runsIn(h.transport)).attributes.map((a) => [a.key, a.value]));
    expect(typed['user.id']).toEqual({ stringValue: '7,8' });
    expect(typed['gen_ai.conversation.id']).toEqual({ stringValue: '42' });
    expect(h.warnings).toEqual([
      'telmoni: a user id that is no string is sent as its text',
      'telmoni: a session id that is no string is sent as its text',
    ]);
  });

  it('sends an error type that is no string as its class, never its message, which is content, else as its text', async () => {
    class RateLimited extends Error {}
    const given: unknown[] = [new RateLimited('quota exceeded for alice@example.com'), RateLimited, 429];
    for (const errorType of given) h.telmoni.run('bot', (run) => run.fail(errorType as string));
    expect(await h.telmoni.flush()).toBe(true);
    const runs = runsIn(h.transport);
    expect(runs.map((run) => attributesOf(run)['error.type'])).toEqual(['RateLimited', 'RateLimited', '429']);
    for (const run of runs) expect(run.status).toEqual({ code: 2 });
    expect(h.transport.text()).not.toContain('alice@example.com');
    expect(h.warnings).toEqual(["telmoni: a span's error type that is no string is sent as its text"]);
  });

  it('cuts its text past ingest\'s caps, or leaves off an id, warned once, where each lost the run whole', async () => {
    const long = 'E'.repeat(129);
    const LongError = { [long]: class extends Error {} }[long] as new () => Error;
    for (let i = 0; i < 2; i++) {
      expect(() =>
        h.telmoni.run('😀'.repeat(129), { userId: 'u'.repeat(129), sessionId: 's'.repeat(201) }, (run) => {
          expect(run.agent).toBe('😀'.repeat(128));
          throw new LongError();
        }),
      ).toThrow(LongError);
    }
    expect(await h.telmoni.flush()).toBe(true);
    for (const span of h.transport.spans()) {
      const sent = attributesOf(span);
      expect(sent['telmoni.agent']).toBe('😀'.repeat(128));
      expect(Object.keys(sent)).not.toContain('user.id');
      expect(Object.keys(sent)).not.toContain('gen_ai.conversation.id');
    }
    const runs = runsIn(h.transport);
    expect(runs.map((run) => run.name)).toEqual([`invoke_agent ${'😀'.repeat(128)}`, `invoke_agent ${'😀'.repeat(128)}`]);
    for (const run of runs) {
      expect(attributesOf(run)['gen_ai.agent.name']).toBe('😀'.repeat(128));
      expect(attributesOf(run)['error.type']).toBe('E'.repeat(128));
    }
    expect([...h.warnings].sort()).toEqual([
      "telmoni: a run's agent must be up to 128 characters; it is sent cut to its first 128",
      'telmoni: a session id must be up to 200 characters; the run is sent without it',
      "telmoni: a span's error type must be up to 128 characters; it is sent cut to its first 128",
      'telmoni: a user id must be up to 128 characters; the run is sent without it',
    ]);
  });

  it("sends its text at ingest's caps whole, counted in characters, never in UTF-8's bytes", async () => {
    h.telmoni.run('😀'.repeat(128), { userId: '😀'.repeat(128), sessionId: '😀'.repeat(200) }, (run) => {
      run.fail('😀'.repeat(128));
    });
    expect(await h.telmoni.flush()).toBe(true);
    const sent = attributesOf(one(runsIn(h.transport)));
    expect([sent['telmoni.agent'], sent['user.id'], sent['error.type']]).toEqual(Array(3).fill('😀'.repeat(128)));
    expect(sent['gen_ai.conversation.id']).toBe('😀'.repeat(200));
    expect(h.warnings).toEqual([]);
  });

  it("keeps a warning whose hook throws out of the agent's call", async () => {
    const t = harness(
      {},
      {
        warn: () => {
          throw new Error("the hook's own");
        },
      },
    );
    const tags = Object.fromEntries(Array.from({ length: 40 }, (_, n) => [`t${n}`, 'v']));
    try {
      expect(t.telmoni.run('bot', { tags }, (run) => run.step(7 as unknown as string, () => 'ran'))).toBe('ran');
      expect(await t.telmoni.flush()).toBe(true);
    } finally {
      await t.telmoni.shutdown();
    }
    expect(t.transport.spans().map((span) => span.name)).toEqual(expect.arrayContaining(['invoke_agent bot', '7']));
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
      expect(spans.map((span) => span.name)).toEqual(['invoke_agent bot']);
      expect(attributesOf(spans[0]!)['telmoni.sample_rate']).toBe(0.25);
    } finally {
      await sampled.telmoni.shutdown();
    }
  });

  it('drops a span that ends after its run', async () => {
    const sampled = harness({ sampleRate: 0.25 });
    try {
      vi.spyOn(Math, 'random').mockReturnValue(0.9);
      let late: Promise<void> | undefined;
      sampled.telmoni.run('bot', (run) => {
        late = run.step('late', () => new Promise<void>((resolve) => setTimeout(resolve, 10)));
      });
      await late;
      vi.spyOn(Math, 'random').mockReturnValue(0.1);
      const kept = sampled.telmoni.run('bot', (run) => run);
      expect(await sampled.telmoni.flush()).toBe(true);
      const spans = sampled.transport.spans();
      expect(new Set(spans.map((span) => attributesOf(span)['telmoni.run_id']))).toEqual(new Set([kept.runId]));
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
