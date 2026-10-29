"""A run recorded after it ended: ``record_run`` and ``sdk/fixtures/recorded.json``."""

from __future__ import annotations

import copy
import dataclasses
import json
import logging
import sys
import threading
import uuid
from collections.abc import Callable, Iterator, Sequence
from datetime import datetime, timedelta
from types import SimpleNamespace
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
    ShutdownError,
    StepRecord,
    Telmoni,
    ToolCallRecord,
    Usage,
    current_run,
    runs,
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
        ["s0", "s1"],
        ["execute_tool t", "execute_tool t", "execute_tool t"],
        ["invoke_agent claude-code"],
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
        client, transport, max_queue_size=4, max_batch_size=100, schedule_delay_s=60
    )
    golden = the_record()
    call = ToolCallRecord(name="t", start=golden.start, end=golden.start)
    try:
        with client.run("live") as live:

            def steps() -> None:
                for n in range(2):
                    with live.step(f"s{n}"):
                        pass

            # Room for one step beside the record's three spans.
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
    ]
    assert [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING] == [
        "telmoni: the export queue was full; 1 spans were dropped"
    ]


def test_a_record_goes_at_once(transport: FakeTransport) -> None:
    """Queued behind a live step, it goes in the export after the step's, with
    no flush."""
    client = Telmoni(Config(api_key="k"), transport=transport)
    processor = with_limits(client, transport, schedule_delay_s=60)
    try:
        with client.run("live"):
            with client.step("live step"):
                pass
            client.record_run(the_record())
            assert transport.wait_for(2), "the record waited for the delay"
    finally:
        processor.shutdown()
        client.shutdown()


def test_a_record_cut_short_sends_nothing(transport: FakeTransport) -> None:
    """Its first call has ended when the next fails; nothing of it is queued."""
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        with (
            mock.patch(
                "telmoni.runs._close",
                wraps=runs._close,
                side_effect=[mock.DEFAULT, RuntimeError("cut")],
            ),
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
RUN = RunRecord(run_id=str(uuid.UUID(int=1)), agent="a", start=START, end=START)
STEP = StepRecord(name="s", start=START, end=START)
TOOL = ToolCallRecord(name="t", start=START, end=START)
MODEL = ModelCallRecord(model="m", start=START, end=START)


class Shifting(Sequence[StepRecord | ModelCallRecord | ToolCallRecord]):
    """Its first items on its first read and others after, as rows changing
    beneath a view would."""

    def __init__(self, *reads: list[StepRecord | ModelCallRecord | ToolCallRecord]):
        self.reads = list(reads)

    def __iter__(self) -> Iterator[StepRecord | ModelCallRecord | ToolCallRecord]:
        return iter(self.reads.pop(0) if len(self.reads) > 1 else self.reads[0])

    def __len__(self) -> int:
        return len(self.reads[0])

    def __getitem__(self, index: int) -> StepRecord | ModelCallRecord | ToolCallRecord:  # type: ignore[override]
        return self.reads[0][index]


@pytest.mark.parametrize(
    ("change", "kind", "message"),
    [
        (lambda record: None, TypeError, "must be a RunRecord"),
        (lambda record: SimpleNamespace(**vars(record)), TypeError, "a RunRecord"),
        (
            lambda record: dataclasses.replace(
                record,
                inside=Shifting([dataclasses.replace(STEP, name=5)], [TOOL]),  # type: ignore[arg-type]
            ),
            TypeError,
            "step's name must be str",
        ),
        ({"agent": " "}, ValueError, "agent's name"),
        ({"agent": None}, TypeError, "run's agent must be str"),
        ({"run_id": "run-1"}, ValueError, "must be a UUID"),
        ({"run_id": uuid.UUID(int=1)}, TypeError, "run_id must be str"),
        ({"run_id": "3d6f_a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d"}, ValueError, "a UUID"),
        ({"run_id": "{3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d"}, ValueError, "a UUID"),
        ({"run_id": "00000000-0000-0000-0000-000000000000"}, ValueError, "nil UUID"),
        ({"user_id": 42}, TypeError, "run's user_id must be str or None"),
        ({"session_id": 7}, TypeError, "session_id must be str or None"),
        ({"tags": ["team"]}, TypeError, "tags must be Mapping or None"),
        ({"tags": {"team": 1}}, TypeError, "tags must map str to str"),
        ({"error_type": ValueError}, TypeError, "error_type must be str or None"),
        ({"cancelled": "false"}, TypeError, "cancelled must be bool"),
        (
            {"inside": [dataclasses.replace(STEP, name=5)]},  # type: ignore[arg-type]
            TypeError,
            "name must be str",
        ),
        (
            {"inside": [dataclasses.replace(MODEL, model=None)]},  # type: ignore[arg-type]
            TypeError,
            "model call's model must be str",
        ),
        (
            {"inside": [dataclasses.replace(MODEL, provider=5)]},  # type: ignore[arg-type]
            TypeError,
            "provider must be str or None",
        ),
        (
            {"inside": [dataclasses.replace(MODEL, operation=5)]},  # type: ignore[arg-type]
            TypeError,
            "operation must be str or None",
        ),
        (
            {"inside": [dataclasses.replace(MODEL, usage={"input_tokens": 1})]},  # type: ignore[arg-type]
            TypeError,
            "usage must be Usage or None",
        ),
        (
            {
                "inside": [
                    dataclasses.replace(
                        STEP,
                        calls=[dataclasses.replace(TOOL, name=None)],  # type: ignore[arg-type]
                    )
                ]
            },
            TypeError,
            "tool call's name must be str",
        ),
        ({"start": datetime(2026, 10, 12, 9)}, ValueError, "needs its timezone"),
        ({"start": at("1969-12-31T23:59:59Z")}, ValueError, "after the Unix epoch"),
        ({"start": at("1970-01-01T00:00:00Z")}, ValueError, "after the Unix epoch"),
        ({"end": at("2600-01-01T00:00:00Z")}, ValueError, r"2\^64 nanoseconds"),
        (
            {"inside": [dataclasses.replace(STEP, name="")]},
            ValueError,
            "step's name cannot be empty",
        ),
        (
            {"inside": [dataclasses.replace(MODEL, model="")]},
            ValueError,
            "model call's model cannot be empty",
        ),
        ({"agent": "a" * 129}, ValueError, "run's agent must be up to 128 characters"),
        ({"user_id": "u" * 129}, ValueError, "run's user_id must be up to 128"),
        ({"session_id": "s" * 201}, ValueError, "run's session_id must be up to 200"),
        ({"error_type": "E" * 129}, ValueError, "run's error_type must be up to 128"),
        (
            {"inside": [dataclasses.replace(STEP, name="n" * 146)]},
            ValueError,
            "step's name must be up to 145",
        ),
        (
            {
                "inside": [
                    dataclasses.replace(
                        STEP, calls=[dataclasses.replace(TOOL, name="t" * 146)]
                    )
                ]
            },
            ValueError,
            "tool call's name must be up to 145",
        ),
        (
            {"inside": [dataclasses.replace(MODEL, model="m" * 129)]},
            ValueError,
            "model call's model must be up to 128",
        ),
        (
            {
                "inside": [
                    dataclasses.replace(MODEL, operation="o" * 17, model="m" * 128)
                ]
            },
            ValueError,
            "operation and model must be up to 144",
        ),
        (
            {"inside": [dataclasses.replace(MODEL, provider="p" * 129)]},
            ValueError,
            "model call's provider must be up to 128",
        ),
        (
            {"inside": [dataclasses.replace(TOOL, error_type="E" * 129)]},
            ValueError,
            "tool call's error_type must be up to 128",
        ),
        ({"start": "2026-10-12T09:00:00Z"}, TypeError, "must be a datetime"),
        ({"end": START - timedelta(seconds=1)}, ValueError, "end before it starts"),
        ({"inside": [RUN]}, TypeError, "item must be"),
        ({"inside": [None]}, TypeError, "item must be"),
        (
            {"inside": [dataclasses.replace(STEP, calls=[STEP])]},  # type: ignore[list-item]
            TypeError,
            "call must",
        ),
        ({"inside": iter([TOOL])}, TypeError, "inside must be a sequence"),
        ({"inside": {TOOL}}, TypeError, "inside must be a sequence"),
        (
            {"inside": [dataclasses.replace(STEP, calls=iter([TOOL]))]},  # type: ignore[arg-type]
            TypeError,
            "calls must be a sequence",
        ),
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
            ValueError,
            "end before it starts",
        ),
    ],
)
def test_a_record_refused_opens_and_sends_nothing(
    transport: FakeTransport,
    change: dict[str, Any] | Callable[[RunRecord], Any],
    kind: type[Exception],
    message: str,
) -> None:
    """Refused before its own span opens, which would ask the root sampler."""
    sampler = Hooked()
    asked: list[None] = []
    sampler.hook = lambda: asked.append(None)
    client = Telmoni(Config(api_key="k", sampler=sampler), transport=transport)
    record = (
        change(the_record())
        if callable(change)
        else dataclasses.replace(the_record(), **change)
    )
    try:
        with pytest.raises(kind, match=message):
            client.record_run(record)
        assert client.flush()
    finally:
        client.shutdown()
    assert transport.requests == []
    assert asked == []


def test_a_record_after_shutdown_is_refused(transport: FakeTransport) -> None:
    """Never answered as sent when it went nowhere."""
    client = Telmoni(Config(api_key="k"), transport=transport)
    client.shutdown()
    with pytest.raises(ShutdownError, match="shut down"):
        client.record_run(the_record())
    assert transport.requests == []


def test_a_record_the_queue_has_no_room_for_is_refused_whole(
    transport: FakeTransport,
) -> None:
    """Refused whole before anything is queued; drained, the queue takes it."""
    client = Telmoni(Config(api_key="k"), transport=transport)
    # Room for four spans, and a worker that will not wake until flushed.
    processor = with_limits(
        client, transport, max_queue_size=4, max_batch_size=100, schedule_delay_s=60
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
            # Two steps hold two of the four slots.
            for n in range(2):
                with client.step(f"s{n}"):
                    pass
            with pytest.raises(QueueFullError, match="3 spans"):
                client.record_run(record)
        assert client.flush()
        client.record_run(record)
        assert client.flush()
    finally:
        processor.shutdown()
        client.shutdown()
    names = [s["name"] for s in transport.spans()]
    assert names.count("execute_tool t") == 2 and len(names) == 6, names


@pytest.mark.parametrize(
    "run_id",
    ["3D6F8A1C-5B2E-4F7A-9C0D-8E1F2A3B4C5D", "3d6f8a1c5b2e4f7a9c0d8e1f2a3b4c5d"],
)
def test_a_run_id_in_capitals_or_without_hyphens_is_sent_written_out(
    telmoni: Telmoni, transport: FakeTransport, run_id: str
) -> None:
    telmoni.record_run(dataclasses.replace(the_record(), run_id=run_id))
    assert telmoni.flush()
    sent = {attributes_of(span)["telmoni.run_id"] for span in transport.spans()}
    assert sent == {"3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d"}


def test_a_model_call_with_no_operation_is_a_chat(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """As the live call takes it."""
    call = dataclasses.replace(MODEL, operation=None)  # type: ignore[arg-type]
    telmoni.record_run(dataclasses.replace(RUN, inside=[call]))
    assert telmoni.flush()
    assert "chat m" in [span["name"] for span in transport.spans()]


def test_a_tool_call_with_an_empty_name_is_sent(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """Ingest reads a tool's empty name as none, keeping the call."""
    telmoni.record_run(
        dataclasses.replace(RUN, inside=[dataclasses.replace(TOOL, name="")])
    )
    assert telmoni.flush()
    assert "execute_tool " in [span["name"] for span in transport.spans()]


def test_a_record_at_ingests_text_caps_is_sent_whole(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """Counted in characters, as ingest counts them, never in UTF-8's bytes."""
    tool = dataclasses.replace(TOOL, name="t" * 145, error_type="E" * 128)
    model = dataclasses.replace(
        MODEL, operation="generate_content", model="m" * 128, provider="p" * 128
    )
    record = dataclasses.replace(
        RUN,
        agent="\u00e9" * 128,
        user_id="u" * 128,
        session_id="s" * 200,
        inside=[dataclasses.replace(STEP, name="n" * 145, calls=[tool]), model],
    )
    telmoni.record_run(record)
    assert telmoni.flush()
    names = {span["name"] for span in transport.spans()}
    widest_call = "generate_content " + "m" * 128
    assert {"n" * 145, widest_call, "execute_tool " + "t" * 145} <= names


def test_a_record_sends_its_first_32_tags_and_its_own_attributes(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """Past the span's room, its tags would crowd out its id and how it ended."""
    tags = {f"t{n:03}": "v" for n in range(130)}
    record = dataclasses.replace(
        the_record(), tags=tags, error_type="Boom", cancelled=True
    )
    telmoni.record_run(record)
    assert telmoni.flush()
    run = next(s for s in transport.spans() if s["name"] == "invoke_agent claude-code")
    attributes = attributes_of(run)
    sent = [k for k in attributes if k.startswith("telmoni.tag.")]
    assert sent == [f"telmoni.tag.t{n:03}" for n in range(32)]
    assert attributes["telmoni.run_id"] == record.run_id
    assert attributes["error.type"] == "Boom"
    assert attributes["telmoni.cancelled"] is True


def test_a_runs_tags_are_those_checked(
    telmoni: Telmoni, transport: FakeTransport
) -> None:
    """Read before its items, though reading those changes them."""
    tags = {"team": "x"}

    class Noting(Sequence[ToolCallRecord]):
        def __len__(self) -> int:
            return 1

        def __getitem__(self, index: int) -> ToolCallRecord:  # type: ignore[override]
            if index:
                raise IndexError(index)
            tags["rows"] = object()  # type: ignore[assignment]
            return TOOL

    telmoni.record_run(dataclasses.replace(RUN, tags=tags, inside=Noting()))
    assert telmoni.flush()
    run = next(s for s in transport.spans() if s["name"] == "invoke_agent a")
    sent = {k: v for k, v in attributes_of(run).items() if k.startswith("telmoni.tag.")}
    assert sent == {"telmoni.tag.team": "x"}


class Rebuilt(Sequence[ModelCallRecord]):
    """Its calls built afresh on each read, as a view over rows builds them."""

    def __init__(self, tokens: list[int]) -> None:
        self.tokens = tokens

    def __len__(self) -> int:
        return len(self.tokens)

    def __getitem__(self, index: int) -> ModelCallRecord:  # type: ignore[override]
        usage = Usage(input_tokens=self.tokens[index])
        return dataclasses.replace(MODEL, model=f"m{index}", usage=usage)


@pytest.mark.parametrize(
    "inside",
    [Rebuilt([1, 2, 3]), [dataclasses.replace(STEP, calls=Rebuilt([1, 2, 3]))]],
)
def test_a_record_is_read_once(
    telmoni: Telmoni,
    transport: FakeTransport,
    inside: Sequence[StepRecord | ModelCallRecord],
) -> None:
    """Each call's tokens on its own span, however its items are built."""
    telmoni.record_run(dataclasses.replace(RUN, inside=inside))
    assert telmoni.flush()
    tokens = {
        span["name"]: attributes_of(span)["gen_ai.usage.input_tokens"]
        for span in transport.spans()
        if span["name"].startswith("chat")
    }
    assert tokens == {"chat m0": 1, "chat m1": 2, "chat m2": 3}
