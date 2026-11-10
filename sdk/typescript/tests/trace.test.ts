import { trace as otel } from '@opentelemetry/api';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { Trace, currentTrace } from '../src/index';
import { type Harness, answer, attributesOf, harness, one, tracesIn } from './helpers';

const HEX32 = /^[0-9a-f]{32}$/;

let h: Harness;

beforeEach(() => {
  h = harness();
});

afterEach(async () => {
  await h.telmoni.shutdown();
});

describe('a trace', () => {
  it("is the agent's span at the root of a trace of its own, with the contract attributes", async () => {
    const opened = h.telmoni.trace(
      'support-bot',
      {
        userId: 'user_1',
        sessionId: 'conv_1',
        tags: { env: 'test', team: 'a' },
      },
      (trace) => {
        expect(trace).toBeInstanceOf(Trace);
        expect(trace.traceId).toMatch(HEX32);
        expect(trace.agent).toBe('support-bot');
        return trace;
      },
    );
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(tracesIn(h.transport));
    expect(span.kind).toBe(1);
    expect(span.traceId).toBe(opened.traceId);
    expect(span.parentSpanId).toBeUndefined();
    expect(span.links).toBeUndefined();
    expect(span.status).toBeUndefined();
    expect(attributesOf(span)).toEqual({
      'gen_ai.operation.name': 'invoke_agent',
      'gen_ai.agent.name': 'support-bot',
      'telmoni.agent': 'support-bot',
      'user.id': 'user_1',
      'gen_ai.conversation.id': 'conv_1',
      'telmoni.tag.env': 'test',
      'telmoni.tag.team': 'a',
    });
  });

  it('is a trace each, a job looping over executions never one', async () => {
    const ids = new Set<string>();
    for (let n = 0; n < 3; n++) h.telmoni.trace('bot', (trace) => ids.add(trace.traceId));
    expect(await h.telmoni.flush()).toBe(true);
    expect(ids.size).toBe(3);
    expect(new Set(tracesIn(h.transport).map((span) => span.traceId))).toEqual(ids);
  });

  it('fails with the class of an error rejected inside it, and rethrows it', async () => {
    class ProviderDown extends Error {}
    await expect(
      h.telmoni.trace('bot', async () => {
        throw new ProviderDown('the message, which is content and never sent');
      }),
    ).rejects.toBeInstanceOf(ProviderDown);
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(tracesIn(h.transport));
    expect(span.status).toEqual({ code: 2 });
    expect(attributesOf(span)['error.type']).toBe('ProviderDown');
    expect(h.transport.text()).not.toContain('never sent');
  });

  it('fails with the class of an error thrown inside it, and rethrows it', async () => {
    expect(() =>
      h.telmoni.trace('bot', () => {
        throw new RangeError('out of range');
      }),
    ).toThrow(RangeError);
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(tracesIn(h.transport));
    expect(span.status).toEqual({ code: 2 });
    expect(attributesOf(span)['error.type']).toBe('RangeError');
    expect(h.transport.text()).not.toContain('out of range');
  });

  it('is marked by fail and cancel', async () => {
    h.telmoni.trace('bot', (trace) => trace.fail('RateLimit'));
    h.telmoni.trace('bot', (trace) => trace.cancel());
    expect(await h.telmoni.flush()).toBe(true);
    const traces = tracesIn(h.transport);
    const failed = traces.find((span) => 'error.type' in attributesOf(span))!;
    const cancelled = traces.find((span) => !('error.type' in attributesOf(span)))!;
    expect(attributesOf(failed)['error.type']).toBe('RateLimit');
    expect(failed.status).toEqual({ code: 2 });
    expect(attributesOf(cancelled)['telmoni.cancelled']).toBe(true);
    expect(cancelled.status).toBeUndefined();
  });

  it('is failed with no type for an empty one, which ingest refuses the trace over', async () => {
    h.telmoni.trace('bot', (trace) => trace.fail(''));
    expect(await h.telmoni.flush()).toBe(true);
    const [failed] = tracesIn(h.transport);
    expect(attributesOf(failed!)['error.type']).toBeUndefined();
    expect(failed!.status).toEqual({ code: 2 });
  });

  it('is cancelled by an AbortError, not failed', async () => {
    const aborted = new DOMException('stopped', 'AbortError');
    await expect(
      h.telmoni.trace('bot', async () => {
        throw aborted;
      }),
    ).rejects.toBe(aborted);
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(tracesIn(h.transport));
    expect(attributesOf(span)['telmoni.cancelled']).toBe(true);
    expect(span.status).toBeUndefined();
    expect(attributesOf(span)['error.type']).toBeUndefined();
  });

  it('returns what its callback returns, sync or async', async () => {
    expect(h.telmoni.trace('bot', () => 42)).toBe(42);
    expect(await h.telmoni.trace('bot', async () => 'later')).toBe('later');
  });

  it('needs an agent', () => {
    expect(() => h.telmoni.trace(' ', () => {})).toThrow(TypeError);
    expect(() => h.telmoni.trace(null as unknown as string, () => {})).toThrow("a trace needs its agent's name");
    // Its text is blank: `String([])` is `''`.
    expect(() => h.telmoni.trace([] as unknown as string, () => {})).toThrow("a trace needs its agent's name");
  });

  it('sends its first 32 tags, warned once of the rest, so none crowds out its own', async () => {
    const tags = Object.fromEntries(Array.from({ length: 40 }, (_, n) => [`t${String(n).padStart(2, '0')}`, 'v']));
    for (let i = 0; i < 2; i += 1) h.telmoni.trace('bot', { tags }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    for (const agent of h.transport.spans().filter((span) => span.name === 'invoke_agent bot')) {
      const sent = Object.keys(attributesOf(agent)).filter((key) => key.startsWith('telmoni.tag.'));
      expect(sent).toEqual(Array.from({ length: 32 }, (_, n) => `telmoni.tag.t${String(n).padStart(2, '0')}`));
    }
    expect(h.warnings).toEqual(['telmoni: a trace carries up to 32 tags, the most ingest keeps; the rest are not sent']);
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
    h.telmoni.trace('bot', { tags }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    const typed = Object.fromEntries(agent.attributes.map((a) => [a.key, a.value]));
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
    tags['😀'.repeat(64)] = '😁'.repeat(256);
    // Past the 32nd, a tag ingest drops is still warned of as dropped.
    tags['w'] = 'y'.repeat(257);
    h.telmoni.trace('bot', { tags }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    const sent = Object.keys(attributesOf(agent)).filter((key) => key.startsWith('telmoni.tag.'));
    const kept = Array.from({ length: 31 }, (_, n) => `telmoni.tag.t${String(n).padStart(2, '0')}`);
    expect(new Set(sent)).toEqual(new Set([...kept, `telmoni.tag.${'😀'.repeat(64)}`]));
    expect(h.warnings).toEqual(["telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent"]);
  });

  it('leaves off a tag of a number ingest writes past 256 characters, which takes no place either', async () => {
    // Ingest writes every digit and never an exponent: 1e256 in 257 of them.
    const tags: Record<string, unknown> = { at: 1e255, small: 1.5e-253, past: 1e256, below: -1e255, tiny: 1e-255 };
    for (let n = 0; n < 30; n += 1) tags[`t${String(n).padStart(2, '0')}`] = '1';
    h.telmoni.trace('bot', { tags: tags as Record<string, string> }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const agent = h.transport.spans().find((span) => span.name === 'invoke_agent bot')!;
    const sent = Object.keys(attributesOf(agent)).filter((key) => key.startsWith('telmoni.tag.'));
    const kept = Array.from({ length: 30 }, (_, n) => `telmoni.tag.t${String(n).padStart(2, '0')}`);
    expect(new Set(sent)).toEqual(new Set(['telmoni.tag.at', 'telmoni.tag.small', ...kept]));
    expect(h.warnings).toEqual(["telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent"]);
  });

  it('sends a user or session that is no string as its text', async () => {
    h.telmoni.trace('bot', { userId: [7, 8] as unknown as string, sessionId: 42 as unknown as string }, () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const typed = Object.fromEntries(one(tracesIn(h.transport)).attributes.map((a) => [a.key, a.value]));
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
    for (const errorType of given) h.telmoni.trace('bot', (trace) => trace.fail(errorType));
    expect(await h.telmoni.flush()).toBe(true);
    const traces = tracesIn(h.transport);
    expect(traces.map((span) => attributesOf(span)['error.type'])).toEqual(['RateLimited', 'RateLimited', '429']);
    for (const span of traces) expect(span.status).toEqual({ code: 2 });
    expect(h.transport.text()).not.toContain('alice@example.com');
    expect(h.warnings).toEqual(["telmoni: a span's error type that is no string is sent as its text"]);
  });

  it("cuts its text past ingest's caps, or leaves off an id, warned once, where each lost the trace whole", async () => {
    const long = 'E'.repeat(129);
    const LongError = { [long]: class extends Error {} }[long] as new () => Error;
    for (let i = 0; i < 2; i++) {
      expect(() =>
        h.telmoni.trace('😀'.repeat(129), { userId: 'u'.repeat(129), sessionId: 's'.repeat(201) }, (trace) => {
          expect(trace.agent).toBe('😀'.repeat(128));
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
    const traces = tracesIn(h.transport);
    expect(traces.map((span) => span.name)).toEqual([`invoke_agent ${'😀'.repeat(128)}`, `invoke_agent ${'😀'.repeat(128)}`]);
    for (const span of traces) {
      expect(attributesOf(span)['gen_ai.agent.name']).toBe('😀'.repeat(128));
      expect(attributesOf(span)['error.type']).toBe('E'.repeat(128));
    }
    expect([...h.warnings].sort()).toEqual([
      'telmoni: a session id must be up to 200 characters; the trace is sent without it',
      "telmoni: a span's error type must be up to 128 characters; it is sent cut to its first 128",
      "telmoni: a trace's agent must be up to 128 characters; it is sent cut to its first 128",
      'telmoni: a user id must be up to 128 characters; the trace is sent without it',
    ]);
  });

  it("sends its text at ingest's caps whole, counted in characters, never in UTF-8's bytes", async () => {
    h.telmoni.trace('😀'.repeat(128), { userId: '😀'.repeat(128), sessionId: '😀'.repeat(200) }, (trace) => {
      trace.fail('😀'.repeat(128));
    });
    expect(await h.telmoni.flush()).toBe(true);
    const sent = attributesOf(one(tracesIn(h.transport)));
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
      expect(t.telmoni.trace('bot', { tags }, (trace) => trace.step(7 as unknown as string, () => 'ran'))).toBe('ran');
      expect(await t.telmoni.flush()).toBe(true);
    } finally {
      await t.telmoni.shutdown();
    }
    expect(t.transport.spans().map((span) => span.name)).toEqual(expect.arrayContaining(['invoke_agent bot', '7']));
  });

  it('is found from its context; without a context manager, from nothing else', () => {
    h.telmoni.trace('bot', (trace) => {
      expect(currentTrace(trace.context)).toBe(trace);
      // No context manager is registered in this suite: the active context is the root.
      expect(currentTrace()).toBeUndefined();
      trace.step('s', (step) => {
        expect(currentTrace(step.context)).toBe(trace);
      });
    });
  });

  it('sends its own messages on its span only when asked and the project allows', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const asked = harness({ captureContent: true }, {}, Array.from({ length: 4 }, () => on));
    try {
      asked.telmoni.trace('warm', () => {});
      expect(await asked.telmoni.flush()).toBe(true);
      asked.telmoni.trace('bot', { inputMessages: [{ role: 'user', content: 'hi' }] }, (trace) => {
        trace.answered({ outputMessages: [{ role: 'assistant', content: 'hello' }] });
      });
      expect(await asked.telmoni.flush()).toBe(true);
    } finally {
      await asked.telmoni.shutdown();
    }
    const agent = attributesOf(one(tracesIn(asked.transport).filter((span) => span.name === 'invoke_agent bot')));
    expect(agent['gen_ai.input.messages']).toBe('[{"role":"user","content":"hi"}]');
    expect(agent['gen_ai.output.messages']).toBe('[{"role":"assistant","content":"hello"}]');
  });

  it('keeps its own messages home without captureContent even when the project allows it', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const quiet = harness({}, {}, Array.from({ length: 4 }, () => on));
    try {
      quiet.telmoni.trace('warm', () => {});
      expect(await quiet.telmoni.flush()).toBe(true);
      expect(quiet.telmoni.contentMode).toBe('on');
      quiet.telmoni.trace('bot', { inputMessages: 'never asked' }, (trace) => {
        trace.answered({ outputMessages: 'never answered' });
      });
      expect(await quiet.telmoni.flush()).toBe(true);
    } finally {
      await quiet.telmoni.shutdown();
    }
    expect(quiet.transport.text()).not.toContain('never asked');
    expect(quiet.transport.text()).not.toContain('never answered');
  });

  it('sends a failure with its message and stack only when asked and the project allows', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const asked = harness({ captureContent: true }, {}, Array.from({ length: 4 }, () => on));
    try {
      asked.telmoni.trace('warm', () => {});
      expect(await asked.telmoni.flush()).toBe(true);
      asked.telmoni.trace('bot', (trace) => trace.fail(new RangeError('took too long')));
      expect(() =>
        asked.telmoni.trace('bot', (trace) =>
          trace.step('plan', () => {
            throw new TypeError('bad plan');
          }),
        ),
      ).toThrow(TypeError);
      asked.telmoni.trace('bot', (trace) => trace.fail('RateLimit'));
      expect(await asked.telmoni.flush()).toBe(true);
    } finally {
      await asked.telmoni.shutdown();
    }
    const [, failed, step, ran, typed] = asked.transport.spans();
    for (const [span, kind, message] of [
      [failed!, 'RangeError', 'took too long'],
      [step!, 'TypeError', 'bad plan'],
      [ran!, 'TypeError', 'bad plan'],
    ] as const) {
      expect(span.status).toEqual({ code: 2, message });
      const event = one(span.events!);
      const sent = attributesOf(event);
      expect(event.name).toBe('exception');
      expect(sent['exception.type']).toBe(kind);
      expect(attributesOf(span)['error.type']).toBe(kind);
      expect(sent['exception.message']).toBe(message);
      expect(sent['exception.stacktrace']).toContain(`${kind}: ${message}`);
    }
    // A type alone carries no message to record.
    expect(typed!.status).toEqual({ code: 2 });
    expect(typed!.events).toBeUndefined();
  });

  it("keeps a failure's message home without captureContent even when the project allows it", async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const quiet = harness({}, {}, Array.from({ length: 4 }, () => on));
    try {
      quiet.telmoni.trace('warm', () => {});
      expect(await quiet.telmoni.flush()).toBe(true);
      quiet.telmoni.trace('bot', (trace) => trace.fail(new RangeError('took too long')));
      expect(await quiet.telmoni.flush()).toBe(true);
    } finally {
      await quiet.telmoni.shutdown();
    }
    const span = quiet.transport.spans().at(-1)!;
    expect(span.status).toEqual({ code: 2 });
    expect(span.events).toBeUndefined();
    expect(attributesOf(span)['error.type']).toBe('RangeError');
    expect(quiet.transport.text()).not.toContain('took too long');
  });

  it("sends the application's own event only when asked and the project allows", async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const asked = harness({ captureContent: true }, {}, Array.from({ length: 4 }, () => on));
    try {
      for (const each of [asked, h]) {
        each.telmoni.trace('warm', () => {});
        expect(await each.telmoni.flush()).toBe(true);
        each.telmoni.trace('bot', (trace) => {
          otel.getSpan(trace.context)!.addEvent('cache miss', { key: 'k1' });
        });
        expect(await each.telmoni.flush()).toBe(true);
      }
    } finally {
      await asked.telmoni.shutdown();
    }
    const event = one(asked.transport.spans().at(-1)!.events!);
    expect(event.name).toBe('cache miss');
    expect(attributesOf(event)).toEqual({ key: 'k1' });
    expect(h.transport.spans().at(-1)!.events).toBeUndefined();
    expect(h.transport.text()).not.toContain('cache miss');
  });

  it('sends no answer given after it ended, with one warning', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const asked = harness({ captureContent: true }, {}, Array.from({ length: 4 }, () => on));
    try {
      asked.telmoni.trace('warm', () => {});
      expect(await asked.telmoni.flush()).toBe(true);
      const ended = asked.telmoni.trace('bot', (trace) => trace);
      for (let n = 0; n < 2; n++) ended.answered({ outputMessages: 'too late' });
      expect(await asked.telmoni.flush()).toBe(true);
    } finally {
      await asked.telmoni.shutdown();
    }
    expect(asked.transport.text()).not.toContain('too late');
    expect(asked.warnings).toEqual([
      'telmoni: a trace must be answered before it ends; an answer given after is not sent',
    ]);
  });
});

describe('the wrapper', () => {
  it('opens a trace per call, the trace its last argument, and keeps the name', async () => {
    const seen: string[] = [];
    function handle(question: string, trace: Trace): string {
      seen.push(trace.traceId);
      return question.toUpperCase();
    }
    const wrapped = h.telmoni.wrap('bot', handle);
    expect(wrapped('a')).toBe('A');
    expect(wrapped('b')).toBe('B');
    expect(wrapped.name).toBe('handle');
    expect(await h.telmoni.flush()).toBe(true);
    expect(tracesIn(h.transport)).toHaveLength(2);
    expect(new Set(seen).size).toBe(2);
  });

  it('works with async callees and the options', async () => {
    const think = h.telmoni.wrap('bot', { sessionId: 'conv' }, async (depth: number, trace: Trace) => {
      await trace.step('think', async () => {
        await Promise.resolve();
      });
      return `${trace.traceId}:${depth}`;
    });
    const first = await think(1);
    const second = await h.telmoni.trace('bot', async (trace) => trace.traceId);
    expect(await h.telmoni.flush()).toBe(true);
    const spans = h.transport.spans();
    expect(spans.filter((span) => span.name === 'invoke_agent bot')).toHaveLength(2);
    const step = spans.find((span) => span.name === 'think')!;
    expect(step.traceId).toBe(first.split(':')[0]);
    expect(attributesOf(step)['gen_ai.conversation.id']).toBe('conv');
    expect(first.split(':')[0]).not.toBe(second);
  });
});

describe('outside a trace', () => {
  it('nothing can be opened, so nothing is sent', async () => {
    // Steps and calls open on a trace's handle, or a step's: the client has no
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
