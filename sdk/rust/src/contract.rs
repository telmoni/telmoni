//! The ingest contract's names, as the core fixes them.
//!
//! The source is `crates/telemetry/src/contract.rs` in `telmoni/telmoni`: the
//! GenAI semantic conventions' names at the commit it pins, ours under
//! `telmoni.*`, and a call's cost under Langfuse's name. A name changes there
//! first, then here.

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
pub(crate) const SAMPLE_RATE: &str = "telmoni.sample_rate";
pub(crate) const CANCELLED: &str = "telmoni.cancelled";
/// Never sent: on every span of a run the sample rate kept out, from its
/// start, so the processor drops each however late it ends.
pub(crate) const SAMPLED_OUT: &str = "telmoni.sampled_out";
/// The deployment, on the resource: the configuration's `environment` and
/// `release`.
pub(crate) const ENVIRONMENT: &str = "deployment.environment.name";
pub(crate) const RELEASE: &str = "service.version";
/// The deployment's longest environment and release, in characters: ingest
/// reads a resource naming a longer one as naming none, without a word.
pub(crate) const ENVIRONMENT_MAX: usize = 128;
pub(crate) const RELEASE_MAX: usize = 128;
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
/// What the call cost in US dollars, as reported: Langfuse's name, since the
/// conventions have none.
pub(crate) const COST: &str = "gen_ai.usage.cost";
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
pub(crate) const DEFAULT_MODEL_OPERATION: &str = "chat";
/// The operations ingest reads as a model call; any other files one elsewhere.
pub(crate) const MODEL_OPERATIONS: [&str; 4] =
    ["chat", "generate_content", "text_completion", "embeddings"];
pub(crate) const TOOL_OPERATION: &str = "execute_tool";

/// The answer's header naming the project's content mode: off or on.
pub(crate) const CONTENT_MODE_HEADER: &str = "Telmoni-Content-Mode";
/// The request's header naming this clock as the export leaves, in nanoseconds
/// since the epoch, by which ingest moves a run from a clock a zone or a date out.
pub(crate) const SENT_AT_HEADER: &str = "Telmoni-Sent-At";

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
/// The largest body an export may be: ingest's `BATCH_MAX_BYTES`, past which it
/// refuses one whole.
pub(crate) const BATCH_MAX_BYTES: usize = 1 << 20;
/// The most spans an export may hold: ingest's `BATCH_SPANS_MAX`, past which it
/// refuses one whole.
pub(crate) const BATCH_SPANS_MAX: usize = 1_000;
/// An attribute's longest key as text, in characters: ingest drops one past it.
pub(crate) const ATTRIBUTE_KEY_MAX: usize = 128;
/// An attribute's longest value as text, in characters: ingest drops one past it.
pub(crate) const ATTRIBUTE_VALUE_MAX: usize = 1_024;
