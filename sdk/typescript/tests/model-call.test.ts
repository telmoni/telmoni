import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import type { Usage } from '../src/index';
import { type FakeTransport, type Harness, type JsonSpan, answer, attributesOf, harness, one } from './helpers';

let h: Harness;

beforeEach(() => {
  h = harness();
});

afterEach(async () => {
  await h.telmoni.shutdown();
});

function calls(transport: FakeTransport): JsonSpan[] {
  return transport.spans().filter((span) => span.name.startsWith('chat '));
}

describe('a model call', () => {
  it("carries the conventions' names", async () => {
    const run = h.telmoni.run('bot', { sessionId: 'conv' }, (run) => {
      run.modelCall('gpt-4o-mini', { provider: 'openai' }, (call) => {
        call.answered({
          responseModel: 'gpt-4o-mini-2024-07-18',
          usage: { inputTokens: 12, outputTokens: 7, cacheReadInputTokens: 3 },
        });
      });
      return run;
    });
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(calls(h.transport));
    expect(span.name).toBe('chat gpt-4o-mini');
    expect(span.kind).toBe(3);
    expect(attributesOf(span)).toEqual({
      'telmoni.run_id': run.runId,
      'telmoni.agent': 'bot',
      'gen_ai.conversation.id': 'conv',
      'gen_ai.operation.name': 'chat',
      'gen_ai.request.model': 'gpt-4o-mini',
      'gen_ai.provider.name': 'openai',
      'gen_ai.response.model': 'gpt-4o-mini-2024-07-18',
      'gen_ai.usage.input_tokens': 12,
      'gen_ai.usage.output_tokens': 7,
      'gen_ai.usage.cache_read.input_tokens': 3,
    });
    const typed = Object.fromEntries(span.attributes.map((a) => [a.key, a.value]));
    expect(typed['gen_ai.usage.input_tokens']).toEqual({ intValue: '12' });
  });

  it('sends what it was asked with as typed request attributes, each absent where not given', async () => {
    h.telmoni.run('bot', (run) => {
      run.modelCall(
        'm',
        { temperature: 1, topP: 0.9, topK: 40, maxTokens: 256, frequencyPenalty: 0.5, presencePenalty: -0.5, seed: 7 },
        (call) => call.answered({ timeToFirstToken: 0.35 }),
      );
      run.modelCall('n', { temperature: 0.2 }, () => undefined);
    });
    expect(await h.telmoni.flush()).toBe(true);
    const typed = (name: string) => {
      const span = one(h.transport.spans().filter((each) => each.name === name));
      return Object.fromEntries(span.attributes.map((a) => [a.key, a.value]));
    };
    const asked = typed('chat m');
    expect(asked['gen_ai.request.temperature']).toEqual({ doubleValue: 1 });
    expect(asked['gen_ai.request.top_p']).toEqual({ doubleValue: 0.9 });
    expect(asked['gen_ai.request.top_k']).toEqual({ doubleValue: 40 });
    expect(asked['gen_ai.request.max_tokens']).toEqual({ intValue: '256' });
    expect(asked['gen_ai.request.frequency_penalty']).toEqual({ doubleValue: 0.5 });
    expect(asked['gen_ai.request.presence_penalty']).toEqual({ doubleValue: -0.5 });
    expect(asked['gen_ai.request.seed']).toEqual({ intValue: '7' });
    expect(asked['gen_ai.response.time_to_first_chunk']).toEqual({ doubleValue: 0.35 });
    const bare = typed('chat n');
    expect(bare['gen_ai.request.temperature']).toEqual({ doubleValue: 0.2 });
    expect(Object.keys(bare).filter((key) => key.startsWith('gen_ai.request.') && key !== 'gen_ai.request.model')).toEqual([
      'gen_ai.request.temperature',
    ]);
    expect(bare['gen_ai.response.time_to_first_chunk']).toBeUndefined();
    expect(h.warnings).toEqual([]);
  });

  it('leaves off a time to first token below 0 or no number, and a parameter no number, each warned once', async () => {
    h.telmoni.run('bot', (run) => {
      run.modelCall('m', (call) => call.answered({ responseModel: 'm-1', timeToFirstToken: -0.1 }));
      run.modelCall('m', (call) => call.answered({ timeToFirstToken: Number.NaN }));
      run.modelCall('n', { maxTokens: Number.NaN, seed: 3.7, temperature: 'warm' as unknown as number, topP: Infinity }, () => undefined);
    });
    expect(await h.telmoni.flush()).toBe(true);
    const [first, second] = h.transport.spans().filter((span) => span.name === 'chat m').map(attributesOf);
    expect(first!['gen_ai.response.model'], 'the call still sent').toBe('m-1');
    expect(first!['gen_ai.response.time_to_first_chunk']).toBeUndefined();
    expect(second!['gen_ai.response.time_to_first_chunk']).toBeUndefined();
    const asked = attributesOf(one(h.transport.spans().filter((span) => span.name === 'chat n')));
    expect(asked['gen_ai.request.max_tokens']).toBeUndefined();
    expect(asked['gen_ai.request.temperature']).toBeUndefined();
    expect(asked['gen_ai.request.top_p']).toBeUndefined();
    expect(asked['gen_ai.request.seed'], 'a fraction cut, as a token count is').toBe(3);
    expect(h.warnings).toEqual([
      'telmoni: gen_ai.response.time_to_first_chunk must be a finite number of seconds from 0; the call is sent without it',
      'telmoni: gen_ai.request.temperature must be a finite number; the call is sent without it',
      'telmoni: gen_ai.request.top_p must be a finite number; the call is sent without it',
      'telmoni: gen_ai.request.max_tokens must be a whole number within 64 bits; the call is sent without it',
    ]);
    expect(h.fields).toStrictEqual([{ kind: 'input' }, { kind: 'input' }, { kind: 'input' }, { kind: 'input' }]);
  });

  it('sends a token count as a whole number from 0, and leaves off another', async () => {
    const usage = { inputTokens: Number.NaN, outputTokens: 12.7, cacheReadInputTokens: Infinity, cacheWriteInputTokens: 2 ** 63 };
    // Ingest refuses the call over a count below zero, or one no number.
    const refused = { inputTokens: -1, outputTokens: 12, cacheReadInputTokens: true as unknown as number, cacheWriteInputTokens: -5 };
    h.telmoni.run('bot', (run) => {
      run.modelCall('m', (call) => call.answered({ usage }));
      run.modelCall('m', (call) => call.answered({ usage: refused }));
      run.modelCall('m', (call) => call.answered({ usage: null as unknown as undefined }));
    });
    const at = Date.parse('2026-10-18T09:00:00.000Z');
    h.telmoni.recordRun({
      runId: '3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d',
      agent: 'bot',
      start: at,
      end: at,
      inside: [{ type: 'model_call', model: 'm', start: at, end: at, usage }],
    });
    expect(await h.telmoni.flush()).toBe(true);
    const spans = h.transport.spans().filter((span) => span.name === 'chat m');
    expect(spans, 'the calls sent, recorded ones too, and the batch').toHaveLength(4);
    const counted = spans.filter((span) => span.attributes.some((a) => a.key.startsWith('gen_ai.usage.')));
    expect(counted, 'the null usage counts nothing').toHaveLength(3);
    for (const span of counted) {
      const typed = Object.fromEntries(span.attributes.map((a) => [a.key, a.value]));
      expect(typed['gen_ai.usage.output_tokens']).toEqual({ intValue: '12' });
      expect(typed['gen_ai.usage.input_tokens']).toBeUndefined();
      expect(typed['gen_ai.usage.cache_read.input_tokens']).toBeUndefined();
      expect(typed['gen_ai.usage.cache_write.input_tokens']).toBeUndefined();
    }
    expect(h.warnings).toHaveLength(3);
    expect(h.warnings.every((warning) => warning.includes('must be a whole number from 0 within 64 bits'))).toBe(true);
  });

  it("reads a count of a bigint, a boxed number or a decimal numeral's text, never hex or blank as Number does", async () => {
    const usage = {
      inputTokens: ' 12.7 ' as unknown as number,
      outputTokens: '0x10' as unknown as number,
      cacheReadInputTokens: '' as unknown as number,
      cacheWriteInputTokens: 5n as unknown as number,
    };
    h.telmoni.run('bot', (run) => {
      run.modelCall('m', {}, (call) => call.answered({ usage }));
      run.modelCall('n', {}, (call) => call.answered({ usage: { inputTokens: Object(7) as number } }));
    });
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(h.transport.spans().filter((each) => each.name === 'chat m'));
    const typed = Object.fromEntries(span.attributes.map((a) => [a.key, a.value]));
    expect(typed['gen_ai.usage.input_tokens']).toEqual({ intValue: '12' });
    expect(typed['gen_ai.usage.output_tokens']).toBeUndefined();
    expect(typed['gen_ai.usage.cache_read.input_tokens']).toBeUndefined();
    expect(typed['gen_ai.usage.cache_write.input_tokens']).toEqual({ intValue: '5' });
    const boxed = one(h.transport.spans().filter((each) => each.name === 'chat n'));
    expect(attributesOf(boxed)['gen_ai.usage.input_tokens']).toBe(7);
    expect(h.warnings).toHaveLength(2);
  });

  it("takes a usage carrying none of Usage's keys as none, with one warning", async () => {
    // A provider's own usage, handed as it came: OpenAI's, then Anthropic's.
    const openai = { prompt_tokens: 12, completion_tokens: 7, total_tokens: 19 };
    const anthropic = { input_tokens: 12, output_tokens: 7, cache_read_input_tokens: 3 };
    const spent = h.telmoni.run('bot', { maxSpend: 1 }, (run) => {
      // Its own, its count unset, warns of nothing: checked first, the warning being once.
      run.modelCall('n', (call) => call.answered({ usage: { inputTokens: undefined } }));
      expect(h.warnings).toEqual([]);
      run.modelCall('m', { provider: 'openai' }, (call) => {
        call.answered({ responseModel: 'm-1', usage: openai as unknown as Usage });
      });
      run.modelCall('m', { provider: 'anthropic' }, (call) => call.answered({ usage: anthropic as unknown as Usage }));
      return run.spent;
    });
    expect(await h.telmoni.flush()).toBe(true);
    const [own, first, second] = calls(h.transport).map(attributesOf);
    expect(first!['gen_ai.response.model']).toBe('m-1');
    for (const held of [own!, first!, second!]) {
      expect(Object.keys(held).filter((key) => key.startsWith('gen_ai.usage.'))).toEqual([]);
    }
    expect(spent).toBe(0);
    expect(h.warnings).toEqual([
      "telmoni: a usage must be telmoni's Usage, the provider's counts copied into it; the call is sent without it",
    ]);
  });

  it('sends a provider or response model as its text, and an empty one not at all', async () => {
    // Ingest refuses the call over an empty one, or over an array.
    h.telmoni.run('bot', (run) => {
      run.modelCall('m', { provider: ['open', 'ai'] as unknown as string }, (call) => call.answered({ responseModel: '' }));
      run.modelCall('m', { provider: '' }, (call) => call.answered({ responseModel: 5 as unknown as string }));
    });
    expect(await h.telmoni.flush()).toBe(true);
    const [first, second] = calls(h.transport).map(attributesOf);
    expect(first!['gen_ai.provider.name']).toBe('open,ai');
    expect(first!['gen_ai.response.model']).toBeUndefined();
    expect(second!['gen_ai.provider.name']).toBeUndefined();
    expect(second!['gen_ai.response.model']).toBe('5');
    expect(h.warnings).toEqual([
      'telmoni: a provider that is no string is sent as its text',
      'telmoni: a response model that is no string is sent as its text',
    ]);
  });

  it('takes a blank operation as chat', async () => {
    // As one left out is: sent blank, the span would be named ' m' and
    // ingest would read it as a step. One that is no string is none either.
    h.telmoni.run('bot', (run) => {
      for (const operation of ['', '  ', 5 as unknown as string]) run.modelCall('m', { operation }, () => {});
    });
    expect(await h.telmoni.flush()).toBe(true);
    const spans = h.transport.spans().filter((span) => span.name === 'chat m');
    expect(spans, h.transport.spans().map((span) => span.name).join(', ')).toHaveLength(3);
    for (const span of spans) expect(attributesOf(span)['gen_ai.operation.name']).toBe('chat');
  });

  it("keeps its call where its model is empty or its text past ingest's caps, warned once", async () => {
    h.telmoni.run('bot', (run) => {
      for (let i = 0; i < 2; i++) {
        run.modelCall('', () => {});
        run.modelCall('m'.repeat(129), { provider: 'p'.repeat(129), operation: 'generate_content' }, (call) => {
          call.answered({ responseModel: 'r'.repeat(129) });
        });
        run.modelCall('m'.repeat(128), { operation: 'o'.repeat(20) }, () => {});
      }
      // At their caps: the longest operation and a model at its cap fit.
      run.modelCall('😀'.repeat(128), { operation: 'generate_content' }, (call) => {
        call.answered({ responseModel: '😀'.repeat(128) });
      });
    });
    expect(await h.telmoni.flush()).toBe(true);
    const sent = Object.fromEntries(
      h.transport
        .spans()
        .filter((span) => span.name !== 'invoke_agent bot')
        .map((span) => [span.name, attributesOf(span)]),
    );
    expect(Object.keys(sent).sort()).toEqual(
      [
        'chat',
        `generate_content ${'m'.repeat(128)}`,
        `chat ${'m'.repeat(128)}`,
        `generate_content ${'😀'.repeat(128)}`,
      ].sort(),
    );
    expect(Object.keys(sent['chat']!)).not.toContain('gen_ai.request.model');
    expect(sent['chat']!['gen_ai.operation.name']).toBe('chat');
    const cut = sent[`generate_content ${'m'.repeat(128)}`]!;
    expect([cut['gen_ai.request.model'], cut['gen_ai.provider.name'], cut['gen_ai.response.model']]).toEqual([
      'm'.repeat(128),
      'p'.repeat(128),
      'r'.repeat(128),
    ]);
    expect(sent[`generate_content ${'😀'.repeat(128)}`]!['gen_ai.response.model']).toBe('😀'.repeat(128));
    expect([...h.warnings].sort()).toEqual([
      'telmoni: a model call needs its model; one without is sent named by its operation alone',
      "telmoni: a model call's operation must be chat, generate_content, text_completion or embeddings, which ingest reads as a model call; it is sent as chat",
      'telmoni: a model must be up to 128 characters; it is sent cut to its first 128',
      'telmoni: a provider must be up to 128 characters; it is sent cut to its first 128',
      'telmoni: a response model must be up to 128 characters; it is sent cut to its first 128',
    ]);
  });

  it('has content taken off before it leaves', async () => {
    h.telmoni.run('bot', (run) => {
      run.modelCall('m', { inputMessages: [{ role: 'user', content: 'my secret prompt' }] }, (call) => {
        call.answered({ outputMessages: [{ role: 'assistant', content: 'the answer' }] });
      });
    });
    expect(await h.telmoni.flush()).toBe(true);
    const keys = Object.keys(attributesOf(one(calls(h.transport))));
    expect(keys).not.toContain('gen_ai.input.messages');
    expect(keys).not.toContain('gen_ai.output.messages');
    expect(h.transport.text()).not.toContain('my secret prompt');
    expect(h.transport.text()).not.toContain('the answer');
  });

  it('reads a content mode with blanks about it trimmed, as every SDK does', async () => {
    const trimmed = harness({}, {}, [answer(200, { 'Telmoni-Content-Mode': ' on\t' }, '{}')]);
    trimmed.telmoni.run('bot', () => {});
    expect(await trimmed.telmoni.flush()).toBe(true);
    expect(trimmed.telmoni.contentMode).toBe('on');
    await trimmed.telmoni.shutdown();
  });

  it('sends content only when asked and the project allows', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{"partialSuccess":null}');
    const asked = harness({ captureContent: true }, {}, Array.from({ length: 6 }, () => on));
    try {
      // Before the first answer the mode is unknown, so content stays home.
      asked.telmoni.run('bot', (run) => run.modelCall('m', { inputMessages: 'first' }, () => {}));
      expect(await asked.telmoni.flush()).toBe(true);
      expect(asked.telmoni.contentMode).toBe('on');
      asked.telmoni.run('bot', (run) =>
        run.modelCall('m', { inputMessages: 'second', systemInstructions: ['be brief'] }, (call) => {
          call.answered({ outputMessages: [{ role: 'assistant', content: 'ok' }] });
        }),
      );
      expect(await asked.telmoni.flush()).toBe(true);
    } finally {
      await asked.telmoni.shutdown();
    }
    const [first, second] = calls(asked.transport);
    expect(attributesOf(first!)).not.toHaveProperty('gen_ai.input.messages');
    const attributes = attributesOf(second!);
    expect(attributes['gen_ai.input.messages']).toBe('second');
    expect(attributes['gen_ai.system_instructions']).toBe('["be brief"]');
    expect(attributes['gen_ai.output.messages']).toBe('[{"role":"assistant","content":"ok"}]');
  });

  it('keeps content home without captureContent even when the project allows it', async () => {
    const on = answer(200, { 'Telmoni-Content-Mode': 'on' }, '{}');
    const quiet = harness({}, {}, Array.from({ length: 4 }, () => on));
    try {
      quiet.telmoni.run('bot', () => {});
      expect(await quiet.telmoni.flush()).toBe(true);
      expect(quiet.telmoni.contentMode).toBe('on');
      quiet.telmoni.run('bot', (run) => run.modelCall('m', { inputMessages: 'never' }, () => {}));
      expect(await quiet.telmoni.flush()).toBe(true);
    } finally {
      await quiet.telmoni.shutdown();
    }
    expect(quiet.transport.text()).not.toContain('never');
  });

  it('records the error class of a failure', async () => {
    class ConnectionError extends Error {}
    await h.telmoni.run('bot', async (run) => {
      await expect(
        run.modelCall('m', { operation: 'generate_content' }, async () => {
          throw new ConnectionError('provider down');
        }),
      ).rejects.toBeInstanceOf(ConnectionError);
    });
    expect(await h.telmoni.flush()).toBe(true);
    const span = one(h.transport.spans().filter((span) => span.name === 'generate_content m'));
    expect(span.status).toEqual({ code: 2 });
    expect(attributesOf(span)['error.type']).toBe('ConnectionError');
    expect(h.transport.text()).not.toContain('provider down');
  });

  it('returns what its callback returns', async () => {
    const reply = await h.telmoni.run('bot', (run) =>
      run.modelCall('m', async (call) => {
        call.answered({ responseModel: 'm-2' });
        return 'the reply';
      }),
    );
    expect(reply).toBe('the reply');
  });
});
