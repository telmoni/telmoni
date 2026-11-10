//! The ingest contract's names, as the core fixes them.
//!
//! The source is `crates/telemetry/src/contract.rs` in `telmoni/telmoni`: the
//! GenAI semantic conventions' names at the commit it pins, ours under
//! `telmoni.*`, and a call's cost under Langfuse's name. A name changes there
//! first, then here.

// ── Ours ─────────────────────────────────────────────────────────────────────

/// The instrumentation scope every span the SDK opens is under.
pub(crate) const SCOPE: &str = "telmoni";
/// The span of no length a trace sends as it opens, exported at once, so the
/// trace is seen before it ends.
pub(crate) const TRACE_START: &str = "telmoni.trace.start";
pub(crate) const DISTRO_NAME: &str = "telemetry.distro.name";
pub(crate) const DISTRO: &str = "telmoni";
pub(crate) const AGENT: &str = "telmoni.agent";
pub(crate) const USER_ID: &str = "user.id";
pub(crate) const TAG_PREFIX: &str = "telmoni.tag.";
/// The most tags a span carries: ingest's `TAGS_MAX`.
pub(crate) const TAGS_MAX: usize = 32;
/// The most characters a tag's key holds: ingest's `TAG_KEY_MAX`.
pub(crate) const TAG_KEY_MAX: usize = 64;
/// The most characters a tag's value holds: ingest's `TAG_VALUE_MAX`.
pub(crate) const TAG_VALUE_MAX: usize = 256;
pub(crate) const CANCELLED: &str = "telmoni.cancelled";
/// How many documents a retrieval returned, counted from the list it was
/// handed: the conventions name no such count.
pub(crate) const RETRIEVAL_RETURNED: &str = "telmoni.retrieval.returned";
/// What the scrubbers took out of a span's content, by kind, as JSON.
pub(crate) const SCRUBBED: &str = "telmoni.scrubbed";
/// The deployment, on the resource: the configuration's `environment` and
/// `release`.
pub(crate) const ENVIRONMENT: &str = "deployment.environment.name";
pub(crate) const RELEASE: &str = "service.version";
/// The deployment's longest environment and release, in characters: ingest
/// reads a resource naming a longer one as naming none, without a word.
pub(crate) const ENVIRONMENT_MAX: usize = 128;
pub(crate) const RELEASE_MAX: usize = 128;
/// The W3C headers a trace crosses services in: the span's context, and the
/// agent as a baggage member.
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
/// What a retrieval searched, a vector store or an index, or a memory: its
/// name.
pub(crate) const DATA_SOURCE_ID: &str = "gen_ai.data_source.id";
pub(crate) const MEMORY_STORE_ID: &str = "gen_ai.memory.store.id";
pub(crate) const RETRIEVAL_TOP_K: &str = "gen_ai.retrieval.top_k";
/// How many records a memory search returned.
pub(crate) const MEMORY_RECORD_COUNT: &str = "gen_ai.memory.record.count";
/// In seconds, from the request to the answer's first chunk.
pub(crate) const TIME_TO_FIRST_CHUNK: &str = "gen_ai.response.time_to_first_chunk";

pub(crate) const REQUEST_REASONING_LEVEL: &str = "gen_ai.request.reasoning.level";
/// The prompt a live model call used, by name and version: metadata, sent
/// whatever the content mode.
pub(crate) const PROMPT_NAME: &str = "gen_ai.prompt.name";
pub(crate) const PROMPT_VERSION: &str = "gen_ai.prompt.version";

pub(crate) const INPUT_MESSAGES: &str = "gen_ai.input.messages";
pub(crate) const OUTPUT_MESSAGES: &str = "gen_ai.output.messages";
pub(crate) const SYSTEM_INSTRUCTIONS: &str = "gen_ai.system_instructions";
pub(crate) const TOOL_CALL_ARGUMENTS: &str = "gen_ai.tool.call.arguments";
pub(crate) const TOOL_CALL_RESULT: &str = "gen_ai.tool.call.result";
pub(crate) const RETRIEVAL_QUERY_TEXT: &str = "gen_ai.retrieval.query.text";
pub(crate) const RETRIEVAL_DOCUMENTS: &str = "gen_ai.retrieval.documents";
pub(crate) const MEMORY_QUERY_TEXT: &str = "gen_ai.memory.query.text";
pub(crate) const MEMORY_RECORDS: &str = "gen_ai.memory.records";

/// What the exporter takes off a span unless asked and the project allows.
/// The content fields the exporter cut so a span fits an export, by name.
pub(crate) const CONTENT_CUT: &str = "telmoni.content_cut";
/// A failure, as the conventions' `exception` event carries it: content.
pub(crate) const EXCEPTION_EVENT: &str = "exception";
pub(crate) const EXCEPTION_TYPE: &str = "exception.type";
pub(crate) const EXCEPTION_MESSAGE: &str = "exception.message";
/// A score, as OpenTelemetry's evaluation event carries it, with Telmoni's
/// own type, range and version beside it.
pub(crate) const EVALUATION_EVENT: &str = "gen_ai.evaluation.result";
pub(crate) const EVALUATION_NAME: &str = "gen_ai.evaluation.name";
pub(crate) const EVALUATION_VALUE: &str = "gen_ai.evaluation.score.value";
pub(crate) const EVALUATION_LABEL: &str = "gen_ai.evaluation.score.label";
pub(crate) const SCORE_TYPE: &str = "telmoni.score.type";
pub(crate) const SCORE_RANGE_MIN: &str = "telmoni.score.range_min";
pub(crate) const SCORE_RANGE_MAX: &str = "telmoni.score.range_max";
pub(crate) const SCORE_VERSION: &str = "telmoni.score.version";
/// A score's own keys: metadata, where any other attribute of its event is
/// content, as an application's own `gen_ai.evaluation.explanation` is.
pub(crate) const SCORE_KEYS: [&str; 7] = [
    EVALUATION_NAME,
    EVALUATION_VALUE,
    EVALUATION_LABEL,
    SCORE_TYPE,
    SCORE_RANGE_MIN,
    SCORE_RANGE_MAX,
    SCORE_VERSION,
];
/// A score sent once its span ended rides the queue on a span of this name,
/// holding `/v1/scores`' item, which the exporter takes out and never sends.
pub(crate) const SCORE_SPAN: &str = "telmoni.score";
pub(crate) const SCORE_SENT: &str = "telmoni.score.sent";
pub(crate) const SCORES_PATH: &str = "/v1/scores";
/// The server's caps on a score, in characters, and on what one body carries.
pub(crate) const SCORE_NAME_MAX: usize = 64;
pub(crate) const SCORE_CATEGORY_MAX: usize = 64;
pub(crate) const SCORE_VERSION_MAX: usize = 64;
pub(crate) const SCORES_SENT_MAX: usize = 100;
/// The SDK's own cap a span, half its 32 events, so scores never crowd out
/// the exception event or the application's own.
pub(crate) const SCORES_PER_SPAN_MAX: usize = 16;
/// The key a status message is handed to the mask by.
pub(crate) const STATUS_DESCRIPTION: &str = "otel.status_description";

const CONTENT: &[&str] = &[
    INPUT_MESSAGES,
    OUTPUT_MESSAGES,
    SYSTEM_INSTRUCTIONS,
    "gen_ai.tool.definitions",
    TOOL_CALL_ARGUMENTS,
    TOOL_CALL_RESULT,
    RETRIEVAL_QUERY_TEXT,
    RETRIEVAL_DOCUMENTS,
    MEMORY_QUERY_TEXT,
    MEMORY_RECORDS,
    "gen_ai.prompt.variable",
    "user.email",
    "user.full_name",
    "user.name",
    CONTENT_CUT,
    EXCEPTION_TYPE,
    EXCEPTION_MESSAGE,
    "exception.stacktrace",
    STATUS_DESCRIPTION,
    "telmoni.events",
];

/// Whether `key` is content.
pub(crate) fn is_content(key: &str) -> bool {
    CONTENT.contains(&key)
}

pub(crate) const AGENT_OPERATION: &str = "invoke_agent";
pub(crate) const DEFAULT_MODEL_OPERATION: &str = "chat";
/// The operations ingest reads as a model call; any other files one elsewhere.
pub(crate) const MODEL_OPERATIONS: [&str; 4] =
    ["chat", "generate_content", "text_completion", "embeddings"];
pub(crate) const TOOL_OPERATION: &str = "execute_tool";
/// A search of a vector store or an index, and of a memory: ingest reads both
/// as a retrieval.
pub(crate) const RETRIEVAL_OPERATION: &str = "retrieval";
pub(crate) const SEARCH_MEMORY: &str = "search_memory";

/// The content fields whose messages' `blob` parts the exporter lifts out:
/// each image or file put to [`MEDIA_PATH`] and sent as a `file` part naming it.
pub(crate) const MEDIA_FIELDS: [&str; 3] = [INPUT_MESSAGES, OUTPUT_MESSAGES, SYSTEM_INSTRUCTIONS];
/// Where an image or file is put, by the lowercase hex SHA-256 of its bytes.
pub(crate) const MEDIA_PATH: &str = "/v1/media/";
/// A `file` part's `file_id` naming a file put to [`MEDIA_PATH`]: this, then its SHA-256.
pub(crate) const FILE_ID_PREFIX: &str = "telmoni:";
/// The largest file the platform keeps: one past it is named but never put.
pub(crate) const MEDIA_MAX_BYTES: usize = 5 * 1024 * 1024;
/// A file's `Content-Type` where its part names no type a header can carry.
pub(crate) const MEDIA_DEFAULT_TYPE: &str = "application/octet-stream";

/// The variable [`Config::from_env`](crate::Config::from_env) reads
/// `capture_content` from: `1` turns it on, and anything else, or nothing,
/// leaves it off.
pub(crate) const CAPTURE_CONTENT_ENV: &str = "TELMONI_CAPTURE_CONTENT";
/// The variables the scrubbers are read from where the code sets neither:
/// `0` alone turns the secrets one off, and `1` alone the PII one on.
pub(crate) const SCRUB_SECRETS_ENV: &str = "TELMONI_SCRUB_SECRETS";
pub(crate) const SCRUB_PII_ENV: &str = "TELMONI_SCRUB_PII";

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

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ Every name the server keeps as content, and no other, is held back
    /// (`sdk/fixtures/content.json`).
    #[test]
    fn the_content_held_back_is_what_the_fixture_names() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/content.json");
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("the fixture reads"))
                .expect("the fixture parses");
        let mut named: Vec<&str> = fixture["content"]
            .as_array()
            .expect("a list of names")
            .iter()
            .map(|name| name.as_str().expect("a name"))
            .collect();
        let mut held = CONTENT.to_vec();
        named.sort_unstable();
        held.sort_unstable();
        assert_eq!(held, named);
    }
}
