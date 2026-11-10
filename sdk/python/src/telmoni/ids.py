"""Trace and span ids: random, as OpenTelemetry's own generator makes them,
except for the spans of a recorded trace, whose ids are derived from the
trace's id, so that a record sent twice is the same bytes and ingest's claim
on the body answers it once.
"""

from __future__ import annotations

import re
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar

from opentelemetry.sdk.trace.id_generator import IdGenerator, RandomIdGenerator

# In a context variable rather than on the generator: another thread opening
# a span of its own while a record is built must never take its ids.
_GIVEN: ContextVar[tuple[int, int] | None] = ContextVar("telmoni.ids", default=None)
_TRACE_ID = re.compile(r"[0-9a-fA-F]{32}")


class Ids(IdGenerator):
    def __init__(self) -> None:
        self._random = RandomIdGenerator()

    def generate_trace_id(self) -> int:
        given = _GIVEN.get()
        return given[0] if given is not None else self._random.generate_trace_id()

    def generate_span_id(self) -> int:
        given = _GIVEN.get()
        return given[1] if given is not None else self._random.generate_span_id()


@contextmanager
def given(trace_id: int, span_id: int) -> Iterator[None]:
    """The ids the span opened in this context, until the block ends, takes."""
    token = _GIVEN.set((trace_id, span_id))
    try:
        yield
    finally:
        _GIVEN.reset(token)


def span_id(trace_id: int, place: int) -> int:
    """The id of a recorded trace's span at ``place``: the first eight bytes of
    the version 5 UUID (RFC 9562) named by the place, in decimal, under the
    trace's 16 bytes. Its version's bits are among those eight, so it is never
    zero."""
    namespace = uuid.UUID(int=trace_id)
    return int.from_bytes(uuid.uuid5(namespace, str(place)).bytes[:8], "big")


def trace_id(text: object) -> int | None:
    """A trace's id as OpenTelemetry's number: 32 hex digits, any case, other
    than all zeros; None for anything else."""
    if not isinstance(text, str) or _TRACE_ID.fullmatch(text) is None:
        return None
    read = int(text, 16)
    return read if read != 0 else None
