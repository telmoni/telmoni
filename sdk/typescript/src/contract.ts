/**
 * The ingest contract's names, as the core fixes them.
 *
 * The source is `crates/telemetry/src/contract.rs` in `telmoni/telmoni`: the
 * GenAI semantic conventions' names at the commit it pins, and ours under
 * `telmoni.*`. A name changes there first, then here.
 */

// ── Ours ─────────────────────────────────────────────────────────────────────

/** The instrumentation scope every span the SDK opens is under. */
export const SCOPE = 'telmoni';
export const DISTRO_NAME = 'telemetry.distro.name';
export const DISTRO = 'telmoni';
export const RUN_ID = 'telmoni.run_id';
export const AGENT = 'telmoni.agent';
export const USER_ID = 'user.id';
export const TAG_PREFIX = 'telmoni.tag.';
/** The most tags a run carries: ingest keeps no more (its `TAGS_MAX`). */
export const TAGS_MAX = 32;
/** A tag's longest key and value, in characters: ingest drops a tag past either. */
export const TAG_KEY_MAX = 64;
export const TAG_VALUE_MAX = 256;
/**
 * Ingest's longest text of each, in characters: it refuses a span past one. A
 * name holds a model call's `<operation> <model>`, a model at its cap.
 */
export const NAME_MAX = 145;
export const AGENT_MAX = 128;
export const ERROR_TYPE_MAX = 128;
export const MODEL_MAX = 128;
export const PROVIDER_MAX = 128;
export const USER_ID_MAX = 128;
export const SESSION_ID_MAX = 200;
export const BOUND_STALL_AFTER_S = 'telmoni.bound.stall_after_s';
export const BOUND_MAX_STEPS = 'telmoni.bound.max_steps';
export const BOUND_MAX_COST_USD = 'telmoni.bound.max_cost_usd';
export const SAMPLE_RATE = 'telmoni.sample_rate';
export const CANCELLED = 'telmoni.cancelled';
/** Never sent: on every span of a run the sample rate kept out, from its start, so the processor drops each however late it ends. */
export const SAMPLED_OUT = 'telmoni.sampled_out';
/** The span of no duration that opens a run, sent at once. */
export const RUN_START = 'telmoni.run.start';
/** The deployment, on the resource: the client's `environment` and `release`. */
export const ENVIRONMENT = 'deployment.environment.name';
export const RELEASE = 'service.version';
/** The W3C headers a run crosses services in: the span's context, and the run's id and agent as baggage members. */
export const TRACEPARENT = 'traceparent';
export const BAGGAGE = 'baggage';

// ── The conventions' ─────────────────────────────────────────────────────────

export const OPERATION = 'gen_ai.operation.name';
export const AGENT_NAME = 'gen_ai.agent.name';
export const TOOL_NAME = 'gen_ai.tool.name';
export const ERROR_TYPE = 'error.type';
export const RESPONSE_MODEL = 'gen_ai.response.model';
export const REQUEST_MODEL = 'gen_ai.request.model';
export const PROVIDER = 'gen_ai.provider.name';
export const INPUT_TOKENS = 'gen_ai.usage.input_tokens';
export const OUTPUT_TOKENS = 'gen_ai.usage.output_tokens';
export const CACHE_READ_INPUT_TOKENS = 'gen_ai.usage.cache_read.input_tokens';
export const CACHE_WRITE_INPUT_TOKENS = 'gen_ai.usage.cache_write.input_tokens';
export const SESSION_ID = 'gen_ai.conversation.id';

export const INPUT_MESSAGES = 'gen_ai.input.messages';
export const OUTPUT_MESSAGES = 'gen_ai.output.messages';
export const SYSTEM_INSTRUCTIONS = 'gen_ai.system_instructions';
export const TOOL_CALL_ARGUMENTS = 'gen_ai.tool.call.arguments';
export const TOOL_CALL_RESULT = 'gen_ai.tool.call.result';

/** What the exporter takes off a span unless asked and the project allows. */
export const CONTENT: ReadonlySet<string> = new Set([
  INPUT_MESSAGES,
  OUTPUT_MESSAGES,
  SYSTEM_INSTRUCTIONS,
  'gen_ai.tool.definitions',
  TOOL_CALL_ARGUMENTS,
  TOOL_CALL_RESULT,
  'gen_ai.retrieval.query.text',
  'gen_ai.retrieval.documents',
  'gen_ai.memory.query.text',
  'gen_ai.memory.records',
  'gen_ai.prompt.variable',
  'user.email',
  'user.full_name',
  'user.name',
]);

export const MODEL_OPERATIONS: ReadonlySet<string> = new Set([
  'chat',
  'generate_content',
  'text_completion',
  'embeddings',
]);
export const TOOL_OPERATION = 'execute_tool';

/** The answer's header naming the project's content mode: off, on or sealed. */
export const CONTENT_MODE_HEADER = 'Telmoni-Content-Mode';
/**
 * The request's header naming this clock as the export leaves, in nanoseconds
 * since the epoch, by which ingest moves a run from a clock a zone or a date out.
 */
export const SENT_AT_HEADER = 'Telmoni-Sent-At';
