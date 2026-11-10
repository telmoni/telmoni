"""A sampler that records every span and decides nothing of its own.

A trace of its own takes the decision of the sampler configured, else the
application's own, so an application sampling at a tenth keeps a tenth of its
own spans inside traces as outside them; a continued trace, and every span
inside one, takes its parent's sampled flag. Nothing is ever dropped here: the
SDK's processor exports recorded spans whether the flag is set or not.
"""

from __future__ import annotations

from collections.abc import Sequence

from opentelemetry import trace
from opentelemetry.context import Context
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.sampling import (
    ALWAYS_ON,
    Decision,
    Sampler,
    SamplingResult,
)
from opentelemetry.trace import Link, SpanKind, TraceState
from opentelemetry.util.types import Attributes


class RecordingSampler(Sampler):
    def __init__(self, root: Sampler | None = None) -> None:
        self._root = root

    def should_sample(
        self,
        parent_context: Context | None,
        trace_id: int,
        name: str,
        kind: SpanKind | None = None,
        attributes: Attributes = None,
        links: Sequence[Link] | None = None,
        trace_state: TraceState | None = None,
    ) -> SamplingResult:
        parent = trace.get_current_span(parent_context).get_span_context()
        if parent.is_valid:
            sampled = parent.trace_flags.sampled
            return SamplingResult(_recording(sampled), attributes, parent.trace_state)
        decision = self._root_sampler().should_sample(
            parent_context, trace_id, name, kind, attributes, links, trace_state
        )
        sampled = decision.decision is Decision.RECORD_AND_SAMPLE
        return SamplingResult(_recording(sampled), attributes, decision.trace_state)

    def _root_sampler(self) -> Sampler:
        if self._root is not None:
            return self._root
        # Public on OpenTelemetry's own provider; a proxy or a no-op provider,
        # when the application set none, samples everything, as its default
        # ParentBased(ALWAYS_ON) would.
        provider = trace.get_tracer_provider()
        if isinstance(provider, TracerProvider):
            return provider.sampler
        return ALWAYS_ON

    def get_description(self) -> str:
        return "TelmoniRecordingSampler"


def _recording(sampled: bool) -> Decision:
    return Decision.RECORD_AND_SAMPLE if sampled else Decision.RECORD_ONLY
