import { AsyncLocalStorage } from 'node:async_hooks';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { ROOT_CONTEXT } from '@opentelemetry/api';
import type { Context, ContextManager } from '@opentelemetry/api';

import { expect } from 'vitest';

import { Telmoni } from '../src/index';
import type { Config, Hooks, Transport, Warning } from '../src/index';

const FIXTURES = fileURLToPath(new URL('../../fixtures/', import.meta.url));

export function loadFixture<T>(name: string): T {
  return JSON.parse(readFileSync(`${FIXTURES}${name}`, 'utf8')) as T;
}

export interface AnswerFixture {
  status: number;
  headers: Record<string, string>;
  body: unknown;
  expect: string;
  warning: string | null;
  fields: { kind: string; status?: number; problem_type?: string } | null;
}

/** One answer from the platform, its body already encoded. */
export interface Answer {
  status: number;
  headers: Record<string, string>;
  body: string;
}

export function answer(status: number, headers: Record<string, string>, body: string): Answer {
  return { status, headers, body };
}

export function answerFrom(fixture: AnswerFixture): Answer {
  return answer(fixture.status, { ...fixture.headers }, JSON.stringify(fixture.body));
}

export function kept(): Answer {
  return answerFrom(loadFixture<AnswerFixture>('answers/kept.json'));
}

export function answers(name: string, times = 1): Answer[] {
  const one = answerFrom(loadFixture<AnswerFixture>(`answers/${name}.json`));
  return Array.from({ length: times }, () => ({ ...one, headers: { ...one.headers } }));
}

/** Every answer under `answers/`, by name. */
export function answerNames(): string[] {
  return readdirSync(`${FIXTURES}answers`)
    .filter((file) => file.endsWith('.json'))
    .map((file) => file.slice(0, -'.json'.length))
    .sort();
}

/** The line an answer's fixture says it logs, met on every attempt: none, or that one. */
export function warningOf(name: string): string[] {
  const { warning } = loadFixture<AnswerFixture>(`answers/${name}.json`);
  return warning === null ? [] : [warning];
}

/** What the fixture's warning hands beside its line, as the hook's `Warning`: none, or that one. */
export function fieldsOf(name: string): Warning[] {
  const { fields } = loadFixture<AnswerFixture>(`answers/${name}.json`);
  if (fields === null) return [];
  const { problem_type: problemType, ...rest } = fields;
  return [{ ...rest, ...(problemType === undefined ? {} : { problemType }) } as Warning];
}

export interface JsonAttribute {
  key: string;
  value: Record<string, unknown>;
}

export interface JsonSpan {
  traceId: string;
  spanId: string;
  parentSpanId?: string;
  name: string;
  kind: number;
  startTimeUnixNano: string;
  endTimeUnixNano: string;
  attributes: JsonAttribute[];
  status?: { code: number };
}

export interface ExportBody {
  resourceSpans: Array<{
    resource: { attributes: JsonAttribute[] };
    scopeSpans: Array<{ scope: { name: string; version: string }; spans: JsonSpan[] }>;
  }>;
}

export interface Request {
  url: string;
  headers: Record<string, string>;
  body: string;
}

/** Records every export and answers from a script, then with `kept`. */
export class FakeTransport {
  readonly requests: Request[] = [];
  answers: Array<Answer | Error>;
  #waiters: Array<{ count: number; settle: () => void }> = [];

  constructor(answers: Array<Answer | Error> = []) {
    this.answers = [...answers];
  }

  readonly send: Transport = async (url, init) => {
    this.requests.push({ url, headers: { ...init.headers }, body: init.body });
    const next = this.answers.shift() ?? kept();
    this.#notify();
    if (next instanceof Error) throw next;
    const headers = next.headers;
    return {
      status: next.status,
      headers: {
        get(name: string): string | null {
          const key = Object.keys(headers).find((k) => k.toLowerCase() === name.toLowerCase());
          return key === undefined ? null : (headers[key] ?? null);
        },
      },
      text: () => Promise.resolve(next.body),
    };
  };

  /** Resolves true once `count` requests arrived; false when the timeout passes first. */
  waitFor(count: number, timeoutMs = 5000): Promise<boolean> {
    if (this.requests.length >= count) return Promise.resolve(true);
    return new Promise((resolve) => {
      const waiter = {
        count,
        settle: () => {
          clearTimeout(timer);
          resolve(true);
        },
      };
      const timer = setTimeout(() => {
        this.#waiters = this.#waiters.filter((w) => w !== waiter);
        resolve(false);
      }, timeoutMs);
      this.#waiters.push(waiter);
    });
  }

  #notify(): void {
    const ready = this.#waiters.filter((w) => this.requests.length >= w.count);
    this.#waiters = this.#waiters.filter((w) => !ready.includes(w));
    for (const waiter of ready) waiter.settle();
  }

  bodies(): ExportBody[] {
    return this.requests.map((request) => JSON.parse(request.body) as ExportBody);
  }

  spans(): JsonSpan[] {
    return this.bodies().flatMap((body) =>
      body.resourceSpans.flatMap((resourceSpans) => resourceSpans.scopeSpans.flatMap((scopeSpans) => scopeSpans.spans)),
    );
  }

  /** Everything that left, headers and bodies, for what must not be in it. */
  text(): string {
    return JSON.stringify(this.requests);
  }
}

/** A span's attributes as plain values; a count comes back as a number. */
export function attributesOf(span: JsonSpan): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const attribute of span.attributes) {
    const value = attribute.value;
    out[attribute.key] = 'intValue' in value ? Number(value['intValue']) : Object.values(value)[0];
  }
  return out;
}

export function runsIn(transport: FakeTransport): JsonSpan[] {
  return transport.spans().filter((span) => span.name.startsWith('invoke_agent '));
}

export function byName(transport: FakeTransport): Record<string, JsonSpan> {
  return Object.fromEntries(transport.spans().map((span) => [span.name, span]));
}

export function one<T>(items: T[]): T {
  expect(items).toHaveLength(1);
  return items[0] as T;
}

export interface Harness {
  telmoni: Telmoni;
  transport: FakeTransport;
  warnings: string[];
  /** Each warning's `Warning`, beside its line in `warnings`. */
  fields: Warning[];
  sleeps: number[];
}

/** A client over a fake transport, its warnings and sleeps recorded. */
export function harness(config: Config = {}, hooks: Hooks = {}, scripted: Array<Answer | Error> = []): Harness {
  const transport = new FakeTransport(scripted);
  const warnings: string[] = [];
  const fields: Warning[] = [];
  const sleeps: number[] = [];
  const telmoni = new Telmoni(
    { apiKey: 'telmoni_test_key', ...config },
    {
      transport: transport.send,
      sleep: (seconds) => {
        sleeps.push(seconds);
        return Promise.resolve();
      },
      warn: (message, warning) => {
        warnings.push(message);
        fields.push(warning);
      },
      ...hooks,
    },
  );
  return { telmoni, transport, warnings, fields, sleeps };
}

/**
 * What OpenTelemetry's Node SDK registers: the active context follows the
 * asynchronous flow, so `context.with` reaches callbacks and awaits.
 */
export class AsyncLocalStorageContextManager implements ContextManager {
  readonly #storage = new AsyncLocalStorage<Context>();

  active(): Context {
    return this.#storage.getStore() ?? ROOT_CONTEXT;
  }

  with<A extends unknown[], F extends (...args: A) => ReturnType<F>>(
    ctx: Context,
    fn: F,
    thisArg?: ThisParameterType<F>,
    ...args: A
  ): ReturnType<F> {
    return this.#storage.run(ctx, () => fn.call(thisArg, ...args) as ReturnType<F>);
  }

  bind<T>(ctx: Context, target: T): T {
    if (typeof target !== 'function') return target;
    const manager = this;
    return function (this: unknown, ...args: unknown[]): unknown {
      return manager.with(ctx, () => (target as (...inner: unknown[]) => unknown).apply(this, args));
    } as unknown as T;
  }

  enable(): this {
    return this;
  }

  disable(): this {
    this.#storage.disable();
    return this;
  }
}
