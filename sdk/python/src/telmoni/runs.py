"""Runs, steps, model calls and tool calls, as context managers and decorators.

A run is a span of its own in whatever trace is current, opened by
``telmoni.run.start``; every span inside it carries the run's id, agent,
user and conversation, so ingest knows each span's run without waiting
for the trace's tree. Nothing is sent outside a run.
"""

from __future__ import annotations

import asyncio
import functools
import inspect
import json
import logging
import random
import uuid
from collections.abc import Callable, Mapping
from contextvars import Token
from typing import TYPE_CHECKING, Any, Generic, TypeVar

from opentelemetry import context, trace
from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.trace import Span, SpanKind, Status, StatusCode

from . import contract
from .models import Bounds, Usage

if TYPE_CHECKING:
    from .client import Telmoni

logger = logging.getLogger("telmoni")

_RUN_KEY = context.create_key("telmoni.run")

H = TypeVar("H", bound="Handle")
C = TypeVar("C", bound="Handle")
F = TypeVar("F", bound=Callable[..., Any])


def current_run(ctx: context.Context | None = None) -> Run | None:
    """The run open in this context, if any."""
    run = context.get_value(_RUN_KEY, ctx)
    return run if isinstance(run, Run) else None


class Handle:
    """A span the SDK opened, until its ``_end``."""

    def __init__(self, span: Span) -> None:
        self._span = span
        self._token: Token[context.Context] | None = None
        self._ended = False

    def _attach(self, ctx: context.Context) -> None:
        """Make ``ctx`` current until ``_end``, so spans opened inside nest."""
        self._token = context.attach(ctx)

    def fail(self, error_type: str) -> None:
        """Mark it failed, with the error's class and never its message."""
        self._span.set_attribute(contract.ERROR_TYPE, error_type)
        self._span.set_status(Status(StatusCode.ERROR))

    def cancel(self) -> None:
        """Mark it cancelled by the agent, which is neither ok nor an error."""
        self._span.set_attribute(contract.CANCELLED, True)

    def _end(self, exc: BaseException | None) -> None:
        if self._ended:
            return
        self._ended = True
        if isinstance(exc, (KeyboardInterrupt, asyncio.CancelledError)):
            self.cancel()
        elif exc is not None:
            self.fail(type(exc).__name__)
        self._span.end()
        if self._token is not None:
            context.detach(self._token)


class Run(Handle):
    def __init__(
        self,
        client: Telmoni,
        span: Span,
        *,
        run_id: str,
        agent: str,
        inherited: Mapping[str, str],
    ) -> None:
        super().__init__(span)
        self._client = client
        self.run_id = run_id
        self.agent = agent
        self._inherited = dict(inherited)

    @property
    def inherited(self) -> dict[str, str]:
        """What every span inside the run carries."""
        return dict(self._inherited)

    def step(self, name: str) -> Scope[Step]:
        return self._client.step(name, run=self)

    def model_call(
        self,
        model: str,
        *,
        provider: str | None = None,
        operation: str = "chat",
        input_messages: object | None = None,
        system_instructions: object | None = None,
    ) -> Scope[ModelCall]:
        return self._client.model_call(
            model,
            provider=provider,
            operation=operation,
            input_messages=input_messages,
            system_instructions=system_instructions,
            run=self,
        )

    def tool_call(
        self, name: str, *, arguments: object | None = None
    ) -> Scope[ToolCall]:
        return self._client.tool_call(name, arguments=arguments, run=self)


class Step(Handle):
    pass


class ModelCall(Handle):
    def answered(
        self,
        *,
        response_model: str | None = None,
        usage: Usage | None = None,
        output_messages: object | None = None,
    ) -> None:
        """What the model answered: its name, its tokens, and its messages."""
        if response_model is not None:
            self._span.set_attribute(contract.RESPONSE_MODEL, response_model)
        if usage is not None:
            for key, value in (
                (contract.INPUT_TOKENS, usage.input_tokens),
                (contract.OUTPUT_TOKENS, usage.output_tokens),
                (contract.CACHE_READ_INPUT_TOKENS, usage.cache_read_input_tokens),
                (contract.CACHE_WRITE_INPUT_TOKENS, usage.cache_write_input_tokens),
            ):
                if value is not None:
                    self._span.set_attribute(key, value)
        if output_messages is not None:
            self._span.set_attribute(
                contract.OUTPUT_MESSAGES, _as_json(output_messages)
            )


class ToolCall(Handle):
    def returned(self, result: object) -> None:
        """What the tool returned: content, sent only when asked and allowed."""
        self._span.set_attribute(contract.TOOL_CALL_RESULT, _as_json(result))


class Scope(Generic[H]):
    """Opens a handle on ``with``, ``async with``, or each call of a decorated
    function; ``None`` from ``_open`` means nothing is opened (no run)."""

    def __init__(self, open_handle: Callable[[], H | None]) -> None:
        self._open = open_handle
        self._handle: H | None = None

    def __enter__(self) -> H | Nothing:
        self._handle = self._open()
        return self._handle if self._handle is not None else Nothing()

    def __exit__(self, exc_type: object, exc: BaseException | None, tb: object) -> None:
        if self._handle is not None:
            self._handle._end(exc)
            self._handle = None

    async def __aenter__(self) -> H | Nothing:
        return self.__enter__()

    async def __aexit__(
        self, exc_type: object, exc: BaseException | None, tb: object
    ) -> None:
        self.__exit__(exc_type, exc, tb)

    def __call__(self, fn: F) -> F:
        if inspect.iscoroutinefunction(fn):

            @functools.wraps(fn)
            async def wrapped_async(*args: Any, **kwargs: Any) -> Any:
                async with Scope(self._open):
                    return await fn(*args, **kwargs)

            return wrapped_async  # type: ignore[return-value]

        @functools.wraps(fn)
        def wrapped(*args: Any, **kwargs: Any) -> Any:
            with Scope(self._open):
                return fn(*args, **kwargs)

        return wrapped  # type: ignore[return-value]


class RunScope(Scope[Run]):
    """A run always opens, so its handle is never the no-op one."""

    def __enter__(self) -> Run:
        handle = self._open()
        if handle is None:
            raise RuntimeError("a run did not open")
        self._handle = handle
        return handle

    async def __aenter__(self) -> Run:
        return self.__enter__()


class Nothing:
    """The handle given outside a run: every method does nothing."""

    run_id = ""
    agent = ""

    def fail(self, error_type: str) -> None:
        return None

    def cancel(self) -> None:
        return None

    def answered(self, **kwargs: object) -> None:
        return None

    def returned(self, result: object) -> None:
        return None


class Opener:
    """Builds the handles for one client; the client holds one."""

    def __init__(self, client: Telmoni) -> None:
        self._client = client
        self._warned_outside = False

    def run(
        self,
        agent: str,
        *,
        user_id: str | None,
        conversation_id: str | None,
        tags: Mapping[str, str] | None,
        bounds: Bounds | None,
        sample_rate: float | None,
    ) -> Run:
        if not agent.strip():
            raise ValueError("a run needs its agent's name")
        rate = self._client.config.sample_rate if sample_rate is None else sample_rate
        if not 0 < rate <= 1:
            raise ValueError("sample_rate must be above 0 and up to 1")
        run_id = str(uuid.uuid4())
        inherited: dict[str, str] = {contract.RUN_ID: run_id, contract.AGENT: agent}
        if user_id:
            inherited[contract.USER_ID] = user_id
        if conversation_id:
            inherited[contract.CONVERSATION_ID] = conversation_id
        attributes: dict[str, Any] = {
            contract.OPERATION: "invoke_agent",
            contract.AGENT_NAME: agent,
            **inherited,
            **_bounds(bounds),
            contract.SAMPLE_RATE: float(rate),
        }
        for key, value in (tags or {}).items():
            attributes[f"{contract.TAG_PREFIX}{key}"] = value
        parent = context.get_current()
        span = self._start(
            f"invoke_agent {agent}", parent, attributes, SpanKind.INTERNAL
        )
        if rate < 1 and random.random() >= rate:
            self._client.processor.sample_out(run_id)
        ctx = trace.set_span_in_context(span, parent)
        start = span.start_time if isinstance(span, ReadableSpan) else None
        # OpenTelemetry sends a span only once it ends; without this one a run
        # stalled in its first call would never be heard.
        opener = self._start(
            contract.RUN_START,
            ctx,
            {**inherited, **_bounds(bounds), contract.SAMPLE_RATE: float(rate)},
            SpanKind.INTERNAL,
            start_time=start,
        )
        opener.end(end_time=start)
        run = Run(self._client, span, run_id=run_id, agent=agent, inherited=inherited)
        # The run is found from the context by step(), model_call() and
        # tool_call() called on the client rather than on the run.
        run._attach(context.set_value(_RUN_KEY, run, ctx))
        return run

    def step(self, name: str, run: Run | None) -> Step | None:
        run = run or self._outside("step")
        if run is None:
            return None
        return self._child(run, Step, name, run.inherited, SpanKind.INTERNAL)

    def model_call(
        self,
        model: str,
        run: Run | None,
        *,
        provider: str | None,
        operation: str,
        input_messages: object | None,
        system_instructions: object | None,
    ) -> ModelCall | None:
        run = run or self._outside("model_call")
        if run is None:
            return None
        # Sent blank, the span would be named " <model>" and ingest would read
        # it as a step.
        if not (operation and operation.strip()):
            operation = "chat"
        attributes: dict[str, Any] = {
            **run.inherited,
            contract.OPERATION: operation,
            contract.REQUEST_MODEL: model,
        }
        if provider:
            attributes[contract.PROVIDER] = provider
        if input_messages is not None:
            attributes[contract.INPUT_MESSAGES] = _as_json(input_messages)
        if system_instructions is not None:
            attributes[contract.SYSTEM_INSTRUCTIONS] = _as_json(system_instructions)
        return self._child(
            run, ModelCall, f"{operation} {model}", attributes, SpanKind.CLIENT
        )

    def tool_call(
        self, name: str, run: Run | None, *, arguments: object | None
    ) -> ToolCall | None:
        run = run or self._outside("tool_call")
        if run is None:
            return None
        attributes: dict[str, Any] = {
            **run.inherited,
            contract.OPERATION: contract.TOOL_OPERATION,
            contract.TOOL_NAME: name,
        }
        if arguments is not None:
            attributes[contract.TOOL_CALL_ARGUMENTS] = _as_json(arguments)
        return self._child(
            run, ToolCall, f"execute_tool {name}", attributes, SpanKind.INTERNAL
        )

    def _outside(self, what: str) -> Run | None:
        run = current_run()
        if run is None and not self._warned_outside:
            self._warned_outside = True
            logger.warning(
                "telmoni: %s opened outside a run; nothing is sent outside a run", what
            )
        return run

    def _child(
        self,
        run: Run,
        handle: type[C],
        name: str,
        attributes: Mapping[str, Any],
        kind: SpanKind,
    ) -> C:
        parent = context.get_current()
        if current_run(parent) is not run:
            # A context this run is not in, a worker thread's say, puts the
            # span under the run itself rather than at the root of a trace of
            # its own, and carries the run, so calls inside find it.
            parent = context.set_value(
                _RUN_KEY, run, trace.set_span_in_context(run._span, parent)
            )
        span = self._start(name, parent, attributes, kind)
        opened = handle(span)
        opened._attach(trace.set_span_in_context(span, parent))
        return opened

    def _start(
        self,
        name: str,
        parent: context.Context,
        attributes: Mapping[str, Any],
        kind: SpanKind,
        start_time: int | None = None,
    ) -> Span:
        # The exception's message and stack trace are content: the handle
        # records the class alone, so neither is recorded nor the status set.
        return self._client.tracer.start_span(
            name,
            context=parent,
            kind=kind,
            attributes=dict(attributes),
            start_time=start_time,
            record_exception=False,
            set_status_on_exception=False,
        )


def _bounds(bounds: Bounds | None) -> dict[str, Any]:
    if bounds is None:
        return {}
    out: dict[str, Any] = {}
    if bounds.stall_after_s is not None:
        out[contract.BOUND_STALL_AFTER_S] = int(bounds.stall_after_s)
    if bounds.max_steps is not None:
        out[contract.BOUND_MAX_STEPS] = int(bounds.max_steps)
    if bounds.max_cost_usd is not None:
        out[contract.BOUND_MAX_COST_USD] = float(bounds.max_cost_usd)
    return out


def _as_json(value: object) -> str:
    if isinstance(value, str):
        return value
    try:
        return json.dumps(value, default=str)
    except (TypeError, ValueError):
        # A cycle, or a key no JSON has: the value's own text, so content
        # never raises into the agent.
        return str(value)
