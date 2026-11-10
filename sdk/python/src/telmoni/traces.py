"""Traces, steps, model calls, tool calls and retrievals, as context managers
and decorators.

A trace is one execution of an agent: its agent's span at the root of a trace
of its own, linked to whatever span the application had current; every span
inside it carries the agent, user and session, so ingest knows each span's
trace without waiting for its tree. Nothing is sent outside a trace.
"""

from __future__ import annotations

import asyncio
import contextvars
import functools
import inspect
import itertools
import json
import math
import threading
import traceback
from collections.abc import (
    AsyncIterator,
    Callable,
    Generator,
    Iterator,
    Mapping,
    Sequence,
)
from contextlib import aclosing
from contextvars import Token
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from types import MappingProxyType
from typing import TYPE_CHECKING, Any, Generic, TypeAlias, TypeVar

from opentelemetry import context, trace
from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.trace import (
    Link,
    NonRecordingSpan,
    Span,
    SpanContext,
    SpanKind,
    Status,
    StatusCode,
)

from . import contract, guard, ids, scores
from .models import (
    CallRecord,
    ModelCallRecord,
    StepRecord,
    ToolCallRecord,
    TraceRecord,
    Usage,
    WarningInfo,
    _blank,
    _from_zero,
)

if TYPE_CHECKING:
    from .client import Telmoni

_TRACE_KEY = context.create_key("telmoni.trace")
# Every warning here is about what the code handed the SDK.
_INPUT = WarningInfo("input")

H = TypeVar("H", bound="Handle")
C = TypeVar("C", bound="Handle")
F = TypeVar("F", bound=Callable[..., Any])


def current_trace(ctx: context.Context | None = None) -> Trace | None:
    """The trace open in this context, if any."""
    opened = context.get_value(_TRACE_KEY, ctx)
    return opened if isinstance(opened, Trace) else None


class Handle:
    """A span the SDK opened, until its ``_end``."""

    def __init__(self, span: Span, client: Telmoni) -> None:
        self._span = span
        self._client = client
        self._token: Token[context.Context] | None = None
        self._ended = False
        self._scored = 0
        self._scoring = threading.Lock()

    @property
    def span_id(self) -> str:
        """Its span's id, 16 lowercase hex digits, which a late score names."""
        return format(self._span.get_span_context().span_id, "016x")

    def score(
        self,
        name: str,
        value: bool | float | str,
        *,
        range: Sequence[float] | None = None,
        version: str | None = None,
    ) -> None:
        """Score its span now: a number, within ``range`` where given, a yes or
        no, or a category, by the scorer's ``version``. One breaking a score's
        rules is not sent, with one warning."""
        warn_once = self._client.exporter.warn_once
        judgement = scores.judged(name, value, range, version, warn_once)
        if judgement is None:
            return
        # Under the lock the end takes, so no score lands once the span ended.
        with self._scoring:
            why = None
            if self._ended:
                why = scores.ENDED
            elif self._scored >= contract.SCORES_PER_SPAN_MAX:
                why = scores.FULL
            else:
                self._scored += 1
                self._span.add_event(contract.EVALUATION_EVENT, judgement.event())
        # Warned once the lock is let go: the sink is the application's.
        if why is not None:
            scores.refused(why, warn_once)

    def _attach(self, ctx: context.Context) -> None:
        """Make ``ctx`` current until ``_end``, so spans opened inside nest."""
        self._token = context.attach(ctx)

    def fail(self, error_type: str | BaseException | type[BaseException]) -> None:
        """Mark it failed with the error's class; an exception's message and
        stack trace are content, kept on the span for the exporter's lock."""
        warn_once = self._client.exporter.warn_once
        text = _error_type(error_type, warn_once) if error_type else ""
        # Ingest refuses the span over an empty one, and with the agent's, the trace.
        if text:
            text = _cut(text, contract.ERROR_TYPE_MAX, "span's error type", warn_once)
            self._span.set_attribute(contract.ERROR_TYPE, text)
        if not isinstance(error_type, BaseException):
            self._span.set_status(Status(StatusCode.ERROR))
            return
        message = _str(error_type)
        attributes = {
            contract.EXCEPTION_MESSAGE: message,
            contract.EXCEPTION_STACKTRACE: "".join(
                traceback.format_exception(error_type)
            ),
        }
        if text:
            attributes[contract.EXCEPTION_TYPE] = text
        self._span.add_event(contract.EXCEPTION_EVENT, attributes)
        self._span.set_status(Status(StatusCode.ERROR, message or None))

    def cancel(self) -> None:
        """Mark it cancelled by the agent, which is neither ok nor an error."""
        self._span.set_attribute(contract.CANCELLED, True)

    def _end(self, exc: BaseException | None) -> None:
        if not self._ending():
            return
        # A consumer that stops reading a generator cancels, as an interrupt does.
        if isinstance(exc, (KeyboardInterrupt, asyncio.CancelledError, GeneratorExit)):
            self.cancel()
        elif exc is not None:
            self.fail(exc)
        self._span.end()
        if self._token is not None:
            context.detach(self._token)

    def _ending(self) -> bool:
        """Whether this end is its first; a second does nothing."""
        with self._scoring:
            ended, self._ended = self._ended, True
        return not ended


class Trace(Handle):
    """One execution of an agent: its agent's span, and the trace it heads,
    whose id ``trace_id`` holds as 32 hex digits."""

    def __init__(
        self,
        client: Telmoni,
        span: Span,
        *,
        trace_id: str,
        agent: str,
        inherited: Mapping[str, str],
        caps: guard.Guard | None = None,
    ) -> None:
        super().__init__(span, client)
        self.trace_id = trace_id
        self.agent = agent
        self._inherited = dict(inherited)
        self._guard = caps or guard.Guard(None, None)

    @property
    def inherited(self) -> dict[str, str]:
        """What every span inside the trace carries."""
        return dict(self._inherited)

    @property
    def spent(self) -> float:
        """What the trace's model calls have cost so far in US dollars, counted
        as its spend cap counts them, in this process."""
        return self._guard.spent

    def answered(self, *, output_messages: object | None = None) -> None:
        """What the agent answered: its messages. An answer given after the
        trace ended is not sent."""
        if self._ended:
            self._client.exporter.warn_once(
                "trace:late",
                "telmoni: a trace must be answered before it ends; an answer given "
                "after is not sent",
                _INPUT,
            )
            return
        if output_messages is not None:
            self._span.set_attribute(
                contract.OUTPUT_MESSAGES, _as_json(output_messages)
            )

    def step(self, name: str) -> Scope[Step]:
        return self._client.step(name, trace=self)

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
    ) -> Scope[ModelCall]:
        return self._client.model_call(
            model,
            provider=provider,
            operation=operation,
            prompt=prompt,
            prompt_version=prompt_version,
            input_messages=input_messages,
            system_instructions=system_instructions,
            trace=self,
        )

    def tool_call(
        self, name: str, *, arguments: object | None = None
    ) -> Scope[ToolCall]:
        return self._client.tool_call(name, arguments=arguments, trace=self)

    def retrieval(
        self,
        source: str,
        *,
        query: object | None = None,
        top_k: int | None = None,
        memory: bool = False,
    ) -> Scope[Retrieval]:
        return self._client.retrieval(
            source, query=query, top_k=top_k, memory=memory, trace=self
        )


class Step(Handle):
    pass


class ModelCall(Handle):
    # What the guard prices it by, its usage as its span holds it, and what it
    # last counted to its trace.
    _trace: Trace | None = None
    _provider = ""
    _model = ""
    _held: Mapping[str, int | float] = MappingProxyType({})
    _counted = 0.0

    def __init__(self, span: Span, client: Telmoni) -> None:
        super().__init__(span, client)
        # Answers from several threads are held and counted one at a time.
        self._answering = threading.Lock()

    def answered(
        self,
        *,
        response_model: str | None = None,
        usage: Usage | None = None,
        output_messages: object | None = None,
        time_to_first_token: float | None = None,
    ) -> None:
        """What the model answered: its name, its tokens, its messages, and the
        seconds to its first token. An answer given after the call ended is not
        sent, and counts nothing."""
        warn_once = self._client.exporter.warn_once
        model = None
        # Ingest refuses the call over an empty one.
        if response_model is not None and (
            named := _text(response_model, "response model", warn_once)
        ):
            model = _cut(named, contract.MODEL_MAX, "response model", warn_once)
        sent = _usage(usage, warn_once)
        messages = None if output_messages is None else _as_json(output_messages)
        first_token = None
        if time_to_first_token is not None:
            # Ingest refuses the call over one below 0 or not finite, as a cost.
            first_token = _from_zero(time_to_first_token)
            if first_token is None:
                warn_once(
                    f"call:{contract.TIME_TO_FIRST_CHUNK}",
                    f"telmoni: {contract.TIME_TO_FIRST_CHUNK} must be a finite "
                    "number of seconds from 0; the call is sent without it",
                    _INPUT,
                )
        warning: tuple[str, str] | None = None
        with self._answering:
            if self._ended:
                warning = (
                    "call:late",
                    (
                        "telmoni: a model call must be answered before it ends; an "
                        "answer given after is not sent and counts nothing toward the "
                        "trace's spend"
                    ),
                )
            else:
                if model is not None:
                    # The name the call is priced by, as ingest reads it.
                    self._model = model
                    self._span.set_attribute(contract.RESPONSE_MODEL, model)
                self._span.set_attributes(sent)
                if messages is not None:
                    self._span.set_attribute(contract.OUTPUT_MESSAGES, messages)
                if first_token is not None:
                    self._span.set_attribute(contract.TIME_TO_FIRST_CHUNK, first_token)
                self._held = {**self._held, **sent}
                if self._trace is not None:
                    warning = self._count(self._trace)
        # Given once the lock is released: a logging handler is the application's.
        if warning is not None:
            warn_once(*warning, _INPUT)

    def _ending(self) -> bool:
        # Under the lock its answers are held under, so none lands past the end.
        with self._answering:
            return super()._ending()

    def _count(self, owner: Trace) -> tuple[str, str] | None:
        """Its cost, reported, else priced by its name, else nothing, to its
        trace's spend in place of what it counted; and the warning that calls
        for."""
        held = self._held
        tokens = guard.Tokens(
            int(held.get(contract.INPUT_TOKENS, 0)),
            int(held.get(contract.OUTPUT_TOKENS, 0)),
            int(held.get(contract.CACHE_READ_INPUT_TOKENS, 0)),
            int(held.get(contract.CACHE_CREATION_INPUT_TOKENS, 0)),
        )
        cost = held.get(contract.COST)
        if cost is None:
            cost = guard.priced(
                self._client.prices, self._provider, self._model, tokens
            )
        warning = None
        if cost is None:
            cost = 0.0
            if owner._guard.capped and tokens.some():
                warning = (
                    f"unpriced:{self._provider}:{self._model}",
                    (
                        "telmoni: no price for "
                        f"{self._model or 'a call with no model'} "
                        f"({self._provider or 'no provider'}), which reported no "
                        "cost; it counts nothing toward the trace's spend cap"
                    ),
                )
        owner._guard.counts(self._counted, cost)
        self._counted = cost
        return warning


class ToolCall(Handle):
    def returned(self, result: object) -> None:
        """What the tool returned: content, sent only when asked and allowed."""
        self._span.set_attribute(contract.TOOL_CALL_RESULT, _as_json(result))


class Retrieval(Handle):
    # Whether it searched a memory, whose names its finds go under.
    _memory = False

    def returned(self, documents: list[Any] | tuple[Any, ...]) -> None:
        """What the search found, a list: content, sent only when asked and
        allowed, and how many it holds, sent in every mode."""
        # Refused whole: a count once set stays, beside any documents set after.
        if not isinstance(documents, (list, tuple)):
            self._client.exporter.warn_once(
                "retrieval:documents",
                "telmoni: a retrieval's documents must be a list; they are neither "
                "sent nor counted",
                _INPUT,
            )
            return
        found, count = (
            (contract.MEMORY_RECORDS, contract.MEMORY_RECORD_COUNT)
            if self._memory
            else (contract.RETRIEVAL_DOCUMENTS, contract.RETRIEVAL_RETURNED)
        )
        # One call, under the span's one lock, so the last list sets both.
        self._span.set_attributes({found: _as_json(documents), count: len(documents)})


class Scope(Generic[H]):
    """Opens a handle on ``with``, ``async with``, or each call of a decorated
    function; ``None`` from ``_open`` means nothing is opened (no trace)."""

    def __init__(self, open_handle: Callable[[], H | None]) -> None:
        self._open = open_handle
        self._handle: H | None = None
        # Held while open: entered again meanwhile, nested or from another task
        # or thread, it would lose the first handle and detach the wrong context.
        self._busy = threading.Lock()

    def __enter__(self) -> H | Nothing:
        if not self._busy.acquire(blocking=False):
            raise RuntimeError(
                "this scope is open; call telmoni.step() (or trace, model_call, "
                "tool_call, retrieval) again for each block that opens one"
            )
        try:
            self._handle = self._open()
        except BaseException:
            self._busy.release()
            raise
        return self._handle if self._handle is not None else Nothing()

    def __exit__(self, exc_type: object, exc: BaseException | None, tb: object) -> None:
        handle, self._handle = self._handle, None
        self._busy.release()
        if handle is not None:
            handle._end(exc)

    async def __aenter__(self) -> H | Nothing:
        return self.__enter__()

    async def __aexit__(
        self, exc_type: object, exc: BaseException | None, tb: object
    ) -> None:
        self.__exit__(exc_type, exc, tb)

    def __call__(self, fn: F) -> F:
        # A generator function runs nothing when called: the scope spans its
        # iteration instead.
        if inspect.isasyncgenfunction(fn):

            @functools.wraps(fn)
            async def wrapped_agen(*args: Any, **kwargs: Any) -> AsyncIterator[Any]:
                async with Scope(self._open), aclosing(fn(*args, **kwargs)) as items:
                    async for item in items:
                        yield item

            return wrapped_agen  # type: ignore[return-value]
        if inspect.isgeneratorfunction(fn):

            @functools.wraps(fn)
            def wrapped_gen(*args: Any, **kwargs: Any) -> Generator[Any, Any, Any]:
                items = _spanning(Scope(self._open), fn(*args, **kwargs))
                return (yield from _InContext(items))

            return wrapped_gen  # type: ignore[return-value]
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


def _spanning(
    scope: Scope[Any], items: Generator[Any, Any, Any]
) -> Generator[Any, Any, Any]:
    with scope:
        return (yield from items)


class _InContext(Generator[Any, Any, Any]):
    """A generator resumed in one context of its own, whatever context asks for
    an item: a thread pool's copy per item would lose the scope's."""

    def __init__(self, items: Generator[Any, Any, Any]) -> None:
        self._items = items
        self._own = contextvars.copy_context()

    def send(self, value: Any) -> Any:
        return self._own.run(self._items.send, value)

    def throw(self, *exc: Any) -> Any:
        return self._own.run(self._items.throw, *exc)


class TraceScope(Scope[Trace]):
    """A trace always opens, so its handle is never the no-op one."""

    def __enter__(self) -> Trace:
        super().__enter__()
        handle = self._handle
        if handle is None:
            self.__exit__(None, None, None)
            raise RuntimeError("a trace did not open")
        return handle

    async def __aenter__(self) -> Trace:
        return self.__enter__()


class Nothing:
    """The handle given outside a trace: every method does nothing."""

    trace_id = ""
    agent = ""
    span_id = ""

    def score(self, name: str, value: object, **kwargs: object) -> None:
        return None

    def fail(self, error_type: str | BaseException | type[BaseException]) -> None:
        return None

    def cancel(self) -> None:
        return None

    def answered(self, **kwargs: object) -> None:
        return None

    def returned(self, *given: object, **named: object) -> None:
        return None


class Opener:
    """Builds the handles for one client; the client holds one."""

    def __init__(self, client: Telmoni) -> None:
        self._client = client

    def trace(
        self,
        agent: str,
        *,
        user_id: str | None,
        session_id: str | None,
        tags: Mapping[str, str] | None,
        max_steps: int | None = None,
        max_spend: float | None = None,
        input_messages: object | None = None,
    ) -> Trace:
        warn_once = self._client.exporter.warn_once
        given: object = agent
        if given is None or _blank(_str(given)):
            raise ValueError("a trace needs its agent's name")
        agent = _cut(
            _text(given, "trace's agent", warn_once),
            contract.AGENT_MAX,
            "trace's agent",
            warn_once,
        )
        caps = guard.Guard(*guard.caps(max_steps, max_spend))
        user = _text(user_id, "user id", warn_once) if user_id is not None else ""
        if user and not _within(
            user, contract.USER_ID_MAX, "user id", "trace", warn_once
        ):
            user = ""
        session = (
            _text(session_id, "session id", warn_once) if session_id is not None else ""
        )
        if session and not _within(
            session, contract.SESSION_ID_MAX, "session id", "trace", warn_once
        ):
            session = ""
        inherited = _inherited(agent, user, session)
        name, attributes = _agent_opening(agent, inherited, tags, warn_once)
        if input_messages is not None:
            attributes[contract.INPUT_MESSAGES] = _as_json(input_messages)
        application = context.get_current()
        # A trace of its own, linked to the span the application has current:
        # ingest reads each trace whole, never a tree the application heads.
        current = trace.get_current_span(application).get_span_context()
        links = [Link(current)] if current.is_valid else []
        span = self._start(
            name, context.Context(), attributes, SpanKind.INTERNAL, links=links
        )
        ctx = trace.set_span_in_context(span, application)
        # The trace's opening, under its agent's span and at its start: the
        # processor sends it at once, so the trace is seen open before it ends.
        opened_at = span.start_time if isinstance(span, ReadableSpan) else None
        start = self._start(
            contract.TRACE_START, ctx, inherited, SpanKind.INTERNAL, opened_at
        )
        start.end(end_time=opened_at)
        opened = Trace(
            self._client,
            span,
            trace_id=format(span.get_span_context().trace_id, "032x"),
            agent=agent,
            inherited=inherited,
            caps=caps,
        )
        # The trace is found from the context by step(), model_call() and
        # tool_call() called on the client rather than on the trace.
        opened._attach(context.set_value(_TRACE_KEY, opened, ctx))
        return opened

    def continued(self, remote: SpanContext, agent: str) -> Trace:
        """A trace another service opened, continued here under the span the
        headers named: a trace whose own span is the caller's, non-recording
        here, so its steps and calls nest under it and carry the agent, and
        nothing of it is sent or ended from this side."""
        warn_once = self._client.exporter.warn_once
        agent = _cut(agent, contract.AGENT_MAX, "trace's agent", warn_once)
        return Trace(
            self._client,
            NonRecordingSpan(remote),
            trace_id=format(remote.trace_id, "032x"),
            agent=agent,
            inherited=_inherited(agent, None, None),
        )

    def step(self, name: str, owner: Trace | None) -> Step | None:
        owner = owner or self._outside("step")
        if owner is None:
            return None
        owner._guard.opens(step=True)
        warn_once = self._client.exporter.warn_once
        name = _text(name, "step's name", warn_once)
        # Ingest refuses a span named nothing.
        if not name:
            warn_once(
                "step:name",
                "telmoni: a step needs its name; one without is sent named step",
                _INPUT,
            )
            name = "step"
        name = _cut(name, contract.NAME_MAX, "step's name", warn_once)
        return self._child(owner, Step, name, owner.inherited, SpanKind.INTERNAL)

    def model_call(
        self,
        model: str,
        owner: Trace | None,
        *,
        provider: str | None,
        operation: str,
        prompt: str | None,
        prompt_version: str | None,
        input_messages: object | None,
        system_instructions: object | None,
    ) -> ModelCall | None:
        owner = owner or self._outside("model_call")
        if owner is None:
            return None
        owner._guard.opens(step=False)
        warn_once = self._client.exporter.warn_once
        model = _text(model, "model", warn_once)
        # Ingest refuses a call over an empty model, but keeps one with none.
        if model:
            model = _cut(model, contract.MODEL_MAX, "model", warn_once)
        else:
            warn_once(
                "call:model",
                "telmoni: a model call needs its model; one without is sent "
                "named by its operation alone",
                _INPUT,
            )
        named = _text(provider, "provider", warn_once) if provider is not None else ""
        provider = _cut(named, contract.PROVIDER_MAX, "provider", warn_once)
        operation = _operation(operation, warn_once)
        prompt, prompt_version = _prompt(prompt, prompt_version, warn_once)
        name, own = _model_call_opening(
            model, operation, provider, prompt, prompt_version
        )
        attributes: dict[str, Any] = {**owner.inherited, **own}
        if input_messages is not None:
            attributes[contract.INPUT_MESSAGES] = _as_json(input_messages)
        if system_instructions is not None:
            attributes[contract.SYSTEM_INSTRUCTIONS] = _as_json(system_instructions)
        call = self._child(owner, ModelCall, name, attributes, SpanKind.CLIENT)
        call._trace = owner
        call._provider = provider
        call._model = model
        return call

    def tool_call(
        self, name: str, owner: Trace | None, *, arguments: object | None
    ) -> ToolCall | None:
        owner = owner or self._outside("tool_call")
        if owner is None:
            return None
        owner._guard.opens(step=False)
        warn_once = self._client.exporter.warn_once
        name = _cut(
            _text(name, "tool's name", warn_once),
            contract.NAME_MAX,
            "tool's name",
            warn_once,
        )
        span_name, own = _tool_call_opening(name)
        attributes: dict[str, Any] = {**owner.inherited, **own}
        if arguments is not None:
            attributes[contract.TOOL_CALL_ARGUMENTS] = _as_json(arguments)
        return self._child(owner, ToolCall, span_name, attributes, SpanKind.INTERNAL)

    def retrieval(
        self,
        source: str,
        owner: Trace | None,
        *,
        query: object | None,
        top_k: int | None,
        memory: bool,
    ) -> Retrieval | None:
        owner = owner or self._outside("retrieval")
        if owner is None:
            return None
        owner._guard.opens(step=False)
        warn_once = self._client.exporter.warn_once
        operation, source_key, query_key = (
            (
                contract.SEARCH_MEMORY,
                contract.MEMORY_STORE_ID,
                contract.MEMORY_QUERY_TEXT,
            )
            if memory
            else (
                contract.RETRIEVAL_OPERATION,
                contract.DATA_SOURCE_ID,
                contract.RETRIEVAL_QUERY_TEXT,
            )
        )
        attributes: dict[str, Any] = {**owner.inherited, contract.OPERATION: operation}
        name = operation
        source = _text(source, "retrieval's source", warn_once)
        # Ingest names a retrieval by its source, and one with none by its span.
        if _blank(source):
            warn_once(
                "retrieval:source",
                "telmoni: a retrieval needs its source; one without is sent named "
                "by its operation alone",
                _INPUT,
            )
        else:
            source = _cut(source, contract.NAME_MAX, "retrieval's source", warn_once)
            attributes[source_key] = source
            if not memory:
                name = f"{operation} {source}"
        if top_k is not None:
            asked = _whole(top_k, _COUNT)
            if asked is None:
                warn_once(
                    f"retrieval:{contract.RETRIEVAL_TOP_K}",
                    f"telmoni: {contract.RETRIEVAL_TOP_K} must be a whole number "
                    "from 0 within 64 bits; the retrieval is sent without it",
                    _INPUT,
                )
            else:
                attributes[contract.RETRIEVAL_TOP_K] = asked
        if query is not None:
            attributes[query_key] = _as_json(query)
        retrieval = self._child(owner, Retrieval, name, attributes, SpanKind.CLIENT)
        retrieval._memory = bool(memory)
        return retrieval

    def record(self, record: TraceRecord) -> None:
        """Send a trace that has ended as the live calls would have sent it.

        Its trace id is the record's, whatever trace is current; each span's id
        is derived from that id and the span's place in the record. Its spans
        are queued whole once built, the agent's own last, and travel in
        exports of their own, so the same record sent again is the same bytes,
        whatever else is sent meanwhile.
        Raises ``QueueFullError``, sending nothing, when the export queue has
        no room for the whole record, and ``ShutdownError`` once the client is
        shut down.
        """
        # Checked whole before anything opens, so a refused record sends nothing.
        if not isinstance(record, TraceRecord):
            raise TypeError("a recorded trace must be a TraceRecord")
        _check(record)
        # Read before the items, whose reading runs the caller's code.
        tags = dict((record.tags or {}).items())
        if not all(isinstance(text, str) for pair in tags.items() for text in pair):
            raise TypeError("a recorded trace's tags must map str to str")
        if _blank(record.agent):
            raise ValueError("a trace needs its agent's name")
        trace_id = ids.trace_id(record.trace_id)
        if trace_id is None:
            raise ValueError(
                "a recorded trace's id must be 32 hex digits, not all zeros"
            )
        items = _read(record)
        held = list(_walk(items))
        spans = [record, *held]
        for each in spans:
            start, end = _nanos(each.start), _nanos(each.end)
            # Ingest reads a time as nanoseconds from the epoch in 64 bits, and
            # refuses the export over one that is not.
            if not (0 < start < _NANOS_END and 0 < end < _NANOS_END):
                raise ValueError(
                    "a recorded time must fall after the Unix epoch and less than "
                    "2^64 nanoseconds after it"
                )
            if end < start:
                raise ValueError("a recorded span cannot end before it starts")
        inherited = _inherited(record.agent, record.user_id, record.session_id)
        agent_name, attributes = _agent_opening(
            record.agent, inherited, tags, self._client.exporter.warn_once
        )
        for key, messages in (
            (contract.INPUT_MESSAGES, record.input_messages),
            (contract.OUTPUT_MESSAGES, record.output_messages),
        ):
            if messages is not None:
                attributes[key] = _as_json(messages)
        # Queued whole or not at all: a record that outgrows the queue's room
        # would lose its tail, the agent's own span first.
        need = len(spans)
        # Worked out before the hold: a count that warns runs the application's
        # logging handlers, which may record a trace of their own.
        warn_once = self._client.exporter.warn_once
        usages = {
            id(each): _usage(each.usage, warn_once)
            for each in spans
            if isinstance(each, ModelCallRecord)
        }
        operations = {
            id(each): _operation(each.operation, warn_once)
            for each in spans
            if isinstance(each, ModelCallRecord)
        }
        # Content too, whose encoding may run the caller's ``__str__``.
        contents = {id(each): _record_content(each) for each in held}
        judged = {id(each): _recorded_scores(each, warn_once) for each in spans}
        places = itertools.count()

        def open_span(
            name: str,
            parent: context.Context,
            attributes: Mapping[str, Any],
            kind: SpanKind,
            at: datetime,
        ) -> Span:
            span_id = ids.span_id(trace_id, next(places))
            with ids.given(trace_id, span_id):
                return self._start(name, parent, attributes, kind, _nanos(at))

        def call(item: CallRecord, parent: context.Context) -> None:
            if isinstance(item, ModelCallRecord):
                name, own = _model_call_opening(
                    item.model, operations[id(item)], item.provider
                )
                if item.reasoning_effort:
                    own[contract.REQUEST_REASONING_LEVEL] = item.reasoning_effort
                own.update(usages[id(item)])
                kind = SpanKind.CLIENT
            else:
                name, own = _tool_call_opening(item.name)
                kind = SpanKind.INTERNAL
            attributes = {**inherited, **own, **contents[id(item)]}
            _close(
                open_span(name, parent, attributes, kind, item.start),
                item,
                judged[id(item)],
            )

        def build(items: Sequence[_Read], parent: context.Context) -> None:
            for item, inside in items:
                if not isinstance(item, StepRecord):
                    call(item, parent)
                    continue
                step = open_span(
                    item.name,
                    parent,
                    {**inherited, **contents[id(item)]},
                    SpanKind.INTERNAL,
                    item.start,
                )
                build(inside, trace.set_span_in_context(step, parent))
                _close(step, item, judged[id(item)])

        with self._client.processor.hold(trace_id, need):
            root = context.Context()
            span = open_span(
                agent_name, root, attributes, SpanKind.INTERNAL, record.start
            )
            build(items, trace.set_span_in_context(span, root))
            _close(span, record, judged[id(record)])

    def _outside(self, what: str) -> Trace | None:
        opened = current_trace()
        if opened is None:
            self._client.exporter.warn_once(
                "outside",
                f"telmoni: {what} opened outside a trace; "
                "nothing is sent outside a trace",
                _INPUT,
            )
        return opened

    def _child(
        self,
        owner: Trace,
        handle: type[C],
        name: str,
        attributes: Mapping[str, Any],
        kind: SpanKind,
    ) -> C:
        parent = context.get_current()
        if current_trace(parent) is not owner:
            # A context this trace is not in, a worker thread's say, puts the
            # span under the agent's span rather than at the root of a trace
            # of its own, and carries the trace, so calls inside find it.
            parent = context.set_value(
                _TRACE_KEY, owner, trace.set_span_in_context(owner._span, parent)
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
        links: Sequence[Link] = (),
    ) -> Span:
        # The handle records a failure itself, its type cut to ingest's cap.
        return self._client.tracer.start_span(
            name,
            context=parent,
            kind=kind,
            attributes=dict(attributes),
            links=links,
            start_time=start_time,
            record_exception=False,
            set_status_on_exception=False,
        )


def _usage(
    usage: object, warn_once: Callable[[str, str, WarningInfo], None]
) -> dict[str, int | float]:
    """The counts as whole numbers from 0, and the cost as a finite number of
    dollars from 0, which alone ingest takes; another is left off the call with
    one warning, not the call refused."""
    if usage is None:
        return {}
    if not isinstance(usage, Usage):
        # A provider's own, Anthropic's or OpenAI's, lacks a field read below.
        warn_once(
            "usage:foreign",
            "telmoni: a usage must be telmoni's Usage, the provider's counts copied "
            "into it; the call is sent without it",
            _INPUT,
        )
        return {}
    counts = (
        (contract.INPUT_TOKENS, usage.input_tokens),
        (contract.OUTPUT_TOKENS, usage.output_tokens),
        (contract.CACHE_READ_INPUT_TOKENS, usage.cache_read_input_tokens),
        (contract.CACHE_CREATION_INPUT_TOKENS, usage.cache_write_input_tokens),
        (contract.REASONING_OUTPUT_TOKENS, usage.reasoning_output_tokens),
    )
    out: dict[str, int | float] = {}
    for key, value in counts:
        if value is None:
            continue
        count = _whole(value, _COUNT)
        if count is None:
            warn_once(
                f"usage:{key}",
                f"telmoni: {key} must be a whole number from 0 within 64 bits; "
                "the call is sent without it",
                _INPUT,
            )
        else:
            out[key] = count
    if usage.cost is not None:
        cost = _from_zero(usage.cost)
        if cost is None:
            warn_once(
                f"usage:{contract.COST}",
                f"telmoni: {contract.COST} must be a finite number of US dollars "
                "from 0; the call is sent without it",
                _INPUT,
            )
        else:
            out[contract.COST] = cost
    return out


def _inherited(
    agent: str, user_id: str | None, session_id: str | None
) -> dict[str, str]:
    """What every span of a trace carries, live or recorded: its agent, and
    its user and session where it has them."""
    inherited = {contract.AGENT: agent}
    if user_id:
        inherited[contract.USER_ID] = user_id
    if session_id:
        inherited[contract.SESSION_ID] = session_id
    return inherited


def _agent_opening(
    agent: str,
    inherited: Mapping[str, str],
    tags: Mapping[str, str] | None,
    warn_once: Callable[[str, str, WarningInfo], None],
) -> tuple[str, dict[str, Any]]:
    """The agent's span name and attributes, live or recorded, its tags first:
    past the span's 128, Python's SDK drops the oldest."""
    attributes = _tags(tags, warn_once)
    attributes.update(
        {
            contract.OPERATION: contract.AGENT_OPERATION,
            contract.AGENT_NAME: agent,
            **inherited,
        }
    )
    return f"{contract.AGENT_OPERATION} {agent}", attributes


def _operation(
    given: object, warn_once: Callable[[str, str, WarningInfo], None]
) -> str:
    """A model call's operation as ingest reads one: the default for a blank one,
    and, with one warning, for one ingest would file as a step, an agent or a
    tool call."""
    if not isinstance(given, str) or _blank(given):
        return contract.DEFAULT_MODEL_OPERATION
    if given in contract.MODEL_OPERATIONS:
        return given
    warn_once(
        "call:operation",
        "telmoni: a model call's operation must be chat, generate_content, "
        "text_completion or embeddings, which ingest reads as a model call; "
        "it is sent as chat",
        _INPUT,
    )
    return contract.DEFAULT_MODEL_OPERATION


def _model_call_opening(
    model: str,
    operation: str,
    provider: str | None,
    prompt: str = "",
    prompt_version: str = "",
) -> tuple[str, dict[str, Any]]:
    """A model call's span name and own attributes, live or recorded, its
    operation one ingest reads and an empty model, provider or prompt left off."""
    attributes: dict[str, Any] = {contract.OPERATION: operation}
    if model:
        attributes[contract.REQUEST_MODEL] = model
    if provider:
        attributes[contract.PROVIDER] = provider
    if prompt:
        attributes[contract.PROMPT_NAME] = prompt
        if prompt_version:
            attributes[contract.PROMPT_VERSION] = prompt_version
    return (f"{operation} {model}" if model else operation), attributes


def _prompt(
    prompt: object | None,
    version: object | None,
    warn_once: Callable[[str, str, WarningInfo], None],
) -> tuple[str, str]:
    """A live call's prompt and its version: a blank one is none, and one past
    its cap or a version without its name is left off with one warning."""

    def kept(value: object | None, what: str) -> str:
        text = "" if value is None else _text(value, what, warn_once)
        if _blank(text) or not _within(
            text, contract.ATTRIBUTE_VALUE_MAX, what, "call", warn_once
        ):
            return ""
        return text

    name = kept(prompt, "prompt's name")
    sent = kept(version, "prompt's version")
    if sent and not name:
        warn_once(
            "call:prompt_version",
            "telmoni: a prompt's version needs its prompt's name; "
            "the call is sent without it",
            _INPUT,
        )
        return "", ""
    return name, sent


def _tool_call_opening(name: str) -> tuple[str, dict[str, Any]]:
    """A tool call's span name and own attributes, live or recorded."""
    return f"{contract.TOOL_OPERATION} {name}", {
        contract.OPERATION: contract.TOOL_OPERATION,
        contract.TOOL_NAME: name,
    }


def _tags(
    tags: Mapping[str, str] | None, warn_once: Callable[[str, str, WarningInfo], None]
) -> dict[str, Any]:
    """A trace's tags as its agent's span's attributes: the first `TAGS_MAX`
    of those ingest keeps, since past them they would crowd out the span's own."""
    kept: dict[str, Any] = {}
    for key, value in (tags or {}).items():
        if value is None:
            continue
        name, sent = _str(key), _tag_value(value)
        # Ingest drops it before counting, so it takes no place among the 32.
        if not 0 < len(name) <= contract.TAG_KEY_MAX or (
            _shown(sent) > contract.TAG_VALUE_MAX
        ):
            warn_once(
                "trace:tag",
                "telmoni: a tag's key must be 1 to 64 characters and its value "
                "up to 256; the tag is not sent",
                _INPUT,
            )
        elif len(kept) == contract.TAGS_MAX:
            warn_once(
                "trace:tags",
                "telmoni: a trace carries up to 32 tags, the most ingest keeps; "
                "the rest are not sent",
                _INPUT,
            )
        elif isinstance(sent, str):
            kept[f"{contract.TAG_PREFIX}{name}"] = _text(value, "tag", warn_once)
        else:
            kept[f"{contract.TAG_PREFIX}{name}"] = sent
    return kept


def _tag_value(value: object) -> Any:
    """A tag's value: a bool, a 64-bit int or a finite float as it is, which
    ingest writes as its own text, and anything else as Python's."""
    if isinstance(value, bool) or (
        isinstance(value, int) and -(2**63) <= value < 2**63
    ):
        return value
    if isinstance(value, float) and math.isfinite(value):
        return value
    return _str(value)


def _shown(value: str | float) -> int:
    """The characters of ingest's text for a tag's value: a float's as Rust
    writes it, every digit and no exponent, so 1e300 takes 301."""
    if isinstance(value, str):
        return len(value)
    if not isinstance(value, float):
        return len(str(value if isinstance(value, bool) else int(value)))
    sign, digits, exponent = Decimal(repr(value)).normalize().as_tuple()
    point = len(digits) + int(exponent)
    if int(exponent) >= 0:
        return sign + point
    return sign + (len(digits) + 1 if point > 0 else 2 - point + len(digits))


def _text(
    value: object, what: str, warn_once: Callable[[str, str, WarningInfo], None]
) -> str:
    """`value` as text: one that is no string, an `Enum`'s member say, goes as
    its `str()` with one warning, where a span named by it would lose its batch."""
    if isinstance(value, str):
        return value
    warn_once(
        f"text:{what}",
        f"telmoni: a {what} that is no string is sent as its text",
        _INPUT,
    )
    return _str(value)


def _error_type(
    value: object, warn_once: Callable[[str, str, WarningInfo], None]
) -> str:
    """An error type that is no string: an exception's class, or a class's
    name, never its message, which is content; anything else as its text."""
    if isinstance(value, BaseException):
        return type(value).__name__
    if isinstance(value, type) and issubclass(value, BaseException):
        return value.__name__
    return _text(value, "span's error type", warn_once)


def _cut(
    value: str, most: int, what: str, warn_once: Callable[[str, str, WarningInfo], None]
) -> str:
    """`value` up to ingest's cap, in characters, cut there with one warning,
    since ingest refuses a span past it."""
    if len(value) <= most:
        return value
    warn_once(
        f"cut:{what}",
        f"telmoni: a {what} must be up to {most} characters; "
        f"it is sent cut to its first {most}",
        _INPUT,
    )
    return value[:most]


def _within(
    value: str,
    most: int,
    what: str,
    span: str,
    warn_once: Callable[[str, str, WarningInfo], None],
) -> bool:
    """Whether an id is within ingest's cap; one past it is left off its trace
    or call with one warning, since cut it would name another."""
    if len(value) <= most:
        return True
    warn_once(
        f"long:{what}",
        f"telmoni: a {what} must be up to {most} characters; "
        f"the {span} is sent without it",
        _INPUT,
    )
    return False


def _str(value: object) -> str:
    """`str(value)`, or its class's name where its own `__str__` raises."""
    try:
        return str(value)
    except Exception:  # noqa: BLE001 - never into the agent's call
        return type(value).__name__


_EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)
# One past the last nanosecond ingest reads, in 64 bits: in 2554.
_NANOS_END = 2**64


def _nanos(at: datetime) -> int:
    """A recorded time as OpenTelemetry's nanoseconds, in whole microseconds,
    as a datetime holds it."""
    if not isinstance(at, datetime):
        raise TypeError("a recorded time must be a datetime")
    if at.tzinfo is None or at.utcoffset() is None:
        raise ValueError("a recorded time needs its timezone")
    return (at - _EPOCH) // timedelta(microseconds=1) * 1000


# An item as read, a step with what it holds.
_Read: TypeAlias = "tuple[StepRecord | CallRecord, tuple[_Read, ...]]"
# How deep steps nest, the trace's own at 1: past any agent's subagents, and
# an end to a step that holds itself.
_STEP_DEPTH_MAX = 16


def _read(record: TraceRecord) -> tuple[_Read, ...]:
    """The record's items, each step with what it holds, read once and checked,
    so the items checked are the items built; a set, in no order, is refused."""
    inside = record.inside
    if not isinstance(inside, Sequence):
        raise TypeError(
            "a recorded trace's inside must be a sequence, in the order it ran"
        )
    return _read_items(inside, 1)


def _read_items(given: Sequence[object], depth: int) -> tuple[_Read, ...]:
    items: list[_Read] = []
    for item in given:
        if isinstance(item, (ModelCallRecord, ToolCallRecord)):
            _check(item)
            items.append((item, ()))
            continue
        if not isinstance(item, StepRecord):
            raise TypeError(
                "a recorded item must be a StepRecord, a ModelCallRecord "
                "or a ToolCallRecord"
            )
        if depth > _STEP_DEPTH_MAX:
            raise ValueError(
                f"a recorded step must be nested at most {_STEP_DEPTH_MAX} deep"
            )
        _check(item)
        calls = item.calls
        if not isinstance(calls, Sequence):
            raise TypeError(
                "a recorded step's calls must be a sequence, in the order they ran"
            )
        items.append((item, _read_items(calls, depth + 1)))
    return tuple(items)


def _walk(items: Sequence[_Read]) -> Iterator[StepRecord | CallRecord]:
    """Every item read, depth first, a step before what it holds: the order of
    their places."""
    for item, inside in items:
        yield item
        yield from _walk(inside)


_NONE = type(None)
_TEXT: tuple[type, ...] = (str,)
_UNSET_OR_TEXT: tuple[type, ...] = (str, _NONE)
# What each recorded field may hold: OpenTelemetry would warn of anything else
# inside the hold, where a handler may record a trace, or send it wrong.
_FIELDS: dict[type, tuple[str, dict[str, tuple[type, ...]]]] = {
    TraceRecord: (
        "trace",
        {
            "trace_id": _TEXT,
            "agent": _TEXT,
            "user_id": _UNSET_OR_TEXT,
            "session_id": _UNSET_OR_TEXT,
            "tags": (Mapping, _NONE),
        },
    ),
    StepRecord: ("step", {"name": _TEXT}),
    ModelCallRecord: (
        "model call",
        {
            "model": _TEXT,
            "provider": _UNSET_OR_TEXT,
            "operation": _UNSET_OR_TEXT,
            "reasoning_effort": _UNSET_OR_TEXT,
            "usage": (Usage, _NONE),
        },
    ),
    ToolCallRecord: ("tool call", {"name": _TEXT}),
}


# The longest each recorded text may be, in characters, as ingest caps it.
_CAPS: dict[type, dict[str, int]] = {
    TraceRecord: {
        "agent": contract.AGENT_MAX,
        "user_id": contract.USER_ID_MAX,
        "session_id": contract.SESSION_ID_MAX,
    },
    StepRecord: {"name": contract.NAME_MAX},
    ModelCallRecord: {
        "model": contract.MODEL_MAX,
        "provider": contract.PROVIDER_MAX,
        "reasoning_effort": contract.ATTRIBUTE_VALUE_MAX,
    },
    ToolCallRecord: {"name": contract.NAME_MAX},
}


def _check(item: TraceRecord | StepRecord | CallRecord) -> None:
    what, fields = next(
        entry for kind, entry in _FIELDS.items() if isinstance(item, kind)
    )
    for field, kinds in {
        **fields,
        "error_type": _UNSET_OR_TEXT,
        "cancelled": (bool,),
        "scores": (Sequence,),
    }.items():
        if not isinstance(getattr(item, field), kinds):
            names = " or ".join(
                "None" if kind is _NONE else kind.__name__ for kind in kinds
            )
            raise TypeError(f"a recorded {what}'s {field} must be {names}")
    if not all(isinstance(score, Mapping) for score in item.scores):
        raise TypeError(
            f"a recorded {what}'s scores must each map name, value, range and version"
        )
    # Ingest refuses a step or a model named nothing; a tool's it reads as none.
    named = "model" if isinstance(item, ModelCallRecord) else "name"
    if isinstance(item, (StepRecord, ModelCallRecord)) and getattr(item, named) == "":
        raise ValueError(f"a recorded {what}'s {named} cannot be empty")
    caps = next(caps for kind, caps in _CAPS.items() if isinstance(item, kind))
    for field, most in {**caps, "error_type": contract.ERROR_TYPE_MAX}.items():
        text = getattr(item, field)
        if text and len(text) > most:
            raise ValueError(
                f"a recorded {what}'s {field} must be up to {most} characters"
            )


def _record_content(item: StepRecord | CallRecord) -> dict[str, str]:
    """A recorded step's or call's content as JSON, which the exporter's lock
    sends or keeps back."""
    fields: tuple[tuple[str, object], ...]
    if isinstance(item, ModelCallRecord):
        fields = (
            (contract.INPUT_MESSAGES, item.input_messages),
            (contract.OUTPUT_MESSAGES, item.output_messages),
        )
    elif isinstance(item, ToolCallRecord):
        fields = (
            (contract.TOOL_CALL_ARGUMENTS, item.arguments),
            (contract.TOOL_CALL_RESULT, item.result),
        )
    else:
        fields = (
            (contract.INPUT_MESSAGES, item.input_messages),
            (contract.OUTPUT_MESSAGES, item.output_messages),
        )
    return {key: _as_json(value) for key, value in fields if value is not None}


def _recorded_scores(
    item: TraceRecord | StepRecord | CallRecord,
    warn_once: Callable[[str, str, WarningInfo], None],
) -> list[dict[str, Any]]:
    """A recorded span's scores as their events, each held to a score's rules;
    one breaking them, or past the span's cap, is left off with one warning."""
    events: list[dict[str, Any]] = []
    for given in item.scores:
        judgement = scores.judged(
            given.get("name"),
            given.get("value"),
            given.get("range"),
            given.get("version"),
            warn_once,
        )
        if judgement is None:
            continue
        if len(events) == contract.SCORES_PER_SPAN_MAX:
            scores.refused(scores.FULL, warn_once)
            continue
        events.append(judgement.event())
    return events


def _close(
    span: Span,
    item: TraceRecord | StepRecord | ModelCallRecord | ToolCallRecord,
    events: Sequence[Mapping[str, Any]],
) -> None:
    end = _nanos(item.end)
    for attributes in events:
        span.add_event(contract.EVALUATION_EVENT, attributes, end)
    if item.error_type:
        span.set_attribute(contract.ERROR_TYPE, item.error_type)
        span.set_status(Status(StatusCode.ERROR))
    if item.cancelled:
        span.set_attribute(contract.CANCELLED, True)
    span.end(end_time=end)


# What ingest takes: a token count from 0 that fits 64 bits.
_COUNT = range(2**63)


def _whole(value: float, allowed: range) -> int | None:
    """`value` as a whole number in `allowed`, exact where given as one; None
    for anything else, a `bool` among them, though Python counts it an int."""
    if isinstance(value, bool):
        return None
    try:
        number = int(value) if isinstance(value, int) else int(float(value))
    except (TypeError, ValueError, OverflowError):
        return None
    return number if number in allowed else None


def _as_json(value: object) -> str:
    if isinstance(value, str):
        return value
    try:
        # Compact UTF-8, as the other three SDKs and the export's own body write it.
        return json.dumps(
            value, default=_str, ensure_ascii=False, separators=(",", ":")
        )
    except Exception:  # noqa: BLE001 - never into the agent's call
        # Whatever will not serialize goes as its text, or its class's name.
        return _str(value)
