/**
 * Runs, steps, model calls and tool calls, as callbacks on handles.
 *
 * A run is a span of its own in whatever trace is current; every span inside
 * it carries the run's id, agent, user and session, so ingest knows each
 * span's run without waiting for the trace's tree. Steps and calls open on a
 * run's handle, or a step's, so nothing can be opened — or sent — outside a run.
 */

import { randomUUID } from 'node:crypto';

import { ROOT_CONTEXT, SpanKind, SpanStatusCode, context, createContextKey, trace } from '@opentelemetry/api';
import type { Attributes, Context, HrTime, Span, SpanContext, Tracer } from '@opentelemetry/api';

import * as contract from './contract.js';
import { errorType, isAbort } from './errors.js';
import { Guard, type OwnRates, type Tokens, ownRates, priced } from './guard.js';
import { type Ids, spanIdAt } from './ids.js';
import type { Processor } from './processor.js';
import type {
  CallRecord,
  ModelAnswer,
  ModelCallOptions,
  RecordOutcome,
  RecordTime,
  ResolvedConfig,
  RunOptions,
  RunRecord,
  StepRecord,
  ToolCallOptions,
  Usage,
} from './types.js';

const RUN_KEY = createContextKey('telmoni.run');
const END: unique symbol = Symbol('telmoni.end');
const GUARD: unique symbol = Symbol('telmoni.guard');

type Outcome = { failed: false } | { failed: true; error: unknown };

/**
 * The run open in `ctx`, if any; the active context by default, which an
 * application running OpenTelemetry's context manager carries into callbacks.
 */
export function currentRun(ctx: Context = context.active()): Run | undefined {
  const run = ctx.getValue(RUN_KEY);
  return run instanceof Run ? run : undefined;
}

/** A span the SDK opened, until its callback settles. */
export class Handle {
  /**
   * The OpenTelemetry context with this span current: for an application
   * without a context manager to nest its own spans under ours explicitly.
   */
  readonly context: Context;
  protected readonly span: Span;
  readonly #warnOnce: WarnOnce;
  #ended = false;

  constructor(span: Span, parent: Context, run: Run | undefined, warnOnce: WarnOnce) {
    this.span = span;
    this.#warnOnce = warnOnce;
    // A run is found from any context inside it; a run's own handle is the run.
    this.context = trace.setSpan(parent, span).setValue(RUN_KEY, run ?? this);
  }

  /** Mark it failed, with the error's class and never its message. */
  fail(errorType: string): void {
    const given: unknown = errorType;
    const text = given ? typeOfError(given, this.#warnOnce) : '';
    // Ingest refuses the span over an empty one, and with a run's, the run.
    if (text !== '') {
      this.span.setAttribute(contract.ERROR_TYPE, cut(text, contract.ERROR_TYPE_MAX, "span's error type", this.#warnOnce));
    }
    this.span.setStatus({ code: SpanStatusCode.ERROR });
  }

  /** Mark it cancelled by the agent, which is neither ok nor an error. */
  cancel(): void {
    this.span.setAttribute(contract.CANCELLED, true);
  }

  /** Whether its span has ended. */
  protected get ended(): boolean {
    return this.#ended;
  }

  [END](outcome: Outcome): void {
    if (this.#ended) return;
    this.#ended = true;
    if (outcome.failed) {
      if (isAbort(outcome.error)) this.cancel();
      else this.fail(errorType(outcome.error));
    }
    this.span.end();
  }
}

/** A handle steps and calls open under: a run, or a step. */
export abstract class Parent extends Handle {
  readonly #opener: Opener;
  readonly #run: Run;

  constructor(opener: Opener, span: Span, parent: Context, run: Run | undefined) {
    super(span, parent, run, opener.warnOnce);
    this.#opener = opener;
    // Only a Run passes no run: it is its own.
    this.#run = run ?? (this as unknown as Run);
  }

  step<R>(name: string, fn: (step: Step) => R): R {
    return within(this.#opener.step(this.#run, this.context, name), fn);
  }

  modelCall<R>(model: string, fn: (call: ModelCall) => R): R;
  modelCall<R>(model: string, options: ModelCallOptions, fn: (call: ModelCall) => R): R;
  modelCall<R>(
    model: string,
    a: ModelCallOptions | ((call: ModelCall) => R),
    b?: (call: ModelCall) => R,
  ): R {
    const [options, fn] = split<ModelCallOptions, (call: ModelCall) => R>(a, b);
    return within(this.#opener.modelCall(this.#run, this.context, model, options ?? {}), fn);
  }

  toolCall<R>(name: string, fn: (tool: ToolCall) => R): R;
  toolCall<R>(name: string, options: ToolCallOptions, fn: (tool: ToolCall) => R): R;
  toolCall<R>(name: string, a: ToolCallOptions | ((tool: ToolCall) => R), b?: (tool: ToolCall) => R): R {
    const [options, fn] = split<ToolCallOptions, (tool: ToolCall) => R>(a, b);
    return within(this.#opener.toolCall(this.#run, this.context, name, options ?? {}), fn);
  }
}

export class Run extends Parent {
  readonly runId: string;
  readonly agent: string;
  /** Whether the sample rate kept this run out: nothing of it is sent, nor handed to another service. */
  readonly sampledOut: boolean;
  readonly #inherited: Attributes;
  readonly [GUARD]: Guard;

  constructor(
    opener: Opener,
    span: Span,
    parent: Context,
    runId: string,
    agent: string,
    inherited: Attributes,
    sampledOut = false,
    guard = new Guard(),
  ) {
    super(opener, span, parent, undefined);
    this.runId = runId;
    this.agent = agent;
    this.sampledOut = sampledOut;
    this.#inherited = { ...inherited };
    this[GUARD] = guard;
  }

  /** What every span inside the run carries. */
  get inherited(): Attributes {
    return { ...this.#inherited };
  }

  /** What the run's model calls have cost so far in US dollars, counted as its spend cap counts them, in this process. */
  get spent(): number {
    return this[GUARD].spent;
  }
}

export class Step extends Parent {}

/** What a model call is priced by: its name as sent, and the project's own prices. */
interface Priced {
  readonly provider: string;
  readonly model: string;
  readonly prices: OwnRates;
}

export class ModelCall extends Handle {
  readonly #warnOnce: WarnOnce;
  readonly #run: Run;
  readonly #provider: string;
  readonly #prices: OwnRates;
  #model: string;
  /** Its usage as its span holds it, each attribute as last sent. */
  #held: Attributes = {};
  #counted = 0;

  constructor(span: Span, parent: Context, run: Run, warnOnce: WarnOnce, by: Priced) {
    super(span, parent, run, warnOnce);
    this.#warnOnce = warnOnce;
    this.#run = run;
    this.#provider = by.provider;
    this.#model = by.model;
    this.#prices = by.prices;
  }

  /**
   * What the model answered: its name, its tokens, and its messages. An answer
   * given after the call ended is not sent, and counts nothing.
   */
  answered(answer: ModelAnswer): void {
    const named = filled(answer.responseModel, 'response model', this.#warnOnce);
    const model = named === undefined ? undefined : cut(named, contract.MODEL_MAX, 'response model', this.#warnOnce);
    const sent = usageOf(answer.usage, this.#warnOnce);
    if (this.ended) {
      this.#warnOnce(
        'call:late',
        'telmoni: a model call must be answered before it ends; an answer given after is not sent ' +
          "and counts nothing toward the run's spend",
      );
      return;
    }
    if (model !== undefined) {
      // The name the call is priced by, as ingest reads it.
      this.#model = model;
      this.span.setAttribute(contract.RESPONSE_MODEL, model);
    }
    this.span.setAttributes(sent);
    this.#held = { ...this.#held, ...sent };
    if (answer.outputMessages !== undefined) {
      this.span.setAttribute(contract.OUTPUT_MESSAGES, asJson(answer.outputMessages));
    }
    this.#count();
  }

  /** Its cost to its run's spend, in place of what it counted before: reported, else priced, else nothing. */
  #count(): void {
    const held = this.#held;
    const count = (key: string): number => {
      const value = held[key];
      return typeof value === 'number' ? value : 0;
    };
    const tokens: Tokens = {
      input: count(contract.INPUT_TOKENS),
      output: count(contract.OUTPUT_TOKENS),
      cacheRead: count(contract.CACHE_READ_INPUT_TOKENS),
      cacheWrite: count(contract.CACHE_WRITE_INPUT_TOKENS),
    };
    const reported = held[contract.COST];
    let cost = typeof reported === 'number' ? reported : priced(this.#prices, this.#provider, this.#model, tokens);
    const guard = this.#run[GUARD];
    if (cost === undefined) {
      cost = 0;
      if (guard.capped && Object.values(tokens).some((n) => n > 0)) {
        this.#warnOnce(
          `unpriced:${this.#provider}:${this.#model}`,
          `telmoni: no price for ${this.#model || 'a call with no model'} (${this.#provider || 'no provider'}), ` +
            "which reported no cost; it counts nothing toward the run's spend cap",
        );
      }
    }
    guard.counts(this.#counted, cost);
    this.#counted = cost;
  }
}

export class ToolCall extends Handle {
  /** What the tool returned: content, sent only when asked and allowed. */
  returned(result: unknown): void {
    this.span.setAttribute(contract.TOOL_CALL_RESULT, asJson(result));
  }
}

/**
 * Runs `fn` with the handle's span current and ends the span when `fn`
 * settles, synchronously or once its promise does: an error thrown or
 * rejected marks the span with its class and is rethrown.
 */
export function within<H extends Handle, R>(handle: H, fn: (handle: H) => R): R {
  let result: R;
  try {
    result = context.with(handle.context, fn, undefined, handle);
  } catch (error) {
    handle[END]({ failed: true, error });
    throw error;
  }
  if (isPromiseLike(result)) {
    return result.then(
      (value) => {
        handle[END]({ failed: false });
        return value;
      },
      (error: unknown) => {
        handle[END]({ failed: true, error });
        throw error;
      },
    ) as unknown as R;
  }
  handle[END]({ failed: false });
  return result;
}

/** `(name, fn)` or `(name, options, fn)`: the options may be left out. */
export function split<O, F>(a: O | F, b: F | undefined): [O | undefined, F] {
  if (typeof a === 'function') return [undefined, a as F];
  if (b === undefined) throw new TypeError('a callback is needed');
  return [a as O, b];
}

/** The client's warning, once per key. */
type WarnOnce = (key: string, text: string) => void;

/** Builds the handles for one client; the client holds one. */
export class Opener {
  readonly #tracer: Tracer;
  readonly #processor: Processor;
  readonly #config: ResolvedConfig;
  readonly #ids: Ids;
  readonly #warnOnce: WarnOnce;
  /** The project's own prices, which the guard counts a call by first. */
  readonly #prices: OwnRates;

  constructor(tracer: Tracer, processor: Processor, config: ResolvedConfig, ids: Ids, warnOnce: WarnOnce) {
    this.#tracer = tracer;
    this.#processor = processor;
    this.#config = config;
    this.#ids = ids;
    this.#warnOnce = warnOnce;
    this.#prices = ownRates(config.prices);
  }

  /** The client's warning, once per key, which every handle gives. */
  get warnOnce(): WarnOnce {
    return this.#warnOnce;
  }

  run(given: string, options: RunOptions): Run {
    const named: unknown = given;
    if (!isSet(named) || textOf(named).trim() === '') {
      throw new TypeError("a run needs its agent's name");
    }
    const agent = cut(asText(named, "run's agent", this.#warnOnce), contract.AGENT_MAX, "run's agent", this.#warnOnce);
    const rate: unknown = options.sampleRate ?? this.#config.sampleRate;
    if (typeof rate !== 'number' || !(rate > 0 && rate <= 1)) {
      throw new RangeError('sampleRate must be a number above 0 and up to 1');
    }
    const guard = Guard.of(options);
    const sampledOut = rate < 1 && Math.random() >= rate;
    const runId = randomUUID();
    const inherited: Attributes = { [contract.RUN_ID]: runId, [contract.AGENT]: agent };
    const user = filled(options.userId, 'user id', this.#warnOnce);
    if (user !== undefined && fits(user, contract.USER_ID_MAX, 'user id', this.#warnOnce)) {
      inherited[contract.USER_ID] = user;
    }
    const session = filled(options.sessionId, 'session id', this.#warnOnce);
    if (session !== undefined && fits(session, contract.SESSION_ID_MAX, 'session id', this.#warnOnce)) {
      inherited[contract.SESSION_ID] = session;
    }
    // On every span of the run from its start, for the processor to drop: a
    // sampler's drop would clear the flag the application's spans inherit.
    if (sampledOut) inherited[contract.SAMPLED_OUT] = true;
    const attributes: Attributes = {
      [contract.OPERATION]: 'invoke_agent',
      [contract.AGENT_NAME]: agent,
      ...inherited,
      [contract.SAMPLE_RATE]: rate,
      ...tagsOf(Object.entries(options.tags ?? {}), this.#warnOnce),
    };
    const parent = context.active();
    const span = this.#start(`invoke_agent ${agent}`, parent, attributes, SpanKind.INTERNAL);
    return new Run(this, span, parent, runId, agent, inherited, sampledOut, guard);
  }

  /**
   * A run another service opened, continued here under the span the headers
   * named: a run whose own span is the caller's, non-recording here, so its
   * steps and calls nest under it and carry the run, and nothing of it is
   * sent or ended from this side.
   */
  continued(remote: SpanContext, runId: string, given: string): Run {
    const agent = cut(given, contract.AGENT_MAX, "run's agent", this.#warnOnce);
    const inherited: Attributes = { [contract.RUN_ID]: runId, [contract.AGENT]: agent };
    return new Run(this, trace.wrapSpanContext(remote), context.active(), runId, agent, inherited);
  }

  step(run: Run, parent: Context, given: string): Step {
    run[GUARD].opens(true);
    let name = asText(given, "step's name", this.#warnOnce);
    // Ingest refuses a span named nothing.
    if (name === '') {
      this.#warnOnce('step:name', 'telmoni: a step needs its name; one without is sent named step');
      name = 'step';
    }
    const span = this.#start(cut(name, contract.NAME_MAX, "step's name", this.#warnOnce), parent, run.inherited, SpanKind.INTERNAL);
    return new Step(this, span, parent, run);
  }

  modelCall(run: Run, parent: Context, given: string, options: ModelCallOptions): ModelCall {
    run[GUARD].opens(false);
    const asked = asText(given, 'model', this.#warnOnce);
    // Sent blank, the span would be named ` <model>` and ingest would read it
    // as a step.
    const operation =
      typeof options.operation === 'string' && options.operation.trim() !== '' ? options.operation : 'chat';
    const attributes: Attributes = { ...run.inherited, [contract.OPERATION]: operation };
    // Ingest refuses a call over an empty model, but keeps one with none.
    const model = asked === '' ? undefined : cut(asked, contract.MODEL_MAX, 'model', this.#warnOnce);
    if (model === undefined) {
      this.#warnOnce(
        'call:model',
        'telmoni: a model call needs its model; one without is sent named by its operation alone',
      );
    } else {
      attributes[contract.REQUEST_MODEL] = model;
    }
    const named = filled(options.provider, 'provider', this.#warnOnce);
    const provider = named === undefined ? '' : cut(named, contract.PROVIDER_MAX, 'provider', this.#warnOnce);
    if (provider !== '') attributes[contract.PROVIDER] = provider;
    if (isSet(options.inputMessages)) {
      attributes[contract.INPUT_MESSAGES] = asJson(options.inputMessages);
    }
    if (isSet(options.systemInstructions)) {
      attributes[contract.SYSTEM_INSTRUCTIONS] = asJson(options.systemInstructions);
    }
    const name = cut(
      model === undefined ? operation : `${operation} ${model}`,
      contract.NAME_MAX,
      "model call's name",
      this.#warnOnce,
    );
    const span = this.#start(name, parent, attributes, SpanKind.CLIENT);
    return new ModelCall(span, parent, run, this.#warnOnce, { provider, model: model ?? '', prices: this.#prices });
  }

  toolCall(run: Run, parent: Context, given: string, options: ToolCallOptions): ToolCall {
    run[GUARD].opens(false);
    const name = cut(asText(given, "tool's name", this.#warnOnce), contract.NAME_MAX, "tool's name", this.#warnOnce);
    const attributes: Attributes = {
      ...run.inherited,
      [contract.OPERATION]: contract.TOOL_OPERATION,
      [contract.TOOL_NAME]: name,
    };
    if (isSet(options.arguments)) {
      attributes[contract.TOOL_CALL_ARGUMENTS] = asJson(options.arguments);
    }
    const span = this.#start(`execute_tool ${name}`, parent, attributes, SpanKind.INTERNAL);
    return new ToolCall(span, parent, run, this.#warnOnce);
  }

  /**
   * A run that has ended, sent as the live calls would have sent it.
   *
   * Its trace is its own, named by the run's id, whatever context is active;
   * each span's id is derived from that id and the span's place in the record.
   * Its spans are queued whole once built, the run's own last, and travel in
   * exports of their own, so the same record sent again is the same bytes,
   * whatever else is sent meanwhile. Throws `QueueFullError`, queuing nothing,
   * when the export queue has no room for the whole record.
   */
  record(record: RunRecord): void {
    const agent: unknown = record.agent;
    if (typeof agent !== 'string' || agent.trim() === '') {
      throw new TypeError("a run needs its agent's name");
    }
    if ([...agent].length > contract.AGENT_MAX) {
      throw new RangeError(`a recorded run's agent must be up to ${contract.AGENT_MAX} characters`);
    }
    const runHex = uuidHex(record.runId);
    // Checked whole before anything is queued, so a record refused sends
    // nothing rather than half a run.
    check(record, 'run');
    // Read before the items, whose getters may change them.
    const tags = record.tags ?? {};
    const tagged = isObject(tags) ? Object.entries(tags) : undefined;
    if (tagged === undefined || tagged.some(([, value]) => typeof value !== 'string')) {
      throw new TypeError("a recorded run's tags must be an object of strings");
    }
    const items = read(record);
    for (const each of [record, ...items.flatMap(([item, calls]) => [item, ...calls])]) {
      const [start, end] = [hrTimeOf(each.start), hrTimeOf(each.end)];
      // Ingest reads a time as nanoseconds from the epoch in 64 bits, and
      // refuses the export over one that is not.
      if (!readable(start) || !readable(end)) {
        throw new RangeError('a recorded time must fall after the Unix epoch and less than 2^64 nanoseconds after it');
      }
      if (later(start, end)) throw new RangeError('a recorded span cannot end before it starts');
    }
    const rate = this.#config.sampleRate;
    const runId = [
      runHex.slice(0, 8),
      runHex.slice(8, 12),
      runHex.slice(12, 16),
      runHex.slice(16, 20),
      runHex.slice(20),
    ].join('-');
    const inherited: Attributes = { [contract.RUN_ID]: runId, [contract.AGENT]: record.agent };
    if (record.userId) inherited[contract.USER_ID] = record.userId;
    if (record.sessionId) inherited[contract.SESSION_ID] = record.sessionId;
    const attributes: Attributes = {
      [contract.OPERATION]: 'invoke_agent',
      [contract.AGENT_NAME]: record.agent,
      ...inherited,
      [contract.SAMPLE_RATE]: rate,
    };
    Object.assign(attributes, tagsOf(tagged, this.#warnOnce));
    // Sampled out, no span is built: none of a record's ends after it, and
    // the same record sent again draws again.
    if (rate < 1 && Math.random() >= rate) return;
    // Worked out before the record is built: a count that warns calls the
    // `warn` hook, which may record a run of its own.
    const usages = new Map<CallRecord, Attributes>();
    for (const [item, calls] of items) {
      for (const each of item.type === 'step' ? calls : [item]) {
        if (each.type === 'model_call') usages.set(each, usageOf(each.usage, this.#warnOnce));
      }
    }
    let place = 0;
    const open = (name: string, parent: Context, spanAttributes: Attributes, kind: SpanKind, at: RecordTime): Span =>
      this.#ids.given(runHex, spanIdAt(runHex, place++), () =>
        this.#tracer.startSpan(name, { kind, attributes: spanAttributes, startTime: hrTimeOf(at) }, parent),
      );
    const call = (item: CallRecord, parent: Context): void => {
      if (item.type === 'model_call') {
        const operation = typeof item.operation === 'string' && item.operation.trim() !== '' ? item.operation : 'chat';
        const callAttributes: Attributes = {
          ...inherited,
          [contract.OPERATION]: operation,
          [contract.REQUEST_MODEL]: item.model,
        };
        if (item.provider) callAttributes[contract.PROVIDER] = item.provider;
        Object.assign(callAttributes, usages.get(item));
        close(open(`${operation} ${item.model}`, parent, callAttributes, SpanKind.CLIENT, item.start), item);
      } else {
        const callAttributes: Attributes = {
          ...inherited,
          [contract.OPERATION]: contract.TOOL_OPERATION,
          [contract.TOOL_NAME]: item.name,
        };
        close(open(`execute_tool ${item.name}`, parent, callAttributes, SpanKind.INTERNAL, item.start), item);
      }
    };
    // Queued whole or not at all: a record that outgrows the queue's room
    // would lose its tail, the run's own span first.
    this.#processor.record(runHex, spansIn(items), () => {
      const span = open(`invoke_agent ${record.agent}`, ROOT_CONTEXT, attributes, SpanKind.INTERNAL, record.start);
      const ctx = trace.setSpan(ROOT_CONTEXT, span);
      for (const [item, calls] of items) {
        if (item.type === 'step') {
          const step = open(item.name, ctx, inherited, SpanKind.INTERNAL, item.start);
          const inner = trace.setSpan(ctx, step);
          for (const each of calls) call(each, inner);
          close(step, item);
        } else {
          call(item, ctx);
        }
      }
      close(span, record);
    });
  }

  #start(name: string, parent: Context, attributes: Attributes, kind: SpanKind): Span {
    return this.#tracer.startSpan(name, { kind, attributes }, parent);
  }
}

const UUID = /^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$/i;

/** A run id's 32 hex digits, lowercase: the trace's id. */
function uuidHex(runId: unknown): string {
  if (typeof runId !== 'string' || !UUID.test(runId) || ![32, 36].includes(runId.length)) {
    throw new TypeError("a recorded run's id must be a UUID");
  }
  const hex = runId.replaceAll('-', '').toLowerCase();
  // The run's id is its trace's, and a trace id of zeros is no trace at all.
  if (/^0+$/.test(hex)) throw new TypeError("a recorded run's id must be a UUID other than the nil UUID");
  return hex;
}

/**
 * A recorded time as `[seconds, nanoseconds]`, worked out here: a span handed
 * a number no larger than `performance.now()` reads it as a performance
 * reading rather than milliseconds since the epoch, while it takes an
 * `HrTime` as given.
 */
function hrTimeOf(time: RecordTime): HrTime {
  const ms = time instanceof Date ? time.getTime() : time;
  if (typeof ms !== 'number' || !Number.isFinite(ms)) {
    throw new TypeError('a recorded time must be a Date or milliseconds since the Unix epoch');
  }
  const seconds = Math.floor(ms / 1000);
  const nanos = Math.round((ms - seconds * 1000) * 1_000_000);
  return nanos >= 1_000_000_000 ? [seconds + 1, nanos - 1_000_000_000] : [seconds, nanos];
}

function later(a: HrTime, b: HrTime): boolean {
  return a[0] > b[0] || (a[0] === b[0] && a[1] > b[1]);
}

type Item = StepRecord | CallRecord;

/** How many spans a record queues: the run, then each step and each call. */
function spansIn(items: ReadonlyArray<readonly [Item, readonly CallRecord[]]>): number {
  return items.reduce((spans, [, calls]) => spans + 1 + calls.length, 1);
}

/** The record's items, each step with its calls, read once and checked, so the items checked are the items built. */
function read(record: RunRecord): Array<[Item, readonly CallRecord[]]> {
  const inside = record.inside ?? [];
  // Cast: narrowing the name itself by `Array.isArray` makes its items `any`.
  if (!Array.isArray(inside as unknown)) {
    throw new TypeError("a recorded run's inside must be an array, in the order it ran");
  }
  const items: Array<[Item, readonly CallRecord[]]> = [];
  for (const item of inside) {
    if (item?.type === 'model_call' || item?.type === 'tool_call') {
      check(item, item.type);
      items.push([item, []]);
      continue;
    }
    if (item?.type !== 'step') {
      throw new TypeError("a recorded item's type must be 'step', 'model_call' or 'tool_call'");
    }
    check(item, 'step');
    const given = item.calls ?? [];
    if (!Array.isArray(given as unknown)) {
      throw new TypeError("a recorded step's calls must be an array, in the order they ran");
    }
    const calls = [...given];
    for (const each of calls) {
      if (each?.type !== 'model_call' && each?.type !== 'tool_call') {
        throw new TypeError("a step's recorded call's type must be 'model_call' or 'tool_call'");
      }
      check(each, each.type);
    }
    items.push([item, calls]);
  }
  return items;
}

type Field = readonly [name: string, type: 'string' | 'boolean' | 'object', optional: boolean];

/**
 * Each recorded field's type, `null` or `undefined` too where optional:
 * OpenTelemetry would warn of another mid-build, or send it wrong.
 */
const FIELDS: Record<'run' | Item['type'], readonly Field[]> = {
  run: [
    ['userId', 'string', true],
    ['sessionId', 'string', true],
  ],
  step: [['name', 'string', false]],
  model_call: [
    ['model', 'string', false],
    ['provider', 'string', true],
    ['operation', 'string', true],
    ['usage', 'object', true],
  ],
  tool_call: [['name', 'string', false]],
};
const OUTCOME: readonly Field[] = [
  ['errorType', 'string', true],
  ['cancelled', 'boolean', true],
];

/** The longest each recorded text may be, in characters, as ingest caps it. */
const CAPS: Record<keyof typeof FIELDS, Readonly<Record<string, number>>> = {
  run: { userId: contract.USER_ID_MAX, sessionId: contract.SESSION_ID_MAX },
  step: { name: contract.NAME_MAX },
  model_call: { model: contract.MODEL_MAX, provider: contract.PROVIDER_MAX },
  tool_call: { name: contract.NAME_MAX },
};

function check(item: RunRecord | Item, kind: keyof typeof FIELDS): void {
  const what = kind.replace('_', ' ');
  const caps: Readonly<Record<string, number>> = { ...CAPS[kind], errorType: contract.ERROR_TYPE_MAX };
  const read: Record<string, unknown> = {};
  for (const [field, type, optional] of [...FIELDS[kind], ...OUTCOME]) {
    const value: unknown = (item as unknown as Record<string, unknown>)[field];
    read[field] = value;
    if (optional && (value === undefined || value === null)) continue;
    if (!(type === 'object' ? isObject(value) : typeof value === type)) {
      const article = type === 'object' ? 'an' : 'a';
      throw new TypeError(`a recorded ${what}'s ${field} must be ${article} ${type}`);
    }
    // Ingest refuses a step or a model named nothing; a tool's it reads as none.
    if (value === '' && ((kind === 'step' && field === 'name') || field === 'model')) {
      throw new TypeError(`a recorded ${what}'s ${field} cannot be empty`);
    }
    const most = caps[field];
    if (most !== undefined && typeof value === 'string' && [...value].length > most) {
      throw new RangeError(`a recorded ${what}'s ${field} must be up to ${most} characters`);
    }
  }
  // Its span is named `<operation> <model>`, a name ingest caps too.
  if (kind === 'model_call') {
    const operation = typeof read.operation === 'string' && read.operation.trim() !== '' ? read.operation : 'chat';
    if ([...operation].length + [...String(read.model)].length > contract.NAME_MAX - 1) {
      throw new RangeError(
        `a recorded model call's operation and model must be up to ${contract.NAME_MAX - 1} characters together`,
      );
    }
  }
}

/** A literal or a class's instance: an array, a `Map` or a boxed primitive is of `typeof` object too. */
function isObject(value: unknown): value is object {
  return Object.prototype.toString.call(value) === '[object Object]';
}

function close(span: Span, outcome: RecordOutcome & { end: RecordTime }): void {
  if (outcome.errorType) {
    span.setAttribute(contract.ERROR_TYPE, outcome.errorType);
    span.setStatus({ code: SpanStatusCode.ERROR });
  }
  if (outcome.cancelled) span.setAttribute(contract.CANCELLED, true);
  span.end(hrTimeOf(outcome.end));
}

/** `Usage`'s keys, by which a usage is told from another's. */
const USAGE_KEYS = [
  'inputTokens',
  'outputTokens',
  'cacheReadInputTokens',
  'cacheWriteInputTokens',
  'cost',
] as const satisfies ReadonlyArray<keyof Usage>;

/**
 * The counts as integers, which alone ingest takes; one JSON's integers cannot
 * carry would go as `intValue: "NaN"` and lose the batch, so it is left off,
 * warned once.
 */
function usageOf(usage: Usage | undefined, warnOnce: WarnOnce): Attributes {
  const out: Attributes = {};
  if (!isSet(usage)) return out;
  // A provider's own, OpenAI's or Anthropic's, would read as no counts without a word.
  const given: object = Object(usage);
  if (!USAGE_KEYS.some((key) => key in given)) {
    warnOnce(
      'usage:foreign',
      "telmoni: a usage must be telmoni's Usage, the provider's counts copied into it; the call is sent without it",
    );
    return out;
  }
  for (const [key, value] of [
    [contract.INPUT_TOKENS, usage.inputTokens],
    [contract.OUTPUT_TOKENS, usage.outputTokens],
    [contract.CACHE_READ_INPUT_TOKENS, usage.cacheReadInputTokens],
    [contract.CACHE_WRITE_INPUT_TOKENS, usage.cacheWriteInputTokens],
  ] as const) {
    if (!isSet(value)) continue;
    const count = whole(value, 0, 2 ** 63);
    if (count === undefined) {
      warnOnce(
        `usage:${key}`,
        `telmoni: ${key} must be a whole number from 0 within 64 bits; the call is sent without it`,
      );
    } else {
      out[key] = count;
    }
  }
  if (isSet(usage.cost)) {
    const given: unknown = usage.cost;
    // Text, which Python's SDK leaves off, is no cost either.
    const cost = typeof given === 'bigint' || given instanceof Number ? Number(given) : given;
    if (typeof cost === 'number' && Number.isFinite(cost) && cost >= 0) {
      out[contract.COST] = cost;
    } else {
      warnOnce(
        `usage:${contract.COST}`,
        `telmoni: ${contract.COST} must be a finite number of US dollars from 0; the call is sent without it`,
      );
    }
  }
  return out;
}

/** `value` truncated to a whole number from `least` and below `end`, as ingest takes one, or undefined. */
function whole(value: unknown, least: number, end: number): number | undefined {
  const number = Math.trunc(numberOf(value));
  return Number.isFinite(number) && number >= least && number < end ? number : undefined;
}

const DECIMAL = /^\s*[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?\s*$/i;

/**
 * A number, a bigint or a decimal numeral's text: `Number` would make 1 of
 * `true`, 0 of `''` and 16 of `'0x10'`, which Python's SDK leaves off.
 */
function numberOf(value: unknown): number {
  if (typeof value === 'number') return value;
  if (typeof value === 'bigint' || value instanceof Number) return Number(value);
  return typeof value === 'string' && DECIMAL.test(value) ? Number(value) : Number.NaN;
}

/**
 * A run's tags as its span's attributes: the first `TAGS_MAX` of those
 * ingest keeps, since past them they would crowd out the run's own.
 */
function tagsOf(given: ReadonlyArray<readonly [string, unknown]>, warnOnce: WarnOnce): Attributes {
  const kept: Attributes = {};
  let count = 0;
  for (const [key, value] of given) {
    if (!isSet(value)) continue;
    const sent = tagValue(value);
    const length = [...key].length;
    // Ingest drops it before counting, so it takes no place among the 32.
    if (length === 0 || length > contract.TAG_KEY_MAX || shown(sent) > contract.TAG_VALUE_MAX) {
      warnOnce(
        'run:tag',
        "telmoni: a tag's key must be 1 to 64 characters and its value up to 256; the tag is not sent",
      );
    } else if (count === contract.TAGS_MAX) {
      warnOnce('run:tags', 'telmoni: a run carries up to 32 tags, the most ingest keeps; the rest are not sent');
    } else {
      kept[`${contract.TAG_PREFIX}${key}`] = typeof sent === 'string' ? asText(value, 'tag', warnOnce) : sent;
      count += 1;
    }
  }
  return kept;
}

/**
 * A tag's value: a boolean or a finite number as it is, which ingest writes
 * as its own text, and anything else as its `String()`.
 */
function tagValue(value: unknown): string | number | boolean {
  if (typeof value === 'boolean' || (typeof value === 'number' && Number.isFinite(value))) return value;
  return textOf(value);
}

/**
 * The characters of ingest's text for a tag's value: a number's as Rust
 * writes it, every digit and no exponent, so 1e300 takes 301.
 */
function shown(value: string | number | boolean): number {
  if (typeof value !== 'number') return [...String(value)].length;
  const [mantissa, exponent] = Math.abs(value).toExponential().split('e') as [string, string];
  const digits = mantissa.replace('.', '').length;
  const point = Number(exponent) + 1;
  const length = point >= digits ? point : point > 0 ? digits + 1 : 2 - point + digits;
  return length + (value < 0 ? 1 : 0);
}

/**
 * `value` as text: one that is no string goes as its `String()` with one
 * warning, where a span named by it would lose its batch at ingest.
 */
function asText(value: unknown, what: string, warnOnce: WarnOnce): string {
  if (typeof value === 'string') return value;
  warnOnce(`text:${what}`, `telmoni: a ${what} that is no string is sent as its text`);
  return textOf(value);
}

/**
 * An error type that is no string: an error's class, or a class's name, never
 * its message, which is content; anything else as its text.
 */
function typeOfError(value: unknown, warnOnce: WarnOnce): string {
  if (typeof value === 'string') return value;
  if (typeof value === 'object' && value !== null) return errorType(value);
  if (typeof value === 'function' && value.name !== '') return value.name;
  return asText(value, "span's error type", warnOnce);
}

/** `value` up to ingest's cap, in characters, cut there with one warning, since ingest refuses a span past it. */
function cut(value: string, most: number, what: string, warnOnce: WarnOnce): string {
  // UTF-16's units are never fewer than its characters.
  if (value.length <= most) return value;
  const characters = [...value];
  if (characters.length <= most) return value;
  warnOnce(`cut:${what}`, `telmoni: a ${what} must be up to ${most} characters; it is sent cut to its first ${most}`);
  return characters.slice(0, most).join('');
}

/** Whether an id is within ingest's cap; one past it is left off with one warning, since cut it would name another. */
function fits(value: string, most: number, what: string, warnOnce: WarnOnce): boolean {
  if (value.length <= most || [...value].length <= most) return true;
  warnOnce(`long:${what}`, `telmoni: a ${what} must be up to ${most} characters; the run is sent without it`);
  return false;
}

/** `String(value)`, or its type where that throws, as a null-prototype object's does. */
function textOf(value: unknown): string {
  try {
    return String(value);
  } catch {
    return typeof value;
  }
}

/** `value` as `asText` reads it, or undefined where unset or empty, which ingest refuses. */
function filled(value: unknown, what: string, warnOnce: WarnOnce): string | undefined {
  const text = isSet(value) ? asText(value, what, warnOnce) : '';
  return text === '' ? undefined : text;
}

/** Whether ingest reads `time`: after the epoch, and within 64 bits of nanoseconds, in 2554. */
function readable([seconds, nanos]: HrTime): boolean {
  const after = seconds > 0 || (seconds === 0 && nanos > 0);
  const within = seconds < 18_446_744_073 || (seconds === 18_446_744_073 && nanos < 709_551_616);
  return after && within;
}

function asJson(value: unknown): string {
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value, (_key, item: unknown) => (typeof item === 'bigint' ? String(item) : item)) ?? String(value);
  } catch {
    // A cycle, or a value that refuses to serialize: its own text, so
    // content never throws into the agent.
    return String(value);
  }
}

function isSet<T>(value: T | null | undefined): value is T {
  return value !== null && value !== undefined;
}

function isPromiseLike(value: unknown): value is PromiseLike<unknown> {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as { then?: unknown }).then === 'function'
  );
}
