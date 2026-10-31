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
DISTRO_NAME = "telemetry.distro.name"
DISTRO = "telmoni"
RUN_ID = "telmoni.run_id"
AGENT = "telmoni.agent"
USER_ID = "user.id"
TAG_PREFIX = "telmoni.tag."
TAGS_MAX = 32
"""The most tags a run carries: ingest keeps no more (its ``TAGS_MAX``)."""
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
SAMPLE_RATE = "telmoni.sample_rate"
CANCELLED = "telmoni.cancelled"
SAMPLED_OUT = "telmoni.sampled_out"
"""Never sent: on every span of a run the sample rate kept out, from its start,
so the processor drops each however late it ends."""
ENVIRONMENT = "deployment.environment.name"
"""The deployment, on the resource: the client's ``environment`` and ``release``."""
RELEASE = "service.version"
ENVIRONMENT_MAX = 128
RELEASE_MAX = 128
"""The deployment's longest environment and release, in characters: ingest
reads a resource naming a longer one as naming none, without a word."""
TRACEPARENT = "traceparent"
"""The W3C headers a run crosses services in: the span's context, and the
run's id and agent as baggage members."""
BAGGAGE = "baggage"

# ── The conventions' ─────────────────────────────────────────────────────────

OPERATION = "gen_ai.operation.name"
AGENT_NAME = "gen_ai.agent.name"
TOOL_NAME = "gen_ai.tool.name"
ERROR_TYPE = "error.type"
RESPONSE_MODEL = "gen_ai.response.model"
REQUEST_MODEL = "gen_ai.request.model"
REQUEST_TEMPERATURE = "gen_ai.request.temperature"
REQUEST_TOP_P = "gen_ai.request.top_p"
REQUEST_TOP_K = "gen_ai.request.top_k"
REQUEST_MAX_TOKENS = "gen_ai.request.max_tokens"
REQUEST_FREQUENCY_PENALTY = "gen_ai.request.frequency_penalty"
REQUEST_PRESENCE_PENALTY = "gen_ai.request.presence_penalty"
REQUEST_SEED = "gen_ai.request.seed"
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

INPUT_MESSAGES = "gen_ai.input.messages"
OUTPUT_MESSAGES = "gen_ai.output.messages"
SYSTEM_INSTRUCTIONS = "gen_ai.system_instructions"
TOOL_CALL_ARGUMENTS = "gen_ai.tool.call.arguments"
TOOL_CALL_RESULT = "gen_ai.tool.call.result"
CONTENT_CUT = "telmoni.content_cut"
"""The content fields the exporter cut so a span fits an export, by name."""

CONTENT = frozenset(
    {
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
        CONTENT_CUT,
    }
)
"""What the exporter takes off a span unless asked and the project allows."""

MODEL_OPERATIONS = frozenset(
    {"chat", "generate_content", "text_completion", "embeddings"}
)
"""The operations ingest reads as a model call; any other files one elsewhere."""
RUN_OPERATION = "invoke_agent"
"""A run's operation, and the first word of its span's name."""
DEFAULT_MODEL_OPERATION = "chat"
"""A model call's operation when it names none."""
TOOL_OPERATION = "execute_tool"

CONTENT_MODE_HEADER = "Telmoni-Content-Mode"
"""The answer's header naming the project's content mode: off or on."""
SENT_AT_HEADER = "Telmoni-Sent-At"
"""The request's header naming this clock as the export leaves, in nanoseconds
since the epoch, by which ingest moves a run from a clock a zone or a date out."""
