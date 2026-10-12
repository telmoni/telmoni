from __future__ import annotations

from collections.abc import Callable, Mapping

from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import SpanLimits, TracerProvider
from opentelemetry.trace import Tracer

from . import contract
from ._version import __version__
from .exporter import Exporter, Transport
from .ids import Ids
from .models import Bounds, Config, RunRecord
from .processor import Processor
from .runs import ModelCall, Opener, Run, RunScope, Scope, Step, ToolCall
from .sampler import RecordingSampler


class Telmoni:
    """The client: a tracer provider of its own, and the runs it records.

    The provider is built part by part — resource, sampler, limits, processor —
    so no ``OTEL_*`` variable reaches it, and an application's own OpenTelemetry
    is neither replaced, sent to Telmoni, nor changed.
    """

    def __init__(
        self,
        config: Config | None = None,
        *,
        transport: Transport | None = None,
        sleep: Callable[[float], None] | None = None,
    ) -> None:
        self.config = config or Config()
        # Before the exporter takes it: an endpoint set to plain HTTP after the
        # configuration was built would carry the key in the clear.
        self.config._normalize()
        self.exporter = Exporter(self.config, transport=transport, sleep=sleep)
        self.processor = Processor(self.exporter)
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
            span_limits=SpanLimits(
                max_attributes=128,
                max_events=0,
                max_links=0,
                max_span_attributes=128,
                max_event_attributes=0,
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
        """The project's content mode, learned from the first answer."""
        return self.exporter.content_mode

    def run(
        self,
        agent: str,
        *,
        user_id: str | None = None,
        session_id: str | None = None,
        tags: Mapping[str, str] | None = None,
        bounds: Bounds | None = None,
        sample_rate: float | None = None,
    ) -> RunScope:
        """A run of ``agent``: ``with telmoni.run("bot") as run:`` or ``@telmoni.run("bot")``."""
        return RunScope(
            lambda: self._opener.run(
                agent,
                user_id=user_id,
                session_id=session_id,
                tags=tags,
                bounds=bounds,
                sample_rate=sample_rate,
            )
        )

    def step(self, name: str, *, run: Run | None = None) -> Scope[Step]:
        """A step inside the current run, or ``run``'s; nothing outside one."""
        return Scope(lambda: self._opener.step(name, run))

    def model_call(
        self,
        model: str,
        *,
        provider: str | None = None,
        operation: str = "chat",
        input_messages: object | None = None,
        system_instructions: object | None = None,
        run: Run | None = None,
    ) -> Scope[ModelCall]:
        """A model call inside the current run; ``answered()`` records the reply."""
        return Scope(
            lambda: self._opener.model_call(
                model,
                run,
                provider=provider,
                operation=operation,
                input_messages=input_messages,
                system_instructions=system_instructions,
            )
        )

    def tool_call(
        self, name: str, *, arguments: object | None = None, run: Run | None = None
    ) -> Scope[ToolCall]:
        """A tool call inside the current run; ``returned()`` records its result."""
        return Scope(lambda: self._opener.tool_call(name, run, arguments=arguments))

    def record_run(self, record: RunRecord) -> None:
        """Send a run that has already ended, from its record, as the live
        calls would have sent it: for a job that keeps its own log, or a hook
        that runs after the turn it reports. The same record sent again is the
        same bytes, which ingest answers once.

        Raises ``ValueError``, sending nothing, for a blank agent, a run id
        that is not a UUID, a time without its timezone, or a span that ends
        before it starts.
        """
        self._opener.record(record)

    def flush(self, timeout_s: float = 30.0) -> bool:
        """Send everything queued; True once the platform has it."""
        return self.processor.force_flush(int(timeout_s * 1000))

    def shutdown(self) -> None:
        """Flush, then stop the exporter's thread."""
        self._provider.shutdown()
