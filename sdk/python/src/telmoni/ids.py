"""Trace and span ids: random, as OpenTelemetry's own generator makes them,
except for the spans of a recorded run, whose ids are derived from the run's
id, so that a record sent twice is the same bytes and ingest's claim on the
body answers it once.
"""

from __future__ import annotations

import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar

from opentelemetry.sdk.trace.id_generator import IdGenerator, RandomIdGenerator

# In a context variable rather than on the generator: another thread opening
# a span of its own while a record is built must never take its ids.
_GIVEN: ContextVar[tuple[int, int] | None] = ContextVar("telmoni.ids", default=None)


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


def span_id(run_id: uuid.UUID, place: int) -> int:
    """The id of a recorded run's span at ``place``: the first eight bytes of
    the version 5 UUID (RFC 9562) named by the place, in decimal, under the
    run's id. Its version's bits are among those eight, so it is never zero."""
    return int.from_bytes(uuid.uuid5(run_id, str(place)).bytes[:8], "big")
