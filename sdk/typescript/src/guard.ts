/**
 * The guard: a run's opt-in caps on its steps and its spend, counted in this
 * process with no call to Telmoni, thrown at the step, model call or tool call
 * the run would open past one.
 *
 * A call's cost is counted as the server counts it: as reported, else its
 * tokens at its price in the table this release carries (`model_prices.json`,
 * LiteLLM's, whose notice it holds), else nothing; so a call with neither
 * stops nothing. A price handed to the client comes before the table's, and
 * the server never sees it.
 */

import { CapExceededError } from './errors.js';
import file from './model_prices.json';
import type { Price, RunOptions } from './types.js';

/** One name's price, each in US dollars a token. */
interface Rate {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
}

/** A call's counts as sent, one left off counting 0. */
export interface Tokens {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
}

/** Prices by provider, then model. */
type Rates<T> = ReadonlyMap<string, ReadonlyMap<string, T>>;

/** A project's own prices, as the client holds them. */
export type OwnRates = Rates<Rate>;

interface FileRow {
  provider: string;
  model: string;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
}

/**
 * What a call's tokens cost: its input less the cache's at the input price,
 * each cache count at its own, and its output at the output price.
 */
function costOf(rate: Rate, tokens: Tokens): number {
  const uncached = Math.max(tokens.input - tokens.cacheRead - tokens.cacheWrite, 0);
  return (
    uncached * rate.input + tokens.cacheRead * rate.cacheRead + tokens.cacheWrite * rate.cacheWrite + tokens.output * rate.output
  );
}

let table: Map<string, Map<string, Rate>> | undefined;

/** The price table this release carries, indexed once. */
function tableRates(): Rates<Rate> {
  if (table === undefined) {
    const rows = new Map<string, Map<string, Rate>>();
    for (const row of (file as unknown as { prices: FileRow[] }).prices) {
      let models = rows.get(row.provider);
      if (models === undefined) rows.set(row.provider, (models = new Map()));
      models.set(row.model, {
        input: row.input,
        output: row.output,
        cacheRead: row.cache_read,
        cacheWrite: row.cache_write,
      });
    }
    table = rows;
  }
  return table;
}

/** A project's own prices by name, a name given twice its last. */
export function ownRates(prices: readonly Price[]): OwnRates {
  const rates = new Map<string, Map<string, Rate>>();
  for (const price of prices) {
    let models = rates.get(price.provider);
    if (models === undefined) rates.set(price.provider, (models = new Map()));
    models.set(price.model, {
      input: price.input,
      output: price.output,
      cacheRead: price.cacheRead ?? price.input,
      cacheWrite: price.cacheWrite ?? price.input,
    });
  }
  return rates;
}

/**
 * What a call that reported no cost cost: at its own price, else at the
 * table's, by its provider and model as sent, whole; undefined for a name
 * neither holds.
 */
export function priced(own: OwnRates, provider: string, model: string, tokens: Tokens): number | undefined {
  const rate = own.get(provider)?.get(model) ?? tableRates().get(provider)?.get(model);
  return rate === undefined ? undefined : costOf(rate, tokens);
}

/** A price as the client takes it: a name a call can carry, and numbers from 0. */
export function checkedPrice(given: unknown): Price {
  const price = given as Partial<Price> | null;
  if (
    typeof price !== 'object' ||
    price === null ||
    typeof price.provider !== 'string' ||
    price.provider === '' ||
    typeof price.model !== 'string' ||
    price.model === ''
  ) {
    throw new TypeError('a price names its provider and its model');
  }
  const amounts = [price.input, price.output, price.cacheRead ?? 0, price.cacheWrite ?? 0];
  if (!amounts.every((amount) => typeof amount === 'number' && Number.isFinite(amount) && amount >= 0)) {
    throw new RangeError('a price is a finite number of US dollars a token, from 0');
  }
  return Object.freeze({ ...price }) as Price;
}

/** One run's caps, and what it has used of them. */
export class Guard {
  readonly #maxSteps: number | undefined;
  readonly #maxSpend: number | undefined;
  #steps = 0;
  #spent = 0;

  constructor(maxSteps?: number, maxSpend?: number) {
    this.#maxSteps = maxSteps;
    this.#maxSpend = maxSpend;
  }

  /** The run's options' caps, or why they are none: whole steps above 0, and a spend above 0. */
  static of(options: RunOptions): Guard {
    const steps: unknown = options.maxSteps ?? undefined;
    if (steps !== undefined && !(typeof steps === 'number' && Number.isInteger(steps) && steps >= 1)) {
      throw new RangeError('maxSteps must be a whole number above 0');
    }
    const spend: unknown = options.maxSpend ?? undefined;
    if (spend !== undefined && !(typeof spend === 'number' && Number.isFinite(spend) && spend > 0)) {
      throw new RangeError('maxSpend must be a finite number of US dollars above 0');
    }
    return new Guard(steps, spend);
  }

  get spent(): number {
    return this.#spent;
  }

  /** Whether the run has a spend cap, which a call priced at nothing never reaches. */
  get capped(): boolean {
    return this.#maxSpend !== undefined;
  }

  /** Counts a step opening, or throws where the run reached a cap: its spend for whatever opens, its steps for a step. */
  opens(step: boolean): void {
    if (this.#maxSpend !== undefined && this.#spent >= this.#maxSpend) {
      throw new CapExceededError('spend', this.#maxSpend, this.#spent);
    }
    if (step) {
      if (this.#maxSteps !== undefined && this.#steps >= this.#maxSteps) {
        throw new CapExceededError('steps', this.#maxSteps, this.#steps);
      }
      this.#steps += 1;
    }
  }

  /** A call's cost counted again: `now` in place of `before`. */
  counts(before: number, now: number): void {
    this.#spent += now - before;
  }
}
