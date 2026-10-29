from __future__ import annotations

import gc
import logging
import multiprocessing
import os
import weakref
from collections.abc import Mapping
from pathlib import Path

import pytest

from conftest import FakeTransport, answer_from, kept, load_fixture
from telmoni import Config, Telmoni
from telmoni.exporter import Answer, Exporter
from telmoni.processor import Processor


def test_the_queue_is_bounded_and_drops_are_counted_once(
    transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    client = Telmoni(Config(api_key="k"), transport=transport)
    # A tiny queue, and a worker that will not wake until flushed.
    client.processor.shutdown()
    exporter = Exporter(client.config, transport=transport)
    processor = Processor(
        exporter, max_queue_size=2, max_batch_size=100, schedule_delay_s=60
    )
    client.processor = processor
    client._provider._active_span_processor._span_processors = (processor,)
    try:
        with client.run("bot"):
            for n in range(5):
                with client.step(f"s{n}"):
                    pass
        assert client.flush()
    finally:
        processor.shutdown()
        client.shutdown()
    names = sorted(s["name"] for s in transport.spans())
    assert names == ["s0", "s1"], names
    warnings = [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]
    assert (
        len(warnings) == 1
        and "queue was full" in warnings[0]
        and "4 spans" in warnings[0]
    )


def test_a_warning_whose_handler_raises_never_ends_the_worker(
    transport: FakeTransport,
) -> None:
    """Its own warnings guarded as the agent's are, the next flush still sends."""

    class Raising(logging.Handler):
        def handle(self, record: logging.LogRecord) -> bool:
            raise RuntimeError("the handler's own")

    client = Telmoni(Config(api_key="k"), transport=transport)
    client.processor.shutdown()
    exporter = Exporter(client.config, transport=transport)
    processor = Processor(
        exporter, max_queue_size=2, max_batch_size=100, schedule_delay_s=60
    )
    client.processor = processor
    client._provider._active_span_processor._span_processors = (processor,)
    handler = Raising()
    logging.getLogger("telmoni").addHandler(handler)
    try:
        with client.run("bot"):
            for n in range(5):
                with client.step(f"s{n}"):
                    pass
        assert client.flush(timeout_s=5)
        with client.run("later"):
            pass
        assert client.flush(timeout_s=5)
    finally:
        logging.getLogger("telmoni").removeHandler(handler)
        processor.shutdown()
        client.shutdown()
    assert "invoke_agent later" in {s["name"] for s in transport.spans()}


def test_shutdown_drains_what_is_queued(transport: FakeTransport) -> None:
    client = Telmoni(Config(api_key="k"), transport=transport)
    with client.run("bot"):
        pass
    client.shutdown()
    assert [s["name"] for s in transport.spans()] == ["invoke_agent bot"]
    client.shutdown()


def test_shutdown_drains_until_its_deadline_then_sends_nothing_more(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr("telmoni.processor.SHUTDOWN_TIMEOUT_S", 0.2)
    # Every answer asks for 2 s, past the deadline, through the exporter's own
    # wait; a batch of one leaves the run's own span queued behind its step.
    rate_limited = answer_from(load_fixture("answers/rate-limited.json"))
    transport = FakeTransport([rate_limited] * 10)
    client = Telmoni(Config(api_key="k"), transport=transport)
    client.processor.shutdown()
    processor = Processor(
        Exporter(client.config, transport=transport),
        max_batch_size=1,
        schedule_delay_s=60,
    )
    client.processor = processor
    client._provider._active_span_processor._span_processors = (processor,)
    with client.run("bot"), client.step("s"):
        pass
    assert not client.flush(timeout_s=0.01)
    assert transport.wait_for(1), "the step's first attempt"
    processor.shutdown()
    # Past it the worker is gone, so nothing more is sent however long the
    # process lives, and a flush has nothing left to wait for.
    processor._thread.join(5)
    assert not processor._thread.is_alive()
    assert len(transport.requests) == 1
    assert client.flush()


def test_a_batch_fills_and_goes_without_a_flush(transport: FakeTransport) -> None:
    client = Telmoni(Config(api_key="k"), transport=transport)
    client.processor.shutdown()
    processor = Processor(
        Exporter(client.config, transport=transport),
        max_batch_size=3,
        schedule_delay_s=60,
    )
    client.processor = processor
    client._provider._active_span_processor._span_processors = (processor,)
    try:
        with client.run("bot"):
            for n in range(3):
                with client.step(f"s{n}"):
                    pass
            assert transport.wait_for(1), "three steps fill a batch of three"
    finally:
        processor.shutdown()
        client.shutdown()


@pytest.mark.skipif(not hasattr(os, "fork"), reason="no fork on this platform")
# Forking a process with a thread is what this test is about.
@pytest.mark.filterwarnings("ignore:This process:DeprecationWarning")
def test_a_forked_child_gets_a_worker_of_its_own(transport: FakeTransport) -> None:
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        with client.run("bot"):
            pass
        assert client.flush()
        before = len(transport.requests)
        pid = os.fork()
        if pid == 0:
            # The child: the parent's worker thread did not come along, so a
            # new one must have, or nothing from here is ever sent.
            sent = False
            try:
                with client.run("child"):
                    pass
                # The flush returns once the new worker has exported, so the
                # child's own copy of the transport has taken its requests.
                sent = client.flush() and len(transport.requests) > before
            finally:
                os._exit(0 if sent else 1)
        _, status = os.waitpid(pid, 0)
        assert os.waitstatus_to_exitcode(status) == 0, "the child sent nothing"
    finally:
        client.shutdown()


@pytest.mark.skipif(not hasattr(os, "fork"), reason="no fork on this platform")
@pytest.mark.filterwarnings("ignore:This process:DeprecationWarning")
@pytest.mark.parametrize("built_in", ["the parent", "the child"])
def test_a_multiprocessing_child_sends_its_queue_as_it_exits(
    tmp_path: Path, built_in: str
) -> None:
    sent = tmp_path / "sent"

    def transport(url: str, headers: Mapping[str, str], body: bytes) -> Answer:
        # The child's export, seen by the parent through a file.
        with sent.open("ab") as file:
            file.write(b"export\n")
        return kept()

    parent = Telmoni(Config(api_key="k"), transport=transport)

    def record() -> None:
        client = (
            parent
            if built_in == "the parent"
            else Telmoni(Config(api_key="k"), transport=transport)
        )
        with client.run("bot"):
            pass

    try:
        child = multiprocessing.get_context("fork").Process(target=record)
        child.start()
        child.join(60)
        assert child.exitcode == 0
        assert sent.is_file(), "the child's queue was lost as it exited"
        assert sent.read_bytes() == b"export\n"
    finally:
        parent.shutdown()


def test_a_client_shut_down_is_not_held_by_its_fork_hook(
    transport: FakeTransport,
) -> None:
    client = Telmoni(Config(api_key="k"), transport=transport)
    processor = weakref.ref(client.processor)
    client.shutdown()
    del client
    gc.collect()
    assert processor() is None, "the processor, its exporter and the key outlive it"
