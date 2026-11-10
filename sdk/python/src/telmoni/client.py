from __future__ import annotations

from collections.abc import Callable, Mapping, MutableMapping, Sequence

from opentelemetry import trace
from opentelemetry.context import Context
from opentelemetry.metrics import NoOpMeterProvider
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import SpanLimits, TracerProvider
from opentelemetry.trace import Tracer

from . import contract, guard, scores
from ._version import __version__
from .exporter import Exporter, Transport
from .ids import Ids
from .models import Config, TraceRecord
from .processor import Batching, Processor
from .propagation import read_headers, write_headers
from .sampler import RecordingSampler
from .traces import (
    ModelCall,
    Opener,
    Retrieval,
    Scope,
    Step,
    ToolCall,
    Trace,
    TraceScope,
    current_trace,
)

FLUSH_MAX_S = 2_147_483.647
"""The longest a flush waits, as long as a JavaScript timer can: about 24.8 days."""


class Telmoni:
    """The client: a tracer provider of its own, and the traces it records.

    Built part by part, its meter a no-op, so no ``OTEL_*`` variable changes
    what it sends, and the application's own OpenTelemetry stays as it was.
    """

    def __init__(
        self,
        config: Config | None = None,
        *,
        transport: Transport | None = None,
        sleep: Callable[[float], None] | None = None,
        batching: Batching | None = None,
    ) -> None:
        self.config = config or Config()
        # Before the exporter takes it: an endpoint set to plain HTTP after the
        # configuration was built would carry the key in the clear.
        self.config._normalize()
        # The project's own prices, which the guard counts a call by first.
        self.prices = guard.own(self.config.prices)
        self.exporter = Exporter(self.config, transport=transport, sleep=sleep)
        self.processor = Processor(self.exporter, batching)
        resource = Resource(
            {
                contract.DISTRO_NAME: contract.DISTRO,
                "telemetry.distro.version": __version__,
            }
        )
        unlimited = SpanLimits.UNSET
        provider = TracerProvider(
            sampler=RecordingSampler(self.config.sampler),
            resource=resource,
            shutdown_on_exit=True,
            id_generator=Ids(),
            # Else an OTEL_* variable sends its span metrics to the application.
            meter_provider=NoOpMeterProvider(),
            span_limits=SpanLimits(
                max_attributes=128,
                max_events=32,
                # One: the application's span current as a trace opens.
                max_links=1,
                max_span_attributes=128,
                max_event_attributes=32,
                max_link_attributes=0,
                max_attribute_length=unlimited,
                max_span_attribute_length=unlimited,
            ),
        )
        # OpenTelemetry switches a provider off by OTEL_SDK_DISABLED as it is
        # built; that variable is the application's, for its own providers.
        provider._disabled = False
        provider.add_span_processor(self.processor)
        self._provider = provider
        self.tracer: Tracer = provider.get_tracer(contract.SCOPE, __version__)
        self._opener = Opener(self)

    @classmethod
    def from_env(cls) -> Telmoni:
        return cls(Config.from_env())

    @property
    def content_mode(self) -> str | None:
        """The project's content mode, `off` or `on`, as the last answer named
        it; None while unheard."""
        return self.exporter.content_mode

    def trace(
        self,
        agent: str,
        *,
        user_id: str | None = None,
        session_id: str | None = None,
        tags: Mapping[str, str] | None = None,
        max_steps: int | None = None,
        max_spend: float | None = None,
        input_messages: object | None = None,
    ) -> TraceScope:
        """One execution of ``agent``: ``with telmoni.trace("bot") as trace:``
        or ``@telmoni.trace("bot")``, a trace of its own, linked to whatever
        span the application has current.

        ``user_id`` is the end user the trace is for, as your own id for them,
        which ingest hashes; ``session_id`` the session its traces share.
        ``max_steps`` and ``max_spend``, in US dollars, are the guard's caps:
        once the trace reaches one, what it opens next raises
        ``CapExceededError``. ``input_messages`` is content, sent only when
        asked and allowed.
        """
        return TraceScope(
            lambda: self._opener.trace(
                agent,
                user_id=user_id,
                session_id=session_id,
                tags=tags,
                max_steps=max_steps,
                max_spend=max_spend,
                input_messages=input_messages,
            )
        )

    def step(self, name: str, *, trace: Trace | None = None) -> Scope[Step]:
        """A step inside the current trace, or ``trace``; nothing outside one."""
        return Scope(lambda: self._opener.step(name, trace))

    def model_call(
        self,
        model: str,
        *,
        provider: str | None = None,
        operation: str = contract.DEFAULT_MODEL_OPERATION,
        prompt: str | None = None,
        prompt_version: str | None = None,
        input_messages: object | None = None,
        system_instructions: object | None = None,
        trace: Trace | None = None,
    ) -> Scope[ModelCall]:
        """A model call inside the current trace; ``answered()`` records the reply.

        ``prompt`` and ``prompt_version`` name the prompt it used, sent in every
        content mode: a prompt, never a person, who goes in the trace's ``user_id``.
        """
        return Scope(
            lambda: self._opener.model_call(
                model,
                trace,
                provider=provider,
                operation=operation,
                prompt=prompt,
                prompt_version=prompt_version,
                input_messages=input_messages,
                system_instructions=system_instructions,
            )
        )

    def tool_call(
        self,
        name: str,
        *,
        arguments: object | None = None,
        trace: Trace | None = None,
    ) -> Scope[ToolCall]:
        """A tool call inside the current trace; ``returned()`` records its result."""
        return Scope(lambda: self._opener.tool_call(name, trace, arguments=arguments))

    def retrieval(
        self,
        source: str,
        *,
        query: object | None = None,
        top_k: int | None = None,
        memory: bool = False,
        trace: Trace | None = None,
    ) -> Scope[Retrieval]:
        """A search of ``source``, or with ``memory`` of a memory store, inside
        the current trace; ``returned()`` records what it found."""
        return Scope(
            lambda: self._opener.retrieval(
                source, trace, query=query, top_k=top_k, memory=memory
            )
        )

    def record_trace(self, record: TraceRecord) -> None:
        """Send a trace that has already ended, as the live calls would have; a
        field of the wrong type or value raises, sending nothing."""
        self._opener.record(record)

    def score(
        self,
        name: str,
        value: bool | float | str,
        *,
        trace_id: str | None = None,
        span_id: str | None = None,
        session_id: str | None = None,
        range: Sequence[float] | None = None,
        version: str | None = None,
    ) -> None:
        """Score what has ended: a trace by its id, one of its spans by its
        ``span_id`` too, or a session, posted to ``/v1/scores`` with the next
        export. One breaking a score's rules is not sent, with one warning."""
        warn_once = self.exporter.warn_once
        judgement = scores.judged(name, value, range, version, warn_once)
        if judgement is None:
            return
        target = scores.target(trace_id, span_id, session_id, warn_once)
        if target is None:
            return
        # A span of a trace of its own, through the queue, so no second queue
        # holds what flush and shutdown drain; never sent as a span.
        self.tracer.start_span(
            contract.SCORE_SPAN,
            context=Context(),
            attributes={contract.SCORE_SENT: judgement.sent(target)},
        ).end()

    def inject(self, headers: MutableMapping[str, str]) -> None:
        """Write into ``headers`` what the next service continues the trace
        from: the W3C ``traceparent`` of the span current inside the trace, and
        the agent as the W3C ``baggage`` member ``telmoni.agent``, beside
        whatever members the headers hold. Outside a trace, nothing is written.
        """
        opened = current_trace()
        if opened is None:
            return
        span = trace.get_current_span().get_span_context()
        if not span.is_valid:
            return
        write_headers(headers, span, opened.agent)

    def extract(self, headers: Mapping[str, str]) -> Trace | None:
        """The trace ``headers`` name, continued here: its steps and calls —
        ``telmoni.step(name, trace=trace)`` or ``trace.step(name)`` — take the
        caller's span as their parent and carry the caller's agent, so the
        trace shows the work where it was asked for. Nothing ends a continued
        trace here; the caller's own span does. ``None`` for headers that name
        no trace.
        """
        read = read_headers(headers)
        if read is None:
            return None
        return self._opener.continued(read.span_context, read.agent)

    def flush(self, timeout_s: float = 30.0) -> bool:
        """Send everything queued; True once nothing is left to send, kept or not,
        and at once after a shutdown. A batch refused or dropped is warned of."""
        # NaN or none waits not at all; infinity, or past the cap, the cap.
        seconds = min(timeout_s, FLUSH_MAX_S) if timeout_s > 0 else 0.0
        return self.processor.force_flush(int(seconds * 1000))

    def shutdown(self) -> None:
        """Flush, then stop the exporter's thread."""
        self._provider.shutdown()
