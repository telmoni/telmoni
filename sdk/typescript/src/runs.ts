/**
 * Runs, steps, model calls and tool calls, as callbacks on handles.
 *
 * A run is a span of its own in whatever trace is current, opened by
 * `telmoni.run.start`; every span inside it carries the run's id, agent,
 * user and conversation, so ingest knows each span's run without waiting
 * for the trace's tree. Steps and calls open on a run's handle, or a step's,
 * so nothing can be opened — or sent — outside a run.
 */

import { randomUUID } from 'node:crypto';

import { SpanKind, SpanStatusCode, context, createContextKey, trace } from '@opentelemetry/api';
import type { Attributes, Context, HrTime, Span, SpanOptions, Tracer } from '@opentelemetry/api';
import type { ReadableSpan } from '@opentelemetry/sdk-trace-base';

import * as contract from './contract.js';
import { errorType, isAbort } from './errors.js';
import type { Processor } from './processor.js';
import type {
  Bounds,
  ModelAnswer,
  ModelCallOptions,
  ResolvedConfig,
  RunOptions,
  ToolCallOptions,
} from './types.js';

const RUN_KEY = createContextKey('telmoni.run');
const END: unique symbol = Symbol('telmoni.end');

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
  #ended = false;

  constructor(span: Span, parent: Context, run: Run | undefined) {
    this.span = span;
    // A run is found from any context inside it; a run's own handle is the run.
    this.context = trace.setSpan(parent, span).setValue(RUN_KEY, run ?? this);
  }

  /** Mark it failed, with the error's class and never its message. */
  fail(errorType: string): void {
    this.span.setAttribute(contract.ERROR_TYPE, errorType);
    this.span.setStatus({ code: SpanStatusCode.ERROR });
  }

  /** Mark it cancelled by the agent, which is neither ok nor an error. */
  cancel(): void {
    this.span.setAttribute(contract.CANCELLED, true);
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
    super(span, parent, run);
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
  readonly #inherited: Attributes;

  constructor(
    opener: Opener,
    span: Span,
    parent: Context,
    runId: string,
    agent: string,
    inherited: Attributes,
  ) {
    super(opener, span, parent, undefined);
    this.runId = runId;
    this.agent = agent;
    this.#inherited = { ...inherited };
  }

  /** What every span inside the run carries. */
  get inherited(): Attributes {
    return { ...this.#inherited };
  }
}

export class Step extends Parent {}

export class ModelCall extends Handle {
  /** What the model answered: its name, its tokens, and its messages. */
  answered(answer: ModelAnswer): void {
    if (answer.responseModel !== undefined) {
      this.span.setAttribute(contract.RESPONSE_MODEL, answer.responseModel);
    }
    const usage = answer.usage;
    if (usage !== undefined) {
      for (const [key, value] of [
        [contract.INPUT_TOKENS, usage.inputTokens],
        [contract.OUTPUT_TOKENS, usage.outputTokens],
        [contract.CACHE_READ_INPUT_TOKENS, usage.cacheReadInputTokens],
        [contract.CACHE_WRITE_INPUT_TOKENS, usage.cacheWriteInputTokens],
      ] as const) {
        if (value !== undefined) this.span.setAttribute(key, Math.trunc(value));
      }
    }
    if (answer.outputMessages !== undefined) {
      this.span.setAttribute(contract.OUTPUT_MESSAGES, asJson(answer.outputMessages));
    }
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

/** Builds the handles for one client; the client holds one. */
export class Opener {
  readonly #tracer: Tracer;
  readonly #processor: Processor;
  readonly #config: ResolvedConfig;

  constructor(tracer: Tracer, processor: Processor, config: ResolvedConfig) {
    this.#tracer = tracer;
    this.#processor = processor;
    this.#config = config;
  }

  run(agent: string, options: RunOptions): Run {
    if (agent.trim() === '') throw new TypeError("a run needs its agent's name");
    const rate = options.sampleRate ?? this.#config.sampleRate;
    if (!(rate > 0 && rate <= 1)) throw new RangeError('sampleRate must be above 0 and up to 1');
    const runId = randomUUID();
    const inherited: Attributes = { [contract.RUN_ID]: runId, [contract.AGENT]: agent };
    if (options.userId) inherited[contract.USER_ID] = options.userId;
    if (options.conversationId) inherited[contract.CONVERSATION_ID] = options.conversationId;
    const bounds = boundsOf(options.bounds);
    const attributes: Attributes = {
      [contract.OPERATION]: 'invoke_agent',
      [contract.AGENT_NAME]: agent,
      ...inherited,
      ...bounds,
      [contract.SAMPLE_RATE]: rate,
    };
    for (const [key, value] of Object.entries(options.tags ?? {})) {
      attributes[`${contract.TAG_PREFIX}${key}`] = value;
    }
    const parent = context.active();
    const span = this.#start(`invoke_agent ${agent}`, parent, attributes, SpanKind.INTERNAL);
    // Decided once, here, and applied in the processor: never through the
    // trace's sampled flag, which is the application's.
    if (rate < 1 && Math.random() >= rate) this.#processor.sampleOut(runId);
    const run = new Run(this, span, parent, runId, agent, inherited);
    // OpenTelemetry sends a span only once it ends; without this one a run
    // stalled in its first call would never be heard.
    const start = startTimeOf(span);
    const opening: SpanOptions = {
      kind: SpanKind.INTERNAL,
      attributes: { ...inherited, ...bounds, [contract.SAMPLE_RATE]: rate },
    };
    if (start !== undefined) opening.startTime = start;
    this.#tracer.startSpan(contract.RUN_START, opening, run.context).end(start);
    return run;
  }

  step(run: Run, parent: Context, name: string): Step {
    const span = this.#start(name, parent, run.inherited, SpanKind.INTERNAL);
    return new Step(this, span, parent, run);
  }

  modelCall(run: Run, parent: Context, model: string, options: ModelCallOptions): ModelCall {
    // Sent blank, the span would be named ` <model>` and ingest would read it
    // as a step.
    const operation =
      typeof options.operation === 'string' && options.operation.trim() !== '' ? options.operation : 'chat';
    const attributes: Attributes = {
      ...run.inherited,
      [contract.OPERATION]: operation,
      [contract.REQUEST_MODEL]: model,
    };
    if (options.provider) attributes[contract.PROVIDER] = options.provider;
    if (isSet(options.inputMessages)) {
      attributes[contract.INPUT_MESSAGES] = asJson(options.inputMessages);
    }
    if (isSet(options.systemInstructions)) {
      attributes[contract.SYSTEM_INSTRUCTIONS] = asJson(options.systemInstructions);
    }
    const span = this.#start(`${operation} ${model}`, parent, attributes, SpanKind.CLIENT);
    return new ModelCall(span, parent, run);
  }

  toolCall(run: Run, parent: Context, name: string, options: ToolCallOptions): ToolCall {
    const attributes: Attributes = {
      ...run.inherited,
      [contract.OPERATION]: contract.TOOL_OPERATION,
      [contract.TOOL_NAME]: name,
    };
    if (isSet(options.arguments)) {
      attributes[contract.TOOL_CALL_ARGUMENTS] = asJson(options.arguments);
    }
    const span = this.#start(`execute_tool ${name}`, parent, attributes, SpanKind.INTERNAL);
    return new ToolCall(span, parent, run);
  }

  #start(name: string, parent: Context, attributes: Attributes, kind: SpanKind): Span {
    return this.#tracer.startSpan(name, { kind, attributes }, parent);
  }
}

function boundsOf(bounds: Bounds | undefined): Attributes {
  const out: Attributes = {};
  if (bounds === undefined) return out;
  if (isSet(bounds.stallAfterSeconds)) out[contract.BOUND_STALL_AFTER_S] = Math.trunc(bounds.stallAfterSeconds);
  if (isSet(bounds.maxSteps)) out[contract.BOUND_MAX_STEPS] = Math.trunc(bounds.maxSteps);
  if (isSet(bounds.maxCostUsd)) out[contract.BOUND_MAX_COST_USD] = bounds.maxCostUsd;
  return out;
}

/** The SDK's span carries its start; the api's type does not say so. */
function startTimeOf(span: Span): HrTime | undefined {
  const start = (span as Partial<ReadableSpan>).startTime;
  return Array.isArray(start) ? start : undefined;
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
