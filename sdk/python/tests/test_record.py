"""A run recorded after it ended: ``record_run`` and ``sdk/fixtures/recorded.json``."""

from __future__ import annotations

import copy
import dataclasses
import json
import logging
import sys
import threading
import uuid
from collections.abc import Callable
from datetime import datetime, timedelta
from typing import Any
from unittest import mock

import pytest
from opentelemetry import trace
from opentelemetry.sdk.trace.sampling import Decision, Sampler, SamplingResult

from conftest import FakeTransport, attributes_of, load_fixture
from telmoni import (
    Config,
    ModelCallRecord,
    QueueFullError,
    RunRecord,
    StepRecord,
    Telmoni,
    ToolCallRecord,
    Usage,
    current_run,
)
from telmoni.exporter import Exporter
from telmoni.processor import Processor

VERSIONED = {"telemetry.sdk.version", "telemetry.distro.version"}


def at(text: str) -> datetime:
    return datetime.fromisoformat(text.replace("Z", "+00:00"))


def call_from(spec: dict[str, Any]) -> ModelCallRecord | ToolCallRecord:
    if "model_call" in spec:
        call = spec["model_call"]
        return ModelCallRecord(
            model=call["model"],
            provider=call.get("provider"),
            operation=call.get("operation", "chat"),
            start=at(call["start"]),
            end=at(call["end"]),
            usage=Usage(**call["usage"]) if "usage" in call else None,
            error_type=call.get("error_type"),
            cancelled=call.get("cancelled", False),
        )
    tool = spec["tool_call"]
    return ToolCallRecord(
        name=tool["name"],
        start=at(tool["start"]),
        end=at(tool["end"]),
        error_type=tool.get("error_type"),
        cancelled=tool.get("cancelled", False),
    )


def record_from(spec: dict[str, Any]) -> RunRecord:
    inside: list[StepRecord | ModelCallRecord | ToolCallRecord] = []
    for item in spec["inside"]:
        if "step" in item:
            step = item["step"]
            inside.append(
                StepRecord(
                    name=step["name"],
                    start=at(step["start"]),
                    end=at(step["end"]),
                    calls=[call_from(each) for each in step.get("calls", [])],
                    error_type=step.get("error_type"),
                    cancelled=step.get("cancelled", False),
                )
            )
        else:
            inside.append(call_from(item))
    return RunRecord(
        run_id=spec["run_id"],
        agent=spec["agent"],
        user_id=spec.get("user_id"),
        session_id=spec.get("session_id"),
        tags=spec.get("tags"),
        start=at(spec["start"]),
        end=at(spec["end"]),
        inside=inside,
        error_type=spec.get("error_type"),
        cancelled=spec.get("cancelled", False),
    )


def the_record() -> RunRecord:
    return record_from(load_fixture("recorded.json")["scenario"]["record"])


def normalize(body: dict[str, Any]) -> dict[str, Any]:
    """The fixture's normalization, as its README sets it out."""
    body = copy.deepcopy(body)
    resource_spans = body["resourceSpans"]
    assert len(resource_spans) == 1
    scope_spans = resource_spans[0]["scopeSpans"]
    assert len(scope_spans) == 1
    scope_spans[0]["scope"]["version"] = "<version>"
    for span in scope_spans[0]["spans"]:
        span["attributes"] = sorted(span["attributes"], key=lambda a: a["key"])
    resource = resource_spans[0]["resource"]
    resource["attributes"] = sorted(resource["attributes"], key=lambda a: a["key"])
    for attribute in resource["attributes"]:
        if attribute["key"] == "telemetry.sdk.language":
            attribute["value"]["stringValue"] = "<language>"
        elif attribute["key"] in VERSIONED:
            attribute["value"]["stringValue"] = "<version>"
    return body


def test_the_record_matches_the_golden_export(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    fixture = load_fixture("recorded.json")
    telmoni.record_run(record_from(fixture["scenario"]["record"]))
    assert telmoni.flush()
    bodies = transport.bodies()
    assert len(bodies) == 1, "a record travels in one export"
    assert normalize(bodies[0]) == fixture["export"]
    for _url, headers, _body in transport.requests:
        assert headers["Authorization"] == "Bearer telmoni_test_key"
        assert headers["User-Agent"].startswith("telmoni-python/")


def test_the_same_record_sent_again_is_the_same_bytes() -> None:
    record = the_record()
    sent: list[list[bytes]] = []
    # Again and again, since a worker that took a batch mid-record would split
    # it only now and then.
    for _ in range(20):
        transport = FakeTransport()
        client = Telmoni(Config(api_key="k"), transport=transport)
        try:
            client.record_run(record)
            assert client.flush()
        finally:
            client.shutdown()
        sent.append([body for _url, _headers, body in transport.requests])
    assert all(bodies == sent[0] for bodies in sent)
    assert len(sent[0]) == 1


def trace_of(record: RunRecord) -> str:
    return record.run_id.replace("-", "")


def traces_in(body: bytes) -> set[str]:
    return {
        span["traceId"]
        for resource_spans in json.loads(body)["resourceSpans"]
        for scope_spans in resource_spans["scopeSpans"]
        for span in scope_spans["spans"]
    }


def with_limits(client: Telmoni, transport: FakeTransport, **limits: Any) -> Processor:
    """The client's processor swapped for one of these limits."""
    client.processor.shutdown()
    processor = Processor(Exporter(client.config, transport=transport), **limits)
    client.processor = processor
    client._provider._active_span_processor._span_processors = (processor,)
    return processor


def test_a_record_travels_in_exports_of_its_own(transport: FakeTransport) -> None:
    """A span queued before it or ending after it, and a record beside it, go
    in other exports, so it is the same bytes as when sent alone."""
    record = the_record()
    beside = dataclasses.replace(record, run_id=str(uuid.uuid4()))
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        with client.run("live"):
            assert transport.wait_for(1)  # the live run's start goes alone
            with client.step("live step"):
                pass
            client.record_run(record)
            client.record_run(beside)
        assert client.flush()
        sent = len(transport.requests)
        client.record_run(record)
        assert client.flush()
    finally:
        client.shutdown()
    bodies = [body for _url, _headers, body in transport.requests]
    assert all(len(traces_in(body)) == 1 for body in bodies)
    alone = [body for body in bodies[:sent] if traces_in(body) == {trace_of(record)}]
    assert alone == bodies[sent:]
    assert any(traces_in(body) == {trace_of(beside)} for body in bodies[:sent])


def test_a_record_larger_than_a_batch_is_cut_from_its_own_start(
    transport: FakeTransport,
) -> None:
    client = Telmoni(Config(api_key="k"), transport=transport)
    processor = with_limits(client, transport, max_batch_size=3, schedule_delay_s=60)
    golden = the_record()
    call = ToolCallRecord(name="t", start=golden.start, end=golden.start)
    record = dataclasses.replace(golden, inside=[call, call, call])
    try:
        with client.run("bot"):
            assert transport.wait_for(1)
            for n in range(2):
                with client.step(f"s{n}"):
                    pass
            client.record_run(record)
        assert client.flush()
    finally:
        processor.shutdown()
        client.shutdown()
    assert [
        [span["name"] for span in spans_in(body)] for body in transport.bodies()
    ] == [
        ["telmoni.run.start"],
        ["s0", "s1"],
        ["telmoni.run.start", "execute_tool t", "execute_tool t"],
        ["execute_tool t", "invoke_agent claude-code"],
        ["invoke_agent bot"],
    ]


def spans_in(body: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        span
        for resource_spans in body["resourceSpans"]
        for scope_spans in resource_spans["scopeSpans"]
        for span in scope_spans["spans"]
    ]


def record_beside_live_steps(transport: FakeTransport) -> None:
    """The record sent while another thread ends a live run's steps."""
    client = Telmoni(Config(api_key="k"), transport=transport)
    stop = threading.Event()
    try:
        with client.run("live") as live:

            def ending() -> None:
                while not stop.is_set():
                    with live.step("live step"):
                        pass

            thread = threading.Thread(target=ending)
            thread.start()
            try:
                client.record_run(the_record())
            finally:
                stop.set()
                thread.join()
        assert client.flush()
    finally:
        client.shutdown()


def test_a_span_ending_while_a_record_is_built_travels_apart(
    transport: FakeTransport,
) -> None:
    """A live step ending on another thread meanwhile is in no export of the
    record's: with a thread switch every microsecond, one lands inside it."""
    interval = sys.getswitchinterval()
    sys.setswitchinterval(1e-6)
    try:
        for _ in range(20):
            record_beside_live_steps(transport)
    finally:
        sys.setswitchinterval(interval)
    assert all(len(traces_in(body)) == 1 for _url, _headers, body in transport.requests)


def test_a_handler_may_record_a_run_while_a_record_warns(
    transport: FakeTransport,
) -> None:
    """A record's warning is given before it is built, so a handler recording
    a run from inside another thread's warning waits on nothing of the SDK's."""
    golden = the_record()
    call = ModelCallRecord(
        model="m",
        start=golden.start,
        end=golden.start,
        usage=Usage(input_tokens=float("nan")),  # type: ignore[arg-type]
    )
    warned = dataclasses.replace(golden, inside=[call])
    inner = dataclasses.replace(golden, run_id=str(uuid.uuid4()), inside=[])
    client = Telmoni(Config(api_key="k"), transport=transport)
    gate = threading.Lock()
    entered, warning, stuck = threading.Event(), threading.Event(), threading.Event()

    class Recording(logging.Handler):
        def handle(self, record: logging.LogRecord) -> bool:
            # The lock logging takes around each emit, given up after a while,
            # so a regression fails rather than hangs.
            if not gate.acquire(timeout=5):
                stuck.set()
                return False
            try:
                if threading.current_thread() is other:
                    entered.set()
                    warning.wait(5)
                    client.record_run(inner)
            finally:
                gate.release()
            return True

    warn_once = client.exporter.warn_once

    def flagged(key: str, text: str) -> None:
        warning.set()
        warn_once(key, text)

    client.exporter.warn_once = flagged  # type: ignore[method-assign]
    handler = Recording()
    logging.getLogger("telmoni").addHandler(handler)
    other = threading.Thread(
        target=logging.getLogger("telmoni").warning, args=("else",), daemon=True
    )
    try:
        other.start()
        assert entered.wait(5)
        client.record_run(warned)
        other.join(5)
        assert not stuck.is_set(), "the warning waited on a record's lock"
        assert client.flush()
    finally:
        logging.getLogger("telmoni").removeHandler(handler)
        client.shutdown()
    bodies = [body for _url, _headers, body in transport.requests]
    assert sorted(min(traces_in(body)) for body in bodies) == sorted(
        [trace_of(warned), trace_of(inner)]
    )
    assert all(len(traces_in(body)) == 1 for body in bodies)


class Hooked(Sampler):
    """Calls its hook once, as the next span with no parent starts: a
    record's own span, mid-build."""

    def __init__(self) -> None:
        self.hook: Callable[[], None] | None = None

    def should_sample(self, *args: Any, **kwargs: Any) -> SamplingResult:
        hook, self.hook = self.hook, None
        if hook is not None:
            hook()
        return SamplingResult(Decision.RECORD_AND_SAMPLE)

    def get_description(self) -> str:
        return "Hooked"


def test_a_records_room_is_kept_while_it_is_built(
    transport: FakeTransport, caplog: pytest.LogCaptureFixture
) -> None:
    """A live step ending while it is built is dropped past the record's
    room, so the queue never holds more than its size."""
    sampler = Hooked()
    client = Telmoni(Config(api_key="k", sampler=sampler), transport=transport)
    processor = with_limits(
        client, transport, max_queue_size=5, max_batch_size=100, schedule_delay_s=60
    )
    golden = the_record()
    call = ToolCallRecord(name="t", start=golden.start, end=golden.start)
    try:
        with client.run("live") as live:
            assert transport.wait_for(1)

            def steps() -> None:
                for n in range(2):
                    with live.step(f"s{n}"):
                        pass

            # Room for one step beside the record's four spans.
            sampler.hook = steps
            client.record_run(dataclasses.replace(golden, inside=[call, call]))
            assert client.flush()
        assert client.flush()
    finally:
        processor.shutdown()
        client.shutdown()
    assert sorted(span["name"] for span in transport.spans()) == [
        "execute_tool t",
        "execute_tool t",
        "invoke_agent claude-code",
        "invoke_agent live",
        "s0",
        "telmoni.run.start",
        "telmoni.run.start",
    ]
    assert [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING] == [
        "telmoni: the export queue was full; 1 spans were dropped"
    ]


def test_a_record_goes_at_once(transport: FakeTransport) -> None:
    """Queued behind a live step, it goes in the export after the step's, as a
    live run's start would, with no flush."""
    client = Telmoni(Config(api_key="k"), transport=transport)
    processor = with_limits(client, transport, schedule_delay_s=60)
    try:
        with client.run("live"):
            assert transport.wait_for(1)
            with client.step("live step"):
                pass
            client.record_run(the_record())
            assert transport.wait_for(3), "the record waited for the delay"
    finally:
        processor.shutdown()
        client.shutdown()


def test_a_record_cut_short_sends_nothing(transport: FakeTransport) -> None:
    """Its start has ended when the next span fails; nothing of it is queued."""
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        with (
            mock.patch("telmoni.runs._close", side_effect=RuntimeError("cut")),
            pytest.raises(RuntimeError, match="cut"),
        ):
            client.record_run(the_record())
        assert client.flush()
    finally:
        client.shutdown()
    assert transport.requests == []


def test_a_record_takes_a_trace_of_its_own_and_leaves_the_context_alone(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    record = the_record()
    with telmoni.run("live") as live:
        current = trace.get_current_span()
        telmoni.record_run(record)
        assert trace.get_current_span() is current
        assert current_run() is live
    assert telmoni.flush()
    recorded = [
        span
        for span in transport.spans()
        if attributes_of(span)["telmoni.run_id"] == record.run_id
    ]
    assert {span["traceId"] for span in recorded} == {record.run_id.replace("-", "")}
    root = next(s for s in recorded if s["name"] == "invoke_agent claude-code")
    assert "parentSpanId" not in root


def test_a_failed_run_is_marked_on_its_own_span(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    record = dataclasses.replace(
        the_record(), error_type="RateLimitError", cancelled=False, inside=()
    )
    telmoni.record_run(record)
    assert telmoni.flush()
    run = next(s for s in transport.spans() if s["name"] == "invoke_agent claude-code")
    assert run["status"] == {"code": 2}
    assert attributes_of(run)["error.type"] == "RateLimitError"
    assert "telmoni.cancelled" not in attributes_of(run)


def test_the_sample_rate_drops_a_record_whole(transport: FakeTransport) -> None:
    client = Telmoni(Config(api_key="k", sample_rate=0.25), transport=transport)
    try:
        with mock.patch("telmoni.runs.random.random", return_value=0.9):
            client.record_run(the_record())
        assert client.flush()
    finally:
        client.shutdown()
    assert transport.requests == []


START = at("2026-10-12T09:00:00.000Z")


@pytest.mark.parametrize(
    ("change", "message"),
    [
        ({"agent": " "}, "agent's name"),
        ({"run_id": "run-1"}, "must be a UUID"),
        ({"run_id": "00000000-0000-0000-0000-000000000000"}, "nil UUID"),
        ({"start": datetime(2026, 10, 12, 9)}, "needs its timezone"),
        ({"end": START - timedelta(seconds=1)}, "end before it starts"),
        (
            {
                "inside": [
                    StepRecord(
                        name="s",
                        start=START,
                        end=START + timedelta(seconds=2),
                        calls=[
                            ToolCallRecord(
                                name="t", start=START + timedelta(seconds=1), end=START
                            )
                        ],
                    )
                ]
            },
            "end before it starts",
        ),
    ],
)
def test_a_record_refused_sends_nothing(
    telmoni: Telmoni,
    transport: FakeTransport,
    change: dict[str, Any],
    message: str,
) -> None:
    record = dataclasses.replace(the_record(), **change)
    with pytest.raises(ValueError, match=message):
        telmoni.record_run(record)
    assert telmoni.flush()
    assert transport.requests == []


def test_a_record_the_queue_has_no_room_for_is_refused_whole(
    transport: FakeTransport,
) -> None:
    """Refused whole before anything is queued; drained, the queue takes it."""
    client = Telmoni(Config(api_key="k"), transport=transport)
    # Room for five spans, and a worker that will not wake until flushed.
    processor = with_limits(
        client, transport, max_queue_size=5, max_batch_size=100, schedule_delay_s=60
    )
    golden = the_record()
    record = dataclasses.replace(
        golden,
        inside=[
            ToolCallRecord(name="t", start=golden.start, end=golden.start),
            ToolCallRecord(name="t", start=golden.start, end=golden.start),
        ],
    )
    try:
        with client.run("bot"):
            # The start goes alone; two steps then hold two of the five slots.
            assert transport.wait_for(1)
            for n in range(2):
                with client.step(f"s{n}"):
                    pass
            with pytest.raises(QueueFullError, match="4 spans"):
                client.record_run(record)
        assert client.flush()
        client.record_run(record)
        assert client.flush()
    finally:
        processor.shutdown()
        client.shutdown()
    names = [s["name"] for s in transport.spans()]
    assert names.count("execute_tool t") == 2 and len(names) == 8, names
