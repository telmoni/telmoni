//! The ingest contract's names, as the core fixes them.
//!
//! The source is `crates/telemetry/src/contract.rs` in `telmoni/telmoni`: the
//! GenAI semantic conventions' names at the commit it pins, and ours under
//! `telmoni.*`. A name changes there first, then here.

// ── Ours ─────────────────────────────────────────────────────────────────────

/// The instrumentation scope every span the SDK opens is under.
pub(crate) const SCOPE: &str = "telmoni";
pub(crate) const DISTRO_NAME: &str = "telemetry.distro.name";
pub(crate) const DISTRO: &str = "telmoni";
pub(crate) const RUN_ID: &str = "telmoni.run_id";
pub(crate) const AGENT: &str = "telmoni.agent";
pub(crate) const USER_ID: &str = "user.id";
pub(crate) const TAG_PREFIX: &str = "telmoni.tag.";
/// The most tags a span carries: ingest's `TAGS_MAX`.
pub(crate) const TAGS_MAX: usize = 32;
/// The most characters a tag's key holds: ingest's `TAG_KEY_MAX`.
pub(crate) const TAG_KEY_MAX: usize = 64;
/// The most characters a tag's value holds: ingest's `TAG_VALUE_MAX`.
pub(crate) const TAG_VALUE_MAX: usize = 256;
pub(crate) const BOUND_STALL_AFTER_S: &str = "telmoni.bound.stall_after_s";
pub(crate) const BOUND_MAX_STEPS: &str = "telmoni.bound.max_steps";
pub(crate) const BOUND_MAX_COST_USD: &str = "telmoni.bound.max_cost_usd";
pub(crate) const SAMPLE_RATE: &str = "telmoni.sample_rate";
pub(crate) const CANCELLED: &str = "telmoni.cancelled";
/// Never sent: on every span of a run the sample rate kept out, from its
/// start, so the processor drops each however late it ends.
pub(crate) const SAMPLED_OUT: &str = "telmoni.sampled_out";
/// The span of no duration that opens a run, sent at once.
pub(crate) const RUN_START: &str = "telmoni.run.start";
/// The deployment, on the resource: the configuration's `environment` and
/// `release`.
pub(crate) const ENVIRONMENT: &str = "deployment.environment.name";
pub(crate) const RELEASE: &str = "service.version";
/// The W3C headers a run crosses services in: the span's context, and the
/// run's id and agent as baggage members.
pub(crate) const TRACEPARENT: &str = "traceparent";
pub(crate) const BAGGAGE: &str = "baggage";

// ── The conventions' ─────────────────────────────────────────────────────────

pub(crate) const OPERATION: &str = "gen_ai.operation.name";
pub(crate) const AGENT_NAME: &str = "gen_ai.agent.name";
pub(crate) const TOOL_NAME: &str = "gen_ai.tool.name";
pub(crate) const ERROR_TYPE: &str = "error.type";
pub(crate) const RESPONSE_MODEL: &str = "gen_ai.response.model";
pub(crate) const REQUEST_MODEL: &str = "gen_ai.request.model";
pub(crate) const PROVIDER: &str = "gen_ai.provider.name";
pub(crate) const INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
pub(crate) const OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
pub(crate) const CACHE_READ_INPUT_TOKENS: &str = "gen_ai.usage.cache_read.input_tokens";
pub(crate) const CACHE_WRITE_INPUT_TOKENS: &str = "gen_ai.usage.cache_write.input_tokens";
pub(crate) const SESSION_ID: &str = "gen_ai.conversation.id";

pub(crate) const INPUT_MESSAGES: &str = "gen_ai.input.messages";
pub(crate) const OUTPUT_MESSAGES: &str = "gen_ai.output.messages";
pub(crate) const SYSTEM_INSTRUCTIONS: &str = "gen_ai.system_instructions";
pub(crate) const TOOL_CALL_ARGUMENTS: &str = "gen_ai.tool.call.arguments";
pub(crate) const TOOL_CALL_RESULT: &str = "gen_ai.tool.call.result";

/// What the exporter takes off a span unless asked and the project allows.
const CONTENT: &[&str] = &[
    INPUT_MESSAGES,
    OUTPUT_MESSAGES,
    SYSTEM_INSTRUCTIONS,
    "gen_ai.tool.definitions",
    TOOL_CALL_ARGUMENTS,
    TOOL_CALL_RESULT,
    "gen_ai.retrieval.query.text",
    "gen_ai.retrieval.documents",
    "gen_ai.memory.query.text",
    "gen_ai.memory.records",
    "gen_ai.prompt.variable",
    "user.email",
    "user.full_name",
    "user.name",
];

/// Whether `key` is content.
pub(crate) fn is_content(key: &str) -> bool {
    CONTENT.contains(&key)
}

pub(crate) const RUN_OPERATION: &str = "invoke_agent";
pub(crate) const MODEL_OPERATION_DEFAULT: &str = "chat";
pub(crate) const TOOL_OPERATION: &str = "execute_tool";

/// The answer's header naming the project's content mode: off, on or sealed.
pub(crate) const CONTENT_MODE_HEADER: &str = "Telmoni-Content-Mode";

// ── Ingest's caps, in characters ─────────────────────────────────────────────

/// A span's name, a step's and a tool's among them: ingest's `NAME_MAX`,
/// room for a model call's `<operation> <model>`, a model at its cap.
pub(crate) const NAME_MAX: usize = 145;
/// `telmoni.agent`: ingest's `AGENT_MAX`.
pub(crate) const AGENT_MAX: usize = 128;
/// `error.type`: ingest's `ERROR_TYPE_MAX`.
pub(crate) const ERROR_TYPE_MAX: usize = 128;
/// `gen_ai.request.model`: ingest's `MODEL_MAX`.
pub(crate) const MODEL_MAX: usize = 128;
/// `gen_ai.provider.name`: ingest's `PROVIDER_MAX`.
pub(crate) const PROVIDER_MAX: usize = 128;
/// `user.id`, before ingest hashes it: ingest's `USER_ID_MAX`.
pub(crate) const USER_ID_MAX: usize = 128;
/// `gen_ai.conversation.id`: ingest's `SESSION_ID_MAX`.
pub(crate) const SESSION_ID_MAX: usize = 200;
