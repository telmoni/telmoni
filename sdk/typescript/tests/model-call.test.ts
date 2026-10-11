import { afterEach, beforeEach, describe, expect, it } from 'vitest';

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
    const run = h.telmoni.run('bot', { conversationId: 'conv' }, (run) => {
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
