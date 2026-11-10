"""The ingest contract's names, as the core fixes them.

The source is ``crates/telemetry/src/contract.rs`` in ``telmoni/telmoni``: the
GenAI semantic conventions' names at the commit it pins, ours under
``telmoni.*``, and a call's cost under Langfuse's name. A name changes there
first, then here.
"""

from __future__ import annotations

# ── Ours ─────────────────────────────────────────────────────────────────────

SCOPE = "telmoni"
"""The instrumentation scope every span the SDK opens is under."""
TRACE_START = "telmoni.trace.start"
"""The span of no length a trace sends as it opens, exported at once, so the
trace is seen before it ends."""
DISTRO_NAME = "telemetry.distro.name"
DISTRO = "telmoni"
AGENT = "telmoni.agent"
USER_ID = "user.id"
TAG_PREFIX = "telmoni.tag."
TAGS_MAX = 32
"""The most tags a trace carries: ingest keeps no more (its ``TAGS_MAX``)."""
TAG_KEY_MAX = 64
TAG_VALUE_MAX = 256
"""A tag's longest key and value, in characters: ingest drops a tag past either."""
NAME_MAX = 145
AGENT_MAX = 128
ERROR_TYPE_MAX = 128
MODEL_MAX = 128
PROVIDER_MAX = 128
USER_ID_MAX = 128
SESSION_ID_MAX = 200
"""Ingest's longest text of each, in characters: it refuses a span past one. A
name holds a model call's ``<operation> <model>``, a model at its cap."""
BATCH_MAX_BYTES = 1 << 20
"""The largest body an export may be: ingest refuses one past it whole."""
BATCH_SPANS_MAX = 1000
"""The most spans an export may hold: ingest refuses one past it whole."""
ATTRIBUTE_KEY_MAX = 128
ATTRIBUTE_VALUE_MAX = 1024
"""An attribute's longest key and value as text, in characters: ingest drops one
past either."""
CANCELLED = "telmoni.cancelled"
RETRIEVAL_RETURNED = "telmoni.retrieval.returned"
"""How many documents a retrieval returned, counted from the list it was handed:
the conventions name no such count."""
ENVIRONMENT = "deployment.environment.name"
"""The deployment, on the resource: the client's ``environment`` and ``release``."""
RELEASE = "service.version"
ENVIRONMENT_MAX = 128
RELEASE_MAX = 128
"""The deployment's longest environment and release, in characters: ingest
reads a resource naming a longer one as naming none, without a word."""
TRACEPARENT = "traceparent"
"""The W3C headers a trace crosses services in: the span's context, and the
trace's agent as a baggage member."""
BAGGAGE = "baggage"

# ── The conventions' ─────────────────────────────────────────────────────────

OPERATION = "gen_ai.operation.name"
AGENT_NAME = "gen_ai.agent.name"
TOOL_NAME = "gen_ai.tool.name"
ERROR_TYPE = "error.type"
RESPONSE_MODEL = "gen_ai.response.model"
REQUEST_MODEL = "gen_ai.request.model"
REQUEST_REASONING_LEVEL = "gen_ai.request.reasoning.level"
PROMPT_NAME = "gen_ai.prompt.name"
PROMPT_VERSION = "gen_ai.prompt.version"
"""The prompt a live model call used, by name and version: metadata, sent
whatever the content mode."""
TIME_TO_FIRST_CHUNK = "gen_ai.response.time_to_first_chunk"
"""In seconds, from the request to the answer's first chunk."""
PROVIDER = "gen_ai.provider.name"
INPUT_TOKENS = "gen_ai.usage.input_tokens"
OUTPUT_TOKENS = "gen_ai.usage.output_tokens"
CACHE_READ_INPUT_TOKENS = "gen_ai.usage.cache_read.input_tokens"
CACHE_WRITE_INPUT_TOKENS = "gen_ai.usage.cache_write.input_tokens"
COST = "gen_ai.usage.cost"
"""What the call cost in US dollars, as reported: Langfuse's name, since the
conventions have none."""
SESSION_ID = "gen_ai.conversation.id"
DATA_SOURCE_ID = "gen_ai.data_source.id"
MEMORY_STORE_ID = "gen_ai.memory.store.id"
"""What a retrieval searched, a vector store or an index, or a memory: its name."""
RETRIEVAL_TOP_K = "gen_ai.retrieval.top_k"
MEMORY_RECORD_COUNT = "gen_ai.memory.record.count"
"""How many records a memory search returned."""

INPUT_MESSAGES = "gen_ai.input.messages"
OUTPUT_MESSAGES = "gen_ai.output.messages"
SYSTEM_INSTRUCTIONS = "gen_ai.system_instructions"
TOOL_CALL_ARGUMENTS = "gen_ai.tool.call.arguments"
TOOL_CALL_RESULT = "gen_ai.tool.call.result"
RETRIEVAL_QUERY_TEXT = "gen_ai.retrieval.query.text"
RETRIEVAL_DOCUMENTS = "gen_ai.retrieval.documents"
MEMORY_QUERY_TEXT = "gen_ai.memory.query.text"
MEMORY_RECORDS = "gen_ai.memory.records"
CONTENT_CUT = "telmoni.content_cut"
SCRUBBED = "telmoni.scrubbed"
"""What the scrubbers took out of a span's content, counted by kind, as JSON with
its keys sorted (``{"pii:email":1,"secret:aws_key":2}``), and ``dropped:<field>``
for a field one failed on, sent without it; absent where they took out nothing."""
# A failure, as the conventions' `exception` event carries it: content.
EXCEPTION_EVENT = "exception"
EXCEPTION_TYPE = "exception.type"
EXCEPTION_MESSAGE = "exception.message"
EXCEPTION_STACKTRACE = "exception.stacktrace"
"""The content fields the exporter cut so a span fits an export, by name."""
STATUS_DESCRIPTION = "otel.status_description"
"""The key a status message is handed to the mask by."""

# A score, as the conventions' evaluation event carries it, Telmoni's own
# beside: a yes or no's type, a number's range and the scorer's version.
EVALUATION_EVENT = "gen_ai.evaluation.result"
EVALUATION_NAME = "gen_ai.evaluation.name"
EVALUATION_VALUE = "gen_ai.evaluation.score.value"
EVALUATION_LABEL = "gen_ai.evaluation.score.label"
SCORE_TYPE = "telmoni.score.type"
SCORE_RANGE_MIN = "telmoni.score.range_min"
SCORE_RANGE_MAX = "telmoni.score.range_max"
SCORE_VERSION = "telmoni.score.version"
SCORE_KEYS = frozenset(
    {
        EVALUATION_NAME,
        EVALUATION_VALUE,
        EVALUATION_LABEL,
        SCORE_TYPE,
        SCORE_RANGE_MIN,
        SCORE_RANGE_MAX,
        SCORE_VERSION,
    }
)
"""An evaluation event's metadata, which leaves whatever the content lock says
and is never masked: ingest reads a score from it alone."""
SCORE_SPAN = "telmoni.score"
SCORE_SENT = "telmoni.score.sent"
"""A late score's carrier span, and the attribute holding its item as JSON: the
exporter takes it out of the batch and posts the item to ``SCORES_PATH``."""
SCORES_PATH = "/v1/scores"
SCORE_NAME_MAX = 64
SCORE_CATEGORY_MAX = 64
SCORE_VERSION_MAX = 64
"""A score's longest name, category and version, in characters."""
SCORES_PER_SPAN_MAX = 16
"""The most scores one span carries: a span holds 32 events, the exception's among them."""
SCORES_SENT_MAX = 100
"""The most scores one ``POST /v1/scores`` carries."""

CONTENT = frozenset(
    {
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
        EXCEPTION_STACKTRACE,
        STATUS_DESCRIPTION,
        "telmoni.events",
    }
)
"""What the exporter takes off a span unless asked and the project allows."""

MODEL_OPERATIONS = frozenset(
    {"chat", "generate_content", "text_completion", "embeddings"}
)
"""The operations ingest reads as a model call; any other files one elsewhere."""
AGENT_OPERATION = "invoke_agent"
"""A trace's agent span's operation, and the first word of its name."""
DEFAULT_MODEL_OPERATION = "chat"
"""A model call's operation when it names none."""
TOOL_OPERATION = "execute_tool"
RETRIEVAL_OPERATION = "retrieval"
SEARCH_MEMORY = "search_memory"
"""A search of a vector store or an index, and of a memory: ingest reads both as
a retrieval."""

MEDIA_FIELDS = frozenset({INPUT_MESSAGES, OUTPUT_MESSAGES, SYSTEM_INSTRUCTIONS})
"""The content fields whose messages' ``blob`` parts the exporter lifts out:
each image or file put to ``MEDIA_PATH`` and sent as a ``file`` part naming it."""
MEDIA_PATH = "/v1/media/"
"""Where an image or file is put, by the lowercase hex SHA-256 of its bytes."""
FILE_ID_PREFIX = "telmoni:"
"""A ``file`` part's ``file_id`` naming a file put to ``MEDIA_PATH``: this, then
its SHA-256."""
MEDIA_MAX_BYTES = 5 * 1024 * 1024
"""The largest file the platform keeps: one past it is named but never put."""
MEDIA_DEFAULT_TYPE = "application/octet-stream"
"""A file's ``Content-Type`` where its part names no type a header can carry."""

CAPTURE_CONTENT_ENV = "TELMONI_CAPTURE_CONTENT"
"""The variable ``Config.from_env`` reads ``capture_content`` from: ``1``
turns it on, and anything else, or nothing, leaves it off."""
SCRUB_SECRETS_ENV = "TELMONI_SCRUB_SECRETS"
"""The variable a configuration reads ``scrub_secrets`` from where its code left it
unset: ``0`` turns the secrets' scrubber off, and anything else, or nothing,
leaves it on."""
SCRUB_PII_ENV = "TELMONI_SCRUB_PII"
"""The variable a configuration reads ``scrub_pii`` from where its code left it
unset: ``1`` turns personal data's scrubber on, and anything else, or nothing,
leaves it off."""

CONTENT_MODE_HEADER = "Telmoni-Content-Mode"
"""The answer's header naming the project's content mode: off or on."""
SENT_AT_HEADER = "Telmoni-Sent-At"
"""The request's header naming this clock as the export leaves, in nanoseconds
since the epoch, by which ingest moves a trace from a clock a zone or a date out."""
