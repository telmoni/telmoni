"""The SDK's own batch processor: OpenTelemetry's batching and bounded queue,
without the check that skips a span whose trace is not sampled.

A run inside a trace the application does not sample is still sent; the run's
own sample rate is applied here, by run, never through the trace's flag. The
queue is bounded, so a slow Telmoni never slows an agent: past its size, spans
are dropped and counted. ``telmoni.run.start`` wakes the worker at once.
"""

from __future__ import annotations

import logging
import os
import threading
import time
from collections import deque
from collections.abc import Sequence

from opentelemetry.context import Context
from opentelemetry.sdk.trace import ReadableSpan, Span, SpanProcessor

from . import contract
from .exporter import Exporter

logger = logging.getLogger("telmoni")

MAX_QUEUE_SIZE = 2048
MAX_BATCH_SIZE = 512
SCHEDULE_DELAY_S = 5.0
SHUTDOWN_TIMEOUT_S = 30.0


class Processor(SpanProcessor):
    def __init__(
        self,
        exporter: Exporter,
        *,
        max_queue_size: int = MAX_QUEUE_SIZE,
        max_batch_size: int = MAX_BATCH_SIZE,
        schedule_delay_s: float = SCHEDULE_DELAY_S,
    ) -> None:
        self._exporter = exporter
        self._max_queue_size = max_queue_size
        self._max_batch_size = max_batch_size
        self._schedule_delay_s = schedule_delay_s
        self._lock = threading.Lock()
        self._wake = threading.Condition(self._lock)
        self._queue: deque[ReadableSpan] = deque()
        self._sampled_out: set[str] = set()
        self._dropped = 0
        self._urgent = False
        self._flushing = 0
        self._shutdown = False
        self._exporting = False
        self._thread = threading.Thread(
            target=self._work, name="telmoni-exporter", daemon=True
        )
        self._thread.start()
        # A forked child inherits no thread: gunicorn's and celery's prefork
        # workers, or `multiprocessing`, would queue spans nobody sends.
        if hasattr(os, "register_at_fork"):
            os.register_at_fork(after_in_child=self._after_fork)

    def _after_fork(self) -> None:
        """The child of a fork: the parent's worker is not in it, and its lock
        may have been copied held, so both are made anew. The parent's queue
        is the parent's to send."""
        self._lock = threading.Lock()
        self._wake = threading.Condition(self._lock)
        self._queue.clear()
        self._dropped = 0
        self._urgent = False
        self._flushing = 0
        self._exporting = False
        if not self._shutdown:
            self._thread = threading.Thread(
                target=self._work, name="telmoni-exporter", daemon=True
            )
            self._thread.start()

    def sample_out(self, run_id: str) -> None:
        """Drop every span of this run, the run's own among them."""
        with self._lock:
            self._sampled_out.add(run_id)

    def on_start(self, span: Span, parent_context: Context | None = None) -> None:
        return None

    def on_end(self, span: ReadableSpan) -> None:
        attributes = span.attributes or {}
        run_id = attributes.get(contract.RUN_ID)
        with self._lock:
            if self._shutdown:
                return
            if isinstance(run_id, str) and run_id in self._sampled_out:
                if attributes.get(contract.OPERATION) in contract.RUN_OPERATIONS:
                    self._sampled_out.discard(run_id)
                return
            if len(self._queue) >= self._max_queue_size:
                self._dropped += 1
                return
            self._queue.append(span)
            if span.name == contract.RUN_START:
                self._urgent = True
            if self._urgent or len(self._queue) >= self._max_batch_size:
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
        with self._lock:
            if self._shutdown:
                return
            self._shutdown = True
            self._wake.notify_all()
        self._thread.join(SHUTDOWN_TIMEOUT_S)

    def _work(self) -> None:
        while True:
            with self._lock:
                if not self._due():
                    self._wake.wait(timeout=self._schedule_delay_s)
                if self._shutdown and not self._queue:
                    return
                size = min(len(self._queue), self._max_batch_size)
                batch = [self._queue.popleft() for _ in range(size)]
                dropped, self._dropped = self._dropped, 0
                self._urgent = False
                self._exporting = bool(batch)
            if dropped:
                logger.warning(
                    "telmoni: the export queue was full; %d spans were dropped", dropped
                )
            if batch:
                self._export(batch)
            with self._lock:
                self._exporting = False
                self._wake.notify_all()

    def _due(self) -> bool:
        """Under the lock: whether the worker should export without waiting."""
        return (
            self._shutdown
            or self._flushing > 0
            or self._urgent
            or len(self._queue) >= self._max_batch_size
        )

    def _export(self, batch: Sequence[ReadableSpan]) -> None:
        try:
            self._exporter.export(batch)
        except Exception as error:  # noqa: BLE001 - never reaches the agent
            # The class alone: a message or a traceback could quote a span.
            logger.warning(
                "telmoni: an export failed (%s); the batch was dropped",
                type(error).__name__,
            )
