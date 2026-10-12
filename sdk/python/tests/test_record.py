"""A run recorded after it ended: ``record_run`` and ``sdk/fixtures/recorded.json``."""

from __future__ import annotations

import copy
import dataclasses
from datetime import datetime, timedelta
from typing import Any
from unittest import mock

import pytest
from opentelemetry import trace

from conftest import FakeTransport, attributes_of, load_fixture
from telmoni import (
    Config,
    ModelCallRecord,
    RunRecord,
    StepRecord,
    Telmoni,
    ToolCallRecord,
    Usage,
    current_run,
)

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
