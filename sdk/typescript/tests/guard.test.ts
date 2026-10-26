/**
 * The guard: a run's caps on its steps and spend, counted in this process as
 * the server counts a call's cost, replayed from `sdk/fixtures/guard.json`.
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { CapExceededError, type Price, type Run, type Telmoni, type Usage } from '../src/index';
import { attributesOf, harness, loadFixture, one } from './helpers';

interface AnswerSpec {
  response_model?: string;
  usage?: {
    input_tokens?: number;
    output_tokens?: number;
    cache_read_input_tokens?: number;
    cache_write_input_tokens?: number;
    cost?: number;
  };
}

interface CallSpec extends AnswerSpec {
  name: string;
  provider?: string;
  model: string;
  answers?: AnswerSpec[];
  counted: number;
}

type Op = 'step' | 'tool_call' | { model_call: number };

interface RunSpec {
  name: string;
  max_steps?: number;
  max_spend?: number;
  ops: Op[];
  refused: { at: number; cap: 'steps' | 'spend'; limit: number; used: number } | null;
  spent: number;
}

interface Guard {
  prices: Array<{
    provider: string;
    model: string;
    input: number;
    output: number;
    cache_read?: number;
    cache_write?: number;
  }>;
  calls: CallSpec[];
  runs: RunSpec[];
}

const FIXTURE = loadFixture<Guard>('guard.json');
const CORE_PRICES = fileURLToPath(new URL('../../../crates/telemetry/model_prices.json', import.meta.url));
const CARRIED = fileURLToPath(new URL('../src/model_prices.json', import.meta.url));

function handed(): Price[] {
  return FIXTURE.prices.map((price) => {
    const out: Price = { provider: price.provider, model: price.model, input: price.input, output: price.output };
    if (price.cache_read !== undefined) out.cacheRead = price.cache_read;
    if (price.cache_write !== undefined) out.cacheWrite = price.cache_write;
    return out;
  });
}

function usageOf(answer: AnswerSpec): Usage | undefined {
  if (answer.usage === undefined) return undefined;
  return {
    inputTokens: answer.usage.input_tokens,
    outputTokens: answer.usage.output_tokens,
    cacheReadInputTokens: answer.usage.cache_read_input_tokens,
    cacheWriteInputTokens: answer.usage.cache_write_input_tokens,
    cost: answer.usage.cost,
  };
}

function call(run: Run, spec: CallSpec): void {
  run.modelCall(spec.model, { provider: spec.provider }, (modelCall) => {
    for (const answer of spec.answers ?? [spec]) {
      modelCall.answered({ responseModel: answer.response_model, usage: usageOf(answer) });
    }
  });
}

function perform(run: Run, op: Op): void {
  if (op === 'step') run.step('step', () => undefined);
  else if (op === 'tool_call') run.toolCall('lookup', () => undefined);
  else call(run, FIXTURE.calls[op.model_call]!);
}

async function closed(telmoni: Telmoni): Promise<void> {
  expect(await telmoni.flush()).toBe(true);
  await telmoni.shutdown();
}

describe('guard.json', () => {
  it('counts each call as the server counts it', async () => {
    const { telmoni } = harness({ prices: handed() });
    telmoni.run('guarded', (run) => {
      for (const spec of FIXTURE.calls) {
        const before = run.spent;
        call(run, spec);
        expect(run.spent - before, spec.name).toBeCloseTo(spec.counted, 12);
      }
    });
    await closed(telmoni);
  });

  it.each(FIXTURE.runs.map((spec) => [spec.name, spec] as const))('%s', async (_name, spec) => {
    const { telmoni, transport } = harness({ prices: handed() });
    const options: { maxSteps?: number; maxSpend?: number } = {};
    if (spec.max_steps !== undefined) options.maxSteps = spec.max_steps;
    if (spec.max_spend !== undefined) options.maxSpend = spec.max_spend;
    telmoni.run('guarded', options, (run) => {
      for (const [at, op] of spec.ops.entries()) {
        if (spec.refused !== null && at === spec.refused.at) {
          let thrown: unknown;
          try {
            perform(run, op);
          } catch (error) {
            thrown = error;
          }
          expect(thrown).toBeInstanceOf(CapExceededError);
          const refusal = thrown as CapExceededError;
          expect(refusal.cap).toBe(spec.refused.cap);
          expect(refusal.limit).toBeCloseTo(spec.refused.limit, 12);
          expect(refusal.used).toBeCloseTo(spec.refused.used, 12);
          break;
        }
        perform(run, op);
      }
      expect(run.spent).toBeCloseTo(spec.spent, 12);
    });
    await closed(telmoni);
    const performed = spec.refused === null ? spec.ops : spec.ops.slice(0, spec.refused.at);
    const opened = transport.spans().filter((span) => span.name !== 'invoke_agent guarded');
    expect(opened, 'a refused op opened nothing').toHaveLength(performed.length);
  });
});

describe('the guard', () => {
  it('fails the run it escapes with its class', async () => {
    const { telmoni, transport } = harness();
    expect(() =>
      telmoni.run('guarded', { maxSteps: 1 }, (run) => {
        run.step('first', () => undefined);
        run.step('second', () => undefined);
      }),
    ).toThrow(/its step cap of 1/);
    await closed(telmoni);
    const run = one(transport.spans().filter((span) => span.name === 'invoke_agent guarded'));
    expect(attributesOf(run)['error.type']).toBe('CapExceededError');
    expect(run.status).toEqual({ code: 2 });
  });

  it('prices a name no price is handed for by the table the release carries', async () => {
    const rows = (JSON.parse(readFileSync(CARRIED, 'utf8')) as { prices: Array<Record<string, unknown>> }).prices;
    const row = rows.find((r) => r['provider'] === 'openai' && r['model'] === 'gpt-4o-mini')!;
    const asHanded: Price = {
      provider: 'openai',
      model: 'gpt-4o-mini',
      input: row['input'] as number,
      output: row['output'] as number,
      cacheRead: row['cache_read'] as number,
      cacheWrite: row['cache_write'] as number,
    };
    const spent: number[] = [];
    for (const prices of [[], [asHanded]]) {
      const { telmoni } = harness({ prices });
      telmoni.run('priced', (run) => {
        run.modelCall('gpt-4o-mini', { provider: 'openai' }, (modelCall) => {
          modelCall.answered({ usage: { inputTokens: 10_000, outputTokens: 2_000, cacheReadInputTokens: 4_000 } });
        });
        spent.push(run.spent);
      });
      await closed(telmoni);
    }
    expect(spent[0]).toBeGreaterThan(0);
    expect(spent[0], "the table's row, as if it were handed").toBe(spent[1]);
  });

  it('counts a call answered again as its span holds it', async () => {
    const { telmoni, transport } = harness({ prices: [{ provider: 'p', model: 'm', input: 1e-6, output: 1e-5 }] });
    let spent = Number.NaN;
    telmoni.run('guarded', (run) => {
      run.modelCall('m', { provider: 'p' }, (modelCall) => {
        // A stream's counts, answered as they arrive.
        modelCall.answered({ usage: { inputTokens: 2000 } });
        modelCall.answered({ usage: { outputTokens: 100 } });
      });
      spent = run.spent;
    });
    await closed(telmoni);
    const held = attributesOf(one(transport.spans().filter((span) => span.name === 'chat m')));
    expect(spent).toBeCloseTo(
      (held['gen_ai.usage.input_tokens'] as number) * 1e-6 + (held['gen_ai.usage.output_tokens'] as number) * 1e-5,
      12,
    );
    expect(spent).toBeCloseTo(0.003, 12);
  });

  it('is released with the price file whole', () => {
    expect(readFileSync(CARRIED, 'utf8'), 'make prices writes it').toBe(readFileSync(CORE_PRICES, 'utf8'));
  });

  it('warns once of a call with neither a cost nor a price under a spend cap', async () => {
    const { telmoni, warnings } = harness();
    telmoni.run('guarded', { maxSpend: 1 }, (run) => {
      for (let n = 0; n < 3; n++) {
        run.modelCall('my-model', { provider: 'vllm' }, (modelCall) => {
          modelCall.answered({ usage: { inputTokens: 10 } });
        });
      }
    });
    telmoni.run('unguarded', (run) => {
      run.modelCall('other-model', { provider: 'vllm' }, (modelCall) => {
        modelCall.answered({ usage: { inputTokens: 10 } });
      });
    });
    await closed(telmoni);
    expect(warnings.filter((w) => w.includes('counts nothing'))).toEqual([
      "telmoni: no price for my-model (vllm), which reported no cost; it counts nothing toward the run's spend cap",
    ]);
  });

  it.each([
    [{ maxSteps: 0 }, /maxSteps/],
    [{ maxSteps: 1.5 }, /maxSteps/],
    [{ maxSpend: 0 }, /maxSpend/],
    [{ maxSpend: -1 }, /maxSpend/],
    [{ maxSpend: Number.POSITIVE_INFINITY }, /maxSpend/],
    [{ maxSpend: Number.NaN }, /maxSpend/],
    [{ maxSpend: '1' as unknown as number }, /maxSpend/],
  ])('refuses a cap %o that is not above 0', async (options, why) => {
    const { telmoni } = harness();
    expect(() => telmoni.run('guarded', options, () => undefined)).toThrow(why);
    await telmoni.shutdown();
  });

  it('takes a price handed as a name and numbers from 0', () => {
    for (const broken of [
      { provider: '', model: 'm', input: 1e-6, output: 1e-6 },
      { provider: 'p', model: '', input: 1e-6, output: 1e-6 },
      { provider: 'p', model: 'm', input: -1e-6, output: 1e-6 },
      { provider: 'p', model: 'm', input: 1e-6, output: Number.NaN },
      { provider: 'p', model: 'm', input: 1e-6, output: 1e-6, cacheRead: '1' as unknown as number },
    ]) {
      expect(() => harness({ prices: [broken] })).toThrow();
    }
    expect(() => harness({ prices: 'none' as unknown as Price[] })).toThrow(TypeError);
  });

  it('sends a cost as a double, and leaves off one that is no cost with one warning', async () => {
    const { telmoni, transport, warnings } = harness();
    const costs: unknown[] = [3, 0.0123, -0.5, Number.NaN, Number.POSITIVE_INFINITY, true, '0.5'];
    telmoni.run('bot', (run) => {
      for (const cost of costs) {
        run.modelCall('m', (modelCall) => {
          modelCall.answered({ usage: { inputTokens: 1, cost: cost as number } });
        });
      }
    });
    await closed(telmoni);
    const sent = transport
      .spans()
      .filter((span) => span.name === 'chat m')
      .map((span) => span.attributes.find((a) => a.key === 'gen_ai.usage.cost')?.value);
    expect(sent).toEqual([
      { doubleValue: 3 },
      { doubleValue: 0.0123 },
      undefined,
      undefined,
      undefined,
      undefined,
      undefined,
    ]);
    expect(warnings).toEqual([
      'telmoni: gen_ai.usage.cost must be a finite number of US dollars from 0; the call is sent without it',
    ]);
  });
});
