"""The golden fixtures in ``sdk/fixtures``: the export every SDK must send for
the same run, and what each must do with the server's answers."""

from __future__ import annotations

import copy
import http.client
import logging
import re
import time
from typing import Any

import pytest

from conftest import FakeTransport, answer_from, kept, load_fixture
from telmoni import Config, Telmoni, Usage

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$")
HEX32 = re.compile(r"^[0-9a-f]{32}$")
HEX16 = re.compile(r"^[0-9a-f]{16}$")
VERSIONED = {"telemetry.sdk.version", "telemetry.distro.version"}


def normalize(export: dict[str, Any]) -> dict[str, Any]:
    """The fixture's normalization, as its README sets it out."""
    export = copy.deepcopy(export)
    resource_spans = export["resourceSpans"]
    assert len(resource_spans) == 1
    scope_spans = resource_spans[0]["scopeSpans"]
    assert len(scope_spans) == 1
    spans = sorted(scope_spans[0]["spans"], key=lambda s: s["name"])
    trace_ids = {span["traceId"] for span in spans}
    assert len(trace_ids) == 1 and HEX32.match(next(iter(trace_ids)))
    names = {}
    for span in spans:
        assert HEX16.match(span["spanId"])
        names[span["spanId"]] = span["name"]
    run_ids = set()
    for span in spans:
        span["traceId"] = "<trace>"
        span["spanId"] = f"<span:{names[span['spanId']]}>"
        if "parentSpanId" in span:
            assert span["parentSpanId"] in names, span["name"]
            span["parentSpanId"] = f"<span:{names[span['parentSpanId']]}>"
        start, end = int(span["startTimeUnixNano"]), int(span["endTimeUnixNano"])
        assert end >= start
        span["startTimeUnixNano"] = span["endTimeUnixNano"] = "<time>"
        span["attributes"] = sorted(span["attributes"], key=lambda a: a["key"])
        for attribute in span["attributes"]:
            if attribute["key"] == "telmoni.run_id":
                run_ids.add(attribute["value"]["stringValue"])
                attribute["value"]["stringValue"] = "<uuid>"
    assert len(run_ids) == 1 and UUID.match(next(iter(run_ids)))
    scope_spans[0]["spans"] = spans
    scope_spans[0]["scope"]["version"] = "<version>"
    resource = resource_spans[0]["resource"]
    resource["attributes"] = sorted(resource["attributes"], key=lambda a: a["key"])
    for attribute in resource["attributes"]:
        if attribute["key"] == "telemetry.sdk.language":
            attribute["value"]["stringValue"] = "<language>"
        elif attribute["key"] in VERSIONED:
            attribute["value"]["stringValue"] = "<version>"
    return export


def perform(telmoni: Telmoni, scenario: dict[str, Any]) -> None:
    run_spec = scenario["run"]
    with telmoni.run(
        run_spec["agent"],
        user_id=run_spec["user_id"],
        session_id=run_spec["session_id"],
        tags=run_spec["tags"],
        sample_rate=run_spec["sample_rate"],
    ):
        step_spec = run_spec["inside"][0]
        with telmoni.step(step_spec["step"]):
            call_spec = step_spec["inside"][0]["model_call"]
            with telmoni.model_call(
                call_spec["model"],
                provider=call_spec["provider"],
                input_messages=call_spec["input_messages"],
            ) as call:
                call.answered(
                    response_model=call_spec["response_model"],
                    usage=Usage(**call_spec["usage"]),
                    output_messages=call_spec["output_messages"],
                )
        tool_spec = run_spec["inside"][1]["tool_call"]
        with telmoni.tool_call(tool_spec["name"]) as tool:
            tool.fail(tool_spec["fails_with"])
    assert telmoni.flush()


def test_the_run_matches_the_golden_export(
    transport: FakeTransport, sleeps: list[float]
) -> None:
    fixture = load_fixture("run.json")
    scenario = fixture["scenario"]
    telmoni = Telmoni(
        Config(
            api_key="telmoni_test_key",
            capture_content=scenario["capture_content"],
            environment=scenario["environment"],
            release=scenario["release"],
        ),
        transport=transport,
        sleep=sleeps.append,
    )
    try:
        perform(telmoni, scenario)
    finally:
        telmoni.shutdown()
    bodies = transport.bodies()
    assert bodies, "the run was sent"
    gathered = copy.deepcopy(bodies[0])
    gathered["resourceSpans"][0]["scopeSpans"][0]["spans"] = [
        span
        for body in bodies
        for span in body["resourceSpans"][0]["scopeSpans"][0]["spans"]
    ]
    assert normalize(gathered) == fixture["export"]
    for _url, headers, _body in transport.requests:
        assert headers["Authorization"] == "Bearer telmoni_test_key"
        assert headers["Content-Type"] == "application/json"
        assert headers["User-Agent"].startswith("telmoni-python/")
        assert headers["Telmoni-Sent-At"].isdigit()


def test_each_attempt_carries_this_clock_as_it_leaves() -> None:
    """In nanoseconds, a retry its own: ingest moves a run by it."""
    unavailable = answer_from(load_fixture("answers/unavailable.json"))
    transport = FakeTransport([unavailable])
    before = time.time_ns()
    telmoni = Telmoni(
        Config(api_key="k"), transport=transport, sleep=lambda _: time.sleep(0.005)
    )
    try:
        a_run(telmoni)
    finally:
        telmoni.shutdown()
    after = time.time_ns()
    sent = [int(headers["Telmoni-Sent-At"]) for _, headers, _ in transport.requests]
    assert all(before <= at <= after for at in sent), (before, sent, after)
    (_, _, body), (_, _, again) = transport.requests[:2]
    assert body == again, "the same export, sent again"
    assert sent[1] > sent[0]


def warnings_in(caplog: pytest.LogCaptureFixture) -> list[str]:
    return [r.getMessage() for r in caplog.records if r.levelno >= logging.WARNING]


def a_run(telmoni: Telmoni) -> None:
    """One empty run, sent in an export of its own."""
    with telmoni.run("bot"):
        pass
    assert telmoni.flush()


def test_kept(sleeps: list[float], caplog: pytest.LogCaptureFixture) -> None:
    transport = FakeTransport([answer_from(load_fixture("answers/kept.json"))])
    client = Telmoni(Config(api_key="k"), transport=transport, sleep=sleeps.append)
    try:
        assert client.content_mode is None
        a_run(client)
    finally:
        client.shutdown()
    assert warnings_in(caplog) == []
    assert client.content_mode == "off"
    assert sleeps == []


def test_dropped_is_warned_once(caplog: pytest.LogCaptureFixture) -> None:
    dropped = load_fixture("answers/dropped.json")
    transport = FakeTransport([answer_from(dropped)] * 2)
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        a_run(client)
        a_run(client)
    finally:
        client.shutdown()
    warnings = warnings_in(caplog)
    assert len(warnings) == 1, warnings
    assert "gen_ai.input.messages" in warnings[0]
    assert "content; the project's content mode is off" in warnings[0]
    assert "content mode decides whether content is kept" in warnings[0]
    assert "docs/api/ingest#what-is-dropped-at-the-door" in warnings[0]


def test_unlisted_names_the_tag_that_would_keep_it(
    caplog: pytest.LogCaptureFixture,
) -> None:
    transport = FakeTransport([answer_from(load_fixture("answers/unlisted.json"))])
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        a_run(client)
    finally:
        client.shutdown()
    warnings = warnings_in(caplog)
    assert len(warnings) == 1, warnings
    assert "ai.prompt" in warnings[0] and "telmoni.tag.ai.prompt" in warnings[0]


def test_refused_is_warned_once_and_never_retried(
    caplog: pytest.LogCaptureFixture,
) -> None:
    transport = FakeTransport([answer_from(load_fixture("answers/refused.json"))] * 2)
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        a_run(client)
        a_run(client)
    finally:
        client.shutdown()
    warnings = warnings_in(caplog)
    assert len(warnings) == 1, warnings
    assert "a span outside a Telmoni run is refused" in warnings[0]
    assert len(transport.requests) == 2, "one export each run, neither sent again"


def test_rate_limited_waits_retry_after_and_sends_the_same_bytes(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport(
        [answer_from(load_fixture("answers/rate-limited.json")), kept()]
    )
    client = Telmoni(Config(api_key="k"), transport=transport)
    client.exporter._sleep = sleeps.append
    try:
        a_run(client)
    finally:
        client.shutdown()
    assert sleeps == [2.0]
    assert transport.requests[0][2] == transport.requests[1][2]
    assert warnings_in(caplog) == []


def test_unavailable_is_retried_three_times_then_dropped(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    unavailable = answer_from(load_fixture("answers/unavailable.json"))
    transport = FakeTransport([unavailable, unavailable, unavailable, kept()])
    client = Telmoni(Config(api_key="k"), transport=transport)
    client.exporter._sleep = sleeps.append
    try:
        a_run(client)
        a_run(client)
    finally:
        client.shutdown()
    assert sleeps == [1.0, 1.0]
    warnings = warnings_in(caplog)
    assert len(warnings) == 1, warnings
    assert (
        "3 attempts" in warnings[0]
        and "/errors/telemetry/store-unavailable" in warnings[0]
    )
    assert len(transport.requests) == 4, (
        "three for the first run, then the next run's own export"
    )


def test_forbidden_is_not_retried_and_the_next_batch_is_still_sent(
    caplog: pytest.LogCaptureFixture,
) -> None:
    forbidden = answer_from(load_fixture("answers/forbidden.json"))
    transport = FakeTransport([forbidden, forbidden])
    client = Telmoni(Config(api_key="k"), transport=transport)
    try:
        a_run(client)
        a_run(client)
    finally:
        client.shutdown()
    warnings = warnings_in(caplog)
    assert len(warnings) == 1, warnings
    assert "403" in warnings[0] and "/errors/authz/forbidden" in warnings[0]
    assert len(transport.requests) == 2


def test_a_network_failure_is_retried_with_backoff(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport([OSError("refused"), OSError("refused"), kept()])
    client = Telmoni(Config(api_key="k"), transport=transport)
    client.exporter._sleep = sleeps.append
    try:
        a_run(client)
    finally:
        client.shutdown()
    assert sleeps == [1.0, 2.0]
    assert warnings_in(caplog) == []


def test_an_answer_broken_before_its_status_is_retried_like_a_refused_connection(
    sleeps: list[float], caplog: pytest.LogCaptureFixture
) -> None:
    transport = FakeTransport(
        [
            http.client.RemoteDisconnected(""),
            http.client.BadStatusLine(""),
            kept(),
        ]
    )
    client = Telmoni(Config(api_key="k"), transport=transport)
    client.exporter._sleep = sleeps.append
    try:
        a_run(client)
    finally:
        client.shutdown()
    assert sleeps == [1.0, 2.0]
    assert warnings_in(caplog) == []
