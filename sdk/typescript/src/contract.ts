/**
 * The ingest contract's names, as the core fixes them.
 *
 * The source is `crates/telemetry/src/contract.rs` in `telmoni/telmoni`: the
 * GenAI semantic conventions' names at the commit it pins, ours under
 * `telmoni.*`, and a call's cost under Langfuse's name. A name changes there
 * first, then here.
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
/** The largest body an export may be: ingest refuses one past it whole. */
export const BATCH_MAX_BYTES = 1 << 20;
/** The most spans an export may hold: ingest refuses one past it whole. */
export const BATCH_SPANS_MAX = 1000;
/** An attribute's longest key and value as text, in characters: ingest drops one past either. */
export const ATTRIBUTE_KEY_MAX = 128;
export const ATTRIBUTE_VALUE_MAX = 1024;
export const SAMPLE_RATE = 'telmoni.sample_rate';
export const CANCELLED = 'telmoni.cancelled';
/** Never sent: on every span of a run the sample rate kept out, from its start, so the processor drops each however late it ends. */
export const SAMPLED_OUT = 'telmoni.sampled_out';
/** The deployment, on the resource: the client's `environment` and `release`. */
export const ENVIRONMENT = 'deployment.environment.name';
export const RELEASE = 'service.version';
/** The deployment's longest environment and release, in characters: ingest reads a resource naming a longer one as naming none, without a word. */
export const ENVIRONMENT_MAX = 128;
export const RELEASE_MAX = 128;
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
export const REQUEST_TEMPERATURE = 'gen_ai.request.temperature';
export const REQUEST_TOP_P = 'gen_ai.request.top_p';
export const REQUEST_TOP_K = 'gen_ai.request.top_k';
export const REQUEST_MAX_TOKENS = 'gen_ai.request.max_tokens';
export const REQUEST_FREQUENCY_PENALTY = 'gen_ai.request.frequency_penalty';
export const REQUEST_PRESENCE_PENALTY = 'gen_ai.request.presence_penalty';
export const REQUEST_SEED = 'gen_ai.request.seed';
/** In seconds, from the request to the answer's first chunk. */
export const TIME_TO_FIRST_CHUNK = 'gen_ai.response.time_to_first_chunk';
export const INPUT_TOKENS = 'gen_ai.usage.input_tokens';
export const OUTPUT_TOKENS = 'gen_ai.usage.output_tokens';
export const CACHE_READ_INPUT_TOKENS = 'gen_ai.usage.cache_read.input_tokens';
export const CACHE_WRITE_INPUT_TOKENS = 'gen_ai.usage.cache_write.input_tokens';
/** What the call cost in US dollars, as reported: Langfuse's name, since the conventions have none. */
export const COST = 'gen_ai.usage.cost';
export const SESSION_ID = 'gen_ai.conversation.id';

export const INPUT_MESSAGES = 'gen_ai.input.messages';
export const OUTPUT_MESSAGES = 'gen_ai.output.messages';
export const SYSTEM_INSTRUCTIONS = 'gen_ai.system_instructions';
export const TOOL_CALL_ARGUMENTS = 'gen_ai.tool.call.arguments';
export const TOOL_CALL_RESULT = 'gen_ai.tool.call.result';

/** What the exporter takes off a span unless asked and the project allows. */
/** The content fields the exporter cut so a span fits an export, by name. */
export const CONTENT_CUT = 'telmoni.content_cut';

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
  CONTENT_CUT,
]);

export const MODEL_OPERATIONS: ReadonlySet<string> = new Set([
  'chat',
  'generate_content',
  'text_completion',
  'embeddings',
]);
/** A model call's operation when it names none. */
export const DEFAULT_MODEL_OPERATION = 'chat';
export const TOOL_OPERATION = 'execute_tool';
export const RUN_OPERATION = 'invoke_agent';

/** The answer's header naming the project's content mode: off or on. */
export const CONTENT_MODE_HEADER = 'Telmoni-Content-Mode';
/**
 * The request's header naming this clock as the export leaves, in nanoseconds
 * since the epoch, by which ingest moves a run from a clock a zone or a date out.
 */
export const SENT_AT_HEADER = 'Telmoni-Sent-At';
