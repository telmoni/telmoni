"""The ingest contract's names, as the core fixes them.

The source is ``crates/telemetry/src/contract.rs`` in ``telmoni/telmoni``: the
GenAI semantic conventions' names at the commit it pins, and ours under
``telmoni.*``. A name changes there first, then here.
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
BOUND_STALL_AFTER_S = "telmoni.bound.stall_after_s"
BOUND_MAX_STEPS = "telmoni.bound.max_steps"
BOUND_MAX_COST_USD = "telmoni.bound.max_cost_usd"
SAMPLE_RATE = "telmoni.sample_rate"
CANCELLED = "telmoni.cancelled"
SAMPLED_OUT = "telmoni.sampled_out"
"""Never sent: on every span of a run the sample rate kept out, from its start,
so the processor drops each however late it ends."""
RUN_START = "telmoni.run.start"
"""The span of no duration that opens a run, sent at once."""
ENVIRONMENT = "deployment.environment.name"
"""The deployment, on the resource: the client's ``environment`` and ``release``."""
RELEASE = "service.version"
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
PROVIDER = "gen_ai.provider.name"
INPUT_TOKENS = "gen_ai.usage.input_tokens"
OUTPUT_TOKENS = "gen_ai.usage.output_tokens"
CACHE_READ_INPUT_TOKENS = "gen_ai.usage.cache_read.input_tokens"
CACHE_WRITE_INPUT_TOKENS = "gen_ai.usage.cache_write.input_tokens"
SESSION_ID = "gen_ai.conversation.id"

INPUT_MESSAGES = "gen_ai.input.messages"
OUTPUT_MESSAGES = "gen_ai.output.messages"
SYSTEM_INSTRUCTIONS = "gen_ai.system_instructions"
TOOL_CALL_ARGUMENTS = "gen_ai.tool.call.arguments"
TOOL_CALL_RESULT = "gen_ai.tool.call.result"

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
    }
)
"""What the exporter takes off a span unless asked and the project allows."""

MODEL_OPERATIONS = frozenset(
    {"chat", "generate_content", "text_completion", "embeddings"}
)
TOOL_OPERATION = "execute_tool"

CONTENT_MODE_HEADER = "Telmoni-Content-Mode"
"""The answer's header naming the project's content mode: off, on or sealed."""
