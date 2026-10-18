"""Runs, steps, model calls and tool calls, as context managers and decorators.

A run is a span of its own in whatever trace is current, opened by
``telmoni.run.start``; every span inside it carries the run's id, agent,
user and session, so ingest knows each span's run without waiting
for the trace's tree. Nothing is sent outside a run.
"""

from __future__ import annotations

import asyncio
import functools
import inspect
import itertools
import json
import logging
import math
import random
import uuid
from collections.abc import Callable, Iterator, Mapping
from contextvars import Token
from datetime import datetime, timedelta, timezone
from typing import TYPE_CHECKING, Any, Generic, TypeVar

from opentelemetry import context, trace
from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.trace import (
    NonRecordingSpan,
    Span,
    SpanContext,
    SpanKind,
    Status,
    StatusCode,
)

from . import contract, ids
from .models import (
    Bounds,
    CallRecord,
    ModelCallRecord,
    RunRecord,
    StepRecord,
    ToolCallRecord,
    Usage,
)

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

    def __init__(self, span: Span, client: Telmoni) -> None:
        self._span = span
        self._client = client
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
        sampled_out: bool = False,
    ) -> None:
        super().__init__(span, client)
        self.run_id = run_id
        self.agent = agent
        # Whether the sample rate kept this run out: nothing of it is sent,
        # nor handed to another service.
        self.sampled_out = sampled_out
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
        self._span.set_attributes(_usage(usage, self._client.exporter.warn_once))
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
        session_id: str | None,
        tags: Mapping[str, str] | None,
        bounds: Bounds | None,
        sample_rate: float | None,
    ) -> Run:
        if not agent.strip():
            raise ValueError("a run needs its agent's name")
        rate = self._client.config.sample_rate if sample_rate is None else sample_rate
        if not 0 < rate <= 1:
            raise ValueError("sample_rate must be above 0 and up to 1")
        limits = _bounds(bounds, self._client.exporter.warn_once)
        sampled_out = rate < 1 and random.random() >= rate
        run_id = str(uuid.uuid4())
        inherited: dict[str, str] = {contract.RUN_ID: run_id, contract.AGENT: agent}
        if user_id:
            inherited[contract.USER_ID] = user_id
        if session_id:
            inherited[contract.SESSION_ID] = session_id
        # On every span of the run from its start, for the processor to drop:
        # a sampler's drop would clear the flag the application's spans inherit.
        if sampled_out:
            inherited[contract.SAMPLED_OUT] = "true"
        # Tags first: past the span's attribute limit the oldest is dropped,
        # which must be a tag rather than the run's id or its mark.
        attributes: dict[str, Any] = {
            f"{contract.TAG_PREFIX}{key}": value for key, value in (tags or {}).items()
        }
        attributes.update(
            {
                contract.OPERATION: "invoke_agent",
                contract.AGENT_NAME: agent,
                **inherited,
                **limits,
                contract.SAMPLE_RATE: float(rate),
            }
        )
        parent = context.get_current()
        span = self._start(
            f"invoke_agent {agent}", parent, attributes, SpanKind.INTERNAL
        )
        ctx = trace.set_span_in_context(span, parent)
        start = span.start_time if isinstance(span, ReadableSpan) else None
        # OpenTelemetry sends a span only once it ends; without this one a run
        # stalled in its first call would never be heard.
        opener = self._start(
            contract.RUN_START,
            ctx,
            {**inherited, **limits, contract.SAMPLE_RATE: float(rate)},
            SpanKind.INTERNAL,
            start_time=start,
        )
        opener.end(end_time=start)
        run = Run(
            self._client,
            span,
            run_id=run_id,
            agent=agent,
            inherited=inherited,
            sampled_out=sampled_out,
        )
        # The run is found from the context by step(), model_call() and
        # tool_call() called on the client rather than on the run.
        run._attach(context.set_value(_RUN_KEY, run, ctx))
        return run

    def continued(self, remote: SpanContext, run_id: str, agent: str) -> Run:
        """A run another service opened, continued here under the span the
        headers named: a run whose own span is the caller's, non-recording
        here, so its steps and calls nest under it and carry the run, and
        nothing of it is sent or ended from this side."""
        inherited = {contract.RUN_ID: run_id, contract.AGENT: agent}
        return Run(
            self._client,
            NonRecordingSpan(remote),
            run_id=run_id,
            agent=agent,
            inherited=inherited,
        )

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

    def record(self, record: RunRecord) -> None:
        """Send a run that has ended as the live calls would have sent it.

        Its trace is its own, named by the run's id, whatever trace is current;
        each span's id is derived from that id and the span's place in the
        record. Its spans are queued together under the processor's hold, the
        run's start first and the run's own last, so the same record sent again
        is the same bytes. Raises ``QueueFullError``, sending nothing, when
        the export queue has no room for the whole record.
        """
        if not record.agent.strip():
            raise ValueError("a run needs its agent's name")
        try:
            run_uuid = uuid.UUID(record.run_id)
        except (TypeError, ValueError):
            raise ValueError("a recorded run's id must be a UUID") from None
        # The run's id is its trace's, and a trace id of zeros is no trace at all.
        if run_uuid.int == 0:
            raise ValueError(
                "a recorded run's id must be a UUID other than the nil UUID"
            )
        # Checked whole before anything is queued, so a record refused sends
        # nothing rather than half a run.
        for item in _walk(record):
            if _nanos(item.end) < _nanos(item.start):
                raise ValueError("a recorded span cannot end before it starts")
        rate = self._client.config.sample_rate
        run_id = str(run_uuid)
        inherited: dict[str, str] = {
            contract.RUN_ID: run_id,
            contract.AGENT: record.agent,
        }
        if record.user_id:
            inherited[contract.USER_ID] = record.user_id
        if record.session_id:
            inherited[contract.SESSION_ID] = record.session_id
        attributes: dict[str, Any] = {
            contract.OPERATION: "invoke_agent",
            contract.AGENT_NAME: record.agent,
            **inherited,
            contract.SAMPLE_RATE: float(rate),
        }
        for key, value in (record.tags or {}).items():
            attributes[f"{contract.TAG_PREFIX}{key}"] = value
        # Sampled out, no span is built: none of a record's ends after it, and
        # the same record sent again draws again.
        if rate < 1 and random.random() >= rate:
            return
        # Queued whole or not at all: a record that outgrows the queue's room
        # would lose its tail, the run's own span first.
        need = 2 + sum(
            1 + (len(item.calls) if isinstance(item, StepRecord) else 0)
            for item in record.inside
        )
        places = itertools.count()

        def open_span(
            name: str,
            parent: context.Context,
            attributes: Mapping[str, Any],
            kind: SpanKind,
            at: datetime,
        ) -> Span:
            span_id = ids.span_id(run_uuid, next(places))
            with ids.given(run_uuid.int, span_id):
                return self._start(name, parent, attributes, kind, _nanos(at))

        def call(item: CallRecord, parent: context.Context) -> None:
            if isinstance(item, ModelCallRecord):
                operation = item.operation if item.operation.strip() else "chat"
                attributes: dict[str, Any] = {
                    **inherited,
                    contract.OPERATION: operation,
                    contract.REQUEST_MODEL: item.model,
                }
                if item.provider:
                    attributes[contract.PROVIDER] = item.provider
                attributes.update(_usage(item.usage, self._client.exporter.warn_once))
                name, kind = f"{operation} {item.model}", SpanKind.CLIENT
            else:
                attributes = {
                    **inherited,
                    contract.OPERATION: contract.TOOL_OPERATION,
                    contract.TOOL_NAME: item.name,
                }
                name, kind = f"execute_tool {item.name}", SpanKind.INTERNAL
            _close(open_span(name, parent, attributes, kind, item.start), item)

        with self._client.processor.hold(need):
            root = context.Context()
            span = open_span(
                f"invoke_agent {record.agent}",
                root,
                attributes,
                SpanKind.INTERNAL,
                record.start,
            )
            ctx = trace.set_span_in_context(span, root)
            opener = open_span(
                contract.RUN_START,
                ctx,
                {**inherited, contract.SAMPLE_RATE: float(rate)},
                SpanKind.INTERNAL,
                record.start,
            )
            opener.end(end_time=_nanos(record.start))
            for item in record.inside:
                if isinstance(item, StepRecord):
                    step = open_span(
                        item.name, ctx, inherited, SpanKind.INTERNAL, item.start
                    )
                    inner = trace.set_span_in_context(step, ctx)
                    for each in item.calls:
                        call(each, inner)
                    _close(step, item)
                else:
                    call(item, ctx)
            _close(span, record)

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
        opened = handle(span, self._client)
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


def _usage(
    usage: Usage | None, warn_once: Callable[[str, str], None]
) -> dict[str, int]:
    """The counts as integers, which alone ingest takes; one JSON's integers
    cannot carry is left off the call with one warning, not the batch lost."""
    if usage is None:
        return {}
    counts = (
        (contract.INPUT_TOKENS, usage.input_tokens),
        (contract.OUTPUT_TOKENS, usage.output_tokens),
        (contract.CACHE_READ_INPUT_TOKENS, usage.cache_read_input_tokens),
        (contract.CACHE_WRITE_INPUT_TOKENS, usage.cache_write_input_tokens),
    )
    out: dict[str, int] = {}
    for key, value in counts:
        if value is None:
            continue
        count = _whole(value)
        if count is None:
            warn_once(
                f"usage:{key}",
                f"telmoni: {key} must be a finite number within 64 bits; "
                "the call is sent without it",
            )
        else:
            out[key] = count
    return out


_EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)


def _nanos(at: datetime) -> int:
    """A recorded time as OpenTelemetry's nanoseconds, in whole microseconds,
    as a datetime holds it."""
    if at.tzinfo is None or at.utcoffset() is None:
        raise ValueError("a recorded time needs its timezone")
    return (at - _EPOCH) // timedelta(microseconds=1) * 1000


def _walk(
    record: RunRecord,
) -> Iterator[RunRecord | StepRecord | ModelCallRecord | ToolCallRecord]:
    yield record
    for item in record.inside:
        yield item
        if isinstance(item, StepRecord):
            yield from item.calls


def _close(
    span: Span, item: RunRecord | StepRecord | ModelCallRecord | ToolCallRecord
) -> None:
    if item.error_type:
        span.set_attribute(contract.ERROR_TYPE, item.error_type)
        span.set_status(Status(StatusCode.ERROR))
    if item.cancelled:
        span.set_attribute(contract.CANCELLED, True)
    span.end(end_time=_nanos(item.end))


def _bounds(
    bounds: Bounds | None, warn_once: Callable[[str, str], None]
) -> dict[str, Any]:
    """What the run is held to; a bound JSON cannot carry is left off with one
    warning, so a computed one never stops the agent's call nor loses a batch."""
    if bounds is None:
        return {}
    out: dict[str, Any] = {}

    def keep(name: str, key: str, number: float | None) -> None:
        if number is None:
            warn_once(
                f"run:{name}",
                f"telmoni: a run's {name} must be a finite number within 64 bits; "
                "the run is sent without it",
            )
        else:
            out[key] = number

    if bounds.stall_after_s is not None:
        keep(
            "stall_after_s", contract.BOUND_STALL_AFTER_S, _whole(bounds.stall_after_s)
        )
    if bounds.max_steps is not None:
        keep("max_steps", contract.BOUND_MAX_STEPS, _whole(bounds.max_steps))
    if bounds.max_cost_usd is not None:
        keep("max_cost_usd", contract.BOUND_MAX_COST_USD, _finite(bounds.max_cost_usd))
    return out


_INT64 = range(-(2**63), 2**63)


def _whole(value: float) -> int | None:
    """`value` as a whole number within 64 bits, exact where given as one; None
    for what JSON's integers cannot carry."""
    try:
        number = value if isinstance(value, int) else int(float(value))
    except (TypeError, ValueError, OverflowError):
        return None
    return number if number in _INT64 else None


def _finite(value: float) -> float | None:
    """`value` as a finite float; None for NaN, an infinity or no number."""
    try:
        number = float(value)
    except (TypeError, ValueError, OverflowError):
        return None
    return number if math.isfinite(number) else None


def _as_json(value: object) -> str:
    if isinstance(value, str):
        return value
    try:
        return json.dumps(value, default=str)
    except (TypeError, ValueError):
        # A cycle, or a key no JSON has: the value's own text, so content
        # never raises into the agent.
        return str(value)
