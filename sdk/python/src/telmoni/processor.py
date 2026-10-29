"""The SDK's own batch processor: OpenTelemetry's batching and bounded queue,
without the check that skips a span whose trace is not sampled.

A run inside a trace the application does not sample is still sent; the run's
own sample rate is applied here, by run, never through the trace's flag. The
queue is bounded, so a slow Telmoni never slows an agent: past its size, spans
are dropped and counted. A recorded run goes at once, in exports of its own.
"""

from __future__ import annotations

import multiprocessing
import os
import threading
import time
import weakref
from collections import deque
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from multiprocessing import util as multiprocessing_util

from opentelemetry.context import Context
from opentelemetry.sdk.trace import ReadableSpan, Span, SpanProcessor

from . import contract
from .exporter import Exporter, warn

MAX_QUEUE_SIZE = 2048
MAX_BATCH_SIZE = 512
SCHEDULE_DELAY_S = 5.0
SHUTDOWN_TIMEOUT_S = 30.0


class QueueFullError(RuntimeError):
    """``record_run`` refused a record the export queue has no room for,
    queuing nothing of it: flush, or wait for the queue to drain, and record
    it again."""


class ShutdownError(RuntimeError):
    """``record_run`` refused a record once the client was shut down, queuing
    nothing of it."""


@dataclass(frozen=True)
class Batching:
    """How spans are batched on their way out: OpenTelemetry's numbers by
    default. A queue without bound is ``sys.maxsize``."""

    max_queue_size: int = MAX_QUEUE_SIZE
    max_batch_size: int = MAX_BATCH_SIZE
    schedule_delay_s: float = SCHEDULE_DELAY_S


class Processor(SpanProcessor):
    def __init__(self, exporter: Exporter, batching: Batching | None = None) -> None:
        given = batching or Batching()
        self._exporter = exporter
        # A queue or batch of nothing drops every span or takes empty batches
        # forever, and a delay of none spins an idle worker: each at least one,
        # as Rust's, the delay within what a lock's wait takes.
        self._max_queue_size = max(1, given.max_queue_size)
        self._max_batch_size = max(1, given.max_batch_size)
        self._schedule_delay_s = min(
            max(0.001, given.schedule_delay_s), threading.TIMEOUT_MAX
        )
        self._lock = threading.Lock()
        self._wake = threading.Condition(self._lock)
        self._recording = threading.Lock()
        self._queue: deque[ReadableSpan] = deque()
        # The queue's first span is the `_taken`th ever queued; a batch stops
        # at each fence, a record's first span and one past its last.
        self._taken = 0
        self._fences: deque[int] = deque()
        # The record being built, its trace's spans as they end, and its room.
        self._stage: tuple[int, list[ReadableSpan]] | None = None
        self._reserved = 0
        self._dropped = 0
        self._flushing = 0
        self._shutdown = False
        self._exporting = False
        self._thread = threading.Thread(
            target=self._work, name="telmoni-exporter", daemon=True
        )
        self._thread.start()
        # A forked child inherits no worker; held weakly, as no fork hook is removed.
        if hasattr(os, "register_at_fork"):
            weak_after_fork = weakref.WeakMethod(self._after_fork)

            def _after_in_child() -> None:
                if after_fork := weak_after_fork():
                    after_fork()

            os.register_at_fork(after_in_child=_after_in_child)
        # A multiprocessing child leaves through os._exit, past atexit: a
        # finalizer of its own drains it, as atexit drains the main process.
        multiprocessing_util.register_after_fork(self, Processor._drain_at_child_exit)
        if multiprocessing.parent_process() is not None:
            self._drain_at_child_exit()

    def _drain_at_child_exit(self) -> None:
        # Registered in the child, after multiprocessing cleared the parent's.
        multiprocessing_util.Finalize(self, self.shutdown, exitpriority=0)

    def _after_fork(self) -> None:
        """The child of a fork: the parent's worker is not in it, and its lock
        may have been copied held, so both are made anew. The parent's queue
        is the parent's to send."""
        self._lock = threading.Lock()
        self._wake = threading.Condition(self._lock)
        self._recording = threading.Lock()
        self._exporter.after_fork()
        self._queue.clear()
        self._taken = 0
        self._fences.clear()
        self._stage = None
        self._reserved = 0
        self._dropped = 0
        self._flushing = 0
        self._exporting = False
        if not self._shutdown:
            self._thread = threading.Thread(
                target=self._work, name="telmoni-exporter", daemon=True
            )
            self._thread.start()

    @contextmanager
    def hold(self, trace_id: int, need: int) -> Iterator[None]:
        """Stage the spans of the record in ``trace_id``, queued whole as the
        block ends, in exports of their own; refused, holding nothing, once shut
        down or without room for ``need``. A record warns before it: a handler
        recording a run would wait on the block."""
        with self._recording:
            with self._lock:
                if self._shutdown:
                    raise ShutdownError(
                        "the client is shut down; the record was not sent"
                    )
                if len(self._queue) + need > self._max_queue_size:
                    raise QueueFullError(
                        f"the export queue has no room for the record's {need} "
                        "spans; flush or wait, then record it again"
                    )
                staged: list[ReadableSpan] = []
                self._stage = (trace_id, staged)
                self._reserved = need
            built = False
            try:
                yield
                built = True
            finally:
                with self._lock:
                    self._stage = None
                    self._reserved = 0
                    if built and staged and not self._shutdown:
                        first = self._taken + len(self._queue)
                        if first > self._taken:
                            self._fences.append(first)
                        self._fences.append(first + len(staged))
                        self._queue.extend(staged)
                        self._wake.notify_all()

    def on_start(self, span: Span, parent_context: Context | None = None) -> None:
        return None

    def on_end(self, span: ReadableSpan) -> None:
        if contract.SAMPLED_OUT in (span.attributes or {}):
            return
        with self._lock:
            if self._shutdown:
                return
            stage = self._stage
            if (
                stage is not None
                and span.context is not None
                and span.context.trace_id == stage[0]
            ):
                stage[1].append(span)
                return
            if len(self._queue) + self._reserved >= self._max_queue_size:
                self._dropped += 1
                return
            self._queue.append(span)
            if len(self._queue) >= self._max_batch_size:
                self._wake.notify_all()

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        deadline = time.monotonic() + timeout_millis / 1000
        with self._lock:
            if self._shutdown:
                return True
            self._flushing += 1
            self._wake.notify_all()
            try:
                while self._queue or self._exporting:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        return False
                    self._wake.wait(timeout=remaining)
                return True
            finally:
                self._flushing -= 1

    def shutdown(self) -> None:
        """Drains, then takes nothing more. Past the timeout what is left is
        given up: the wait for a retry ends there and no batch or attempt
        follows, as in the other SDKs."""
        with self._lock:
            if self._shutdown:
                return
            self._shutdown = True
            self._wake.notify_all()
        self._thread.join(SHUTDOWN_TIMEOUT_S)
        if self._thread.is_alive():
            with self._lock:
                self._queue.clear()
                self._fences.clear()
            self._exporter.give_up()

    def _work(self) -> None:
        while True:
            with self._lock:
                if not self._due():
                    self._wake.wait(timeout=self._schedule_delay_s)
                if self._shutdown and not self._queue:
                    return
                size = min(len(self._queue), self._max_batch_size)
                if self._fences:
                    size = min(size, self._fences[0] - self._taken)
                batch = [self._queue.popleft() for _ in range(size)]
                self._taken += size
                while self._fences and self._fences[0] <= self._taken:
                    self._fences.popleft()
                dropped, self._dropped = self._dropped, 0
                self._exporting = bool(batch)
            if dropped:
                warn(
                    "telmoni: the export queue was full; %d spans were dropped", dropped
                )
            if batch:
                self._export(batch)
            with self._lock:
                self._exporting = False
                self._wake.notify_all()

    def _due(self) -> bool:
        """Under the lock: whether the worker should export without waiting; a
        record queued goes at once."""
        return (
            self._shutdown
            or self._flushing > 0
            or bool(self._fences)
            or len(self._queue) >= self._max_batch_size
        )

    def _export(self, batch: Sequence[ReadableSpan]) -> None:
        try:
            self._exporter.export(batch)
        except Exception as error:  # noqa: BLE001 - never reaches the agent
            # The class alone: a message or a traceback could quote a span.
            warn(
                "telmoni: an export failed (%s); the batch was dropped",
                type(error).__name__,
            )
