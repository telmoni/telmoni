import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import type { Config } from '../src/index';
import { type FakeTransport, type Harness, type JsonSpan, answer, attributesOf, harness, one } from './helpers';

const DOCUMENTS = [
  { id: 'kb-112', score: 0.91 },
  { id: 'kb-087', score: 0.84 },
];

let h: Harness;

beforeEach(() => {
  h = harness();
});

afterEach(async () => {
  await h.telmoni.shutdown();
});

function searches(transport: FakeTransport): JsonSpan[] {
  return transport.spans().filter((span) => span.name.startsWith('retrieval') || span.name === 'search_memory');
}

/** A client whose content leaves: asked for, and the project's mode on. */
function keeping(config: Config = {}): Harness {
  const on = Array.from({ length: 8 }, () => answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}'));
  return harness({ captureContent: true, ...config }, {}, on);
}

async function sent(kept: Harness): Promise<Array<Record<string, unknown>>> {
  expect(await kept.telmoni.flush()).toBe(true);
  await kept.telmoni.shutdown();
  return searches(kept.transport).map((span) => attributesOf(span));
}

describe('a retrieval', () => {
  it("carries the conventions' names", async () => {
    const run = h.telmoni.run('bot', { userId: 'user' }, (run) => {
      run.retrieval('kb-articles', { query: 'refunds', topK: 5 }, (found) => found.returned(DOCUMENTS));
      return run;
    });
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(searches(h.transport));
    expect(span.name).toBe('retrieval kb-articles');
    expect(span.kind).toBe(3);
    expect(attributesOf(span)).toEqual({
      'telmoni.run_id': run.runId,
      'telmoni.agent': 'bot',
      'user.id': 'user',
      'gen_ai.operation.name': 'retrieval',
      'gen_ai.data_source.id': 'kb-articles',
      'gen_ai.retrieval.top_k': 5,
      'telmoni.retrieval.returned': 2,
    });
    expect(h.transport.text()).not.toContain('refunds');
    expect(h.transport.text()).not.toContain('kb-112');
  });

  it("goes under the memory's names as a memory search, on a step too", async () => {
    const run = h.telmoni.run('bot', (run) => {
      run.step('recall', (step) =>
        step.retrieval('preferences', { topK: 3, memory: true }, (found) => found.returned([{ id: 'mem-41' }])),
      );
      return run;
    });
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(searches(h.transport));
    expect(span.name).toBe('search_memory');
    expect(span.kind).toBe(3);
    expect(attributesOf(span)).toEqual({
      'telmoni.run_id': run.runId,
      'telmoni.agent': 'bot',
      'gen_ai.operation.name': 'search_memory',
      'gen_ai.memory.store.id': 'preferences',
      'gen_ai.retrieval.top_k': 3,
      'gen_ai.memory.record.count': 1,
    });
  });

  it('with a blank source is named by its operation alone, warned once', async () => {
    h.telmoni.run('bot', (run) => {
      for (const source of ['', ' 　']) run.retrieval(source, () => {});
      run.retrieval('\u0085', { memory: true }, () => {});
    });
    expect(await h.telmoni.flush()).toBe(true);
    const spans = searches(h.transport);
    expect(spans.map((span) => span.name)).toEqual(['retrieval', 'retrieval', 'search_memory']);
    for (const span of spans) {
      expect(attributesOf(span)).not.toHaveProperty(['gen_ai.data_source.id']);
      expect(attributesOf(span)).not.toHaveProperty(['gen_ai.memory.store.id']);
    }
    expect(h.warnings).toEqual(['telmoni: a retrieval needs its source; one without is sent named by its operation alone']);
  });

  it("cuts a source past ingest's 145 characters, counted as characters, warned once", async () => {
    h.telmoni.run('bot', (run) => {
      for (const source of ['😀'.repeat(146), '😀'.repeat(146), '😀'.repeat(145)]) run.retrieval(source, () => {});
      run.retrieval('😀'.repeat(146), { memory: true }, () => {});
    });
    expect(await h.telmoni.flush()).toBe(true);
    const spans = searches(h.transport);
    expect(spans.map((span) => span.name)).toEqual([...Array(3).fill(`retrieval ${'😀'.repeat(145)}`), 'search_memory']);
    expect(spans.slice(0, 3).map((span) => attributesOf(span)['gen_ai.data_source.id'])).toEqual(
      Array(3).fill('😀'.repeat(145)),
    );
    expect(attributesOf(spans[3]!)['gen_ai.memory.store.id']).toBe('😀'.repeat(145));
    expect(h.warnings).toEqual([
      "telmoni: a retrieval's source must be up to 145 characters; it is sent cut to its first 145",
    ]);
  });

  it('sends a source that is no string as its text, warned once', async () => {
    h.telmoni.run('bot', (run) => run.retrieval(42 as unknown as string, () => {}));
    expect(await h.telmoni.flush()).toBe(true);
    expect(attributesOf(one(searches(h.transport)))['gen_ai.data_source.id']).toBe('42');
    expect(h.warnings).toEqual(["telmoni: a retrieval's source that is no string is sent as its text"]);
  });

  it('reads its top_k as a token count is, a fraction cut, and leaves off one that is none', async () => {
    const given: unknown[] = [0, 7.9, '4', -1, true, 'many', 2 ** 63, Number.NaN];
    h.telmoni.run('bot', (run) => {
      for (const topK of given) run.retrieval('kb', { topK: topK as number }, () => {});
    });
    expect(await h.telmoni.flush()).toBe(true);
    expect(searches(h.transport).map((span) => attributesOf(span)['gen_ai.retrieval.top_k'])).toEqual([
      0,
      7,
      4,
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
    ]);
    expect(h.warnings).toEqual([
      'telmoni: gen_ai.retrieval.top_k must be a whole number from 0 within 64 bits; the retrieval is sent without it',
    ]);
  });

  it.each([
    ['text', 'kb-112'],
    ['a typed array', new Uint8Array([1, 2])],
    ['an object', { id: 'kb-112' }],
    ['a set', new Set(DOCUMENTS)],
    ['null', null],
    ['a number', 3],
  ])('refuses %s whole as what it returned', async (_what, given) => {
    const kept = keeping();
    kept.telmoni.run('bot', (run) => run.retrieval('kb', (found) => found.returned(given as unknown as unknown[])));
    const [attributes] = await sent(kept);
    expect(attributes).not.toHaveProperty(['telmoni.retrieval.returned']);
    expect(attributes).not.toHaveProperty(['gen_ai.retrieval.documents']);
    expect(kept.warnings).toEqual(["telmoni: a retrieval's documents must be a list; they are neither sent nor counted"]);
    expect(kept.fields).toEqual([{ kind: 'input' }]);
  });

  it('keeps the last list whole, a refused one leaving the last', async () => {
    const more = [...DOCUMENTS, { id: 'kb-203', score: 0.77 }];
    const kept = keeping();
    kept.telmoni.run('bot', (run) => {
      run.retrieval('then-refused', (found) => {
        found.returned(DOCUMENTS);
        found.returned('kb-112' as unknown as unknown[]);
      });
      run.retrieval('refused-then', (found) => {
        found.returned({ id: 'kb-112' } as unknown as unknown[]);
        found.returned(DOCUMENTS);
      });
      run.retrieval('twice', (found) => {
        found.returned(more);
        found.returned(DOCUMENTS.slice(0, 1));
      });
    });
    const pairs = (await sent(kept)).map((attributes) => [
      JSON.parse(attributes['gen_ai.retrieval.documents'] as string) as unknown,
      attributes['telmoni.retrieval.returned'],
    ]);
    expect(pairs).toEqual([
      [DOCUMENTS, 2],
      [DOCUMENTS, 2],
      [DOCUMENTS.slice(0, 1), 1],
    ]);
  });

  it('counts an empty list 0 and sends it as one', async () => {
    const kept = keeping();
    kept.telmoni.run('bot', (run) => {
      run.retrieval('kb', (found) => found.returned([]));
      run.retrieval('prefs', { memory: true }, (found) => found.returned([]));
    });
    const [retrieval, memory] = await sent(kept);
    expect(retrieval).toMatchObject({ 'telmoni.retrieval.returned': 0, 'gen_ai.retrieval.documents': '[]' });
    expect(memory).toMatchObject({ 'gen_ai.memory.record.count': 0, 'gen_ai.memory.records': '[]' });
  });

  it('sends its query and documents only under both locks', async () => {
    const kept = keeping();
    kept.telmoni.run('bot', (run) => {
      run.retrieval('kb', { query: 'refunds' }, (found) => found.returned(DOCUMENTS));
      run.retrieval('prefs', { query: { about: 'tone' }, memory: true }, (found) => found.returned(DOCUMENTS.slice(0, 1)));
    });
    const [retrieval, memory] = await sent(kept);
    expect(retrieval!['gen_ai.retrieval.query.text']).toBe('refunds');
    expect(JSON.parse(retrieval!['gen_ai.retrieval.documents'] as string)).toEqual(DOCUMENTS);
    expect(memory!['gen_ai.memory.query.text']).toBe('{"about":"tone"}');
    expect(JSON.parse(memory!['gen_ai.memory.records'] as string)).toEqual(DOCUMENTS.slice(0, 1));
    expect(memory).not.toHaveProperty(['gen_ai.retrieval.query.text']);
  });

  it('keeps its count when a mask takes documents out', async () => {
    const kept = keeping({ mask: (key, value) => (key === 'gen_ai.retrieval.documents' ? '[]' : value) });
    kept.telmoni.run('bot', (run) => run.retrieval('kb', (found) => found.returned(DOCUMENTS)));
    const [attributes] = await sent(kept);
    expect(attributes).toMatchObject({ 'gen_ai.retrieval.documents': '[]', 'telmoni.retrieval.returned': 2 });
  });

  it('returns what its callback returns, and is failed by what it throws', async () => {
    class TimeoutError extends Error {}
    await h.telmoni.run('bot', async (run) => {
      expect(run.retrieval('kb', () => 'found')).toBe('found');
      await expect(
        run.retrieval('web-index', async () => {
          throw new TimeoutError('took 30 s');
        }),
      ).rejects.toBeInstanceOf(TimeoutError);
    });
    expect(await h.telmoni.flush()).toBe(true);
    const failed = searches(h.transport).find((span) => span.name === 'retrieval web-index')!;
    expect(attributesOf(failed)['error.type']).toBe('TimeoutError');
    expect(failed.status).toEqual({ code: 2 });
  });
});
