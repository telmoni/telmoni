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
export const CANCELLED = 'telmoni.cancelled';
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
export const REQUEST_REASONING_LEVEL = 'gen_ai.request.reasoning.level';
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
/** The key a status message is handed to the mask by. */
export const STATUS_DESCRIPTION = 'otel.status_description';
/** A failure, as the conventions' `exception` event carries it: content. */
export const EXCEPTION_EVENT = 'exception';
export const EXCEPTION_TYPE = 'exception.type';
export const EXCEPTION_MESSAGE = 'exception.message';
export const EXCEPTION_STACKTRACE = 'exception.stacktrace';

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
  EXCEPTION_TYPE,
  EXCEPTION_MESSAGE,
  EXCEPTION_STACKTRACE,
  STATUS_DESCRIPTION,
  'telmoni.events',
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

/**
 * The content fields whose messages' `blob` parts the exporter lifts out: each
 * image or file put to `MEDIA_PATH` and sent as a `file` part naming it.
 */
export const MEDIA_FIELDS: readonly string[] = [INPUT_MESSAGES, OUTPUT_MESSAGES, SYSTEM_INSTRUCTIONS];
/** Where an image or file is put, by the lowercase hex SHA-256 of its bytes. */
export const MEDIA_PATH = '/v1/media/';
/** A `file` part's `file_id` naming a file put to `MEDIA_PATH`: this, then its SHA-256. */
export const FILE_ID_PREFIX = 'telmoni:';
/** The largest file the platform keeps: one past it is named but never put. */
export const MEDIA_MAX_BYTES = 5 * 1024 * 1024;
/** A file's `Content-Type` where its part names no type a header can carry. */
export const MEDIA_DEFAULT_TYPE = 'application/octet-stream';

/** A score on a span still open: OpenTelemetry's evaluation event, Telmoni's beside it. */
export const EVALUATION_EVENT = 'gen_ai.evaluation.result';
export const EVALUATION_NAME = 'gen_ai.evaluation.name';
export const EVALUATION_VALUE = 'gen_ai.evaluation.score.value';
export const EVALUATION_LABEL = 'gen_ai.evaluation.score.label';
export const SCORE_TYPE = 'telmoni.score.type';
export const SCORE_RANGE_MIN = 'telmoni.score.range_min';
export const SCORE_RANGE_MAX = 'telmoni.score.range_max';
export const SCORE_VERSION = 'telmoni.score.version';
/** Where a score of what has ended is sent, a body of up to `SCORES_SENT_MAX`. */
export const SCORES_PATH = '/v1/scores';
/** Never sent as a span: one score bound for `SCORES_PATH`, through the export queue, its item's JSON in `SCORE_SENT`. */
export const SCORE_SPAN = 'telmoni.score';
export const SCORE_SENT = 'telmoni.score.sent';
/** A score's longest name, category and version, in characters. */
export const SCORE_NAME_MAX = 64;
export const SCORE_CATEGORY_MAX = 64;
export const SCORE_VERSION_MAX = 64;
/** The most scores one span carries, so they never take the exception event's room among its 32. */
export const SCORES_PER_SPAN = 16;
export const SCORES_SENT_MAX = 100;

/** The variable `Telmoni.fromEnv` reads `captureContent` from: `1` turns it on, and anything else, or nothing, leaves it off. */
export const CAPTURE_CONTENT_ENV = 'TELMONI_CAPTURE_CONTENT';

/** The answer's header naming the project's content mode: off or on. */
export const CONTENT_MODE_HEADER = 'Telmoni-Content-Mode';
/**
 * The request's header naming this clock as the export leaves, in nanoseconds
 * since the epoch, by which ingest moves a run from a clock a zone or a date out.
 */
export const SENT_AT_HEADER = 'Telmoni-Sent-At';
